//! Property descriptions are shared by HasProperty, size, getter and setter.
//! Allocations here run on the HAL control path, not its I/O callbacks.
use super::host_boundary;
use super::host_boundary::BAD_OBJECT;
use super::host_boundary::BAD_SIZE;
use super::host_boundary::DEVICE;
use super::host_boundary::ILLEGAL;
use super::host_boundary::INPUT;
use super::host_boundary::OUTPUT;
use super::host_boundary::PLUGIN;
use super::host_boundary::UNKNOWN_PROPERTY;
use super::host_boundary::UNSPECIFIED;
use super::state;
use super::system;
use channelbridge_contract::{DEPLOYMENT, SUPPORTED_RATES};
use channelbridge_hal_abi as hal_abi;
use channelbridge_hal_abi::DriverRef;
use channelbridge_hal_abi::ObjectId;
use channelbridge_hal_abi::PropertyAddress;
use channelbridge_hal_abi::RangedFormat;
use channelbridge_hal_abi::StreamFormat;
use channelbridge_hal_abi::ValueRange;
use channelbridge_hal_abi::interface::CfRef;
use core::ffi::c_void;
use core::ptr;
use std::mem;

const GLOBAL: u32 = hal_abi::fourcc(b"glob");
const IN: u32 = hal_abi::fourcc(b"inpt");
const OUT: u32 = hal_abi::fourcc(b"outp");
const _: () = assert!(
    !env!("CARGO_PKG_VERSION").is_empty()
        && env!("CARGO_PKG_VERSION").len() <= channelbridge_contract::DRIVER_IDENTITY_MAX_BYTES
);

enum Value {
    Integer(u32),
    Number(f64),
    Text(&'static str),
    Objects(&'static [u32]),
    Rates,
    Format,
    Formats,
    TranslateUid,
    Layout,
    Diagnostics,
    CustomProperties,
    Identity,
}
impl Value {
    #[must_use]
    fn size(&self) -> u32 {
        match self {
            Self::Integer(_) | Self::TranslateUid => 4,
            Self::Number(_) => 8,
            Self::Text(_) => mem::size_of::<CfRef>() as u32,
            Self::Objects(objects) => objects.len() as u32 * 4,
            Self::Rates => SUPPORTED_RATES.len() as u32 * mem::size_of::<ValueRange>() as u32,
            Self::Format => mem::size_of::<StreamFormat>() as u32,
            Self::Formats => SUPPORTED_RATES.len() as u32 * mem::size_of::<RangedFormat>() as u32,
            Self::Layout => 12,
            Self::Diagnostics => mem::size_of::<CfRef>() as u32,
            Self::CustomProperties => 24,
            Self::Identity => mem::size_of::<CfRef>() as u32,
        }
    }
}

fn describe(object: ObjectId, address: PropertyAddress) -> Result<Value, i32> {
    if ![PLUGIN, DEVICE, INPUT, OUTPUT].contains(&object) {
        return Err(BAD_OBJECT);
    }
    if address.element != 0 || ![GLOBAL, IN, OUT].contains(&address.scope) {
        return Err(UNKNOWN_PROPERTY);
    }
    let selector = address.selector;
    let stream = object == INPUT || object == OUTPUT;
    if object == DEVICE && selector == hal_abi::fourcc(b"cust") && address.scope == GLOBAL {
        return Ok(Value::CustomProperties);
    }
    if selector == hal_abi::fourcc(b"bcls") {
        return Ok(Value::Integer(hal_abi::fourcc(b"aobj")));
    }
    if selector == hal_abi::fourcc(b"clas") {
        return Ok(Value::Integer(hal_abi::fourcc(if object == PLUGIN {
            b"aplg"
        } else if object == DEVICE {
            b"adev"
        } else {
            b"astr"
        })));
    }
    if selector == hal_abi::fourcc(b"stdv") {
        return Ok(Value::Integer(if object == PLUGIN {
            0
        } else if object == DEVICE {
            PLUGIN
        } else {
            DEVICE
        }));
    }
    if selector == hal_abi::fourcc(b"lnam") {
        return Ok(Value::Text(if object == PLUGIN {
            "ChannelBridge Rust"
        } else if object == INPUT {
            "Microphone"
        } else if object == OUTPUT {
            "Send"
        } else {
            DEPLOYMENT.device_name
        }));
    }
    if selector == hal_abi::fourcc(b"lmak") {
        return Ok(Value::Text("ChannelBridge"));
    }
    if selector == hal_abi::fourcc(b"ownd") {
        return Ok(Value::Objects(match object {
            PLUGIN => &[DEVICE],
            DEVICE if address.scope == IN => &[INPUT],
            DEVICE if address.scope == OUT => &[OUTPUT],
            DEVICE => &[INPUT, OUTPUT],
            _ => &[],
        }));
    }
    if object == PLUGIN {
        if address.scope != GLOBAL {
            return Err(UNKNOWN_PROPERTY);
        }
        return match selector {
            s if s == hal_abi::fourcc(b"piid") => Ok(Value::Text(DEPLOYMENT.driver_bundle_id)),
            s if s == hal_abi::fourcc(b"dev#") => Ok(Value::Objects(&[DEVICE])),
            s if [hal_abi::fourcc(b"box#"), hal_abi::fourcc(b"clk#")].contains(&s) => {
                Ok(Value::Objects(&[]))
            }
            s if s == hal_abi::fourcc(b"uidd") => Ok(Value::TranslateUid),
            s if s == hal_abi::fourcc(b"rsrc") => Ok(Value::Text("")),
            _ => Err(UNKNOWN_PROPERTY),
        };
    }
    if stream {
        return match selector {
            s if s == hal_abi::fourcc(b"sact") => Ok(Value::Integer(1)),
            s if s == hal_abi::fourcc(b"sdir") => Ok(Value::Integer(u32::from(object == INPUT))),
            s if s == hal_abi::fourcc(b"term") => {
                Ok(Value::Integer(hal_abi::fourcc(if object == INPUT {
                    b"micr"
                } else {
                    b"spkr"
                })))
            }
            s if s == hal_abi::fourcc(b"schn") => Ok(Value::Integer(1)),
            s if s == hal_abi::fourcc(b"ltnc") => Ok(Value::Integer(0)),
            s if [hal_abi::fourcc(b"sfmt"), hal_abi::fourcc(b"pft ")].contains(&s) => {
                Ok(Value::Format)
            }
            s if [hal_abi::fourcc(b"sfma"), hal_abi::fourcc(b"pfta")].contains(&s) => {
                Ok(Value::Formats)
            }
            _ => Err(UNKNOWN_PROPERTY),
        };
    }
    match selector {
        s if s == hal_abi::fourcc(b"uid ") => Ok(Value::Text(DEPLOYMENT.device_uid)),
        s if s == hal_abi::fourcc(b"muid") => Ok(Value::Text("ChannelBridge_Rust_2ch")),
        s if s == hal_abi::fourcc(b"tran") => Ok(Value::Integer(hal_abi::fourcc(b"virt"))),
        s if s == hal_abi::fourcc(b"akin") => Ok(Value::Objects(&[DEVICE])),
        s if s == hal_abi::fourcc(b"clkd") => Ok(Value::Integer(0)),
        s if s == hal_abi::fourcc(b"livn") || s == hal_abi::fourcc(b"cstb") => {
            Ok(Value::Integer(1))
        }
        s if s == hal_abi::fourcc(b"goin") => Ok(Value::Integer(state::running())),
        s if s == hal_abi::fourcc(b"dflt") => Ok(Value::Integer(u32::from(address.scope == IN))),
        s if [
            hal_abi::fourcc(b"sflt"),
            hal_abi::fourcc(b"hidn"),
            hal_abi::fourcc(b"resc"),
            hal_abi::fourcc(b"resf"),
        ]
        .contains(&s) =>
        {
            Ok(Value::Integer(0))
        }
        s if s == hal_abi::fourcc(b"ltnc") => {
            Ok(Value::Integer(if address.scope == IN { 512 } else { 0 }))
        }
        s if s == hal_abi::fourcc(b"saft") => Ok(Value::Integer(64)),
        s if s == hal_abi::fourcc(b"stm#") => Ok(Value::Objects(if address.scope == IN {
            &[INPUT]
        } else if address.scope == OUT {
            &[OUTPUT]
        } else {
            &[INPUT, OUTPUT]
        })),
        s if s == hal_abi::fourcc(b"ctrl") => Ok(Value::Objects(&[])),
        s if s == hal_abi::fourcc(b"nsrt") => Ok(Value::Number(f64::from(state::rate()))),
        s if s == hal_abi::fourcc(b"nsr#") => Ok(Value::Rates),
        s if s == hal_abi::fourcc(b"ring") => Ok(Value::Integer(super::ZERO_TIMESTAMP_PERIOD)),
        s if s == hal_abi::fourcc(b"clok") => Ok(Value::Integer(hal_abi::fourcc(b"raww"))),
        s if s == hal_abi::fourcc(b"dch2") => Ok(Value::Objects(&[1, 2])),
        s if s == hal_abi::fourcc(b"srnd") => Ok(Value::Layout),
        s if s == hal_abi::fourcc(b"capp") => Ok(Value::Text("")),
        s if s == hal_abi::fourcc(b"cbst") => Ok(Value::Diagnostics),
        s if s == channelbridge_contract::DRIVER_IDENTITY_SELECTOR => Ok(Value::Identity),
        _ => Err(UNKNOWN_PROPERTY),
    }
}

#[must_use]
fn settable(object: ObjectId, address: PropertyAddress) -> bool {
    (object == DEVICE && address.selector == hal_abi::fourcc(b"nsrt"))
        || ([INPUT, OUTPUT].contains(&object)
            && [hal_abi::fourcc(b"sfmt"), hal_abi::fourcc(b"pft ")].contains(&address.selector))
}

#[must_use]
fn format(rate: u32) -> StreamFormat {
    StreamFormat {
        sample_rate: f64::from(rate),
        format_id: hal_abi::fourcc(b"lpcm"),
        format_flags: 9,
        bytes_per_packet: 8,
        frames_per_packet: 1,
        bytes_per_frame: 8,
        channels_per_frame: 2,
        bits_per_channel: 32,
        reserved: 0,
    }
}

unsafe fn address(
    driver: DriverRef,
    pointer: *const PropertyAddress,
) -> Result<PropertyAddress, i32> {
    if !host_boundary::valid(driver) || pointer.is_null() {
        return Err(BAD_OBJECT);
    }
    // SAFETY: HAL guarantees readable C ABI property addresses for this call.
    Ok(unsafe { pointer.read_unaligned() })
}

pub(super) unsafe extern "C" fn has_property(
    driver: DriverRef,
    object: ObjectId,
    _pid: i32,
    pointer: *const PropertyAddress,
) -> u8 {
    let mut found = false;
    let _ = host_boundary::guard(|| {
        // SAFETY: HAL supplies a readable property address for this callback; address rejects null.
        found = unsafe { address(driver, pointer) }
            .and_then(|a| describe(object, a))
            .is_ok();
        0
    });
    u8::from(found)
}
pub(super) unsafe extern "C" fn is_settable(
    driver: DriverRef,
    object: ObjectId,
    _pid: i32,
    pointer: *const PropertyAddress,
    output: *mut u8,
) -> i32 {
    host_boundary::guard(|| {
        if output.is_null() {
            return BAD_SIZE;
        }
        // SAFETY: HAL supplies a readable property address for this callback; address rejects null.
        let a = match unsafe { address(driver, pointer) } {
            Ok(a) => a,
            Err(error) => return error,
        };
        if let Err(error) = describe(object, a) {
            return error;
        }
        // SAFETY: HAL grants a writable Boolean output slot for the duration of this callback.
        unsafe { output.write(u8::from(settable(object, a))) };
        0
    })
}
pub(super) unsafe extern "C" fn data_size(
    driver: DriverRef,
    object: ObjectId,
    _pid: i32,
    pointer: *const PropertyAddress,
    _qualifier_size: u32,
    _qualifier: *const c_void,
    output: *mut u32,
) -> i32 {
    host_boundary::guard(|| {
        if output.is_null() {
            return BAD_SIZE;
        }
        // SAFETY: HAL supplies a readable property address for this callback; address rejects null.
        let a = match unsafe { address(driver, pointer) } {
            Ok(a) => a,
            Err(error) => return error,
        };
        match describe(object, a) {
            Ok(value) => {
                // SAFETY: HAL grants a writable UInt32 output slot for this callback.
                unsafe { output.write(value.size()) };
                0
            }
            Err(error) => error,
        }
    })
}

/// Caller has already checked the exact available size. Only explicit initialized
/// fields are serialized, never struct padding or an internal Rust enum layout.
#[must_use]
fn bytes(value: Value) -> Vec<u8> {
    let mut result = Vec::new();
    match value {
        Value::Integer(v) => result.extend(v.to_ne_bytes()),
        Value::Number(v) => result.extend(v.to_ne_bytes()),
        Value::Objects(values) => {
            for v in values {
                result.extend(v.to_ne_bytes());
            }
        }
        Value::Rates => {
            for rate in SUPPORTED_RATES {
                result.extend(f64::from(*rate).to_ne_bytes());
                result.extend(f64::from(*rate).to_ne_bytes());
            }
        }
        Value::Layout => {
            for v in [101_u32 << 16 | 2, 0, 0] {
                result.extend(v.to_ne_bytes());
            }
        }
        Value::Format => append_format(&mut result, state::rate()),
        Value::Formats => {
            for rate in SUPPORTED_RATES {
                append_format(&mut result, *rate);
                result.extend(f64::from(*rate).to_ne_bytes());
                result.extend(f64::from(*rate).to_ne_bytes());
            }
        }
        Value::Diagnostics => {
            result.extend(channelbridge_contract::PROTOCOL_VERSION.to_ne_bytes());
            result.extend(0_u32.to_ne_bytes());
            let counters = state::runtime()
                .map(|r| r.audio.counters())
                .unwrap_or_default();
            for v in [
                counters.generation,
                counters.written_frames,
                counters.missing_read_frames,
                counters.rejected_write_frames,
                counters.invalid_timestamps,
            ] {
                result.extend(v.to_ne_bytes());
            }
        }
        Value::CustomProperties => {
            // AudioServerPlugInCustomPropertyInfo: CFData property list / CFString, no qualifier.
            // The host needs this descriptor to marshal our diagnostic property.
            for field in [
                hal_abi::fourcc(b"cbst"),
                hal_abi::fourcc(b"plst"),
                0,
                channelbridge_contract::DRIVER_IDENTITY_SELECTOR,
                hal_abi::fourcc(b"cfst"),
                0,
            ] {
                result.extend(field.to_ne_bytes());
            }
        }
        Value::Identity => result.extend_from_slice(env!("CARGO_PKG_VERSION").as_bytes()),
        Value::Text(_) | Value::TranslateUid => {}
    }
    result
}
fn append_format(result: &mut Vec<u8>, rate: u32) {
    let f = format(rate);
    result.extend(f.sample_rate.to_ne_bytes());
    for v in [
        f.format_id,
        f.format_flags,
        f.bytes_per_packet,
        f.frames_per_packet,
        f.bytes_per_frame,
        f.channels_per_frame,
        f.bits_per_channel,
        f.reserved,
    ] {
        result.extend(v.to_ne_bytes());
    }
}

pub(super) unsafe extern "C" fn get_data(
    driver: DriverRef,
    object: ObjectId,
    _pid: i32,
    pointer: *const PropertyAddress,
    qualifier_size: u32,
    qualifier: *const c_void,
    capacity: u32,
    output_size: *mut u32,
    output: *mut c_void,
) -> i32 {
    host_boundary::guard(|| {
        if output_size.is_null() {
            return BAD_SIZE;
        }
        // SAFETY: HAL grants a writable UInt32 output-size slot for this callback.
        unsafe { output_size.write(0) };
        // SAFETY: HAL supplies a readable property address for this callback; address rejects null.
        let a = match unsafe { address(driver, pointer) } {
            Ok(a) => a,
            Err(error) => return error,
        };
        let value = match describe(object, a) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let size = value.size();
        if capacity < size || (output.is_null() && size != 0) {
            return BAD_SIZE;
        }
        match value {
            Value::Diagnostics => {
                let Some(value) = system::data(&bytes(Value::Diagnostics)) else {
                    return UNSPECIFIED;
                };
                // SAFETY: checked pointer-sized output; caller owns the +1 CFData.
                unsafe { output.cast::<CfRef>().write_unaligned(value) };
            }
            Value::Identity => {
                let Some(value) = system::string(env!("CARGO_PKG_VERSION")) else {
                    return UNSPECIFIED;
                };
                // SAFETY: checked pointer-sized output; caller owns the +1 CFString.
                unsafe { output.cast::<CfRef>().write_unaligned(value) };
            }
            Value::Text(text) => {
                let Some(value) = system::string(text) else {
                    return UNSPECIFIED;
                };
                // SAFETY: output was checked to hold one CF reference. The +1
                // retain transfers to the HAL property caller.
                unsafe { output.cast::<CfRef>().write_unaligned(value) };
            }
            Value::TranslateUid => {
                if qualifier_size as usize != mem::size_of::<CfRef>() || qualifier.is_null() {
                    return BAD_SIZE;
                }
                // SAFETY: HAL keeps the qualifier allocation readable for
                // qualifier_size bytes; the CFRef size was checked.
                let uid = unsafe { qualifier.cast::<CfRef>().read_unaligned() };
                // SAFETY: The UID qualifier is a borrowed CF object kept alive
                // by HAL for this property callback.
                let device = if unsafe { system::string_equals(uid, DEPLOYMENT.device_uid) } {
                    DEVICE
                } else {
                    0
                };
                // SAFETY: HAL grants capacity writable bytes; size and non-null
                // output were checked above.
                unsafe { output.cast::<u32>().write_unaligned(device) };
            }
            other => {
                let data = bytes(other);
                if data.len() != size as usize {
                    return UNSPECIFIED;
                }
                if !data.is_empty() {
                    // SAFETY: HAL grants capacity writable bytes disjoint
                    // from the newly allocated serialization buffer.
                    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), output.cast(), data.len()) };
                }
            }
        }
        // SAFETY: HAL grants a writable UInt32 output-size slot for this callback.
        unsafe { output_size.write(size) };
        0
    })
}

pub(super) unsafe extern "C" fn set_data(
    driver: DriverRef,
    object: ObjectId,
    _pid: i32,
    pointer: *const PropertyAddress,
    _qualifier_size: u32,
    _qualifier: *const c_void,
    size: u32,
    input: *const c_void,
) -> i32 {
    host_boundary::guard(|| {
        // SAFETY: HAL supplies a readable property address for this callback; address rejects null.
        let a = match unsafe { address(driver, pointer) } {
            Ok(a) => a,
            Err(error) => return error,
        };
        if let Err(error) = describe(object, a) {
            return error;
        }
        if !settable(object, a) {
            return ILLEGAL;
        }
        if input.is_null() {
            return BAD_SIZE;
        }
        let rate = if object == DEVICE {
            if size != 8 {
                return BAD_SIZE;
            }
            // SAFETY: HAL keeps input readable for size bytes; the f64 size was checked above.
            unsafe { input.cast::<f64>().read_unaligned() }
        } else {
            if size as usize != mem::size_of::<StreamFormat>() {
                return BAD_SIZE;
            }
            // SAFETY: HAL keeps input readable for size bytes; the StreamFormat size
            // was checked above.
            let value = unsafe { input.cast::<StreamFormat>().read_unaligned() };
            let canonical = format(state::rate());
            if value.format_id != canonical.format_id
                || value.format_flags != canonical.format_flags
                || value.bytes_per_packet != 8
                || value.frames_per_packet != 1
                || value.bytes_per_frame != 8
                || value.channels_per_frame != 2
                || value.bits_per_channel != 32
                || value.reserved != 0
            {
                return ILLEGAL;
            }
            value.sample_rate
        };
        if !rate.is_finite() || rate.fract() != 0.0 || rate < 0.0 || rate > f64::from(u32::MAX) {
            return ILLEGAL;
        }
        state::request_rate(rate as u32)
    })
}
