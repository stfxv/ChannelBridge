//! Test-only allocator instrumentation; not linked into either product binary.
use channelbridge_audio::StereoFrame;
use channelbridge_audio::fifo::stereo_queue;
use channelbridge_audio::output_processor::OutputProcessor;
use std::alloc::GlobalAlloc;
use std::alloc::Layout;
use std::alloc::System;
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

fn main() {
    let (mut producer, consumer) = stereo_queue(8192).expect("test queue");
    let mut processor = OutputProcessor::new(consumer, 48_000, 48_000, 704).expect("test DSP");
    let incoming = [StereoFrame::new(0.25, -0.25); 1024];
    let mut outgoing = [StereoFrame::SILENCE; 512];
    // This executable has only one thread. Construction and reporting stay
    // outside the measured region; include startup, steady I/O and recovery.
    CHECKING.store(true, Ordering::Release);
    for index in 0..128 {
        if !(64..68).contains(&index) {
            producer.push(&incoming[..512]);
        }
        processor.render(&mut outgoing, Some(1.001));
    }
    CHECKING.store(false, Ordering::Release);
    let allocated = ALLOCATIONS.load(Ordering::Acquire);
    let freed = DEALLOCATIONS.load(Ordering::Acquire);
    println!(
        "FIFO + output DSP: allocations={allocated}, deallocations={freed} (startup, steady transfer, shortage and recovery)"
    );
    assert_eq!(allocated, 0);
    assert_eq!(freed, 0);
}
