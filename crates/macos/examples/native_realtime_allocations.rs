//! Test-only native buffer/allocator probe; no HAL device is opened.
#[cfg(target_os = "macos")]
mod native {
    use channelbridge_audio::capture_processor::CaptureProcessor;
    use channelbridge_audio::fifo;
    use channelbridge_audio::meters;
    use channelbridge_audio::output_processor::OutputProcessor;
    use channelbridge_audio::routing::AtomicRouting;
    use channelbridge_audio::routing::Mode;
    use channelbridge_audio::routing::Selection;
    use channelbridge_hal_abi as hal_abi;
    use channelbridge_hal_abi::AudioBuffer;
    use channelbridge_hal_abi::AudioBufferList;
    use channelbridge_hal_abi::AudioTimestamp;
    use channelbridge_hal_abi::StreamFormat;
    use channelbridge_macos::buffers::CaptureLayout;
    use channelbridge_macos::capture::CaptureCallback;
    use channelbridge_macos::output::OutputCallback;
    use std::alloc::GlobalAlloc;
    use std::alloc::Layout;
    use std::alloc::System;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    struct CheckedAllocator;
    static CHECKING: AtomicBool = AtomicBool::new(false);
    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    #[global_allocator]
    static ALLOCATOR: CheckedAllocator = CheckedAllocator;

    // SAFETY: every operation delegates to System with the identical allocation
    // layout/pointer; instrumentation only updates pre-existing atomic counters.
    unsafe impl GlobalAlloc for CheckedAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if CHECKING.load(Ordering::Relaxed) {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: GlobalAlloc caller supplies a valid layout.
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if CHECKING.load(Ordering::Relaxed) {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: same allocator/layout contract as alloc.
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if CHECKING.load(Ordering::Relaxed) {
                ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: caller provides a live System allocation and valid new size.
            unsafe { System.realloc(pointer, layout, size) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if CHECKING.load(Ordering::Relaxed) {
                DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: caller provides a live allocation with its original layout.
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    pub fn run() {
        let selected = Selection::new(3, 7, Mode::Stereo, false)
            .expect("fixture channel numbers are nonzero and within the selection encoding");
        let routing = Arc::new(AtomicRouting::new(selected));
        let (writer, reader) = meters::meter_bank(10, 24000000)
            .expect("fixture has 1..=512 channels and a nonzero host clock frequency");
        let (producer, consumer) =
            fifo::stereo_queue(8192).expect("fixture queue capacity is a nonzero power of two");
        let layout = CaptureLayout::try_new(&[StreamFormat {
            sample_rate: 48000.0,
            format_id: hal_abi::fourcc(b"lpcm"),
            format_flags: 9,
            bytes_per_packet: 40,
            frames_per_packet: 1,
            bytes_per_frame: 40,
            channels_per_frame: 10,
            bits_per_channel: 32,
            reserved: 0,
        }])
        .expect("fixture is interleaved float32 PCM with ten channels at a positive rate");
        let processor = CaptureProcessor::new(
            Arc::clone(&routing),
            writer,
            producer,
            48000,
            24000000,
        )
        .expect(
            "fixture rate and host frequency are nonzero; fixed callback scratch allocation fits",
        );
        let capture = CaptureCallback::new(layout, processor);
        let processor = OutputProcessor::new(consumer, 48000, 48000, 768).expect(
            "fixture rates are supported and the target lies above lookahead within queue capacity",
        );
        let output = OutputCallback::new(processor, Arc::clone(&routing))
            .expect("fixture can allocate the fixed 4096-frame callback scratch buffer")
            .with_source_clock(capture.source_clock());
        let mut incoming = [[0.25_f32; 10]; 512];
        let mut outgoing = [[0.0_f32; 2]; 512];
        let input = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 10,
                byte_size: 20480,
                data: incoming.as_mut_ptr().cast(),
            }],
        };
        let mut destination = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer {
                channels: 2,
                byte_size: 4096,
                data: outgoing.as_mut_ptr().cast(),
            }],
        };
        CHECKING.store(true, Ordering::Release);
        for index in 0..128_u64 {
            if index == 8 || index == 100 {
                routing
                    .configure(selected, true, false)
                    .expect("fixture selection is encodable and generation space is not exhausted");
            }
            if index == 16 {
                routing
                    .configure(selected, true, true)
                    .expect("fixture selection is encodable and generation space is not exhausted");
            }
            if index == 32 {
                routing
                    .configure(
                        Selection::new(3, 7, Mode::Stereo, true).expect(
                            "fixture channel numbers are nonzero and within the selection encoding",
                        ),
                        true,
                        true,
                    )
                    .expect(
                        "fixture channel numbers are nonzero and within the selection encoding",
                    );
            }
            if index == 48 {
                routing
                    .configure(selected, true, true)
                    .expect("fixture selection is encodable and generation space is not exhausted");
            }
            if index == 96 {
                routing
                    .configure(selected, false, true)
                    .expect("fixture selection is encodable and generation space is not exhausted");
            }
            let time = AudioTimestamp {
                sample_time: (index * 512) as f64,
                host_time: index * 512 * 500,
                flags: 3,
                ..Default::default()
            };
            if !(64..68).contains(&index) {
                // SAFETY: complete descriptors and disjoint live arrays matching prepared formats.
                unsafe {
                    capture.process(&input, &time, &time);
                }
            }
            // SAFETY: destination describes the live, exclusively writable stereo
            // array for this call.
            unsafe {
                output.process(&mut destination, None);
            }
            let _ = reader.level(2, time.host_time);
        }
        CHECKING.store(false, Ordering::Release);
        let allocated = ALLOCATIONS.load(Ordering::Acquire);
        let freed = DEALLOCATIONS.load(Ordering::Acquire);
        println!(
            "Native capture + output adapters: allocations={allocated}, deallocations={freed} (preview, transfer, mute, shortage, restart)"
        );
        assert_eq!(allocated, 0);
        assert_eq!(freed, 0);
        assert!(capture.stats().written > 4096);
        assert!(output.stats().missing > 0);
        assert_eq!(capture.stats().rejected, 0);
        assert_eq!(output.stats().rejected, 0);
    }
}
#[cfg(target_os = "macos")]
fn main() {
    native::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native adapter probe requires macOS");
}
