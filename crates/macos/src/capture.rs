//! Native input callback adapter. Registration and device lifecycle are separate.
use crate::buffers::{CaptureLayout, InputBuffers};
use channelbridge_audio::{
    FramePosition,
    capture::{CaptureProcessor, CaptureTimestamp},
};
use channelbridge_hal_abi::{AudioBufferList, AudioTimestamp};
use std::{
    cell::UnsafeCell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Ownership is transferred here before IO starts. Control code may read only
/// atomics; it cannot access the processor while callbacks can run.
pub struct CaptureCallback {
    layout: CaptureLayout,
    processor: UnsafeCell<CaptureProcessor>,
    busy: AtomicBool,
    failed: AtomicBool,
    callbacks: AtomicU64,
    rejected: AtomicU64,
    written: AtomicU64,
    overflow: AtomicU64,
}
// SAFETY: CaptureProcessor is Send. The only access to its UnsafeCell requires
// the exclusive atomic gate. All other shared fields are immutable or atomic.
unsafe impl Sync for CaptureCallback {}

#[derive(Clone, Copy, Debug)]
pub struct CaptureStats {
    pub callbacks: u64,
    pub rejected: u64,
    pub written: u64,
    pub overflow: u64,
    pub failed: bool,
}
struct Gate<'a>(&'a AtomicBool);
impl Drop for Gate<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl CaptureCallback {
    pub fn new(layout: CaptureLayout, processor: CaptureProcessor) -> Self {
        Self {
            layout,
            processor: UnsafeCell::new(processor),
            busy: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            written: AtomicU64::new(0),
            overflow: AtomicU64::new(0),
        }
    }
    pub fn stats(&self) -> CaptureStats {
        CaptureStats {
            callbacks: self.callbacks.load(Ordering::Acquire),
            rejected: self.rejected.load(Ordering::Acquire),
            written: self.written.load(Ordering::Acquire),
            overflow: self.overflow.load(Ordering::Acquire),
            failed: self.failed.load(Ordering::Acquire),
        }
    }
    /// # Safety
    /// All nonnull pointers reference complete live HAL descriptors/timestamps;
    /// input payloads are immutable native float32 storage for this invocation.
    /// The owner keeps this context alive until the IOProc is deregistered.
    pub unsafe fn process(
        &self,
        input: *const AudioBufferList,
        now: *const AudioTimestamp,
        input_time: *const AudioTimestamp,
    ) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        if self.failed.load(Ordering::Acquire)
            || self
                .busy
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _gate = Gate(&self.busy);
        let result = catch_unwind(AssertUnwindSafe(|| {
            if now.is_null() {
                return false;
            }
            // SAFETY: callback caller supplies live timestamps and buffers.
            let now = unsafe { now.read_unaligned() };
            if now.flags & 2 == 0 {
                return false;
            }
            let Ok(buffers) = (unsafe { InputBuffers::from_raw(input, &self.layout) }) else {
                return false;
            };
            let timestamp = if input_time.is_null() {
                None
            } else {
                let time = unsafe { input_time.read_unaligned() };
                if time.flags & 3 != 3 {
                    None
                } else {
                    FramePosition::from_sample_time(time.sample_time).map(|frame| {
                        CaptureTimestamp {
                            frame: frame.get(),
                            host_tick: time.host_time,
                        }
                    })
                }
            };
            // SAFETY: the gate grants this callback exclusive processor access;
            // control code never obtains a reference to processor.
            let processor = unsafe { &mut *self.processor.get() };
            let report = processor.process(
                buffers.frames(),
                buffers.channels(),
                now.host_time,
                timestamp,
                |frame, channel| buffers.sample(frame, channel).unwrap_or(0.0),
            );
            self.written
                .fetch_add(report.written as u64, Ordering::Relaxed);
            self.overflow
                .fetch_add(report.overflow as u64, Ordering::Relaxed);
            !report.format_changed && !report.invalid_clock && !report.unavailable_channel
        }));
        match result {
            Ok(true) => {}
            Ok(false) => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.failed.store(true, Ordering::Release);
                self.rejected.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelbridge_audio::{
        StereoFrame,
        fifo::stereo_queue,
        meters::meter_bank,
        routing::{AtomicRouting, Mode, Selection},
    };
    use channelbridge_hal_abi::{AudioBuffer, StreamFormat, fourcc};
    use std::sync::Arc;
    #[test]
    fn native_buffers_reach_preview_and_fifo_without_reentrant_state_access() {
        let selection = Selection::new(2, 1, Mode::Stereo, false).unwrap();
        let routing = Arc::new(AtomicRouting::new(selection));
        let (meters, reader) = meter_bank(2, 48_000).unwrap();
        let (producer, mut consumer) = stereo_queue(1024).unwrap();
        let processor =
            CaptureProcessor::new(Arc::clone(&routing), meters, producer, 48_000, 48_000).unwrap();
        let layout = CaptureLayout::new(&[StreamFormat {
            sample_rate: 48_000.0,
            format_id: fourcc(b"lpcm"),
            format_flags: 9,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        }])
        .unwrap();
        let callback = CaptureCallback::new(layout, processor);
        let mut samples = [[0.25_f32, -0.5]; 64];
        let input = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 512,
                data: samples.as_mut_ptr().cast(),
            }],
        };
        let now = AudioTimestamp {
            host_time: 64,
            flags: 2,
            ..Default::default()
        };
        let mut stamp = AudioTimestamp {
            host_time: 0,
            sample_time: 0.0,
            flags: 3,
            ..Default::default()
        };
        // SAFETY: live immutable float32 payload, matching descriptors/timestamps.
        unsafe {
            callback.process(&input, &now, &stamp);
        }
        assert_eq!(reader.level(1, 64).current, 0.5);
        assert_eq!(consumer.available(), 0);
        let generation = routing
            .configure(selection, true, false)
            .unwrap()
            .generation;
        stamp.sample_time = 64.75;
        stamp.host_time = 64;
        unsafe {
            callback.process(&input, &now, &stamp);
        }
        let mut received = [StereoFrame::SILENCE; 64];
        assert_eq!(
            consumer.pop_generation(&mut received, generation).received,
            64
        );
        assert!(
            received
                .iter()
                .all(|frame| *frame == StereoFrame::new(-0.5, 0.25))
        );
        // A nested/concurrent invocation returns immediately without touching DSP.
        callback.busy.store(true, Ordering::Release);
        unsafe {
            callback.process(&input, &now, &stamp);
        }
        callback.busy.store(false, Ordering::Release);
        assert_eq!(callback.stats().rejected, 1);
        assert_eq!(callback.stats().written, 64);
        unsafe {
            callback.process(&input, std::ptr::null(), &stamp);
        }
        assert_eq!(callback.stats().rejected, 2);
        assert!(!callback.stats().failed);
        assert!(!callback.busy.load(Ordering::Acquire));
    }
}
