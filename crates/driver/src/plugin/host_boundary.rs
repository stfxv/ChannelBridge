//! HAL host boundary. No CoreAudio client calls belong in this module.
#![allow(clippy::too_many_arguments)] // Apple's fixed C callback signatures.

use super::{properties, state, system};
use channelbridge_hal_abi::{interface::*, *};
use core::{ffi::c_void, ptr};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

pub(super) const PLUGIN: u32 = 1;
pub(super) const DEVICE: u32 = 2;
pub(super) const INPUT: u32 = 3;
pub(super) const OUTPUT: u32 = 4;
pub(super) const BAD_OBJECT: i32 = fourcc(b"!obj") as i32;
pub(super) const BAD_SIZE: i32 = fourcc(b"!siz") as i32;
pub(super) const UNKNOWN_PROPERTY: i32 = fourcc(b"who?") as i32;
pub(super) const ILLEGAL: i32 = fourcc(b"nope") as i32;
pub(super) const NOT_READY: i32 = fourcc(b"nrdy") as i32;
pub(super) const UNSUPPORTED: i32 = fourcc(b"unop") as i32;
pub(super) const UNSPECIFIED: i32 = fourcc(b"what") as i32;

static FAILED: AtomicBool = AtomicBool::new(false);
static REFERENCES: AtomicU32 = AtomicU32::new(0);

pub(super) fn guard(body: impl FnOnce() -> i32) -> i32 {
    if FAILED.load(Ordering::Acquire) {
        return UNSPECIFIED;
    }
    // Defense in depth under panic=unwind. Normal failures return statuses;
    // FAILED prevents later callbacks from reusing partially mutated state.
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(status) => status,
        Err(_) => {
            FAILED.store(true, Ordering::Release);
            UNSPECIFIED
        }
    }
}

#[repr(transparent)]
struct ImmutableInterface(DriverInterface);
// SAFETY: The static function table and its null reserved pointer are immutable.
// All mutable plug-in state is synchronized separately.
unsafe impl Sync for ImmutableInterface {}
#[repr(transparent)]
struct ImmutablePointer(*const DriverInterface);
// SAFETY: This points to the immutable static table, never to mutable state.
unsafe impl Sync for ImmutablePointer {}

static INTERFACE: ImmutableInterface = ImmutableInterface(DriverInterface {
    reserved: ptr::null_mut(),
    query_interface,
    add_ref,
    release,
    initialize,
    create_device,
    destroy_device,
    add_device_client,
    remove_device_client,
    perform_configuration_change,
    abort_configuration_change,
    has_property: properties::has_property,
    is_property_settable: properties::is_settable,
    get_property_data_size: properties::data_size,
    get_property_data: properties::get_data,
    set_property_data: properties::set_data,
    start_io,
    stop_io,
    get_zero_timestamp,
    will_do_io,
    begin_io: io_boundary,
    do_io,
    end_io: io_boundary,
});
static INTERFACE_POINTER: ImmutablePointer = ImmutablePointer(&INTERFACE.0);

fn driver_ref() -> DriverRef {
    ptr::addr_of!(INTERFACE_POINTER.0)
        .cast::<*mut c_void>()
        .cast_mut()
}
pub(super) fn valid(driver: DriverRef) -> bool {
    driver == driver_ref()
}

/// CFPlugIn factory used by the test driver bundle.
///
/// # Safety
/// `requested_type` is a valid CFUUID supplied by the CFPlugIn loader, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ChannelBridge_Create(
    _allocator: CfRef,
    requested_type: CfRef,
) -> *mut c_void {
    if requested_type.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The loader guarantees the CFUUID lifetime for this call.
    let requested = unsafe { system::uuid_bytes(requested_type) };
    if requested.bytes != PLUGIN_TYPE_UUID.bytes {
        return ptr::null_mut();
    }
    let status = guard(|| {
        // SAFETY: The factory uses the static driver identity; add_ref only updates the
        // atomic count.
        unsafe { add_ref(driver_ref().cast()) };
        0
    });
    if status == 0 {
        driver_ref().cast()
    } else {
        ptr::null_mut()
    }
}

unsafe extern "C" fn query_interface(
    driver: *mut c_void,
    uuid: UuidBytes,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() {
        return 0x80004003_u32 as i32;
    }
    // SAFETY: IUnknown's caller provides one writable pointer slot.
    unsafe { output.write(ptr::null_mut()) };
    if !valid(driver.cast()) {
        return 0x80004003_u32 as i32;
    }
    if uuid.bytes != DRIVER_INTERFACE_UUID.bytes && uuid.bytes != IUNKNOWN_UUID.bytes {
        return 0x80004002_u32 as i32;
    }
    // SAFETY: Identity was validated; IUnknown grants a writable pointer slot for this call.
    unsafe {
        add_ref(driver);
        output.write(driver)
    };
    0
}

unsafe extern "C" fn add_ref(driver: *mut c_void) -> u32 {
    if !valid(driver.cast()) {
        return 0;
    }
    REFERENCES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
        .unwrap_or(u32::MAX)
        .saturating_add(1)
}

unsafe extern "C" fn release(driver: *mut c_void) -> u32 {
    if !valid(driver.cast()) {
        return 0;
    }
    match REFERENCES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(1)) {
        Ok(1) => {
            state::shutdown();
            0
        }
        Ok(old) => old - 1,
        Err(_) => 0,
    }
}

unsafe extern "C" fn initialize(driver: DriverRef, host: HostRef) -> i32 {
    guard(|| {
        if !valid(driver) || host.is_null() {
            return BAD_OBJECT;
        }
        state::initialize(host)
    })
}

unsafe extern "C" fn create_device(
    driver: DriverRef,
    _description: CfRef,
    _info: *const ClientInfo,
    output: *mut ObjectId,
) -> i32 {
    guard(|| {
        if !valid(driver) {
            return BAD_OBJECT;
        }
        if !output.is_null() {
            // SAFETY: HAL provides a writable ObjectId slot when output is non-null.
            unsafe { output.write(0) };
        }
        UNSUPPORTED
    })
}
unsafe extern "C" fn destroy_device(driver: DriverRef, _device: ObjectId) -> i32 {
    guard(|| {
        if valid(driver) {
            UNSUPPORTED
        } else {
            BAD_OBJECT
        }
    })
}

unsafe extern "C" fn add_device_client(
    driver: DriverRef,
    device: ObjectId,
    info: *const ClientInfo,
) -> i32 {
    guard(|| {
        if !valid(driver) || device != DEVICE || info.is_null() {
            return BAD_OBJECT;
        }
        // SAFETY: HAL supplies a ClientInfo valid for this callback.
        let client = unsafe { info.read_unaligned() };
        state::add_client(client.client_id)
    })
}
unsafe extern "C" fn remove_device_client(
    driver: DriverRef,
    device: ObjectId,
    info: *const ClientInfo,
) -> i32 {
    guard(|| {
        if !valid(driver) || device != DEVICE || info.is_null() {
            return BAD_OBJECT;
        }
        // SAFETY: HAL keeps the ClientInfo descriptor readable throughout this callback.
        state::remove_client(unsafe { info.read_unaligned() }.client_id)
    })
}
unsafe extern "C" fn perform_configuration_change(
    driver: DriverRef,
    device: ObjectId,
    action: u64,
    _info: *mut c_void,
) -> i32 {
    guard(|| {
        if valid(driver) && device == DEVICE {
            state::perform_rate_change(action)
        } else {
            BAD_OBJECT
        }
    })
}
unsafe extern "C" fn abort_configuration_change(
    driver: DriverRef,
    device: ObjectId,
    action: u64,
    _info: *mut c_void,
) -> i32 {
    guard(|| {
        if valid(driver) && device == DEVICE {
            state::abort_rate_change(action)
        } else {
            BAD_OBJECT
        }
    })
}

unsafe extern "C" fn start_io(driver: DriverRef, device: ObjectId, client: u32) -> i32 {
    guard(|| {
        if valid(driver) && device == DEVICE {
            state::start(client)
        } else {
            BAD_OBJECT
        }
    })
}
unsafe extern "C" fn stop_io(driver: DriverRef, device: ObjectId, client: u32) -> i32 {
    guard(|| {
        if valid(driver) && device == DEVICE {
            state::stop(client)
        } else {
            BAD_OBJECT
        }
    })
}

unsafe extern "C" fn get_zero_timestamp(
    driver: DriverRef,
    device: ObjectId,
    _client: u32,
    sample: *mut f64,
    host: *mut u64,
    seed: *mut u64,
) -> i32 {
    guard(|| {
        if !valid(driver) || device != DEVICE {
            return BAD_OBJECT;
        }
        if sample.is_null() || host.is_null() || seed.is_null() {
            return BAD_SIZE;
        }
        let Some(timestamp) = state::timestamp() else {
            return NOT_READY;
        };
        // SAFETY: HAL supplies non-null writable output slots of these types.
        unsafe {
            sample.write(timestamp.sample_frame as f64);
            host.write(timestamp.host_ticks);
            seed.write(timestamp.seed);
        }
        0
    })
}

unsafe extern "C" fn will_do_io(
    driver: DriverRef,
    device: ObjectId,
    _client: u32,
    operation: u32,
    will: *mut u8,
    in_place: *mut u8,
) -> i32 {
    guard(|| {
        if !valid(driver) || device != DEVICE {
            return BAD_OBJECT;
        }
        if will.is_null() || in_place.is_null() {
            return BAD_SIZE;
        }
        // SAFETY: HAL grants distinct writable Boolean output slots for this callback.
        unsafe {
            will.write(u8::from(
                operation == fourcc(b"read") || operation == fourcc(b"rite"),
            ));
            in_place.write(1);
        }
        0
    })
}
unsafe extern "C" fn io_boundary(
    driver: DriverRef,
    device: ObjectId,
    _client: u32,
    _operation: u32,
    frames: u32,
    _cycle: *const IoCycleInfo,
) -> i32 {
    guard(|| {
        if !valid(driver) || device != DEVICE {
            BAD_OBJECT
        } else if frames > 4096 {
            BAD_SIZE
        } else {
            0
        }
    })
}

unsafe extern "C" fn do_io(
    driver: DriverRef,
    device: ObjectId,
    stream: ObjectId,
    _client: u32,
    operation: u32,
    frames: u32,
    cycle: *const IoCycleInfo,
    main: *mut c_void,
    _secondary: *mut c_void,
) -> i32 {
    guard(|| {
        if !valid(driver) || device != DEVICE {
            return BAD_OBJECT;
        }
        if frames > 4096 || cycle.is_null() || (main.is_null() && frames != 0) {
            return BAD_SIZE;
        }
        let read = operation == fourcc(b"read");
        if (read && stream != INPUT) || (!read && stream != OUTPUT) {
            return BAD_OBJECT;
        }
        if !read && operation != fourcc(b"rite") {
            return UNSUPPORTED;
        }
        if frames == 0 {
            return 0;
        }
        // SAFETY: HAL's PCM buffers contain exactly frames * 2 writable floats,
        // and cycle is a valid IOCycleInfo for the duration of this callback.
        let cycle = unsafe { cycle.read_unaligned() };
        let buffer =
            // SAFETY: HAL grants exclusive, aligned PCM storage for frames stereo
            // samples during this callback.
            unsafe { core::slice::from_raw_parts_mut(main.cast::<f32>(), frames as usize * 2) };
        let mut scratch = [channelbridge_audio::StereoFrame::SILENCE; 4096];
        let scratch = &mut scratch[..frames as usize];
        let Some(runtime) = state::runtime() else {
            if read {
                buffer.fill(0.0);
            }
            return NOT_READY;
        };
        if read {
            if cycle.input_time.flags & 1 == 0 {
                buffer.fill(0.0);
                return ILLEGAL;
            }
            runtime.audio.read(cycle.input_time.sample_time, scratch);
            for (pair, value) in buffer.chunks_exact_mut(2).zip(scratch.iter()) {
                pair[0] = value.left;
                pair[1] = value.right;
            }
        } else {
            if cycle.output_time.flags & 1 == 0 {
                return ILLEGAL;
            }
            for (pair, value) in buffer.chunks_exact(2).zip(scratch.iter_mut()) {
                *value = channelbridge_audio::StereoFrame::new(pair[0], pair[1]);
            }
            runtime.audio.write(cycle.output_time.sample_time, scratch);
        }
        0
    })
}
