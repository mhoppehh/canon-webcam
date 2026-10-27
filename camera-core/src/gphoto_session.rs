//! The **PTP session** bound to libgphoto2 through the `gphoto2` crate.
//!
//! # The binding decision (spike result)
//!
//! The choice was settled by inspecting `gphoto2` 3.4.1 rather than by argument
//! (see `docs/adr/0003-gphoto2-crate-binding.md`). The three capabilities the
//! design needs all exist in its public API, and a compile of a spike against
//! libgphoto2 2.5.34 proved them:
//!
//! 1. **Write the `output` property.** `Camera::config_key::<RadioWidget>` then
//!    `RadioWidget::set_choice` then `Camera::set_config`.
//! 2. **Press the viewfinder action.** `Camera::config_key::<Widget>` returns a
//!    `ButtonWidget` (or a toggle or text widget); `ButtonWidget::press`,
//!    `ToggleWidget::set_toggled` and `TextWidget::set_value` are all available.
//! 3. **Return preview frames with their byte payload.**
//!    `Camera::capture_preview` returns a `CameraFile`, and
//!    `CameraFile::get_data` returns the `Box<[u8]>` payload.
//!
//! Unverified, because no camera was attached: which widget type the M50 reports
//! for `/main/actions/viewfinder`, and whether preview capture on an M50 returns
//! full size frames while `output` is `TFT + PC`. Each branch below is written to
//! fail with a message rather than to guess.
//!
//! ```text
//! UNVERIFIED (no camera attached): the M50's widget type for
//! /main/actions/viewfinder, and the frame size that capture_preview returns.
//! ```
//!
//! # Why this binding
//!
//! A single owned `Camera` handle makes "one PTP session per run" structural:
//! the handle is created once, held in [`CameraSession`], and every operation in
//! the run goes through it. No subprocess is started, so the failure mode in
//! `docs/adr/0001-single-ptp-session.md` (the second `gphoto2` process being
//! rejected) cannot arise.
//!
//! `CameraSession` is deliberately not `Clone` and cannot be built except through
//! [`CameraSession::open`], which takes the process wide claim. A second session
//! is therefore an error, not a second handle.
//!
//! # A detected device is not necessarily a camera
//!
//! `gp_camera_autodetect` answers with the `Directory Browse` pseudo-camera when
//! nothing else is attached, and that handle then fails every operation with
//! `Bad parameters`. Opening it would report a **disconnected camera** as a live
//! session, so [`CameraSession::open`] lists the descriptors and refuses any that
//! is not on a bus.

use gphoto2::error::ErrorKind;
use gphoto2::list::CameraDescriptor;
use gphoto2::widget::{RadioWidget, Widget};
use gphoto2::{Camera, Context};

use crate::output_setting::OutputSetting;
use crate::recovery::PtpSessionOpener;
use crate::session::{
    process_session_exclusivity, PtpSession, SessionClaim, SessionError, SessionOperation,
};

/// The Canon property that carries the **output setting**.
const OUTPUT_PROPERTY: &str = "output";

/// The Canon action that turns live view on at the full frame size.
///
/// This is the widget's **name**, not its path. `gp_camera_get_single_config`
/// takes a name: handing it `/main/actions/viewfinder` is refused by an M50 with
/// `Bad parameters`, which reads in the error as though the camera had rejected
/// the action itself. Measured on an M50, firmware 3-1.1.0, libgphoto2 2.5.34:
/// the read fails with the path and succeeds with the name, and
/// `gphoto2 --set-config /main/actions/viewfinder=1` — which walks the whole tree
/// by path — succeeds throughout.
const VIEWFINDER_ACTION: &str = "viewfinder";

/// The action path as a user knows it from `gphoto2 --get-config`, for messages.
const VIEWFINDER_ACTION_PATH: &str = "/main/actions/viewfinder";

/// The port prefix of libgphoto2's `disk:` backend, which browses a directory on
/// this computer. It is not a camera on a bus, so it can never be the M50.
const DIRECTORY_BACKEND_PORT_PREFIX: &str = "disk:";

/// Whether a detected device is a camera on a bus rather than a directory.
/// `gp_camera_autodetect` answers with the `Directory Browse` pseudo-camera when
/// nothing else is attached, and its camera handle then fails every operation
/// with `Bad parameters`. Treating that as a camera would make "no camera
/// attached" look like a working session, so the descriptor is checked first.
fn is_camera_on_a_bus(port: &str) -> bool {
    !port.starts_with(DIRECTORY_BACKEND_PORT_PREFIX)
}

/// A radio choice compared without the spacing or case a camera is free to
/// differ in: `TFT + PC` and `TFT+PC` name the same value.
fn normalise_choice(choice: &str) -> String {
    choice
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn describe_detected(descriptors: &[CameraDescriptor]) -> String {
    if descriptors.is_empty() {
        return "libgphoto2 detected no device at all".to_owned();
    }
    let listed = descriptors
        .iter()
        .map(|descriptor| format!("{} on {}", descriptor.model, descriptor.port))
        .collect::<Vec<_>>()
        .join(", ");
    format!("libgphoto2 detected {listed}, but none of them is a camera on a bus")
}

/// The one PTP session of a run, over libgphoto2.
///
/// Construct with [`CameraSession::open`] and pass it by reference: no method on
/// it opens another session, and it is not `Clone`.
pub struct CameraSession {
    camera: Camera,
    model: String,
    // Held for the life of the session; dropping it releases the claim. The
    // claim is not `Clone`, so this session cannot be duplicated.
    _claim: SessionClaim<'static>,
}

impl CameraSession {
    /// Opens the camera and takes the one session claim.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::AlreadyOpen`] if a session exists, and
    /// [`SessionError::NoCamera`] if no camera can be opened. A camera that is
    /// absent from the USB bus is reported as a **disconnected camera**, which is
    /// a different state from a **wedged camera**.
    pub fn open() -> Result<Self, SessionError> {
        // Taken before the camera is touched, so a second caller never reaches
        // libgphoto2 at all.
        let claim = process_session_exclusivity().claim()?;

        let context = Context::new().map_err(|error| SessionError::NoCamera {
            detail: error.to_string(),
        })?;

        // `autodetect_camera` is deliberately not used: it also answers with the
        // `Directory Browse` pseudo-camera, whose handle then fails every
        // operation with `Bad parameters`, which would report a disconnected
        // camera as a live session.
        let detected = context
            .list_cameras()
            .wait()
            .map_err(|error| SessionError::NoCamera {
                detail: describe(error),
            })?
            .collect::<Vec<_>>();
        let Some(descriptor) = detected
            .iter()
            .find(|descriptor| is_camera_on_a_bus(&descriptor.port))
        else {
            return Err(SessionError::NoCamera {
                detail: describe_detected(&detected),
            });
        };

        let camera =
            context
                .get_camera(descriptor)
                .wait()
                .map_err(|error| SessionError::NoCamera {
                    detail: describe(error),
                })?;
        let model = camera.abilities().model().into_owned();

        Ok(Self {
            camera,
            model,
            _claim: claim,
        })
    }

    fn rejected(operation: SessionOperation, error: gphoto2::Error) -> SessionError {
        SessionError::Camera {
            operation,
            detail: describe(error),
        }
    }
}

/// Opens the one **PTP session** of a run, and a fresh one after a **recovery
/// reset** has destroyed a wedged session.
///
/// It exists so that recovery never has to keep two sessions alive: the wedged
/// session is dropped, `gphoto2 --reset` runs as its own process — the one place
/// where that is correct, per `docs/adr/0001-single-ptp-session.md` — and then
/// this opener takes the session claim again.
#[derive(Debug, Default)]
pub struct CameraSessionOpener;

impl CameraSessionOpener {
    /// A session opener.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PtpSessionOpener for CameraSessionOpener {
    fn open_session(&mut self) -> Result<Box<dyn PtpSession + Send>, SessionError> {
        Ok(Box::new(CameraSession::open()?))
    }
}

/// Includes the libgphoto2 error kind, so a **wedged camera** timeout is visible
/// in the message rather than flattened into an opaque string.
fn describe(error: gphoto2::Error) -> String {
    let kind = error.kind();
    if kind == ErrorKind::Timeout {
        format!("{error} (the camera timed out; it may be a wedged camera)")
    } else {
        error.to_string()
    }
}

impl PtpSession for CameraSession {
    fn model(&self) -> &str {
        &self.model
    }

    fn set_output_setting(&mut self, setting: OutputSetting) -> Result<(), SessionError> {
        let operation = SessionOperation::SetOutputSetting;

        // The `output` property is a radio widget whose choices are the labels
        // `OutputSetting::choice_name` returns.
        let widget = self
            .camera
            .config_key::<RadioWidget>(OUTPUT_PROPERTY)
            .wait()
            .map_err(|error| Self::rejected(operation, error))?;
        widget
            .set_choice(setting.choice_name())
            .map_err(|error| Self::rejected(operation, error))?;
        self.camera
            .set_config(&widget)
            .wait()
            .map_err(|error| Self::rejected(operation, error))?;

        // Read it back. A write the camera accepts and then does not keep is the
        // one failure this program must not paper over: the frame size, and
        // whether the camera's own screen mirrors the feed at all, rest on the
        // value the camera holds rather than the value that was asked for.
        let readback = self
            .camera
            .config_key::<RadioWidget>(OUTPUT_PROPERTY)
            .wait()
            .map_err(|error| Self::rejected(operation, error))?;
        let kept = readback.choice();
        if normalise_choice(&kept) != normalise_choice(setting.choice_name()) {
            return Err(SessionError::Camera {
                operation,
                detail: format!(
                    "the camera accepted `{}` but reports `{}` afterwards. The output setting \
                     decides the frame size and whether the camera's screen shows live view, so a \
                     value the camera does not keep cannot be worked around; set it on the camera \
                     and try again",
                    setting.choice_name(),
                    kept
                ),
            });
        }

        Ok(())
    }

    fn activate_viewfinder_action(&mut self) -> Result<(), SessionError> {
        let operation = SessionOperation::ActivateViewfinderAction;

        let widget = self
            .camera
            .config_key::<Widget>(VIEWFINDER_ACTION)
            .wait()
            .map_err(|error| Self::rejected(operation, error))?;

        // UNVERIFIED (no camera attached): the M50's widget type for this action.
        // All three writable shapes are handled; anything else is refused rather
        // than guessed at.
        match widget {
            Widget::Button(button) => button
                .press(&self.camera)
                .map_err(|error| Self::rejected(operation, error)),
            Widget::Toggle(toggle) => {
                toggle.set_toggled(true);
                self.camera
                    .set_config(&toggle)
                    .wait()
                    .map_err(|error| Self::rejected(operation, error))
            }
            Widget::Text(text) => {
                text.set_value("1")
                    .map_err(|error| Self::rejected(operation, error))?;
                self.camera
                    .set_config(&text)
                    .wait()
                    .map_err(|error| Self::rejected(operation, error))
            }
            Widget::Radio(radio) => {
                radio
                    .set_choice("1")
                    .map_err(|error| Self::rejected(operation, error))?;
                self.camera
                    .set_config(&radio)
                    .wait()
                    .map_err(|error| Self::rejected(operation, error))
            }
            Widget::Range(range) => {
                range.set_value(1.0);
                self.camera
                    .set_config(&range)
                    .wait()
                    .map_err(|error| Self::rejected(operation, error))
            }
            Widget::Group(_) | Widget::Date(_) => Err(SessionError::Camera {
                operation,
                detail: format!(
                    "{VIEWFINDER_ACTION_PATH} is a group or date widget, which cannot hold the \
                     viewfinder action"
                ),
            }),
        }
    }

    fn capture_live_view_frame(&mut self) -> Result<Vec<u8>, SessionError> {
        let operation = SessionOperation::CaptureLiveViewFrame;

        let file = self
            .camera
            .capture_preview()
            .wait()
            .map_err(|error| Self::rejected(operation, error))?;
        let data = file
            .get_data(&self.camera)
            .wait()
            .map_err(|error| Self::rejected(operation, error))?;

        Ok(data.into_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_detected, is_camera_on_a_bus, normalise_choice};
    use gphoto2::list::CameraDescriptor;

    #[test]
    fn an_output_choice_is_compared_without_spacing_or_case() {
        assert_eq!(normalise_choice("TFT + PC"), normalise_choice("tft+pc"));
        assert_ne!(normalise_choice("TFT + PC"), normalise_choice("PC"));
    }

    #[test]
    fn a_camera_on_a_usb_port_is_accepted() {
        assert!(is_camera_on_a_bus("usb:001,004"));
    }

    #[test]
    fn a_camera_on_the_usb_bus_builtin_port_is_accepted() {
        assert!(is_camera_on_a_bus("usb:003,002"));
    }

    #[test]
    fn the_directory_browse_pseudo_camera_is_refused() {
        assert!(!is_camera_on_a_bus("disk:"));
        assert!(!is_camera_on_a_bus("disk:/Volumes/CARD/DCIM"));
    }

    #[test]
    fn the_refusal_says_what_was_detected_and_why_it_is_not_a_camera() {
        let detected = vec![CameraDescriptor {
            model: "Directory Browse".to_owned(),
            port: "disk:".to_owned(),
        }];

        let message = describe_detected(&detected);

        assert!(
            message.contains("Directory Browse"),
            "message was {message:?}"
        );
        assert!(message.contains("disk:"), "message was {message:?}");
        assert!(
            message.contains("none of them is a camera"),
            "message was {message:?}"
        );
    }

    #[test]
    fn the_refusal_says_when_nothing_was_detected_at_all() {
        assert!(describe_detected(&[]).contains("no device"));
    }
}
