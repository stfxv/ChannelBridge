//! Synthetic I/O acceptance probe, restricted to the separate Rust test UID.
//! No physical input, recording, network or default-device modification.
use crate::devices::{self, Error};
use channelbridge_contract::TEST_DEVICE_UID;
use channelbridge_hal_abi::{AudioBufferList, AudioTimestamp, ObjectId, StreamFormat, fourcc};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Debug)]
pub struct Report {
    pub callbacks: u64,
    pub compared_frames: u64,
    pub silent_frames: u64,
    pub missing_after_startup: u64,
    pub mismatched_frames: u64,
    pub invalid_callbacks: u64,
    pub first_missing_input_sample: Option<f64>,
}
impl Report {
    #[must_use]
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
    first_missing: AtomicU64,
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
            first_missing: AtomicU64::new(u64::MAX),
        }
    }
}

fn tone(frame: f64, rate: f64) -> [f32; 2] {
    let phase = frame * (997.0 * std::f64::consts::TAU / rate);
    [(0.25 * phase.sin()) as f32, (0.125 * phase.cos()) as f32]
}

impl crate::io::Handler for Context {
    unsafe fn process(
        &self,
        _now: *const AudioTimestamp,
        input: *const AudioBufferList,
        input_time: *const AudioTimestamp,
        output: *mut AudioBufferList,
        output_time: *const AudioTimestamp,
    ) {
        // SAFETY: Handler forwards HAL callback borrows without retaining them after this call.
        if !unsafe { process(self, input, input_time, output, output_time) } {
            self.invalid.fetch_add(1, Ordering::Relaxed);
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
    // An empty C buffer-list header is smaller than the Rust one-buffer type.
    // Check the count before constructing a reference to the complete structure.
    // SAFETY: HAL supplies live buffer-list headers for this invocation; this reads only the count.
    if unsafe { input.cast::<u32>().read_unaligned() } != 1
        // SAFETY: HAL supplies a live output header for this invocation; this reads only the count.
        || unsafe { output.cast::<u32>().read_unaligned() } != 1
    {
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
    // SAFETY: HAL grants exclusive output storage; the checked range is disjoint from
    // incoming and has f32 alignment.
    let outgoing = unsafe {
        std::slice::from_raw_parts_mut(output.data.cast::<f32>(), output.byte_size as usize / 4)
    };
    let mut silent = 0;
    let mut compared = 0;
    let mut mismatched = 0;
    let mut missing = 0;
    let mut received = context.compared.load(Ordering::Relaxed) != 0;
    let input_frames = incoming.chunks_exact(2).enumerate();
    for (index, frame) in input_frames {
        if frame[0] == 0.0 && frame[1] == 0.0 {
            silent += 1;
            if received {
                missing += 1;
                let _ = context.first_missing.compare_exchange(
                    u64::MAX,
                    (input_time.sample_time.floor() + index as f64).to_bits(),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
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
    let output_frames = outgoing.chunks_exact_mut(2).enumerate();
    for (index, frame) in output_frames {
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
    let available_devices = devices::list()?.into_iter().flatten();
    for device in available_devices {
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
    let mut registration = crate::io::Registration::try_new(device.id, Context::new(device.rate))?;
    let started = registration.start();
    if started.is_ok() {
        std::thread::sleep(duration);
    }
    let context = registration.close()?;
    started?;
    Ok(Report {
        callbacks: context.callbacks.load(Ordering::Acquire),
        compared_frames: context.compared.load(Ordering::Acquire),
        silent_frames: context.silent.load(Ordering::Acquire),
        missing_after_startup: context.missing.load(Ordering::Acquire),
        mismatched_frames: context.mismatched.load(Ordering::Acquire),
        invalid_callbacks: context.invalid.load(Ordering::Acquire),
        first_missing_input_sample: match context.first_missing.load(Ordering::Acquire) {
            u64::MAX => None,
            bits => Some(f64::from_bits(bits)),
        },
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
        let input_frames = incoming.chunks_exact_mut(2).enumerate();
        for (index, frame) in input_frames {
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
