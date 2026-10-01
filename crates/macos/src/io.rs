//! Control-thread IOProc ownership and callback-context lifetime.
use crate::{capture::CaptureCallback, output::OutputCallback};
use channelbridge_hal_abi::{AudioBufferList, AudioTimestamp, ObjectId};
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
        unsafe {
            OutputCallback::process(self, output, None);
        }
    }
}
struct Context<T> {
    handler: T,
    accepting: AtomicBool,
    active: AtomicUsize,
    failed: AtomicBool,
}
struct Active<'a>(&'a AtomicUsize);
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
    context.active.fetch_add(1, Ordering::Acquire);
    let _active = Active(&context.active);
    if context.accepting.load(Ordering::Acquire)
        && !context.failed.load(Ordering::Acquire)
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
#[derive(Debug)]
pub struct IoError {
    pub operation: &'static str,
    pub status: i32,
    pub context_retained: bool,
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

/// Thread-confined owner. Drop never releases memory if Stop/Destroy fails.
pub struct Registration<T: Handler> {
    device: ObjectId,
    id: Option<IoProc>,
    context: Option<Box<Context<T>>>,
    start_attempted: bool,
    api: Api,
    _control_thread: PhantomData<Rc<()>>,
}
impl<T: Handler> Registration<T> {
    pub fn new(device: ObjectId, handler: T) -> Result<Self, IoError> {
        Self::with_api(device, handler, HAL)
    }
    fn with_api(device: ObjectId, handler: T, api: Api) -> Result<Self, IoError> {
        let mut context = Box::new(Context {
            handler,
            accepting: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
        });
        let mut id = None;
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
            });
        }
        if id.is_none() {
            // A malformed successful registration cannot be safely deregistered.
            let _ = Box::into_raw(context);
            return Err(IoError {
                operation: "register returned empty ID",
                status: 0,
                context_retained: true,
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
    pub fn start(&mut self) -> Result<(), IoError> {
        if self.start_attempted {
            return Err(IoError {
                operation: "already started",
                status: 0,
                context_retained: false,
            });
        }
        self.start_attempted = true;
        self.context
            .as_ref()
            .expect("live registration")
            .accepting
            .store(true, Ordering::Release);
        let status = unsafe { (self.api.start)(self.device, self.id.expect("registered ID")) };
        if status != 0 {
            self.context
                .as_ref()
                .expect("live registration")
                .accepting
                .store(false, Ordering::Release);
            return Err(IoError {
                operation: "start",
                status,
                context_retained: false,
            });
        }
        Ok(())
    }
    fn teardown(&mut self) -> Result<(), IoError> {
        let Some(id) = self.id else {
            return Ok(());
        };
        let context = self.context.as_ref().expect("registered context");
        context.accepting.store(false, Ordering::Release);
        if self.start_attempted {
            let status = unsafe { (self.api.stop)(self.device, id) };
            if status != 0 {
                return Err(IoError {
                    operation: "stop",
                    status,
                    context_retained: true,
                });
            }
        }
        let status = unsafe { (self.api.destroy)(self.device, id) };
        if status != 0 {
            return Err(IoError {
                operation: "destroy",
                status,
                context_retained: true,
            });
        }
        self.id = None;
        // HAL has removed this IOProc; retain storage if an invocation still
        // owns it rather than assuming Stop proves synchronous completion.
        if context.active.load(Ordering::Acquire) != 0 {
            return Err(IoError {
                operation: "callback still active",
                status: 0,
                context_retained: true,
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
        }
    }
    unsafe extern "C" fn create(
        _: ObjectId,
        callback: IoProc,
        _: *mut c_void,
        id: *mut Option<IoProc>,
    ) -> i32 {
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
    fn lifecycle_releases_only_after_deregistration_and_retains_on_errors() {
        let api = Api {
            create,
            start,
            stop,
            destroy,
        };
        let drops = Arc::new(AtomicUsize::new(0));
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api).unwrap();
        session.start().unwrap();
        assert!(session.start().is_err());
        drop(session.close().unwrap());
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(DESTROYS.load(Ordering::Relaxed), 1);
        STOP_ERROR.store(-7, Ordering::Relaxed);
        let mut session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api).unwrap();
        session.start().unwrap();
        let error = session.close().err().unwrap();
        assert_eq!(error.operation, "stop");
        assert!(error.context_retained);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(DESTROYS.load(Ordering::Relaxed), 1);
        STOP_ERROR.store(0, Ordering::Relaxed);
        DESTROY_ERROR.store(-8, Ordering::Relaxed);
        let session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api).unwrap();
        drop(session);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        DESTROY_ERROR.store(0, Ordering::Relaxed);
        let session = Registration::with_api(42, TestHandler(Arc::clone(&drops)), api).unwrap();
        session
            .context
            .as_ref()
            .unwrap()
            .active
            .store(1, Ordering::Release);
        assert_eq!(
            session.close().err().unwrap().operation,
            "callback still active"
        );
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        // Never-started registrations need only deregistration.
        drop(Registration::with_api(42, TestHandler(Arc::clone(&drops)), api).unwrap());
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }
}
