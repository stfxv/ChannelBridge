//! Control-thread IOProc ownership and callback-context lifetime.
use crate::{capture::CaptureCallback, output::OutputCallback};
use channelbridge_hal_abi::{AudioBufferList, AudioTimestamp, ObjectId, PropertyAddress, fourcc};
use std::{
    ffi::c_void,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub type IoProc = unsafe extern "C" fn(
    ObjectId,
    *const AudioTimestamp,
    *const AudioBufferList,
    *const AudioTimestamp,
    *mut AudioBufferList,
    *const AudioTimestamp,
    *mut c_void,
) -> i32;
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioDeviceCreateIOProcID(
        device: ObjectId,
        callback: IoProc,
        context: *mut c_void,
        id: *mut Option<IoProc>,
    ) -> i32;
    fn AudioDeviceStart(device: ObjectId, id: IoProc) -> i32;
    fn AudioDeviceStop(device: ObjectId, id: IoProc) -> i32;
    fn AudioDeviceDestroyIOProcID(device: ObjectId, id: IoProc) -> i32;
    fn AudioObjectSetPropertyData(
        object: ObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
}
#[derive(Clone, Copy)]
struct Api {
    create: unsafe extern "C" fn(ObjectId, IoProc, *mut c_void, *mut Option<IoProc>) -> i32,
    start: unsafe extern "C" fn(ObjectId, IoProc) -> i32,
    stop: unsafe extern "C" fn(ObjectId, IoProc) -> i32,
    destroy: unsafe extern "C" fn(ObjectId, IoProc) -> i32,
}
const HAL: Api = Api {
    create: AudioDeviceCreateIOProcID,
    start: AudioDeviceStart,
    stop: AudioDeviceStop,
    destroy: AudioDeviceDestroyIOProcID,
};

pub trait Handler: Send + Sync + 'static {
    /// # Safety
    /// All pointers and payloads have HAL's callback validity/ownership for the
    /// invocation. Implementations must not retain their borrows after return.
    unsafe fn process(
        &self,
        now: *const AudioTimestamp,
        input: *const AudioBufferList,
        input_time: *const AudioTimestamp,
        output: *mut AudioBufferList,
        output_time: *const AudioTimestamp,
    );
}
impl Handler for CaptureCallback {
    unsafe fn process(
        &self,
        now: *const AudioTimestamp,
        input: *const AudioBufferList,
        input_time: *const AudioTimestamp,
        _output: *mut AudioBufferList,
        _output_time: *const AudioTimestamp,
    ) {
        // SAFETY: Handler forwards the same HAL input and timestamp borrows for this invocation.
        unsafe {
            CaptureCallback::process(self, input, now, input_time);
        }
    }
}
impl Handler for OutputCallback {
    unsafe fn process(
        &self,
        _now: *const AudioTimestamp,
        _input: *const AudioBufferList,
        _input_time: *const AudioTimestamp,
        output: *mut AudioBufferList,
        _output_time: *const AudioTimestamp,
    ) {
        // SAFETY: Handler forwards the exclusive HAL output borrow for this invocation.
        unsafe {
            OutputCallback::process(self, output, None);
        }
    }
}
struct Context<T> {
    handler: T,
    admission: AtomicUsize,
    failed: AtomicBool,
}
const CLOSED: usize = 1 << (usize::BITS - 1);
const BAD_DEVICE: i32 = fourcc(b"!dev") as i32;
struct Active<'invocation>(&'invocation AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}
unsafe extern "C" fn dispatch<T: Handler>(
    _device: ObjectId,
    now: *const AudioTimestamp,
    input: *const AudioBufferList,
    input_time: *const AudioTimestamp,
    output: *mut AudioBufferList,
    output_time: *const AudioTimestamp,
    pointer: *mut c_void,
) -> i32 {
    if pointer.is_null() {
        return 0;
    }
    // SAFETY: registration owns this stable allocation until HAL deregisters.
    let context = unsafe { &*pointer.cast::<Context<T>>() };
    // A single atomic linearizes closing against callback admission. Retained
    // closed contexts may receive late HAL calls, but cannot run their handler.
    if context
        .admission
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
            (state & CLOSED == 0).then_some(state + 1)
        })
        .is_err()
    {
        return 0;
    }
    let _active = Active(&context.admission);
    // Defense in depth under panic=unwind. Failure permanently disables handler
    // reuse, while Active still releases admission during unwinding.
    if !context.failed.load(Ordering::Acquire)
        // SAFETY: HAL keeps all callback arguments live; admission keeps context owned
        // until return.
        && catch_unwind(AssertUnwindSafe(|| unsafe {
            context
                .handler
                .process(now, input, input_time, output, output_time);
        }))
        .is_err()
    {
        context.failed.store(true, Ordering::Release);
    }
    // HAL supplies zeroed output; a rejected invocation leaves silence.
    0
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoDisposition {
    Fatal,
    /// The device ID vanished. Storage remains retained, but its admission gate
    /// is closed with no handler running, so a new independent session is safe.
    RemovedDevice,
}
#[derive(Debug, Clone, Copy)]
pub struct IoError {
    pub operation: &'static str,
    pub status: i32,
    pub context_retained: bool,
    pub disposition: IoDisposition,
}
impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed ({}, callback context retained: {})",
            self.operation, self.status, self.context_retained
        )
    }
}
impl std::error::Error for IoError {}

#[derive(Clone, Copy)]
pub enum Direction {
    Input,
    Output,
}
fn disabled_usage(id: IoProc, count: usize) -> Option<(Vec<u64>, u32)> {
    if !(1..=512).contains(&count) {
        return None;
    }
    let size = 12 + count * 4;
    let mut payload = vec![0_u64; size.div_ceil(8)];
    payload[0] = id as usize as u64;
    // Native-endian bytes avoid conflating the UInt32 count with the first flag.
    let bytes = (count as u32).to_ne_bytes();
    payload[1] = u64::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0]);
    Some((payload, size as u32))
}

/// Thread-confined owner. Callback storage is released only after successful
/// deregistration with no admitted invocation still active.
pub struct Registration<T: Handler> {
    device: ObjectId,
    id: Option<IoProc>,
    context: Option<Box<Context<T>>>,
    start_attempted: bool,
    api: Api,
    _control_thread: PhantomData<Rc<()>>,
}
impl<T: Handler> Registration<T> {
    pub fn try_new(device: ObjectId, handler: T) -> Result<Self, IoError> {
        Self::with_api(device, handler, HAL)
    }
    fn with_api(device: ObjectId, handler: T, api: Api) -> Result<Self, IoError> {
        let mut context = Box::new(Context {
            handler,
            admission: AtomicUsize::new(CLOSED),
            failed: AtomicBool::new(false),
        });
        let mut id = None;
        // SAFETY: context is a stable boxed allocation retained until deregistration; id
        // is a writable output slot.
        let status = unsafe {
            (api.create)(
                device,
                dispatch::<T>,
                (&mut *context as *mut Context<T>).cast(),
                &mut id,
            )
        };
        if status != 0 {
            return Err(IoError {
                operation: "register",
                status,
                context_retained: false,
                disposition: IoDisposition::Fatal,
            });
        }
        if id.is_none() {
            // A malformed successful registration cannot be safely deregistered.
            let _ = Box::into_raw(context);
            return Err(IoError {
                operation: "register returned empty ID",
                status: 0,
                context_retained: true,
                disposition: IoDisposition::Fatal,
            });
        }
        Ok(Self {
            device,
            id,
            context: Some(context),
            start_attempted: false,
            api,
            _control_thread: PhantomData,
        })
    }
    pub fn handler(&self) -> &T {
        &self.context.as_ref().expect("live registration").handler
    }
    /// Disable this IOProc's streams in one direction before Start. This does
    /// not change global stream activity or other clients' stream usage.
    pub fn disable_streams(
        &mut self,
        direction: Direction,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.start_attempted {
            return Err("stream usage must be configured before Start".into());
        }
        let scope = match direction {
            Direction::Input => fourcc(b"inpt"),
            Direction::Output => fourcc(b"outp"),
        };
        let streams = crate::devices::property(self.device, fourcc(b"stm#"), scope)?;
        if streams.len() % 4 != 0 || streams.len() / 4 > 512 {
            return Err("invalid stream list".into());
        }
        let count = streams.len() / 4;
        if count == 0 {
            return Ok(());
        }
        let (payload, size) = disabled_usage(self.id.expect("registered ID"), count)
            .ok_or("invalid stream usage size")?;
        let address = PropertyAddress {
            selector: fourcc(b"suse"),
            scope,
            element: 0,
        };
        // SAFETY: initialized aligned payload follows SDK's 8-byte pointer,
        // 4-byte count and UInt32 flags; all streams in this scope are included.
        let status = unsafe {
            AudioObjectSetPropertyData(
                self.device,
                &address,
                0,
                std::ptr::null(),
                size,
                payload.as_ptr().cast(),
            )
        };
        if status != 0 {
            return Err(IoError {
                operation: "disable streams",
                status,
                context_retained: false,
                disposition: IoDisposition::Fatal,
            }
            .into());
        }
        Ok(())
    }
    pub fn start(&mut self) -> Result<(), IoError> {
        if self.start_attempted {
            return Err(IoError {
                operation: "already started",
                status: 0,
                context_retained: false,
                disposition: IoDisposition::Fatal,
            });
        }
        self.start_attempted = true;
        self.context
            .as_ref()
            .expect("live registration")
            .admission
            .store(0, Ordering::Release);
        // SAFETY: The registered ID belongs to this device and its boxed callback
        // context remains owned.
        let status = unsafe { (self.api.start)(self.device, self.id.expect("registered ID")) };
        if status != 0 {
            self.context
                .as_ref()
                .expect("live registration")
                .admission
                .fetch_or(CLOSED, Ordering::AcqRel);
            return Err(IoError {
                operation: "start",
                status,
                context_retained: false,
                disposition: IoDisposition::Fatal,
            });
        }
        Ok(())
    }
    fn teardown(&mut self) -> Result<(), IoError> {
        let Some(id) = self.id else {
            return Ok(());
        };
        let context = self.context.as_ref().expect("registered context");
        context.admission.fetch_or(CLOSED, Ordering::AcqRel);
        let stop_status = if self.start_attempted {
            // SAFETY: The ID is still registered and context remains owned through
            // stop and deregistration.
            unsafe { (self.api.stop)(self.device, id) }
        } else {
            0
        };
        if stop_status != 0 && stop_status != BAD_DEVICE {
            return Err(IoError {
                operation: "stop",
                status: stop_status,
                context_retained: true,
                disposition: IoDisposition::Fatal,
            });
        }
        // SAFETY: The ID is still registered; context is retained until successful
        // deregistration and no active callbacks.
        let status = unsafe { (self.api.destroy)(self.device, id) };
        if status != 0 {
            let inactive = context.admission.load(Ordering::Acquire) == CLOSED;
            return Err(IoError {
                operation: "destroy",
                status,
                context_retained: true,
                disposition: if status == BAD_DEVICE
                    && inactive
                    && !context.failed.load(Ordering::Acquire)
                {
                    IoDisposition::RemovedDevice
                } else {
                    IoDisposition::Fatal
                },
            });
        }
        self.id = None;
        // HAL has removed this IOProc; retain storage if an invocation still
        // owns it rather than assuming Stop proves synchronous completion.
        if context.admission.load(Ordering::Acquire) != CLOSED {
            return Err(IoError {
                operation: "callback still active",
                status: 0,
                context_retained: true,
                disposition: IoDisposition::Fatal,
            });
        }
        Ok(())
    }
    fn retain(&mut self) {
        if let Some(context) = self.context.take() {
            let _ = Box::into_raw(context);
        }
        self.id = None;
    }
    pub fn close(mut self) -> Result<T, IoError> {
        if let Err(error) = self.teardown() {
            self.retain();
            return Err(error);
        }
        if self
            .context
            .as_ref()
            .expect("owned context")
            .failed
            .load(Ordering::Acquire)
        {
            return Err(IoError {
                operation: "callback panic",
                status: 0,
                context_retained: false,
                disposition: IoDisposition::Fatal,
            });
        }
        Ok(self.context.take().expect("owned context").handler)
    }
}
impl<T: Handler> Drop for Registration<T> {
    fn drop(&mut self) {
        if self.teardown().is_err() {
            self.retain();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicI32, AtomicUsize},
    };
    static STOP_ERROR: AtomicI32 = AtomicI32::new(0);
    static DESTROY_ERROR: AtomicI32 = AtomicI32::new(0);
    static DESTROYS: AtomicUsize = AtomicUsize::new(0);
    struct TestHandler(Arc<AtomicUsize>);
    impl Drop for TestHandler {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    impl Handler for TestHandler {
        unsafe fn process(
            &self,
            _: *const AudioTimestamp,
            _: *const AudioBufferList,
            _: *const AudioTimestamp,
            _: *mut AudioBufferList,
            _: *const AudioTimestamp,
        ) {
            self.0.fetch_add(100, Ordering::Relaxed);
        }
    }
    unsafe extern "C" fn create(
        _: ObjectId,
        callback: IoProc,
        _: *mut c_void,
        id: *mut Option<IoProc>,
    ) -> i32 {
        // SAFETY: with_api supplies a writable Option<IoProc> slot for this mock registration call.
        unsafe {
            *id = Some(callback);
        }
        0
    }
    unsafe extern "C" fn start(_: ObjectId, _: IoProc) -> i32 {
        0
    }
    unsafe extern "C" fn stop(_: ObjectId, _: IoProc) -> i32 {
        STOP_ERROR.load(Ordering::Relaxed)
    }
    unsafe extern "C" fn destroy(_: ObjectId, _: IoProc) -> i32 {
        DESTROYS.fetch_add(1, Ordering::Relaxed);
        DESTROY_ERROR.load(Ordering::Relaxed)
    }
    #[test]
    fn closing_admission_observes_inflight_handler_and_rejects_late_calls() {
        struct BlockingHandler {
            entered: std::sync::Barrier,
            release: std::sync::Barrier,
            calls: AtomicUsize,
        }
        impl Handler for BlockingHandler {
            unsafe fn process(
                &self,
                _: *const AudioTimestamp,
                _: *const AudioBufferList,
                _: *const AudioTimestamp,
                _: *mut AudioBufferList,
                _: *const AudioTimestamp,
            ) {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.entered.wait();
                self.release.wait();
            }
        }
        let context = Arc::new(Context {
            handler: BlockingHandler {
                entered: std::sync::Barrier::new(2),
                release: std::sync::Barrier::new(2),
                calls: AtomicUsize::new(0),
            },
            admission: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
        });
        let running = Arc::clone(&context);
        // SAFETY: running retains the context through dispatch; BlockingHandler never
        // dereferences the null audio arguments.
        let callback = std::thread::spawn(move || unsafe {
            dispatch::<BlockingHandler>(
                42,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                Arc::as_ptr(&running).cast_mut().cast(),
            );
        });
        context.handler.entered.wait();
        assert_eq!(context.admission.fetch_or(CLOSED, Ordering::AcqRel), 1);
        // SAFETY: context retains the allocation; closed admission prevents calling the
        // handler with null audio arguments.
        unsafe {
            dispatch::<BlockingHandler>(
                42,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                Arc::as_ptr(&context).cast_mut().cast(),
            );
        }
        assert_eq!(context.handler.calls.load(Ordering::Relaxed), 1);
        assert_eq!(context.admission.load(Ordering::Acquire), CLOSED | 1);
        context.handler.release.wait();
        callback
            .join()
            .expect("fixture worker completes without panic");
        assert_eq!(context.admission.load(Ordering::Acquire), CLOSED);
    }
    #[test]
    fn stream_usage_payload_includes_id_count_and_every_disabled_flag() {
        let (payload, size) = disabled_usage(dispatch::<TestHandler>, 3)
            .expect("three streams fit the supported disabled-usage payload range");
        assert_eq!(size, 24);
        assert_eq!(
            payload[0],
            dispatch::<TestHandler> as *const () as usize as u64
        );
        let bytes =
            // SAFETY: The initialized word allocation covers size bytes and remains
            // borrowed throughout this inspection.
            unsafe { std::slice::from_raw_parts(payload.as_ptr().cast::<u8>(), size as usize) };
        assert_eq!(
            u32::from_ne_bytes(
                bytes[8..12]
                    .try_into()
                    .expect("the stream-count field occupies exactly four bytes")
            ),
            3
        );
        assert!(bytes[12..].iter().all(|byte| *byte == 0));
        assert!(disabled_usage(dispatch::<TestHandler>, 0).is_none());
        assert!(disabled_usage(dispatch::<TestHandler>, 513).is_none());
    }
    #[test]
    fn lifecycle_releases_only_after_deregistration_and_retains_on_errors() {
        let api = Api {
            create,
            start,
            stop,
            destroy,
        };
        let drops = Arc::new(AtomicUsize::new(0));
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .start()
            .expect("new registration is not started and the fake start hook returns success");
        assert!(session.start().is_err());
        drop(
            session
                .close()
                .expect("fake stop and destroy hooks both return success"),
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(DESTROYS.load(Ordering::Relaxed), 1);
        STOP_ERROR.store(-7, Ordering::Relaxed);
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .start()
            .expect("new registration is not started and the fake start hook returns success");
        let error = session
            .close()
            .err()
            .expect("injected stop status -7 makes close fail");
        assert_eq!(error.operation, "stop");
        assert!(error.context_retained);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(DESTROYS.load(Ordering::Relaxed), 1);
        STOP_ERROR.store(0, Ordering::Relaxed);
        DESTROY_ERROR.store(-8, Ordering::Relaxed);
        let session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        drop(session);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        DESTROY_ERROR.store(0, Ordering::Relaxed);
        let session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .context
            .as_ref()
            .expect("new registration owns its context until the consuming close call")
            .admission
            .store(CLOSED | 1, Ordering::Release);
        assert_eq!(
            session
                .close()
                .err()
                .expect("the injected active callback prevents close from releasing its context")
                .operation,
            "callback still active"
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        // A disappearing device still gets a deregistration attempt.
        STOP_ERROR.store(BAD_DEVICE, Ordering::Relaxed);
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .start()
            .expect("new registration is not started and the fake start hook returns success");
        drop(
            session
                .close()
                .expect("removed-device stop still permits successful fake deregistration"),
        );
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        DESTROY_ERROR.store(BAD_DEVICE, Ordering::Relaxed);
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .start()
            .expect("new registration is not started and the fake start hook returns success");
        let pointer = (&**session
            .context
            .as_ref()
            .expect("new registration owns its context until the consuming close call")
            as *const Context<TestHandler>)
            .cast_mut()
            .cast();
        let error = session
            .close()
            .err()
            .expect("injected BAD_DEVICE deregistration status makes close report removal");
        assert_eq!(error.disposition, IoDisposition::RemovedDevice);
        assert!(error.context_retained);
        // HAL can still call the retained pointer; its handler is closed forever.
        // SAFETY: Failed teardown retained the boxed context; closed admission rejects
        // all null audio arguments.
        unsafe {
            dispatch::<TestHandler>(
                42,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                pointer,
            );
        }
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .start()
            .expect("new registration is not started and the fake start hook returns success");
        session
            .context
            .as_ref()
            .expect("new registration owns its context until the consuming close call")
            .admission
            .store(1, Ordering::Release);
        assert_eq!(
            session
                .close()
                .err()
                .expect("the injected active callback makes teardown fatal")
                .disposition,
            IoDisposition::Fatal
        );
        // Startup cleanup can lose the device before Start was attempted.
        let session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        assert_eq!(
            session
                .close()
                .err()
                .expect("injected BAD_DEVICE deregistration status reports removal before start")
                .disposition,
            IoDisposition::RemovedDevice
        );
        let session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .context
            .as_ref()
            .expect("new registration owns its context until the consuming close call")
            .failed
            .store(true, Ordering::Release);
        assert_eq!(
            session
                .close()
                .err()
                .expect("the injected callback failure makes teardown fatal")
                .disposition,
            IoDisposition::Fatal
        );
        DESTROY_ERROR.store(-8, Ordering::Relaxed);
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api)
            .expect("fake create hook registers device 42 and supplies a nonnull callback handle");
        session
            .start()
            .expect("new registration is not started and the fake start hook returns success");
        assert_eq!(
            session
                .close()
                .err()
                .expect("injected destroy status -8 makes teardown fatal")
                .disposition,
            IoDisposition::Fatal
        );
        STOP_ERROR.store(0, Ordering::Relaxed);
        DESTROY_ERROR.store(0, Ordering::Relaxed);
        // Never-started registrations need only deregistration.
        drop(
            Registration::with_api(42, TestHandler(Arc::clone(&drops)), api).expect(
                "fake create hook registers device 42 and supplies a nonnull callback handle",
            ),
        );
        assert_eq!(drops.load(Ordering::Relaxed), 3);
    }
}
