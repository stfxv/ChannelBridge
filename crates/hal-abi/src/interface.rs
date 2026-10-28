//! Apple's AudioServerPlugIn C interface. No HAL client functions are declared.
use crate::{ClientInfo, DriverRef, IoCycleInfo, ObjectId, OsStatus, PropertyAddress, UuidBytes};
use core::ffi::c_void;

pub type CfRef = *const c_void;
pub type HostRef = *const HostInterface;

/// Borrowed host callbacks using Apple's AudioServerPlugIn ABI.
///
/// # Safety
/// Callers must pass the live host reference supplied at initialization. The
/// table and host remain valid until the plug-in finishes all host calls,
/// including calls by its notification worker before shutdown joins it.
/// Pointer arguments must reference initialized ABI values for each call;
/// array lengths specify readable elements and output pointers specify writable,
/// aligned slots. No Rust references may outlive the native call.
#[repr(C)]
pub struct HostInterface {
    pub properties_changed:
        unsafe extern "C" fn(HostRef, ObjectId, u32, *const PropertyAddress) -> OsStatus,
    /// The output receives a +1 CF object on success; the recipient releases it.
    pub copy_from_storage: unsafe extern "C" fn(HostRef, CfRef, *mut CfRef) -> OsStatus,
    pub write_to_storage: unsafe extern "C" fn(HostRef, CfRef, CfRef) -> OsStatus,
    pub delete_from_storage: unsafe extern "C" fn(HostRef, CfRef) -> OsStatus,
    /// `info` is an opaque token returned unchanged to the configuration callback.
    /// Its owner must keep any pointed-to storage live until completion or abort.
    pub request_configuration_change:
        unsafe extern "C" fn(HostRef, ObjectId, u64, *mut c_void) -> OsStatus,
}

pub type IoBoundary =
    unsafe extern "C" fn(DriverRef, ObjectId, u32, u32, u32, *const IoCycleInfo) -> OsStatus;
pub type ClientChange = unsafe extern "C" fn(DriverRef, ObjectId, *const ClientInfo) -> OsStatus;
pub type ConfigurationChange =
    unsafe extern "C" fn(DriverRef, ObjectId, u64, *mut c_void) -> OsStatus;

/// Driver callbacks using Apple's AudioServerPlugIn ABI.
///
/// # Safety
/// The host must use the driver reference returned by the factory while it owns
/// a reference count. Each pointer must satisfy its SDK-defined type, alignment,
/// initialized extent and lifetime for the whole callback. Output slots and
/// mutable audio buffers must be exclusively writable for that invocation.
/// Qualifier and input buffers remain readable for their supplied byte counts;
/// output buffers remain writable for their capacity. Null is permitted only
/// where the SDK permits absence or a zero-length buffer. Callbacks borrow
/// these allocations and may not retain their addresses after return.
#[repr(C)]
pub struct DriverInterface {
    pub reserved: *mut c_void,
    /// Success returns an additional counted reference in the writable output slot.
    pub query_interface: unsafe extern "C" fn(*mut c_void, UuidBytes, *mut *mut c_void) -> i32,
    pub add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    pub release: unsafe extern "C" fn(*mut c_void) -> u32,
    /// The host table remains live through shutdown and completion of host calls.
    pub initialize: unsafe extern "C" fn(DriverRef, HostRef) -> OsStatus,
    pub create_device:
        unsafe extern "C" fn(DriverRef, CfRef, *const ClientInfo, *mut ObjectId) -> OsStatus,
    pub destroy_device: unsafe extern "C" fn(DriverRef, ObjectId) -> OsStatus,
    pub add_device_client: ClientChange,
    pub remove_device_client: ClientChange,
    pub perform_configuration_change: ConfigurationChange,
    pub abort_configuration_change: ConfigurationChange,
    pub has_property: unsafe extern "C" fn(DriverRef, ObjectId, i32, *const PropertyAddress) -> u8,
    pub is_property_settable:
        unsafe extern "C" fn(DriverRef, ObjectId, i32, *const PropertyAddress, *mut u8) -> OsStatus,
    pub get_property_data_size: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        i32,
        *const PropertyAddress,
        u32,
        *const c_void,
        *mut u32,
    ) -> OsStatus,
    /// CF objects returned by copy properties transfer a +1 reference to the host.
    pub get_property_data: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        i32,
        *const PropertyAddress,
        u32,
        *const c_void,
        u32,
        *mut u32,
        *mut c_void,
    ) -> OsStatus,
    pub set_property_data: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        i32,
        *const PropertyAddress,
        u32,
        *const c_void,
        u32,
        *const c_void,
    ) -> OsStatus,
    pub start_io: unsafe extern "C" fn(DriverRef, ObjectId, u32) -> OsStatus,
    pub stop_io: unsafe extern "C" fn(DriverRef, ObjectId, u32) -> OsStatus,
    pub get_zero_timestamp:
        unsafe extern "C" fn(DriverRef, ObjectId, u32, *mut f64, *mut u64, *mut u64) -> OsStatus,
    pub will_do_io:
        unsafe extern "C" fn(DriverRef, ObjectId, u32, u32, *mut u8, *mut u8) -> OsStatus,
    pub begin_io: IoBoundary,
    /// Cycle timestamps remain readable for the invocation. PCM buffer extents
    /// follow the stream format and frame count; mutable buffers may not alias
    /// other active Rust borrows. Secondary storage may be absent for in-place I/O.
    pub do_io: unsafe extern "C" fn(
        DriverRef,
        ObjectId,
        ObjectId,
        u32,
        u32,
        u32,
        *const IoCycleInfo,
        *mut c_void,
        *mut c_void,
    ) -> OsStatus,
    pub end_io: IoBoundary,
}

pub const PLUGIN_TYPE_UUID: UuidBytes = UuidBytes {
    bytes: [
        0x44, 0x3A, 0xBA, 0xB8, 0xE7, 0xB3, 0x49, 0x1A, 0xB9, 0x85, 0xBE, 0xB9, 0x18, 0x70, 0x30,
        0xDB,
    ],
};
pub const DRIVER_INTERFACE_UUID: UuidBytes = UuidBytes {
    bytes: [
        0xEE, 0xA5, 0x77, 0x3D, 0xCC, 0x43, 0x49, 0xF1, 0x8E, 0x00, 0x8F, 0x96, 0xE7, 0xD2, 0x3B,
        0x17,
    ],
};
pub const IUNKNOWN_UUID: UuidBytes = UuidBytes {
    bytes: [0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46],
};
