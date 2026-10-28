use core::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamFormat {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ValueRange {
    pub minimum: f64,
    pub maximum: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RangedFormat {
    pub format: StreamFormat,
    pub rate_range: ValueRange,
}

pub type OsStatus = i32;
pub type ObjectId = u32;
/// Pointer to the interface-table pointer returned by the CFPlugIn factory.
/// Dereferencing either level requires a live counted driver reference.
pub type DriverRef = *mut *mut c_void;

/// Borrowed native payload descriptor. This type neither owns nor retains data.
/// Dereferencing `data` requires the callback's live byte extent and suitable
/// sample alignment; writable access additionally requires exclusive ownership.
#[repr(C)]
pub struct AudioBuffer {
    pub channels: u32,
    pub byte_size: u32,
    pub data: *mut c_void,
}

/// C's variable-length list header. Additional buffers follow the first.
/// `count` does not prove an allocation's extent. A raw pointer may address only
/// the count field when empty, so read it before forming a Rust reference to the
/// full one-buffer type. The native caller must supply all counted descriptors.
#[repr(C)]
pub struct AudioBufferList {
    pub count: u32,
    pub buffers: [AudioBuffer; 1],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SmpteTime {
    pub subframes: i16,
    pub subframe_divisor: i16,
    pub counter: u32,
    pub kind: u32,
    pub flags: u32,
    pub hours: i16,
    pub minutes: i16,
    pub seconds: i16,
    pub frames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioTimestamp {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub word_clock_time: u64,
    pub smpte_time: SmpteTime,
    pub flags: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IoCycleInfo {
    pub cycle_counter: u64,
    pub nominal_frames: u32,
    pub current_time: AudioTimestamp,
    pub input_time: AudioTimestamp,
    pub output_time: AudioTimestamp,
    pub main_host_ticks_per_frame: f64,
    pub device_host_ticks_per_frame: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ClientInfo {
    pub client_id: u32,
    pub process_id: i32,
    pub is_native_endian: u8,
    /// Borrowed CFString supplied by HAL. Copying ClientInfo does not retain it;
    /// use beyond the callback requires a separate CoreFoundation retain.
    pub bundle_id: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UuidBytes {
    pub bytes: [u8; 16],
}

#[must_use]
pub const fn fourcc(value: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;
    #[test]
    fn layout_matches_64_bit_apple_types() {
        assert_eq!(size_of::<AudioTimestamp>(), 64);
        assert_eq!(size_of::<IoCycleInfo>(), 224);
        assert_eq!(offset_of!(IoCycleInfo, current_time), 16);
        assert_eq!(offset_of!(IoCycleInfo, output_time), 144);
        assert_eq!(size_of::<ClientInfo>(), 24);
        assert_eq!(offset_of!(ClientInfo, bundle_id), 16);
        assert_eq!(size_of::<PropertyAddress>(), 12);
    }
}
