//! A **PTP session** that produces frames without a camera.
//!
//! No camera was attached while this work was done, so every behaviour that needs
//! frames is demonstrated against this session instead: a steady run, a run with
//! **leading frames**, a camera that changes frame size and therefore forces a
//! **sink re-open**, and a camera that stops answering with either a
//! **wedged camera** timeout or a **disconnected camera** absence.
//!
//! The frames are real JPEGs, encoded by an encoder and committed under
//! `assets/live-view/`, so the decoder, the marker walk and the sink contract all
//! see genuine camera-shaped data rather than hand-built bytes. Only the two
//! frame sizes this camera has ever produced are available, because those are the
//! only two anybody has measured.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::frame_size::FrameSize;
use crate::output_setting::OutputSetting;
use crate::recovery::PtpSessionOpener;
use crate::session::{PtpSession, SessionError, SessionOperation};

/// The full frame size an M50 produced in `TEST-RESULTS.md`.
pub const FULL_FRAME_SIZE: FrameSize = FrameSize::new(1024, 576);
/// The reduced frame size an M50 produced, and the one the **leading frames**
/// arrive at.
pub const REDUCED_FRAME_SIZE: FrameSize = FrameSize::new(512, 288);

/// The number of **leading frames** `TEST-RESULTS.md` reported. A run may have
/// none, so nothing here depends on it.
pub const OBSERVED_LEADING_FRAMES: usize = 5;

/// A real JPEG at a frame size this camera has produced.
///
/// Returns `None` for any other size: there is no measured frame at any other
/// size, and fabricating one would put a number in the code that no camera ever
/// produced.
#[must_use]
pub const fn live_view_jpeg(frame_size: FrameSize) -> Option<&'static [u8]> {
    match frame_size {
        FULL_FRAME_SIZE => Some(include_bytes!("../assets/live-view/frame-1024x576.jpg")),
        REDUCED_FRAME_SIZE => Some(include_bytes!("../assets/live-view/frame-512x288.jpg")),
        _ => None,
    }
}

/// A session that answers with committed frames instead of a camera.
///
/// Not `Clone`, like the real session, and for the same reason: it stands in for
/// the one **PTP session** of a run.
pub struct SyntheticSession {
    model: String,
    steady_frame_size: FrameSize,
    leading_frames: usize,
    leading_frame_size: FrameSize,
    switch_after: Option<(usize, FrameSize)>,
    failure: Option<(usize, SessionError)>,
    frame_period: Option<Duration>,
    captured: usize,
    log: SharedLog,
}

/// The operations several synthetic sessions were asked to perform, in order.
///
/// Shared rather than owned so a **recovery reset** test can see what the
/// session reopened after the reset was asked to do, not only what the wedged
/// one did.
pub type SharedLog = Arc<Mutex<Vec<String>>>;

impl SyntheticSession {
    /// A session that produces `steady_frame_size` for ever, as fast as it is
    /// asked.
    ///
    /// # Panics
    ///
    /// Panics if no measured frame exists at `steady_frame_size`. Only
    /// [`FULL_FRAME_SIZE`] and [`REDUCED_FRAME_SIZE`] do.
    #[must_use]
    pub fn new(steady_frame_size: FrameSize) -> Self {
        assert!(
            live_view_jpeg(steady_frame_size).is_some(),
            "no measured live view frame exists at {steady_frame_size}; the measured sizes are \
             {FULL_FRAME_SIZE} and {REDUCED_FRAME_SIZE}"
        );
        Self {
            model: "Synthetic M50".to_owned(),
            steady_frame_size,
            leading_frames: 0,
            leading_frame_size: REDUCED_FRAME_SIZE,
            switch_after: None,
            failure: None,
            frame_period: None,
            captured: 0,
            log: SharedLog::default(),
        }
    }

    /// Writes this session's operations into `log` rather than into its own.
    ///
    /// Used by a recovery test, where the session reopened after the **recovery
    /// reset** is not the one the test built last.
    #[must_use]
    pub fn with_shared_log(mut self, log: SharedLog) -> Self {
        self.log = log;
        self
    }

    /// Emits `count` **leading frames** at the reduced frame size first.
    #[must_use]
    pub fn with_leading_frames(mut self, count: usize) -> Self {
        self.leading_frames = count;
        self
    }

    /// Changes to `new_frame_size` after `frames` frames, which is what a camera
    /// whose **output setting** changed does.
    #[must_use]
    pub fn changing_size_after(mut self, frames: usize, new_frame_size: FrameSize) -> Self {
        assert!(
            live_view_jpeg(new_frame_size).is_some(),
            "no measured live view frame exists at {new_frame_size}"
        );
        self.switch_after = Some((frames, new_frame_size));
        self
    }

    /// Stops answering after `frames` frames with `error`.
    ///
    /// A [`SessionError::NoCamera`] is a **disconnected camera**; a
    /// [`SessionError::Camera`] carrying a `PTP Timeout` detail is a **wedged
    /// camera**.
    #[must_use]
    pub fn failing_after(mut self, frames: usize, error: SessionError) -> Self {
        self.failure = Some((frames, error));
        self
    }

    /// Waits `period` before each frame, so the run has a frame rate.
    #[must_use]
    pub fn with_frame_period(mut self, period: Duration) -> Self {
        self.frame_period = Some(period);
        self
    }

    /// A session paced at `frames_per_second`.
    #[must_use]
    pub fn at_rate(self, frames_per_second: f64) -> Self {
        assert!(
            frames_per_second > 0.0,
            "a frame rate must be positive, was {frames_per_second}"
        );
        self.with_frame_period(Duration::from_secs_f64(1.0 / frames_per_second))
    }

    /// The frame size the next capture will deliver.
    #[must_use]
    pub fn current_frame_size(&self) -> FrameSize {
        if self.captured < self.leading_frames {
            return self.leading_frame_size;
        }
        if let Some((after, new_frame_size)) = self.switch_after {
            if self.captured >= after {
                return new_frame_size;
            }
        }
        self.steady_frame_size
    }

    /// How many frames have been delivered.
    #[must_use]
    pub fn frames_delivered(&self) -> usize {
        self.captured
    }

    /// The operations this session was asked to perform, in order.
    #[must_use]
    pub fn log(&self) -> Vec<String> {
        self.log
            .lock()
            .expect("the synthetic session log must not be poisoned")
            .clone()
    }

    fn note(&self, entry: String) {
        self.log
            .lock()
            .expect("the synthetic session log must not be poisoned")
            .push(entry);
    }
}

impl PtpSession for SyntheticSession {
    fn model(&self) -> &str {
        &self.model
    }

    fn set_output_setting(&mut self, setting: OutputSetting) -> Result<(), SessionError> {
        self.note(format!("set-output:{}", setting.raw()));
        Ok(())
    }

    fn activate_viewfinder_action(&mut self) -> Result<(), SessionError> {
        self.note("viewfinder".to_owned());
        Ok(())
    }

    fn capture_live_view_frame(&mut self) -> Result<Vec<u8>, SessionError> {
        self.note("capture".to_owned());

        if let Some((after, error)) = &self.failure {
            if self.captured >= *after {
                return Err(error.clone());
            }
        }
        if let Some(period) = self.frame_period {
            std::thread::sleep(period);
        }

        let frame_size = self.current_frame_size();
        let jpeg = live_view_jpeg(frame_size).unwrap_or_else(|| {
            panic!(
                "the synthetic session was asked for a {frame_size} frame, which no measured \
                 frame covers"
            )
        });
        self.captured += 1;
        Ok(jpeg.to_vec())
    }
}

impl SyntheticSession {
    /// The **wedged camera** failure: still on the bus, but every operation times
    /// out.
    #[must_use]
    pub fn wedged() -> SessionError {
        SessionError::Camera {
            operation: SessionOperation::CaptureLiveViewFrame,
            detail: "PTP Timeout (the camera timed out; it may be a wedged camera)".to_owned(),
        }
    }

    /// The **disconnected camera** failure: absent from the bus.
    #[must_use]
    pub fn disconnected() -> SessionError {
        SessionError::NoCamera {
            detail: "libgphoto2 detected no device at all".to_owned(),
        }
    }
}

/// Hands out synthetic **PTP sessions** in a scripted order.
///
/// This is what a **recovery reset** test needs: the first session is the one
/// that wedges, and the next is the one the reset is supposed to reopen. Running
/// out of scripted sessions is a **disconnected camera**, because a camera that
/// refuses to come back after a reset is not on the bus as far as this opener can
/// tell.
pub struct ScriptedSessionOpener {
    sessions: VecDeque<SyntheticSession>,
    opened: usize,
}

impl ScriptedSessionOpener {
    /// Hands out `sessions` in order, one per [`PtpSessionOpener::open_session`].
    #[must_use]
    pub fn new(sessions: impl IntoIterator<Item = SyntheticSession>) -> Self {
        Self {
            sessions: sessions.into_iter().collect(),
            opened: 0,
        }
    }

    /// How many sessions were handed out.
    #[must_use]
    pub fn opened(&self) -> usize {
        self.opened
    }
}

impl PtpSessionOpener for ScriptedSessionOpener {
    fn open_session(&mut self) -> Result<Box<dyn PtpSession + Send>, SessionError> {
        let Some(session) = self.sessions.pop_front() else {
            return Err(SessionError::NoCamera {
                detail: "the scripted synthetic sessions are exhausted".to_owned(),
            });
        };
        self.opened += 1;
        Ok(Box::new(session))
    }
}
