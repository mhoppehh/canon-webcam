//! The **PTP session**: one exclusive handle to the camera for a whole run.
//!
//! The evidence behind this rule is in `docs/adr/0001-single-ptp-session.md`:
//! running each camera operation as its own `gphoto2` process produced rejected
//! property writes, after which every operation returned `PTP Timeout`. So the
//! session is structural here, not a convention:
//!
//! - [`PtpSession`] is the only way to touch the camera, and it is passed by
//!   reference. Nothing that consumes it can open a second one.
//! - The concrete camera session is not `Clone`, and its only constructor takes
//!   the process wide [`SessionExclusivity`] claim. A second concurrent session
//!   is therefore an [`SessionError::AlreadyOpen`] error rather than a second
//!   handle on the camera.
//! - A **recovery reset** is a recovery action on a wedged camera, not an
//!   exception to this rule: it destroys the session rather than adding one.
//!
//! The viewfinder action does not persist between runs, so it is applied inside
//! the session before every capture. See [`crate::measurement::measure`].

use std::error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::output_setting::OutputSetting;

/// The camera operation that failed, so a refusal is attributable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOperation {
    /// Writing the [`OutputSetting`] to the `output` property.
    SetOutputSetting,
    /// Pressing the viewfinder action.
    ActivateViewfinderAction,
    /// Capturing one live view frame.
    CaptureLiveViewFrame,
}

impl fmt::Display for SessionOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetOutputSetting => formatter.write_str("setting the output setting"),
            Self::ActivateViewfinderAction => formatter.write_str("pressing the viewfinder action"),
            Self::CaptureLiveViewFrame => formatter.write_str("capturing a live view frame"),
        }
    }
}

/// Anything that makes a camera operation impossible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionError {
    /// A second PTP session was requested while one was already open. This is a
    /// bug in the caller, not a condition to retry.
    AlreadyOpen,
    /// No camera could be opened: it is absent from the USB bus, or libgphoto2
    /// refused to open it. This is a **disconnected camera**, not a wedged one.
    NoCamera {
        /// What libgphoto2 said.
        detail: String,
    },
    /// The camera received the operation and rejected it, or timed out.
    Camera {
        /// Which operation failed.
        operation: SessionOperation,
        /// What libgphoto2 said.
        detail: String,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyOpen => formatter
                .write_str("a PTP session is already open; only one session may exist per run"),
            Self::NoCamera { detail } => write!(
                formatter,
                "no camera could be opened; check that the M50 is connected and that no other \
                 program holds it ({detail})"
            ),
            Self::Camera { operation, detail } => {
                write!(formatter, "the camera rejected {operation}: {detail}")
            }
        }
    }
}

impl error::Error for SessionError {}

/// The camera operations a run may perform.
///
/// One session, borrowed. Implementations are the libgphoto2 binding
/// (`crate::gphoto_session::CameraSession`) and, in tests, a fake.
///
/// `liveviewsize` is deliberately absent: on Canon it accepts the write and
/// changes nothing, so exposing it would invite a confidently wrong answer. See
/// `docs/adr/0004-no-live-view-size-selection.md`.
pub trait PtpSession {
    /// The camera model, for the report.
    fn model(&self) -> &str;

    /// Writes the **output setting**. Called on every run, because persistence
    /// across process boundaries is not proven.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the camera rejects the write.
    fn set_output_setting(&mut self, setting: OutputSetting) -> Result<(), SessionError>;

    /// Presses the **viewfinder action**, which turns live view on at the full
    /// frame size. Called on every run, because it does not persist.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the camera rejects the action.
    fn activate_viewfinder_action(&mut self) -> Result<(), SessionError>;

    /// Captures one live view frame and returns its bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the camera stops producing frames.
    fn capture_live_view_frame(&mut self) -> Result<Vec<u8>, SessionError>;
}

/// The process wide claim that makes a second PTP session impossible.
///
/// Tests build their own instance so they never share state with each other;
/// the camera session uses [`process_session_exclusivity`].
#[derive(Debug)]
pub struct SessionExclusivity {
    claimed: AtomicBool,
}

impl SessionExclusivity {
    /// Creates an unclaimed instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
        }
    }

    /// Takes the claim, or reports that somebody else holds it.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::AlreadyOpen`] if the claim is held. The claim is
    /// released when the returned [`SessionClaim`] is dropped.
    pub fn claim(&self) -> Result<SessionClaim<'_>, SessionError> {
        if self.claimed.swap(true, Ordering::AcqRel) {
            Err(SessionError::AlreadyOpen)
        } else {
            Ok(SessionClaim { exclusivity: self })
        }
    }
}

impl Default for SessionExclusivity {
    fn default() -> Self {
        Self::new()
    }
}

/// Proof that the holder owns the one PTP session.
///
/// The claim is not `Clone` and not `Copy`, so it cannot be duplicated, and it
/// releases on drop.
#[derive(Debug)]
pub struct SessionClaim<'a> {
    exclusivity: &'a SessionExclusivity,
}

impl Drop for SessionClaim<'_> {
    fn drop(&mut self) {
        self.exclusivity.claimed.store(false, Ordering::Release);
    }
}

/// The claim the camera session takes: one for the whole process, so the CLI, a
/// future control interface and any library caller all contend for the same one.
#[must_use]
pub fn process_session_exclusivity() -> &'static SessionExclusivity {
    static EXCLUSIVITY: OnceLock<SessionExclusivity> = OnceLock::new();
    EXCLUSIVITY.get_or_init(SessionExclusivity::new)
}
