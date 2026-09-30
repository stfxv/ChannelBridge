//! Synthetic I/O acceptance probe, restricted to the separate Rust test UID.
//! No physical input, recording, network or default-device modification.
use crate::devices::{self, Error};
use channelbridge_contract::TEST_DEVICE_UID;
use channelbridge_hal_abi::{AudioBufferList, AudioTimestamp, ObjectId, StreamFormat, fourcc};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

type IoProc = unsafe extern "C" fn(
    ObjectId,
    *const AudioTimestamp,
    *const AudioBufferList,
    *const AudioTimestamp,
    *mut AudioBufferList,
    *const AudioTimestamp,
    *mut c_void,
) -> i32;
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioDeviceCreateIOProcID(
        device: ObjectId,
        callback: IoProc,
        context: *mut c_void,
        id: *mut *mut c_void,
    ) -> i32;
    fn AudioDeviceDestroyIOProcID(device: ObjectId, id: *mut c_void) -> i32;
    fn AudioDeviceStart(device: ObjectId, id: *mut c_void) -> i32;
    fn AudioDeviceStop(device: ObjectId, id: *mut c_void) -> i32;
}

#[derive(Debug)]
pub struct Report {
    pub callbacks: u64,
    pub compared_frames: u64,
    pub silent_frames: u64,
    pub missing_after_startup: u64,
    pub mismatched_frames: u64,
    pub invalid_callbacks: u64,
}
impl Report {
    pub fn passed(&self) -> bool {
        self.callbacks >= 10
            && self.compared_frames >= 4096
            && self.mismatched_frames == 0
            && self.invalid_callbacks == 0
            && self.missing_after_startup == 0
    }
}
struct Context {
    rate: f64,
    callbacks: AtomicU64,
    compared: AtomicU64,
    silent: AtomicU64,
    missing: AtomicU64,
    mismatched: AtomicU64,
    invalid: AtomicU64,
}
impl Context {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            callbacks: AtomicU64::new(0),
            compared: AtomicU64::new(0),
            silent: AtomicU64::new(0),
            missing: AtomicU64::new(0),
            mismatched: AtomicU64::new(0),
            invalid: AtomicU64::new(0),
        }
    }
}

fn tone(frame: f64, rate: f64) -> [f32; 2] {
    let phase = frame * (997.0 * std::f64::consts::TAU / rate);
    [(0.25 * phase.sin()) as f32, (0.125 * phase.cos()) as f32]
}

unsafe extern "C" fn callback(
    _device: ObjectId,
    _now: *const AudioTimestamp,
    input: *const AudioBufferList,
    input_time: *const AudioTimestamp,
    output: *mut AudioBufferList,
    output_time: *const AudioTimestamp,
    context: *mut c_void,
) -> i32 {
    if context.is_null() {
        return fourcc(b"nope") as i32;
    }
    // SAFETY: Context remains immutable and alive until HAL stops this IOProc.
    let context = unsafe { &*context.cast::<Context>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: HAL supplies callback buffers/timestamps for this invocation.
        unsafe { process(context, input, input_time, output, output_time) }
    }));
    match result {
        Ok(true) => 0,
        _ => {
            context.invalid.fetch_add(1, Ordering::Relaxed);
            fourcc(b"nope") as i32
        }
    }
}

unsafe fn process(
    context: &Context,
    input: *const AudioBufferList,
    input_time: *const AudioTimestamp,
    output: *mut AudioBufferList,
    output_time: *const AudioTimestamp,
) -> bool {
    if input.is_null() || output.is_null() || input_time.is_null() || output_time.is_null() {
        return false;
    }
    // SAFETY: pointers are supplied by HAL; only the first buffer is used after
    // checking this test device's single interleaved stereo layout.
    let (input, output, input_time, output_time) =
        unsafe { (&*input, &mut *output, &*input_time, &*output_time) };
    if input.count != 1 || output.count != 1 {
        return false;
    }
    let input = &input.buffers[0];
    let output = &mut output.buffers[0];
    for buffer in [input, &*output] {
        if buffer.channels != 2
            || buffer.byte_size % 8 != 0
            || buffer.byte_size > 32768
            || buffer.data.is_null()
            || (buffer.data as usize) % align_of::<f32>() != 0
        {
            return false;
        }
    }
    if input_time.flags & 1 == 0
        || output_time.flags & 1 == 0
        || !input_time.sample_time.is_finite()
        || !output_time.sample_time.is_finite()
        || input_time.sample_time < 0.0
        || output_time.sample_time < 0.0
    {
        return false;
    }
    let input_start = input.data as usize;
    let output_start = output.data as usize;
    let Some(input_end) = input_start.checked_add(input.byte_size as usize) else {
        return false;
    };
    let Some(output_end) = output_start.checked_add(output.byte_size as usize) else {
        return false;
    };
    if input_start < output_end && output_start < input_end {
        // This probe requires separate input/output storage; never create
        // overlapping shared and mutable Rust slices for an in-place buffer.
        return false;
    }
    // SAFETY: validated sizes/format; HAL grants read-only input and unique
    // writable output storage for the duration of this callback.
    let incoming = unsafe {
        std::slice::from_raw_parts(input.data.cast::<f32>(), input.byte_size as usize / 4)
    };
    let outgoing = unsafe {
        std::slice::from_raw_parts_mut(output.data.cast::<f32>(), output.byte_size as usize / 4)
    };
    let mut silent = 0;
    let mut compared = 0;
    let mut mismatched = 0;
    let mut missing = 0;
    let mut received = context.compared.load(Ordering::Relaxed) != 0;
    for (index, frame) in incoming.chunks_exact(2).enumerate() {
        if frame[0] == 0.0 && frame[1] == 0.0 {
            silent += 1;
            if received {
                missing += 1;
            }
            continue;
        }
        compared += 1;
        received = true;
        let expected = tone(input_time.sample_time.floor() + index as f64, context.rate);
        if !frame[0].is_finite()
            || !frame[1].is_finite()
            || (frame[0] - expected[0]).abs() > 0.00001
            || (frame[1] - expected[1]).abs() > 0.00001
        {
            mismatched += 1;
        }
    }
    for (index, frame) in outgoing.chunks_exact_mut(2).enumerate() {
        frame.copy_from_slice(&tone(
            output_time.sample_time.floor() + index as f64,
            context.rate,
        ));
    }
    context.callbacks.fetch_add(1, Ordering::Relaxed);
    context.compared.fetch_add(compared, Ordering::Relaxed);
    context.silent.fetch_add(silent, Ordering::Relaxed);
    context.missing.fetch_add(missing, Ordering::Relaxed);
    context.mismatched.fetch_add(mismatched, Ordering::Relaxed);
    true
}

fn verify_format(device: ObjectId, scope: u32) -> Result<(), Error> {
    let streams = devices::property(device, fourcc(b"stm#"), scope)?;
    let stream: [u8; 4] = streams.try_into().map_err(|_| Error::MalformedProperty)?;
    let format = devices::property(u32::from_ne_bytes(stream), fourcc(b"sfmt"), fourcc(b"glob"))?;
    if format.len() != size_of::<StreamFormat>() {
        return Err(Error::MalformedProperty);
    }
    // SAFETY: every bit pattern is valid for StreamFormat's integer/float fields.
    let format = unsafe { format.as_ptr().cast::<StreamFormat>().read_unaligned() };
    if format.format_id != fourcc(b"lpcm")
        || format.format_flags != 9
        || format.channels_per_frame != 2
        || format.bits_per_channel != 32
        || format.bytes_per_frame != 8
        || format.bytes_per_packet != 8
        || format.frames_per_packet != 1
    {
        return Err(Error::MalformedProperty);
    }
    Ok(())
}

/// Capture synthetic samples only from the separate development device.
pub fn run(duration: Duration) -> Result<Report, Box<dyn std::error::Error>> {
    if duration < Duration::from_secs(1) || duration > Duration::from_secs(30) {
        return Err("probe duration must be between 1 and 30 seconds".into());
    }
    let mut selected = None;
    for device in devices::list()?.into_iter().flatten() {
        if device.uid == TEST_DEVICE_UID {
            if selected.is_some() {
                return Err("duplicate Rust test UID".into());
            }
            selected = Some(device);
        }
    }
    let device = selected.ok_or("Rust test driver is not loaded")?;
    verify_format(device.id, fourcc(b"inpt"))?;
    verify_format(device.id, fourcc(b"outp"))?;
    let context = Box::new(Context::new(device.rate));
    let pointer = Box::into_raw(context);
    let mut id = std::ptr::null_mut();
    // SAFETY: context is heap-stable, callback matches the Apple SDK declaration.
    let status = unsafe { AudioDeviceCreateIOProcID(device.id, callback, pointer.cast(), &mut id) };
    if status != 0 {
        // SAFETY: failed registration cannot invoke this callback.
        drop(unsafe { Box::from_raw(pointer) });
        return Err(Error::Hal(status).into());
    }
    // SAFETY: id is a successfully registered IOProc for this device.
    let status = unsafe { AudioDeviceStart(device.id, id) };
    if status == 0 {
        std::thread::sleep(duration);
    }
    // Stop even after a failed Start to ensure any partial startup is quiesced.
    // SAFETY: same registered device/id; called on the control thread.
    let stop = unsafe { AudioDeviceStop(device.id, id) };
    if stop != 0 {
        // Deliberately retain callback storage if HAL cannot confirm stopping.
        // Freeing it could turn an OS error into a callback use-after-free.
        return Err(
            format!("HAL stop failed ({stop}); callback context retained for safety").into(),
        );
    }
    // SAFETY: Stop has returned successfully; HAL no longer invokes this IOProc.
    let destroy = unsafe { AudioDeviceDestroyIOProcID(device.id, id) };
    if destroy != 0 {
        return Err(Error::Hal(destroy).into());
    }
    // SAFETY: callback deregistered and context has exactly one owner.
    let context = unsafe { Box::from_raw(pointer) };
    if status != 0 {
        return Err(Error::Hal(status).into());
    }
    Ok(Report {
        callbacks: context.callbacks.load(Ordering::Acquire),
        compared_frames: context.compared.load(Ordering::Acquire),
        silent_frames: context.silent.load(Ordering::Acquire),
        missing_after_startup: context.missing.load(Ordering::Acquire),
        mismatched_frames: context.mismatched.load(Ordering::Acquire),
        invalid_callbacks: context.invalid.load(Ordering::Acquire),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelbridge_hal_abi::AudioBuffer;
    #[test]
    fn probe_detects_corruption_and_dropouts_after_audio_starts() {
        let context = Context::new(48_000.0);
        let time = AudioTimestamp {
            sample_time: 128.5,
            flags: 1,
            ..Default::default()
        };
        let mut incoming = [0_f32; 128];
        let mut outgoing = [0_f32; 128];
        for (index, frame) in incoming.chunks_exact_mut(2).enumerate() {
            frame.copy_from_slice(&tone(128.0 + index as f64, context.rate));
        }
        let input = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 512,
                data: incoming.as_mut_ptr().cast(),
            }],
        };
        let mut output = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 512,
                data: outgoing.as_mut_ptr().cast(),
            }],
        };
        // SAFETY: disjoint arrays, valid stereo buffers and live timestamps.
        assert!(unsafe { process(&context, &input, &time, &mut output, &time) });
        assert_eq!(context.compared.load(Ordering::Relaxed), 64);
        assert_eq!(context.mismatched.load(Ordering::Relaxed), 0);
        assert_eq!(incoming, outgoing);
        incoming[10] = 0.9;
        incoming[20] = 0.0;
        incoming[21] = 0.0;
        let input = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 512,
                data: incoming.as_mut_ptr().cast(),
            }],
        };
        // SAFETY: same valid storage; only test sample values changed.
        assert!(unsafe { process(&context, &input, &time, &mut output, &time) });
        assert_eq!(context.mismatched.load(Ordering::Relaxed), 1);
        assert_eq!(context.missing.load(Ordering::Relaxed), 1);
        output.buffers[0].byte_size = 511;
        // SAFETY: readable list; malformed size must be rejected before access.
        assert!(!unsafe { process(&context, &input, &time, &mut output, &time) });
    }
}
