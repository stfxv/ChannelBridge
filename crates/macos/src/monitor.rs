//! Read-only acceptance observer for the separate Rust test microphone.
//! Output streams are disabled; samples are reduced to counters and never saved.
use crate::{
    buffers::{CaptureLayout, InputBuffers},
    devices,
    io::{Direction, Handler, Registration},
};
use channelbridge_hal_abi::{AudioBufferList, AudioTimestamp, fourcc};
use std::{
    error::Error,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::Duration,
};

#[derive(Debug)]
pub struct Report {
    pub callbacks: u64,
    pub frames: u64,
    pub nonzero_frames: [u64; 2],
    pub peak: [f32; 2],
    pub invalid_callbacks: u64,
    pub invalid_samples: u64,
}
impl Report {
    pub fn valid(&self) -> bool {
        self.callbacks >= 10
            && self.frames >= 4096
            && self.invalid_callbacks == 0
            && self.invalid_samples == 0
    }
}
struct Observer {
    layout: CaptureLayout,
    callbacks: AtomicU64,
    frames: AtomicU64,
    nonzero: [AtomicU64; 2],
    peak: [AtomicU32; 2],
    invalid_callbacks: AtomicU64,
    invalid_samples: AtomicU64,
}
impl Observer {
    fn new(layout: CaptureLayout) -> Self {
        Self {
            layout,
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            nonzero: [AtomicU64::new(0), AtomicU64::new(0)],
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            invalid_callbacks: AtomicU64::new(0),
            invalid_samples: AtomicU64::new(0),
        }
    }
    fn observe(&self, input: &InputBuffers<'_>) {
        let mut nonzero = [0; 2];
        let mut peak = [0_f32; 2];
        let mut invalid = 0;
        for frame in 0..input.frames() {
            for channel in 0..2 {
                match input.sample(frame, channel) {
                    Some(value) if value.is_finite() => {
                        nonzero[channel] += u64::from(value != 0.0);
                        peak[channel] = peak[channel].max(value.abs());
                    }
                    _ => invalid += 1,
                }
            }
        }
        for channel in 0..2 {
            self.nonzero[channel].fetch_add(nonzero[channel], Ordering::Relaxed);
            // Finite nonnegative IEEE float bits have the same order as u32.
            self.peak[channel].fetch_max(peak[channel].to_bits(), Ordering::Relaxed);
        }
        self.invalid_samples.fetch_add(invalid, Ordering::Relaxed);
        self.frames
            .fetch_add(input.frames() as u64, Ordering::Relaxed);
        self.callbacks.fetch_add(1, Ordering::Relaxed);
    }
    fn report(&self) -> Report {
        Report {
            callbacks: self.callbacks.load(Ordering::Acquire),
            frames: self.frames.load(Ordering::Acquire),
            nonzero_frames: self
                .nonzero
                .each_ref()
                .map(|value| value.load(Ordering::Acquire)),
            peak: self
                .peak
                .each_ref()
                .map(|value| f32::from_bits(value.load(Ordering::Acquire))),
            invalid_callbacks: self.invalid_callbacks.load(Ordering::Acquire),
            invalid_samples: self.invalid_samples.load(Ordering::Acquire),
        }
    }
}
impl Handler for Observer {
    unsafe fn process(
        &self,
        _now: *const AudioTimestamp,
        input: *const AudioBufferList,
        _input_time: *const AudioTimestamp,
        _output: *mut AudioBufferList,
        _output_time: *const AudioTimestamp,
    ) {
        // SAFETY: HAL owns the descriptors and readable samples for this call;
        // the prepared layout is immutable and no borrow escapes the callback.
        match unsafe { InputBuffers::from_raw(input, &self.layout) } {
            Ok(input) => self.observe(&input),
            Err(_) => {
                self.invalid_callbacks.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Observe the test UID without changing its rate, writing audio or selecting it
/// as the default microphone. Requires existing OS permission; never requests it.
pub fn run(duration: Duration) -> Result<Report, Box<dyn Error>> {
    if !(Duration::from_secs(1)..=Duration::from_secs(28_800)).contains(&duration) {
        return Err("monitor duration must be between 1 and 28800 seconds".into());
    }
    if !crate::permission::status().allowed() {
        return Err("microphone permission is not authorized".into());
    }
    let inventory = devices::list()?;
    let mut matches = inventory
        .iter()
        .filter_map(|entry| entry.as_ref().ok())
        .filter(|device| device.uid == channelbridge_contract::TEST_DEVICE_UID);
    let device = matches.next().ok_or("Rust test driver is not loaded")?;
    if matches.next().is_some() {
        return Err("duplicate Rust test UID".into());
    }
    let formats = devices::streams(device.id, fourcc(b"inpt"))?
        .into_iter()
        .map(|stream| stream.virtual_format)
        .collect::<Vec<_>>();
    let layout = CaptureLayout::try_new(&formats)?;
    if layout.channels() != 2 {
        return Err("test microphone must be stereo".into());
    }
    let mut registration = Registration::try_new(device.id, Observer::new(layout))?;
    registration.disable_streams(Direction::Output)?;
    let started = registration.start();
    if started.is_ok() {
        std::thread::sleep(duration);
    }
    let observer = registration.close()?;
    started?;
    Ok(observer.report())
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelbridge_hal_abi::{AudioBuffer, StreamFormat};
    #[test]
    fn observer_distinguishes_channels_silence_invalid_samples_and_headers() {
        let format = StreamFormat {
            sample_rate: 48000.0,
            format_id: fourcc(b"lpcm"),
            format_flags: 9,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };
        let observer = Observer::new(CaptureLayout::try_new(&[format]).expect("fixture is float32 PCM with a positive sample rate and channel count"));
        let mut samples = [0.0_f32, -0.0, -0.25, 0.5, f32::NAN, f32::INFINITY];
        let list = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 24,
                data: samples.as_mut_ptr().cast(),
            }],
        };
        // SAFETY: live matching stereo samples and complete list for this call.
        unsafe {
            observer.process(
                std::ptr::null(),
                &list,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
        }
        let report = observer.report();
        assert_eq!(report.frames, 3);
        assert_eq!(report.nonzero_frames, [1, 1]);
        assert_eq!(report.peak, [0.25, 0.5]);
        assert_eq!(report.invalid_samples, 2);
        assert!(!report.valid());
        let empty = 0_u32;
        // SAFETY: the adapter must reject a readable header with zero buffers.
        unsafe {
            observer.process(
                std::ptr::null(),
                (&empty as *const u32).cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
        }
        assert_eq!(observer.report().invalid_callbacks, 1);
        assert_eq!(observer.report().frames, 3);
    }
}
