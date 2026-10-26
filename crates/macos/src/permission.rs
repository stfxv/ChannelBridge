//! Microphone permission boundary. Querying status never requests access.
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::{NSBundle, ns_string};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
    Unavailable,
}
impl Permission {
    #[must_use]
    pub fn allowed(self) -> bool {
        self == Self::Authorized
    }
}
fn translate(status: AVAuthorizationStatus) -> Permission {
    match status {
        AVAuthorizationStatus::NotDetermined => Permission::NotDetermined,
        AVAuthorizationStatus::Restricted => Permission::Restricted,
        AVAuthorizationStatus::Denied => Permission::Denied,
        AVAuthorizationStatus::Authorized => Permission::Authorized,
        _ => Permission::Unavailable,
    }
}
#[must_use]
pub fn status() -> Permission {
    // SAFETY: framework's immutable constant is audio, the supported media type.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Permission::Unavailable;
    };
    // SAFETY: audio is the framework-provided live media-type constant; this class method
    // only reads authorization.
    translate(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Pending,
    AlreadyDetermined(Permission),
}
#[derive(Default)]
pub struct MicrophonePermission {
    pending: Arc<AtomicBool>,
}
impl MicrophonePermission {
    /// Invoke from an explicit UI action with the built application's main-thread
    /// marker. Completion only updates atomic state; UI polls the actual status.
    pub fn request(&self, _main: MainThreadMarker) -> Result<Request, &'static str> {
        let current = status();
        if current != Permission::NotDetermined {
            return Ok(Request::AlreadyDetermined(current));
        }
        // Missing usage description causes OS termination. Require a nonempty
        // NSString in the running bundle before calling the request API.
        let value = NSBundle::mainBundle()
            .objectForInfoDictionaryKey(ns_string!("NSMicrophoneUsageDescription"));
        let description = value
            .as_ref()
            .and_then(|value| value.downcast_ref::<objc2_foundation::NSString>());
        if description.is_none_or(|value| value.is_empty()) {
            return Err("application bundle has no microphone usage description");
        }
        if self
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(Request::Pending);
        }
        // SAFETY: AVMediaTypeAudio is an immutable framework constant borrowed for the request.
        let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
            self.pending.store(false, Ordering::Release);
            return Err("audio permission API is unavailable");
        };
        let pending = Arc::clone(&self.pending);
        let completion = RcBlock::new(move |_allowed: Bool| {
            pending.store(false, Ordering::Release);
        });
        // SAFETY: supported audio constant, valid retained block; AVFoundation
        // copies the escaping block. No UI or worker-owned data enters it.
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &completion);
        }
        Ok(Request::Pending)
    }
    #[must_use]
    pub fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_authorized_status_can_enable_capture() {
        for state in [
            AVAuthorizationStatus::NotDetermined,
            AVAuthorizationStatus::Restricted,
            AVAuthorizationStatus::Denied,
            AVAuthorizationStatus(-1),
        ] {
            assert!(!translate(state).allowed());
        }
        assert!(translate(AVAuthorizationStatus::Authorized).allowed());
    }
}
