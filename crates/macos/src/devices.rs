//! Bounded property reads on the control thread, outside audio callbacks.
use channelbridge_hal_abi::{ObjectId, PropertyAddress, fourcc};
use std::{ffi::c_void, fmt};

const GLOBAL: u32 = fourcc(b"glob");
const MAX_PROPERTY_BYTES: u32 = 1_048_576;

#[derive(Debug)]
pub enum Error {
    Hal(i32),
    MalformedProperty,
    PropertyTooLarge,
    InvalidString,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hal(status) => write!(f, "CoreAudio status {status}"),
            Self::MalformedProperty => f.write_str("invalid CoreAudio property layout"),
            Self::PropertyTooLarge => f.write_str("CoreAudio property exceeds size limit"),
            Self::InvalidString => f.write_str("invalid CoreAudio string"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Debug)]
pub struct Device {
    pub id: ObjectId,
    pub uid: String,
    pub name: String,
    pub rate: f64,
    pub input_channels: u32,
    pub output_channels: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: ObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: ObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        output: *mut c_void,
    ) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
    fn CFGetTypeID(object: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(string: *const c_void) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        string: *const c_void,
        output: *mut i8,
        capacity: isize,
        encoding: u32,
    ) -> u8;
}

/// The word allocation guarantees 8-byte alignment for HAL's typed writes.
/// Returned bytes are copied only after the reported length is validated.
pub fn property(object: ObjectId, selector: u32, scope: u32) -> Result<Vec<u8>, Error> {
    let address = PropertyAddress {
        selector,
        scope,
        element: 0,
    };
    for _ in 0..3 {
        let mut size = 0;
        // SAFETY: all arguments refer to initialized, live C-compatible storage.
        let status = unsafe {
            AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size)
        };
        if status != 0 {
            return Err(Error::Hal(status));
        }
        if size > MAX_PROPERTY_BYTES {
            return Err(Error::PropertyTooLarge);
        }
        let capacity = size;
        let mut words = vec![0_u64; (size as usize).div_ceil(8).max(1)];
        // SAFETY: writable aligned storage holds at least the advertised capacity.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                words.as_mut_ptr().cast(),
            )
        };
        if status == fourcc(b"!siz") as i32 {
            continue;
        }
        if status != 0 {
            return Err(Error::Hal(status));
        }
        if size > capacity {
            return Err(Error::MalformedProperty);
        }
        // SAFETY: words were zero-initialized and size is within the allocation.
        return Ok(unsafe {
            std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), size as usize)
        }
        .to_vec());
    }
    Err(Error::MalformedProperty)
}

fn ids(bytes: &[u8]) -> Result<Vec<ObjectId>, Error> {
    if bytes.len() % 4 != 0 {
        return Err(Error::MalformedProperty);
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|v| u32::from_ne_bytes([v[0], v[1], v[2], v[3]]))
        .collect())
}

fn text(object: ObjectId, selector: u32) -> Result<String, Error> {
    let bytes = property(object, selector, GLOBAL)?;
    let pointer_bytes: [u8; size_of::<usize>()] =
        bytes.try_into().map_err(|_| Error::MalformedProperty)?;
    let pointer = usize::from_ne_bytes(pointer_bytes) as *const c_void;
    if pointer.is_null() {
        return Err(Error::InvalidString);
    }
    struct OwnedCf(*const c_void);
    impl Drop for OwnedCf {
        fn drop(&mut self) {
            // SAFETY: HAL's CF-valued getter transfers one reference to the client.
            unsafe { CFRelease(self.0) };
        }
    }
    let owned = OwnedCf(pointer);
    // SAFETY: successful CF property getter returns a live CF object.
    if unsafe { CFGetTypeID(owned.0) != CFStringGetTypeID() } {
        return Err(Error::InvalidString);
    }
    const UTF8: u32 = 0x0800_0100;
    // SAFETY: type was checked; string remains retained throughout conversion.
    let capacity = unsafe { CFStringGetMaximumSizeForEncoding(CFStringGetLength(owned.0), UTF8) };
    if !(0..MAX_PROPERTY_BYTES as isize).contains(&capacity) {
        return Err(Error::InvalidString);
    }
    let mut result = vec![0_u8; capacity as usize + 1];
    // SAFETY: the output buffer has the specified capacity including terminator.
    if unsafe {
        CFStringGetCString(
            owned.0,
            result.as_mut_ptr().cast(),
            result.len() as isize,
            UTF8,
        )
    } == 0
    {
        return Err(Error::InvalidString);
    }
    let end = result
        .iter()
        .position(|b| *b == 0)
        .ok_or(Error::InvalidString)?;
    result.truncate(end);
    String::from_utf8(result).map_err(|_| Error::InvalidString)
}

fn channels(object: ObjectId, scope: u32) -> Result<u32, Error> {
    let bytes = property(object, fourcc(b"slay"), scope)?;
    decode_channels(&bytes)
}

fn decode_channels(bytes: &[u8]) -> Result<u32, Error> {
    let count = bytes.get(..4).ok_or(Error::MalformedProperty)?;
    let count = u32::from_ne_bytes([count[0], count[1], count[2], count[3]]) as usize;
    // 64-bit AudioBufferList: count at 0, first 16-byte AudioBuffer at 8.
    let end = count
        .checked_mul(16)
        .and_then(|v| v.checked_add(8))
        .ok_or(Error::MalformedProperty)?;
    if end > bytes.len() {
        return Err(Error::MalformedProperty);
    }
    let mut channels = 0_u32;
    for buffer in bytes[8..end].chunks_exact(16) {
        let value = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
        channels = channels
            .checked_add(value)
            .ok_or(Error::MalformedProperty)?;
    }
    Ok(channels)
}

/// A disappearing device is reported per entry, not silently replaced by another.
pub fn list() -> Result<Vec<Result<Device, Error>>, Error> {
    let objects = ids(&property(1, fourcc(b"dev#"), GLOBAL)?)?;
    Ok(objects
        .into_iter()
        .map(|id| {
            let rate_bytes: [u8; 8] = property(id, fourcc(b"nsrt"), GLOBAL)?
                .try_into()
                .map_err(|_| Error::MalformedProperty)?;
            let rate = f64::from_ne_bytes(rate_bytes);
            if !rate.is_finite() || rate <= 0.0 {
                return Err(Error::MalformedProperty);
            }
            Ok(Device {
                id,
                uid: text(id, fourcc(b"uid "))?,
                name: text(id, fourcc(b"lnam"))?,
                rate,
                input_channels: channels(id, fourcc(b"inpt"))?,
                output_channels: channels(id, fourcc(b"outp"))?,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variable_buffer_list_checks_every_entry_and_overflow() {
        let mut bytes = vec![0_u8; 40];
        bytes[..4].copy_from_slice(&2_u32.to_ne_bytes());
        bytes[8..12].copy_from_slice(&2_u32.to_ne_bytes());
        bytes[24..28].copy_from_slice(&6_u32.to_ne_bytes());
        assert_eq!(decode_channels(&bytes).ok(), Some(8));
        assert!(decode_channels(&bytes[..39]).is_err());
        bytes[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(decode_channels(&bytes).is_err());
    }
}
