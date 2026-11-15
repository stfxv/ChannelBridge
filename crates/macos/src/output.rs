//! Native virtual-output buffer adapter; no device registration or rate changes.
use crate::buffers;
use crate::source_clock::SourceClock;
use channelbridge_audio::StereoFrame;
use channelbridge_audio::output_processor::OutputProcessor;
use channelbridge_audio::routing::AtomicRouting;
use channelbridge_hal_abi::AudioBufferList;
use std::cell::UnsafeCell;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

struct State {
    processor: OutputProcessor,
    scratch: Box<[StereoFrame]>,
    gain: f32,
}
pub struct OutputCallback {
    source_clock: Arc<SourceClock>,
    state: UnsafeCell<State>,
    routing: Arc<AtomicRouting>,
    pub(crate) output_gate: channelbridge_core::worker::OutputGate,
    busy: AtomicBool,
    failed: AtomicBool,
    callbacks: AtomicU64,
    accepted_frames: AtomicU64,
    rejected: AtomicU64,
    missing: AtomicU64,
    priming_callbacks: AtomicU64,
    dropped_lookahead: AtomicU64,
    rendered_frames: AtomicU64,
    fifo_current: AtomicU64,
    fifo_min: AtomicU64,
    fifo_max: AtomicU64,
    buffered_target: AtomicU64,
    correction_ppm: AtomicU64,
    /// Both channel peaks from one completed valid callback, packed together.
    peak: AtomicU64,
}
// SAFETY: State is Send and accessed only under the single-attempt exclusive
// gate. Control code cannot obtain a reference to State; other fields are atomic.
unsafe impl Sync for OutputCallback {}
struct Gate<'gate>(&'gate AtomicBool);
impl Drop for Gate<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[derive(Debug, Clone, Copy)]
pub struct OutputStats {
    pub callbacks: u64,
    /// Completed native buffer processing, including intentional preview silence.
    pub(crate) accepted_frames: u64,
    pub rejected: u64,
    pub missing: u64,
    /// Waiting for startup/recovery buffering, separate from an active underrun.
    pub priming_callbacks: u64,
    pub dropped_lookahead: u64,
    pub rendered_frames: u64,
    pub fifo_current: Option<u64>,
    pub fifo_min: Option<u64>,
    pub fifo_max: Option<u64>,
    pub buffered_target: u64,
    pub correction_ppm: Option<f64>,
    pub peak: Option<[f32; 2]>,
    pub failed: bool,
}
impl OutputCallback {
    /// Allocate working storage on the control thread before registering I/O.
    #[must_use]
    pub fn new(processor: OutputProcessor, routing: Arc<AtomicRouting>) -> Option<Self> {
        let mut scratch = Vec::new();
        scratch.try_reserve_exact(4096).ok()?;
        scratch.resize(4096, StereoFrame::SILENCE);
        let buffered_target = processor.target_frames() as u64;
        Some(Self {
            source_clock: Arc::new(SourceClock::default()),
            state: UnsafeCell::new(State {
                processor,
                scratch: scratch.into_boxed_slice(),
                gain: 0.0,
            }),
            routing,
            output_gate: Default::default(),
            busy: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            accepted_frames: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            missing: AtomicU64::new(0),
            priming_callbacks: AtomicU64::new(0),
            dropped_lookahead: AtomicU64::new(0),
            rendered_frames: AtomicU64::new(0),
            fifo_current: AtomicU64::new(0),
            fifo_min: AtomicU64::new(u64::MAX),
            fifo_max: AtomicU64::new(0),
            buffered_target: AtomicU64::new(buffered_target),
            correction_ppm: AtomicU64::new(f64::NAN.to_bits()),
            peak: AtomicU64::new(u64::MAX),
        })
    }
    #[must_use]
    pub fn with_source_clock(mut self, clock: Arc<SourceClock>) -> Self {
        self.source_clock = clock;
        self
    }
    #[must_use]
    pub fn stats(&self) -> OutputStats {
        let minimum = self.fifo_min.load(Ordering::Acquire);
        let observed = minimum != u64::MAX;
        let correction = f64::from_bits(self.correction_ppm.load(Ordering::Acquire));
        let peak = self.peak.load(Ordering::Acquire);
        let peak = [
            f32::from_bits(peak as u32),
            f32::from_bits((peak >> 32) as u32),
        ];
        OutputStats {
            callbacks: self.callbacks.load(Ordering::Acquire),
            accepted_frames: self.accepted_frames.load(Ordering::Acquire),
            rejected: self.rejected.load(Ordering::Acquire),
            missing: self.missing.load(Ordering::Acquire),
            priming_callbacks: self.priming_callbacks.load(Ordering::Acquire),
            dropped_lookahead: self.dropped_lookahead.load(Ordering::Acquire),
            rendered_frames: self.rendered_frames.load(Ordering::Acquire),
            fifo_current: observed.then(|| self.fifo_current.load(Ordering::Acquire)),
            fifo_min: observed.then_some(minimum),
            fifo_max: observed.then(|| self.fifo_max.load(Ordering::Acquire)),
            buffered_target: self.buffered_target.load(Ordering::Acquire),
            correction_ppm: correction.is_finite().then_some(correction),
            peak: peak.iter().all(|value| value.is_finite()).then_some(peak),
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
        // SAFETY: The process caller grants exclusive, live output storage for the
        // duration of this invocation.
        let Ok(output) = (unsafe { buffers::stereo_output(output) }) else {
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
        // Defense in depth under panic=unwind. A panic permanently disables
        // processor reuse; the gate remains valid and releases during unwinding.
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let route = self.routing.load();
            // SAFETY: this callback owns the state gate, including across unwind.
            let state = unsafe { &mut *self.state.get() };
            state.processor.set_generation(route.generation);
            self.buffered_target
                .store(state.processor.target_frames() as u64, Ordering::Release);
            if !route.sending || self.output_gate.blocked() {
                state.gain = 0.0;
                self.peak.store(0, Ordering::Release);
                return;
            }
            let frames = &mut state.scratch[..output.len()];
            let fifo_before = state.processor.fifo_frames() as u64;
            let report = state.processor.render(
                frames,
                source_speed.or_else(|| self.source_clock.speed(route.generation)),
            );
            self.buffered_target
                .store(state.processor.target_frames() as u64, Ordering::Release);
            // Muted callbacks still advance DSP and consume queued input.
            let target = if route.selection.muted() { 0.0 } else { 1.0 };
            let output_samples = output.iter_mut().zip(frames.iter());
            for (destination, frame) in output_samples {
                if state.gain < target {
                    state.gain = (state.gain + 1.0 / 64.0).min(target);
                } else if state.gain > target {
                    state.gain = (state.gain - 1.0 / 64.0).max(target);
                }
                *destination = [frame.left * state.gain, frame.right * state.gain];
            }
            // Stop may have arrived during the render, including a complete Stop/Start.
            if self.output_gate.blocked() || self.routing.load().generation != route.generation {
                output.fill([0.0; 2]);
                state.gain = 0.0;
            }
            let peak = output.iter().try_fold([0.0_f32; 2], |peak, frame| {
                frame
                    .iter()
                    .all(|value| value.is_finite())
                    .then(|| [peak[0].max(frame[0].abs()), peak[1].max(frame[1].abs())])
            });
            self.peak.store(
                peak.map_or(u64::MAX, |peak| {
                    u64::from(peak[0].to_bits()) | (u64::from(peak[1].to_bits()) << 32)
                }),
                Ordering::Release,
            );
            self.missing
                .fetch_add(report.missing as u64, Ordering::Relaxed);
            if report.priming {
                self.priming_callbacks.fetch_add(1, Ordering::Relaxed);
            }
            self.dropped_lookahead
                .fetch_add(report.dropped_lookahead as u64, Ordering::Relaxed);
            self.rendered_frames
                .fetch_add(report.produced as u64, Ordering::Relaxed);
            let fifo = state.processor.fifo_frames() as u64;
            self.fifo_current.store(fifo, Ordering::Relaxed);
            // Render may move the entire FIFO into interpolation history.
            // Sample both boundaries so that this does not hide input backlog.
            self.fifo_max
                .fetch_max(fifo.max(fifo_before), Ordering::Relaxed);
            self.fifo_min
                .fetch_min(fifo.min(fifo_before), Ordering::Release);
            self.correction_ppm.store(
                state.processor.correction_ppm().to_bits(),
                Ordering::Release,
            );
        }));
        if result.is_err() {
            output.fill([0.0; 2]);
            self.peak.store(0, Ordering::Release);
            self.failed.store(true, Ordering::Release);
            self.rejected.fetch_add(1, Ordering::Relaxed);
        } else {
            self.accepted_frames
                .fetch_add(output.len() as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use channelbridge_audio::fifo;
    use channelbridge_audio::routing::Mode;
    use channelbridge_audio::routing::Selection;
    use channelbridge_hal_abi::AudioBuffer;
    pub(crate) fn undersized_scratch_callback() -> OutputCallback {
        let selection = Selection::new(1, 2, Mode::Stereo, false)
            .expect("both fixture channels fit the selection encoding");
        let routing = Arc::new(AtomicRouting::new(selection));
        routing
            .configure(selection, true, false)
            .expect("new fixture route has generation space");
        let (_, consumer) = fifo::stereo_queue(8192).expect("fixture capacity is a power of two");
        let processor = OutputProcessor::new(consumer, 48000, 48000, 1024)
            .expect("fixture target fits the queue and exceeds filter lookahead");
        let mut callback = OutputCallback::new(processor, routing)
            .expect("fixture can allocate callback scratch storage");
        callback.state.get_mut().scratch = Box::new([]);
        callback
    }
    #[test]
    fn blocked_save_silences_output_and_coalesced_restart_discards_old_audio() {
        use channelbridge_core::controller::Backend;
        use channelbridge_core::worker::{OutputGate, Worker};
        use channelbridge_core::{Command, Config};
        use std::sync::mpsc;
        use std::time::Duration;

        struct GatedBackend {
            gate: mpsc::Sender<OutputGate>,
            saving: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
            routed: mpsc::Sender<bool>,
            routing: Arc<AtomicRouting>,
        }
        impl Backend for GatedBackend {
            type Pending = ();
            type Session = ();
            fn set_output_gate(&mut self, gate: OutputGate) {
                self.gate
                    .send(gate)
                    .expect("test is waiting for the callback gate");
            }
            fn save(&mut self, _: &Config) -> Result<(), String> {
                self.saving.send(()).map_err(|error| error.to_string())?;
                self.release.recv().map_err(|error| error.to_string())
            }
            fn negotiate(&mut self, _: &Config) -> Result<(), String> {
                Ok(())
            }
            fn ready(&mut self, _: &()) -> Result<bool, String> {
                Ok(true)
            }
            fn open(&mut self, _: &Config) -> Result<(), String> {
                Ok(())
            }
            fn healthy(&mut self, _: &(), _: &Config) -> Result<bool, String> {
                Ok(true)
            }
            fn route(
                &mut self,
                _: &(),
                _: &Config,
                sending: bool,
                overview: bool,
            ) -> Result<(), String> {
                self.routing
                    .configure(self.routing.load().selection, sending, overview)
                    .ok_or("fixture routing update failed")?;
                self.routed.send(sending).map_err(|error| error.to_string())
            }
            fn close(&mut self, _: ()) -> Result<(), String> {
                Ok(())
            }
        }

        let selection = Selection::new(1, 2, Mode::Stereo, false)
            .expect("both fixture channels fit the selection encoding");
        let routing = Arc::new(AtomicRouting::new(selection));
        let backend_routing = Arc::clone(&routing);
        let (gate, receive_gate) = mpsc::channel();
        let (saving, save_entered) = mpsc::channel();
        let (release_save, release) = mpsc::channel();
        let (routed, routes) = mpsc::channel();
        let config = Config {
            device_uid: "source".into(),
            ..Config::default()
        };
        let worker = Worker::spawn(config.clone(), move || {
            Ok(GatedBackend {
                gate,
                saving,
                release,
                routed,
                routing: backend_routing,
            })
        })
        .expect("fixture worker can start");
        let timeout = Duration::from_secs(3);
        let (mut producer, consumer) =
            fifo::stereo_queue(8192).expect("fixture capacity is a power of two");
        let processor = OutputProcessor::new(consumer, 48000, 48000, 1024)
            .expect("fixture target fits the queue and exceeds filter lookahead");
        let mut callback = OutputCallback::new(processor, Arc::clone(&routing))
            .expect("fixture can allocate callback scratch storage");
        callback.output_gate = receive_gate
            .recv_timeout(timeout)
            .expect("worker installs the output gate at startup");
        let render = || {
            let mut samples = [[0.0_f32; 2]; 512];
            let mut buffer = AudioBufferList {
                count: 1,
                buffers: [AudioBuffer {
                    channels: 2,
                    byte_size: 4096,
                    data: samples.as_mut_ptr().cast(),
                }],
            };
            // SAFETY: The fixture owns the live stereo descriptors and exclusive sample storage.
            unsafe { callback.process(&mut buffer, None) };
            samples
        };
        worker
            .send(Command::PermissionChanged(true))
            .expect("worker admission is open");
        worker
            .send(Command::Start)
            .expect("manual start bypasses the command queue");
        while !routes
            .recv_timeout(timeout)
            .expect("fixture worker routes its live session")
        {}
        let before_stop = routing.load().generation;
        let old = [StereoFrame::new(0.5, 0.5); 2048];
        for packet in old.chunks(512) {
            assert_eq!(producer.push_generation(packet, before_stop), packet.len());
        }
        assert!(render()[511][0] > 0.49);
        worker
            .send(Command::Configure(config))
            .expect("configuration fits the empty queue");
        save_entered
            .recv_timeout(timeout)
            .expect("worker enters the simulated blocked save");
        worker
            .send(Command::Stop)
            .expect("stop bypasses the blocked save");
        assert!(render().iter().all(|frame| *frame == [0.0; 2]));
        worker
            .send(Command::Start)
            .expect("restart can be requested during the save");
        assert!(render().iter().all(|frame| *frame == [0.0; 2]));
        release_save
            .send(())
            .expect("control worker is waiting in save");
        while routes
            .recv_timeout(timeout)
            .expect("worker retires the old route")
        {}
        assert!(
            routes
                .recv_timeout(timeout)
                .expect("worker applies the queued restart")
        );
        let after_restart = routing.load().generation;
        assert!(after_restart > before_stop);
        assert!(render().iter().all(|frame| *frame == [0.0; 2]));
        let fresh = [StereoFrame::new(-0.25, -0.25); 2048];
        for packet in fresh.chunks(512) {
            assert_eq!(
                producer.push_generation(packet, after_restart),
                packet.len()
            );
        }
        assert!(render()[511][0] < -0.24);
        worker.request_shutdown();
        assert!(render().iter().all(|frame| *frame == [0.0; 2]));
        worker.join().expect("fixture backend exits normally");
    }
    #[test]
    fn diagnostics_separate_buffering_from_active_audio_shortage() {
        let selection = Selection::new(1, 2, Mode::Stereo, false)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let routing = Arc::new(AtomicRouting::new(selection));
        let generation = routing
            .configure(
                selection, /* sending */ true, /* overview */ false,
            )
            .expect("fixture selection is encodable and generation space is not exhausted")
            .generation;
        let (mut producer, consumer) =
            fifo::stereo_queue(8192).expect("fixture queue capacity is a nonzero power of two");
        let processor = OutputProcessor::new(consumer, 48_000, 48_000, 1024).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let callback = OutputCallback::new(processor, Arc::clone(&routing))
            .expect("fixture can allocate the fixed 4096-frame callback scratch buffer");
        assert_eq!(callback.stats().fifo_min, None);
        assert_eq!(callback.stats().correction_ppm, None);
        assert_eq!(callback.stats().peak, None);
        assert_eq!(callback.stats().buffered_target, 1024);
        let mut samples = [[9.0_f32; 2]; 512];
        let mut buffer = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 4096,
                data: samples.as_mut_ptr().cast(),
            }],
        };
        // SAFETY: all calls own the complete native stereo output buffer.
        unsafe { callback.process(&mut buffer, None) };
        assert_eq!(callback.stats().priming_callbacks, 1);
        assert_eq!(callback.stats().missing, 0);
        assert_eq!(callback.stats().peak, Some([0.0; 2]));
        assert_eq!(callback.stats().fifo_min, Some(0));
        assert_eq!(callback.stats().correction_ppm, Some(0.0));
        assert!(samples.iter().all(|sample| *sample == [0.0; 2]));
        for _ in 0..2 {
            assert_eq!(
                producer.push_generation(&[StereoFrame::new(0.5, 0.25); 1024], generation),
                1024
            );
        }
        for _ in 0..6 {
            // SAFETY: The fixture owns the complete stereo descriptor and
            // exclusively borrows its samples for this call.
            unsafe { callback.process(&mut buffer, None) };
        }
        let stats = callback.stats();
        assert!(stats.missing > 0);
        assert!(stats.dropped_lookahead > 0);
        assert!(stats.priming_callbacks > 1);
        assert!(stats.rendered_frames > 0);
        assert!(
            stats
                .fifo_max
                .expect("processed output callbacks publish FIFO occupancy statistics")
                > 0
        );
        assert_eq!(stats.rejected, 0);
        assert!(!stats.failed);
        assert_eq!(stats.buffered_target, 1122);
        assert_eq!(
            producer.push_generation(&[StereoFrame::new(0.5, 0.25); 4096], generation),
            4096
        );
        // SAFETY: The fixture exclusively owns the native stereo output buffer.
        unsafe { callback.process(&mut buffer, None) };
        assert_eq!(callback.stats().buffered_target, 4194);
        routing
            .configure(selection, false, false)
            .expect("fixture route can advance its generation");
        // SAFETY: The fixture exclusively owns the native stereo output buffer.
        unsafe { callback.process(&mut buffer, None) };
        assert_eq!(callback.stats().buffered_target, 1024);
    }
    #[test]
    fn mute_consumes_audio_and_stop_restart_cannot_replay_it() {
        let selection = Selection::new(1, 2, Mode::Stereo, false)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let routing = Arc::new(AtomicRouting::new(selection));
        let generation = routing
            .configure(
                selection, /* sending */ true, /* overview */ false,
            )
            .expect("fixture selection is encodable and generation space is not exhausted")
            .generation;
        let (mut producer, consumer) =
            fifo::stereo_queue(8192).expect("fixture queue capacity is a nonzero power of two");
        let processor = OutputProcessor::new(consumer, 48_000, 48_000, 1024).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let callback = OutputCallback::new(processor, Arc::clone(&routing))
            .expect("fixture can allocate the fixed 4096-frame callback scratch buffer");
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
        for packet in old.chunks(512) {
            assert_eq!(producer.push_generation(packet, generation), packet.len());
        }
        // SAFETY: live, uniquely writable, correctly sized native stereo buffer.
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!((samples[511][0] - 0.5).abs() < 0.0001);
        let peak = callback
            .stats()
            .peak
            .expect("rendered fixture samples publish output peak statistics");
        assert!((peak[0] - 0.5).abs() < 0.0001);
        assert!((peak[1] - 0.25).abs() < 0.0001);
        let muted = Selection::new(1, 2, Mode::Stereo, true)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        assert_eq!(
            routing
                .configure(muted, true, false)
                .expect("fixture selection is encodable and generation space is not exhausted")
                .generation,
            generation
        );
        let fresh = [StereoFrame::new(-0.25, -0.125); 512];
        for _ in 0..32 {
            assert_eq!(producer.push_generation(&fresh, generation), 512);
            // SAFETY: The fixture owns the complete stereo descriptor and
            // exclusively borrows its samples for this call.
            unsafe {
                callback.process(&mut buffer, None);
            }
            assert_eq!(samples[511], [0.0; 2]);
        }
        routing
            .configure(
                selection, /* sending */ true, /* overview */ false,
            )
            .expect("fixture selection is encodable and generation space is not exhausted");
        assert_eq!(callback.stats().peak, Some([0.0; 2]));
        let _ = producer.push_generation(&fresh, generation);
        // SAFETY: The fixture owns the complete stereo descriptor and exclusively
        // borrows its samples for this call.
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!((samples[511][0] + 0.25).abs() < 0.0001);
        routing
            .configure(
                selection, /* sending */ false, /* overview */ false,
            )
            .expect("fixture selection is encodable and generation space is not exhausted");
        // SAFETY: The fixture owns the complete stereo descriptor and exclusively
        // borrows its samples for this call.
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!(samples.iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(callback.stats().peak, Some([0.0; 2]));
        let restarted = routing
            .configure(
                selection, /* sending */ true, /* overview */ false,
            )
            .expect("fixture selection is encodable and generation space is not exhausted")
            .generation;
        for packet in old.chunks(512) {
            assert_eq!(producer.push_generation(packet, restarted), packet.len());
        }
        // SAFETY: The fixture owns the complete stereo descriptor and exclusively
        // borrows its samples for this call.
        unsafe {
            callback.process(&mut buffer, None);
        }
        assert!((samples[511][0] - 0.5).abs() < 0.0001);
        callback.busy.store(true, Ordering::Release);
        // SAFETY: The fixture owns the complete stereo descriptor and exclusively
        // borrows its samples for this call.
        unsafe {
            callback.process(&mut buffer, None);
        }
        callback.busy.store(false, Ordering::Release);
        assert!(samples.iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(callback.stats().rejected, 1);
        assert!(!callback.stats().failed);
    }
}
