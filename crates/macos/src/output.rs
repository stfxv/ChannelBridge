//! Native virtual-output buffer adapter; no device registration or rate changes.
use crate::{buffers::stereo_output, source_clock::SourceClock};
use channelbridge_audio::{StereoFrame, output::OutputProcessor, routing::AtomicRouting};
use channelbridge_hal_abi::AudioBufferList;
use std::{
    cell::UnsafeCell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

struct State {
    processor: OutputProcessor,
    scratch: Box<[StereoFrame]>,
    gain: f32,
}
pub struct OutputCallback {
    source_clock: Arc<SourceClock>,
    state: UnsafeCell<State>,
    routing: Arc<AtomicRouting>,
    busy: AtomicBool,
    failed: AtomicBool,
    callbacks: AtomicU64,
    rejected: AtomicU64,
    missing: AtomicU64,
}
// SAFETY: State is Send and accessed only under the single-attempt exclusive
// gate. Control code cannot obtain a reference to State; other fields are atomic.
unsafe impl Sync for OutputCallback {}
struct Gate<'a>(&'a AtomicBool);
impl Drop for Gate<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[derive(Debug, Clone, Copy)]
pub struct OutputStats {
    pub callbacks: u64,
    pub rejected: u64,
    pub missing: u64,
    pub failed: bool,
}
impl OutputCallback {
    /// Allocate working storage on the control thread before registering I/O.
    pub fn new(processor: OutputProcessor, routing: Arc<AtomicRouting>) -> Option<Self> {
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(4096).ok()?;
        scratch.resize(4096, StereoFrame::SILENCE);
        Some(Self {
            source_clock: Arc::new(SourceClock::default()),
            state: UnsafeCell::new(State {
                processor,
                scratch: scratch.into_boxed_slice(),
                gain: 0.0,
            }),
            routing,
            busy: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            missing: AtomicU64::new(0),
        })
    }
    pub fn with_source_clock(mut self, clock: Arc<SourceClock>) -> Self {
        self.source_clock = clock;
        self
    }
    pub fn stats(&self) -> OutputStats {
        OutputStats {
            callbacks: self.callbacks.load(Ordering::Acquire),
            rejected: self.rejected.load(Ordering::Acquire),
            missing: self.missing.load(Ordering::Acquire),
            failed: self.failed.load(Ordering::Acquire),
        }
    }
    /// # Safety
    /// HAL grants exclusive writable ownership of the complete descriptor list
    /// and its native interleaved float32 stereo payload for this invocation.
    /// The context remains alive until the IOProc is deregistered.
    pub unsafe fn process(&self, output: *mut AudioBufferList, source_speed: Option<f64>) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        // Validate and silence even when processing is rejected. The caller's
        // exclusive output-buffer ownership does not depend on the DSP gate.
        let Ok(output) = (unsafe { stereo_output(output) }) else {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        };
        output.fill([0.0; 2]);
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
            let route = self.routing.load();
            // SAFETY: this callback owns the state gate, including across unwind.
            let state = unsafe { &mut *self.state.get() };
            state.processor.set_generation(route.generation);
            if !route.sending {
                state.gain = 0.0;
                return;
            }
            let frames = &mut state.scratch[..output.len()];
            let report = state.processor.render(
                frames,
                source_speed.or_else(|| self.source_clock.speed(route.generation)),
            );
            // Muted callbacks still advance DSP and consume queued input.
            let target = if route.selection.muted() { 0.0 } else { 1.0 };
            for (destination, frame) in output.iter_mut().zip(frames.iter()) {
                if state.gain < target {
                    state.gain = (state.gain + 1.0 / 64.0).min(target);
                } else if state.gain > target {
                    state.gain = (state.gain - 1.0 / 64.0).max(target);
                }
                *destination = [frame.left * state.gain, frame.right * state.gain];
            }
            self.missing
                .fetch_add(report.missing as u64, Ordering::Relaxed);
        }));
        if result.is_err() {
            output.fill([0.0; 2]);
            self.failed.store(true, Ordering::Release);
            self.rejected.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use channelbridge_audio::{
        fifo::stereo_queue,
        routing::{Mode, Selection},
    };
    use channelbridge_hal_abi::AudioBuffer;
    #[test]
    fn mute_consumes_audio_and_stop_restart_cannot_replay_it() {
        let selection = Selection::new(1, 2, Mode::Stereo, false).unwrap();
        let routing = Arc::new(AtomicRouting::new(selection));
        let generation = routing
            .configure(selection, true, false)
            .unwrap()
            .generation;
        let (mut producer, consumer) = stereo_queue(8192).unwrap();
        let processor = OutputProcessor::new(consumer, 48_000, 48_000, 1024).unwrap();
        let callback = OutputCallback::new(processor, Arc::clone(&routing)).unwrap();
        let mut samples = [[9.0_f32; 2]; 512];
        let mut buffer = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 4096,
                data: samples.as_mut_ptr().cast(),
            }],
        };
        let old = [StereoFrame::new(0.5, 0.25); 2048];
        assert_eq!(producer.push_generation(&old, generation), old.len());
        // SAFETY: live, uniquely writable, correctly sized native stereo buffer.
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!((samples[511][0] - 0.5).abs() < 0.0001);
        let muted = Selection::new(1, 2, Mode::Stereo, true).unwrap();
        assert_eq!(
            routing.configure(muted, true, false).unwrap().generation,
            generation
        );
        let fresh = [StereoFrame::new(-0.25, -0.125); 512];
        for _ in 0..32 {
            assert_eq!(producer.push_generation(&fresh, generation), 512);
            unsafe {
                callback.process(&mut buffer, None);
            }
            assert_eq!(samples[511], [0.0; 2]);
        }
        routing.configure(selection, true, false).unwrap();
        producer.push_generation(&fresh, generation);
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!((samples[511][0] + 0.25).abs() < 0.0001);
        routing.configure(selection, false, false).unwrap();
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!(samples.iter().all(|frame| *frame == [0.0; 2]));
        let restarted = routing
            .configure(selection, true, false)
            .unwrap()
            .generation;
        producer.push_generation(&old, restarted);
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!((samples[511][0] - 0.5).abs() < 0.0001);
        callback.busy.store(true, Ordering::Release);
        unsafe {
            callback.process(&mut buffer, None);
        }
        callback.busy.store(false, Ordering::Release);
        assert!(samples.iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(callback.stats().rejected, 1);
        assert!(!callback.stats().failed);
    }
}
