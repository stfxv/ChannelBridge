//! Test-only native buffer/allocator probe; no HAL device is opened.
#[cfg(target_os = "macos")]
mod native {
    use channelbridge_audio::{
        capture_processor::CaptureProcessor,
        fifo::stereo_queue,
        meters::meter_bank,
        output_processor::OutputProcessor,
        routing::{AtomicRouting, Mode, Selection},
    };
    use channelbridge_hal_abi::{
        AudioBuffer, AudioBufferList, AudioTimestamp, StreamFormat, fourcc,
    };
    use channelbridge_macos::{
        buffers::CaptureLayout, capture::CaptureCallback, output::OutputCallback,
    };
    use std::sync::Arc;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
    };

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
        let selected = Selection::new(3, 7, Mode::Stereo, false).unwrap();
        let routing = Arc::new(AtomicRouting::new(selected));
        let (writer, reader) = meter_bank(10, 24000000).unwrap();
        let (producer, consumer) = stereo_queue(8192).unwrap();
        let layout = CaptureLayout::try_new(&[StreamFormat {
            sample_rate: 48000.0,
            format_id: fourcc(b"lpcm"),
            format_flags: 9,
            bytes_per_packet: 40,
            frames_per_packet: 1,
            bytes_per_frame: 40,
            channels_per_frame: 10,
            bits_per_channel: 32,
            reserved: 0,
        }])
        .unwrap();
        let processor =
            CaptureProcessor::new(Arc::clone(&routing), writer, producer, 48000, 24000000).unwrap();
        let capture = CaptureCallback::new(layout, processor);
        let processor = OutputProcessor::new(consumer, 48000, 48000, 768).unwrap();
        let output = OutputCallback::new(processor, Arc::clone(&routing))
            .unwrap()
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
                routing.configure(selected, true, false).unwrap();
            }
            if index == 16 {
                routing.configure(selected, true, true).unwrap();
            }
            if index == 32 {
                routing
                    .configure(
                        Selection::new(3, 7, Mode::Stereo, true).unwrap(),
                        true,
                        true,
                    )
                    .unwrap();
            }
            if index == 48 {
                routing.configure(selected, true, true).unwrap();
            }
            if index == 96 {
                routing.configure(selected, false, true).unwrap();
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
