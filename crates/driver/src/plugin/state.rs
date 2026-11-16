use super::host_boundary::BAD_OBJECT;
use super::host_boundary::DEVICE;
use super::host_boundary::ILLEGAL;
use super::host_boundary::NOT_READY;
use super::host_boundary::UNSPECIFIED;
use super::system;
use crate::DriverAudio;
use channelbridge_audio::device_clock::{DeviceClock, ZeroTimestamp};
use channelbridge_contract::SUPPORTED_RATES;
use channelbridge_hal_abi::interface::{HostInterface, HostRef};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;

struct Worker {
    sender: SyncSender<Option<u64>>,
    thread: JoinHandle<()>,
}
struct Pending {
    next: u64,
    change: Option<(u64, u32)>,
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
    pending: Mutex<Pending>,
}
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
#[must_use]
pub(super) fn runtime() -> Option<&'static Runtime> {
    RUNTIME.get()
}

#[must_use]
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
        pending: Mutex::new(Pending {
            next: 1,
            change: None,
        }),
    });
    let Some(runtime) = runtime() else {
        return UNSPECIFIED;
    };
    runtime.host.store(host.cast_mut(), Ordering::Release);
    let Ok(mut worker) = runtime.worker.lock() else {
        return UNSPECIFIED;
    };
    if worker.is_none() {
        let (sender, receiver) = mpsc::sync_channel::<Option<u64>>(8);
        let thread = match std::thread::Builder::new()
            .name("ChannelBridge HAL control".into())
            .spawn(move || {
                while let Ok(Some(action)) = receiver.recv() {
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
                            action,
                            core::ptr::null_mut(),
                        )
                    };
                    if status != 0 {
                        if let Ok(mut pending) = runtime.pending.lock() {
                            if pending.change.is_some_and(|(id, _)| id == action) {
                                pending.change = None;
                            }
                        }
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
    let worker = {
        let Ok(mut worker) = runtime.worker.lock() else {
            return;
        };
        let Ok(mut pending) = runtime.pending.lock() else {
            return;
        };
        pending.change = None;
        worker.take()
    };
    if let Some(worker) = worker {
        let _ = worker.sender.send(None);
        let _ = worker.thread.join();
    }
}

#[must_use]
pub(super) fn rate() -> u32 {
    runtime().map_or(48_000, |r| r.rate.load(Ordering::Acquire))
}
#[must_use]
pub(super) fn running() -> u32 {
    runtime().map_or(0, |r| u32::from(r.started.load(Ordering::Acquire) != 0))
}

#[must_use]
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
#[must_use]
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

#[must_use]
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

#[must_use]
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
#[must_use]
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

#[must_use]
pub(super) fn request_rate(rate: u32) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    if !SUPPORTED_RATES.contains(&rate) {
        return ILLEGAL;
    }
    let Ok(worker) = r.worker.lock() else {
        return UNSPECIFIED;
    };
    let Some(worker) = worker.as_ref() else {
        return NOT_READY;
    };
    let Ok(mut pending) = r.pending.lock() else {
        return UNSPECIFIED;
    };
    if pending.change.is_some() {
        return NOT_READY;
    }
    if rate == self::rate() {
        return 0;
    }
    let action = pending.next;
    let Some(next) = action.checked_add(1) else {
        return UNSPECIFIED;
    };
    pending.change = Some((action, rate));
    pending.next = next;
    if worker.sender.try_send(Some(action)).is_err() {
        pending.change = None;
        return NOT_READY;
    }
    0
}

#[must_use]
pub(super) fn perform_rate_change(action: u64) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(mut pending) = r.pending.lock() else {
        return UNSPECIFIED;
    };
    let Some((id, rate)) = pending.change else {
        return ILLEGAL;
    };
    if id != action {
        return ILLEGAL;
    }
    pending.change = None;
    let Ok(_clients) = r.clients.lock() else {
        return UNSPECIFIED;
    };
    if r.started.load(Ordering::Acquire) != 0 {
        return ILLEGAL;
    }
    reset_clock(r, Some(rate))
}

#[must_use]
pub(super) fn abort_rate_change(action: u64) -> i32 {
    let Some(r) = runtime() else {
        return NOT_READY;
    };
    let Ok(mut pending) = r.pending.lock() else {
        return UNSPECIFIED;
    };
    if pending.change.is_some_and(|(id, _)| id == action) {
        pending.change = None;
        0
    } else {
        ILLEGAL
    }
}

#[must_use]
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
        super::ZERO_TIMESTAMP_PERIOD,
        r.audio.counters().generation,
    )?;
    let timestamp = clock.timestamp(system::now())?;
    if before != r.clock_sequence.load(Ordering::Acquire) {
        return None;
    }
    Some(timestamp)
}
