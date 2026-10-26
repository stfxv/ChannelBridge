//! Small Darwin/CoreFoundation boundary. No HAL client APIs.
use channelbridge_hal_abi::{UuidBytes, interface::CfRef};
use core::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Default)]
struct Timebase {
    numerator: u32,
    denominator: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: CfRef, bytes: *const u8, length: isize) -> CfRef;
    fn CFUUIDGetUUIDBytes(uuid: CfRef) -> UuidBytes;
    fn CFStringCreateWithCString(allocator: CfRef, text: *const c_char, encoding: u32) -> CfRef;
    fn CFStringGetCString(value: CfRef, out: *mut c_char, capacity: isize, encoding: u32) -> u8;
    fn CFGetTypeID(value: CfRef) -> usize;
    fn CFStringGetTypeID() -> usize;
}

pub fn now() -> u64 {
    // SAFETY: mach_absolute_time has no pointer or initialization preconditions.
    unsafe { mach_absolute_time() }
}
pub fn frequency() -> Option<u64> {
    let mut base = Timebase::default();
    // SAFETY: base is a writable structure matching mach_timebase_info_data_t.
    if unsafe { mach_timebase_info(&mut base) } != 0 || base.numerator == 0 {
        return None;
    }
    let numerator = 1_000_000_000_u64.checked_mul(u64::from(base.denominator))?;
    if numerator % u64::from(base.numerator) != 0 {
        return None;
    }
    Some(numerator / u64::from(base.numerator))
}
/// Reads the UUID value without taking ownership.
///
/// # Safety
/// `uuid` must be a live CFUUID object for the duration of this call.
pub unsafe fn uuid_bytes(uuid: CfRef) -> UuidBytes {
    // SAFETY: the caller supplies a live CFUUID for this call.
    unsafe { CFUUIDGetUUIDBytes(uuid) }
}

pub fn string(text: &str) -> Option<CfRef> {
    let text = std::ffi::CString::new(text).ok()?;
    // SAFETY: CString is NUL-terminated and lives through the call. CF returns a
    // +1 retained string that the property caller must release per Apple's ABI.
    let result = unsafe {
        CFStringCreateWithCString(core::ptr::null::<c_void>(), text.as_ptr(), 0x08000100)
    };
    (!result.is_null()).then_some(result)
}

/// Compares a borrowed CoreFoundation string with UTF-8 text.
///
/// # Safety
/// A non-null `value` must be a live CoreFoundation object throughout the call.
/// The object is borrowed and its retain count is unchanged.
pub unsafe fn string_equals(value: CfRef, expected: &str) -> bool {
    if value.is_null() {
        return false;
    }
    // SAFETY: qualifier values are validated CF objects supplied by the host.
    if unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return false;
    }
    let mut bytes = [0_i8; 512];
    // SAFETY: the type check established a live CFString; bytes is writable for its stated
    // capacity.
    if unsafe { CFStringGetCString(value, bytes.as_mut_ptr(), bytes.len() as isize, 0x08000100) }
        == 0
    {
        return false;
    }
    // SAFETY: successful CFStringGetCString initialized a NUL-terminated string in bytes.
    let actual = unsafe { std::ffi::CStr::from_ptr(bytes.as_ptr()) };
    actual.to_bytes() == expected.as_bytes()
}

pub fn data(bytes: &[u8]) -> Option<CfRef> {
    // SAFETY: CFData copies the live slice and transfers a +1 reference to caller.
    let result = unsafe { CFDataCreate(core::ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
    (!result.is_null()).then_some(result)
}
