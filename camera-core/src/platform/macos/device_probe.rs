//! The one AVFoundation call ADR-0002's prerequisite P2 needs: is a device
//! called `OBS Virtual Camera` registered?
//!
//! This is deliberately not a camera permission check and not a capture path: an
//! enumeration of device names needs no permission and opens nothing. The
//! discovery session names `[.external, .builtInWideAngleCamera,
//! .continuityCamera, .deskViewCamera]` exactly as the ADR does; `.externalUnknown`
//! is deprecated on macOS 26 and is not asked for.
//!
//! `objc2-av-foundation` is not in this build's offline registry, so the four
//! selectors this check needs are sent directly. That is why the workspace lint is
//! `deny` rather than `forbid`: `avfoundation` below is the only module in the
//! workspace that allows `unsafe_code`, and it holds nothing but the foreign call.

use std::fmt;

/// The `localizedName` an approved OBS camera extension registers.
pub const OBS_VIRTUAL_CAMERA_DEVICE_NAME: &str = "OBS Virtual Camera";

/// The AVFoundation listing could not be asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProbeFailure {
    detail: String,
}

impl DeviceProbeFailure {
    /// Builds a failure with the AVFoundation explanation.
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for DeviceProbeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the AVFoundation camera listing could not be read: {}",
            self.detail
        )
    }
}

impl std::error::Error for DeviceProbeFailure {}

/// How the backend asks whether the virtual camera device exists.
///
/// A trait rather than a free function so that the prerequisite checks and the
/// post-start confirmation can be driven without a camera extension, which is
/// exactly the state this machine is in.
pub trait VirtualCameraDeviceProbe: Send + Sync {
    /// Whether a camera named [`OBS_VIRTUAL_CAMERA_DEVICE_NAME`] is registered.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceProbeFailure`] when AVFoundation cannot be asked at all,
    /// which is a different failure from a listing that lacks the device.
    fn virtual_camera_device_present(&self) -> Result<bool, DeviceProbeFailure>;
}

/// The real probe: an AVFoundation discovery session over the device types
/// ADR-0002 names.
#[derive(Clone, Copy, Debug, Default)]
pub struct AvFoundationDeviceProbe;

impl AvFoundationDeviceProbe {
    /// The probe this platform uses.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Every camera `localizedName` AVFoundation lists, in its own order.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceProbeFailure`] when the discovery session cannot be
    /// created.
    pub fn camera_localized_names(&self) -> Result<Vec<String>, DeviceProbeFailure> {
        avfoundation::camera_localized_names().map_err(DeviceProbeFailure::new)
    }
}

impl VirtualCameraDeviceProbe for AvFoundationDeviceProbe {
    fn virtual_camera_device_present(&self) -> Result<bool, DeviceProbeFailure> {
        let names = self.camera_localized_names()?;
        Ok(names
            .iter()
            .any(|name| name == OBS_VIRTUAL_CAMERA_DEVICE_NAME))
    }
}

#[allow(unsafe_code)]
mod avfoundation {
    use std::ffi::{c_char, CStr};

    use objc2::msg_send;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyClass, AnyObject};

    // The device types ADR-0002 names. `.external` replaced the deprecated
    // `.externalUnknown` on macOS 26, and a Desk View camera is listed under its
    // own type, which is why it is asked for by name.
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVCaptureDeviceTypeExternal: *mut AnyObject;
        static AVCaptureDeviceTypeBuiltInWideAngleCamera: *mut AnyObject;
        static AVCaptureDeviceTypeContinuityCamera: *mut AnyObject;
        static AVCaptureDeviceTypeDeskViewCamera: *mut AnyObject;
        static AVMediaTypeVideo: *mut AnyObject;
    }

    /// `AVCaptureDevicePositionUnspecified`.
    const POSITION_UNSPECIFIED: isize = 0;

    pub(super) fn camera_localized_names() -> Result<Vec<String>, String> {
        autoreleasepool(|_pool| {
            // SAFETY: every call below is a message to an Objective-C object that
            // AVFoundation owns, with the argument types and the return type its
            // header declares; the objects are kept alive by the autorelease pool
            // this function opens, and nothing is retained past the pool.
            unsafe { names_in_session() }
        })
    }

    unsafe fn names_in_session() -> Result<Vec<String>, String> {
        // The discovery session is built by `AVCaptureDeviceDiscoverySession`,
        // not by `AVCaptureDevice`: the class method lives on the session type.
        let session_class = AnyClass::get(c"AVCaptureDeviceDiscoverySession")
            .ok_or_else(|| "AVFoundation is not loaded, so no camera can be listed".to_owned())?;
        let array_class =
            AnyClass::get(c"NSArray").ok_or_else(|| "Foundation is not loaded".to_owned())?;

        let device_types = [
            AVCaptureDeviceTypeExternal,
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeContinuityCamera,
            AVCaptureDeviceTypeDeskViewCamera,
        ];
        let requested: *mut AnyObject = msg_send![
            array_class,
            arrayWithObjects: device_types.as_ptr(),
            count: device_types.len()
        ];
        if requested.is_null() {
            return Err("the device types could not be assembled".to_owned());
        }

        let media_type: *mut AnyObject = AVMediaTypeVideo;
        let session: *mut AnyObject = msg_send![
            session_class,
            discoverySessionWithDeviceTypes: requested,
            mediaType: media_type,
            position: POSITION_UNSPECIFIED
        ];
        if session.is_null() {
            return Err("AVFoundation returned no discovery session".to_owned());
        }

        let devices: *mut AnyObject = msg_send![session, devices];
        if devices.is_null() {
            return Ok(Vec::new());
        }

        let count: usize = msg_send![devices, count];
        let mut names = Vec::with_capacity(count);
        for index in 0..count {
            let device: *mut AnyObject = msg_send![devices, objectAtIndex: index];
            if device.is_null() {
                continue;
            }
            let name: *mut AnyObject = msg_send![device, localizedName];
            if name.is_null() {
                continue;
            }
            let utf8: *const c_char = msg_send![name, UTF8String];
            if utf8.is_null() {
                continue;
            }
            if let Ok(name) = CStr::from_ptr(utf8).to_str() {
                names.push(name.to_owned());
            }
        }
        Ok(names)
    }
}
