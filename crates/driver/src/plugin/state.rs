use super::{
    host_boundary::{BAD_OBJECT, DEVICE, ILLEGAL, NOT_READY, UNSPECIFIED},
    system,
};
use crate::DriverAudio;
use channelbridge_audio::device_clock::{DeviceClock, ZeroTimestamp};
use channelbridge_contract::SUPPORTED_RATES;
use channelbridge_hal_abi::interface::{HostInterface, HostRef};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering},
    sync::{
        Mutex, OnceLock,
        mpsc::{self, SyncSender},
    },
    thread::JoinHandle,
};

struct Worker {
    sender: SyncSender<Option<u32>>,
    thread: JoinHandle<()>,
}
pub(super) struct Runtime {
    pub audio: DriverAudio,
    rate: AtomicU32,
    started: AtomicU32,
    anchor: AtomicU64,
    frequency: u64,
    clock_sequence: AtomicU64,
    host: AtomicPtr<HostInterface>,
    clients: Mutex<BTreeMap<u32, bool>>,
    worker: Mutex<Option<Worker>>,
    pending_rate: AtomicU32,
}
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
pub(super) fn runtime() -> Option<&'static Runtime> {
    RUNTIME.get()
}

pub(super) fn initialize(host: HostRef) -> i32 {
    let Some(audio) = crate::kernel() else {
        return UNSPECIFIED;
    };
    let Some(frequency) = system::frequency() else {
        return UNSPECIFIED;
    };
    let _ = RUNTIME.set(Runtime {
        audio,
        rate: AtomicU32::new(48_000),
        started: AtomicU32::new(0),
        anchor: AtomicU64::new(system::now()),
        frequency,
        clock_sequence: AtomicU64::new(0),
        host: AtomicPtr::new(host.cast_mut()),
        clients: Mutex::new(BTreeMap::new()),
        worker: Mutex::new(None),
        pending_rate: AtomicU32::new(0),
    });
    let Some(runtime) = runtime() else {
        return UNSPECIFIED;
    };
    runtime.host.store(host.cast_mut(), Ordering::Release);
    let Ok(mut worker) = runtime.worker.lock() else {
        return UNSPECIFIED;
    };
    if worker.is_none() {
        let (sender, receiver) = mpsc::sync_channel::<Option<u32>>(8);
        let thread = match std::thread::Builder::new()
            .name("ChannelBridge HAL control".into())
            .spawn(move || {
                while let Ok(Some(rate)) = receiver.recv() {
                    let host = runtime.host.load(Ordering::Acquire);
                    if host.is_null() {
                        continue;
                    }
                    // SAFETY: Initialize's host table remains valid for the plug-in
                    // lifetime. Release joins this worker before the table can die.
                    let status = unsafe {
                        ((*host).request_configuration_change)(
                            host,
                            DEVICE,
                            u64::from(rate),
                            core::ptr::null_mut(),
                        )
                    };
                    if status != 0 {
                        runtime
                            .pending_rate
                            .compare_exchange(rate, 0, Ordering::AcqRel, Ordering::Acquire)
                            .ok();
                    }
                }
            }) {
            Ok(thread) => thread,
            Err(_) => return UNSPECIFIED,
        };
        *worker = Some(Worker { sender, thread });
    }
    0
}

pub(super) fn shutdown() {
    let Some(runtime) = runtime() else {
        return;
    };
    // Do not hold the control mutex while joining: the host may reenter a
    // property setter while a configuration request is finishing.
    let worker = match runtime.worker.lock() {
        Ok(mut worker) => worker.take(),
        Err(_) => return,
    };
    if let Some(worker) = worker {
        let _ = worker.sender.send(None);
        let _ = worker.thread.join();
    }
}

pub(super) fn rate() -> u32 {
    runtime().map_or(48_000, |r| r.rate.load(Ordering::Acquire))
}
pub(super) fn running() -> u32 {
    runtime().map_or(0, |r| u32::from(r.started.load(Ordering::Acquire) != 0))
}

pub(super) fn add_client(id: u32) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(mut clients) = r.clients.lock() else {
        return UNSPECIFIED;
    };
    if clients.contains_key(&id) || clients.len() >= 256 {
        return ILLEGAL;
    }
    clients.insert(id, false);
    0
}
pub(super) fn remove_client(id: u32) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(mut clients) = r.clients.lock() else {
        return UNSPECIFIED;
    };
    match clients.get(&id) {
        Some(false) => {
            clients.remove(&id);
            0
        }
        Some(true) => ILLEGAL,
        None => BAD_OBJECT,
    }
}

fn reset_clock(r: &Runtime, new_rate: Option<u32>) -> i32 {
    r.clock_sequence.fetch_add(1, Ordering::AcqRel);
    if let Some(rate) = new_rate {
        r.rate.store(rate, Ordering::Release);
    }
    r.anchor.store(system::now(), Ordering::Release);
    let status = if r.audio.next_generation().is_some() {
        0
    } else {
        UNSPECIFIED
    };
    r.clock_sequence.fetch_add(1, Ordering::Release);
    status
}

pub(super) fn start(id: u32) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(mut clients) = r.clients.lock() else {
        return UNSPECIFIED;
    };
    let Some(started) = clients.get_mut(&id) else {
        return BAD_OBJECT;
    };
    if *started {
        return ILLEGAL;
    }
    if r.started.load(Ordering::Acquire) == 0 {
        let status = reset_clock(r, None);
        if status != 0 {
            return status;
        }
    }
    *started = true;
    r.started.fetch_add(1, Ordering::Release);
    0
}
pub(super) fn stop(id: u32) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(mut clients) = r.clients.lock() else {
        return UNSPECIFIED;
    };
    let Some(started) = clients.get_mut(&id) else {
        return BAD_OBJECT;
    };
    if !*started {
        return ILLEGAL;
    }
    *started = false;
    r.started.fetch_sub(1, Ordering::Release);
    0
}

pub(super) fn request_rate(rate: u32) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    if !SUPPORTED_RATES.contains(&rate) {
        return ILLEGAL;
    }
    if rate == self::rate() && r.pending_rate.load(Ordering::Acquire) == 0 {
        return 0;
    }
    if r.pending_rate
        .compare_exchange(0, rate, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return NOT_READY;
    }
    let Ok(worker) = r.worker.lock() else {
        r.pending_rate.store(0, Ordering::Release);
        return UNSPECIFIED;
    };
    let Some(worker) = worker.as_ref() else {
        r.pending_rate.store(0, Ordering::Release);
        return NOT_READY;
    };
    if worker.sender.try_send(Some(rate)).is_err() {
        r.pending_rate.store(0, Ordering::Release);
        return NOT_READY;
    }
    0
}

pub(super) fn perform_rate_change(action: u64) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(rate) = u32::try_from(action) else {
        return ILLEGAL;
    };
    if r.pending_rate.load(Ordering::Acquire) != rate || !SUPPORTED_RATES.contains(&rate) {
        return ILLEGAL;
    }
    // A completed host callback consumes this request even when we reject it.
    // Do not depend on the host issuing Abort after a failed Perform call.
    // Clear only this request; a different request must never be retired here.
    let _request = PendingChange {
        pending: &r.pending_rate,
        rate,
    };
    // The host must quiesce all active I/O before changing its timeline.
    let Ok(_clients) = r.clients.lock() else {
        return UNSPECIFIED;
    };
    if r.started.load(Ordering::Acquire) != 0 {
        return ILLEGAL;
    }
    reset_clock(r, Some(rate))
}

struct PendingChange<'a> {
    pending: &'a AtomicU32,
    rate: u32,
}
impl Drop for PendingChange<'_> {
    fn drop(&mut self) {
        let _ = self
            .pending
            .compare_exchange(self.rate, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}
pub(super) fn abort_rate_change(action: u64) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(rate) = u32::try_from(action) else {
        return ILLEGAL;
    };
    if r.pending_rate
        .compare_exchange(rate, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        0
    } else {
        ILLEGAL
    }
}

pub(super) fn timestamp() -> Option<ZeroTimestamp> {
    let r = runtime()?;
    let before = r.clock_sequence.load(Ordering::Acquire);
    if before & 1 != 0 {
        return None;
    }
    let clock = DeviceClock::new(
        r.anchor.load(Ordering::Acquire),
        r.frequency,
        r.rate.load(Ordering::Acquire),
        512,
        r.audio.counters().generation,
    )?;
    let timestamp = clock.timestamp(system::now())?;
    if before != r.clock_sequence.load(Ordering::Acquire) {
        return None;
    }
    Some(timestamp)
}
