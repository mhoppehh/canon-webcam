//! The pipeline that turns captured frames into delivered frames.
//!
//! One **expected frame size**, established by a **calibration window**, and a
//! **virtual camera sink** opened at it. Everything else here is about keeping
//! that promise when the camera stops keeping its side:
//!
//! - a frame whose size is not the expected one is **dropped**, never forwarded,
//!   and counted;
//! - about a second of consecutive mismatches starts a **short recalibration**,
//!   because a transient of the reduced size is not a size change but a camera
//!   that has changed size is;
//! - if the recalibration settles on a new size the sink is closed and
//!   **re-opened** at it. If it settles on nothing, the pipeline reports
//!   [`PipelineEvent::Unsettled`] and leaves the escalation to the caller, which
//!   is the only layer that can tell a **wedged camera** from a **disconnected
//!   camera**.
//!
//! The **leading frames** at the start of a run are handled by the calibration
//! window, never by a discard count: the frame that closes the window is not
//! delivered, because the sink did not exist when it arrived. That also makes the
//! accounting exact — see [`StreamStats`](crate::stats::StreamStats), where every
//! received frame ends as delivered, dropped, or observed by a calibration
//! window.

use std::error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::calibration::{
    CalibrationError, CalibrationOutcome, CalibrationPolicy, CalibrationWindow,
};
use crate::frame_size::FrameSize;
use crate::jpeg::parse_frame_size;
use crate::pixel_format::{LiveViewFrame, PixelFormat};
use crate::sink::{FrameSink, SinkError};
use crate::stats::StreamStats;

/// How long the camera may produce frames of the wrong size before the pipeline
/// stops calling it a transient.
///
/// One second is longer than the five **leading frames** ever observed, and short
/// enough that a real size change is noticed while a user is still looking at the
/// screen.
pub const DEFAULT_MISMATCH_GRACE: Duration = Duration::from_secs(1);

/// Frames a short recalibration window observes.
pub const DEFAULT_RECALIBRATION_FRAMES: usize = 24;
/// How long a short recalibration window stays open.
pub const DEFAULT_RECALIBRATION_DURATION: Duration = Duration::from_secs(1);
/// How often a size must recur in a short recalibration window to be adopted.
pub const DEFAULT_RECALIBRATION_MIN_RECURRENCES: usize = 8;

/// What one frame did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineEvent {
    /// The calibration window is still open; the frame established nothing yet.
    Calibrating,
    /// The sink was opened at this frame size, which is where frames are now
    /// delivered.
    SinkOpened {
        /// The size the sink was opened at: the measured frame size, or the
        /// requested size when resizing is enabled.
        frame_size: FrameSize,
    },
    /// The frame reached the sink.
    Delivered,
    /// The frame was dropped because it was not the expected frame size.
    DroppedSizeMismatch,
    /// The frame was dropped because it carried no readable frame size, or could
    /// not be decoded.
    DroppedMalformed,
    /// A short recalibration is running.
    Recalibrating,
    /// A short recalibration just started. The caller re-applies the
    /// acquisition settings once, here, rather than on every frame.
    RecalibrationStarted,
    /// A size change was confirmed and the sink was closed and re-opened.
    SinkReopened {
        /// The size the sink was re-opened at.
        frame_size: FrameSize,
    },
    /// A recalibration settled on nothing. The caller must find out whether the
    /// camera is **wedged** or **disconnected**, because the remedies differ.
    Unsettled,
}

impl PipelineEvent {
    /// Whether the frame reached the sink.
    #[must_use]
    pub fn delivered(self) -> bool {
        matches!(self, Self::Delivered)
    }
}

/// Why the pipeline could not continue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// The sink failed in a way that is not a contract violation.
    Sink(SinkError),
    /// The opening **calibration window** established no frame size, so there is
    /// nothing to open a sink at.
    Calibration(CalibrationError),
}

impl fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sink(error) => write!(formatter, "{error}"),
            Self::Calibration(error) => write!(formatter, "{error}"),
        }
    }
}

impl error::Error for StreamError {}

impl From<SinkError> for StreamError {
    fn from(error: SinkError) -> Self {
        Self::Sink(error)
    }
}

impl From<CalibrationError> for StreamError {
    fn from(error: CalibrationError) -> Self {
        Self::Calibration(error)
    }
}

/// How the pipeline behaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamPipelineConfig {
    calibration: CalibrationPolicy,
    recalibration: CalibrationPolicy,
    mismatch_grace: Duration,
    resize_to: Option<FrameSize>,
}

impl StreamPipelineConfig {
    /// Builds a configuration from the **calibration window** policy, taking the
    /// short-recalibration defaults.
    #[must_use]
    pub fn new(calibration: CalibrationPolicy) -> Self {
        Self {
            calibration,
            recalibration: CalibrationPolicy::new(
                DEFAULT_RECALIBRATION_FRAMES,
                DEFAULT_RECALIBRATION_DURATION,
                DEFAULT_RECALIBRATION_MIN_RECURRENCES,
            )
            .expect("the default recalibration policy is valid"),
            mismatch_grace: DEFAULT_MISMATCH_GRACE,
            resize_to: None,
        }
    }

    /// Replaces the short-recalibration policy.
    #[must_use]
    pub fn with_recalibration(mut self, recalibration: CalibrationPolicy) -> Self {
        self.recalibration = recalibration;
        self
    }

    /// Replaces how long a run of mismatches is tolerated.
    #[must_use]
    pub fn with_mismatch_grace(mut self, mismatch_grace: Duration) -> Self {
        self.mismatch_grace = mismatch_grace;
        self
    }

    /// Resizes every delivered frame to `target`.
    ///
    /// This is **resizing**, not resolution: it reveals nothing the camera did
    /// not send. It exists only because a consumer application can refuse a
    /// 1024x576 device and accept a larger geometry.
    #[must_use]
    pub fn with_resize_to(mut self, target: FrameSize) -> Self {
        self.resize_to = Some(target);
        self
    }

    /// The **calibration window** policy.
    #[must_use]
    pub fn calibration(&self) -> &CalibrationPolicy {
        &self.calibration
    }

    /// The short recalibration policy.
    #[must_use]
    pub fn recalibration(&self) -> &CalibrationPolicy {
        &self.recalibration
    }

    /// How long a run of mismatches is tolerated.
    #[must_use]
    pub fn mismatch_grace(&self) -> Duration {
        self.mismatch_grace
    }

    /// The frame size every delivered frame is resized to, if any.
    #[must_use]
    pub fn resize_to(&self) -> Option<FrameSize> {
        self.resize_to
    }
}

/// Turns captured frames into delivered frames.
pub struct StreamPipeline {
    sink: Box<dyn FrameSink>,
    stats: Arc<StreamStats>,
    calibration: CalibrationWindow,
    recalibration_policy: CalibrationPolicy,
    mismatch_grace: Duration,
    resize_to: Option<FrameSize>,
    expected_frame_size: Option<FrameSize>,
    sink_frame_size: Option<FrameSize>,
    recalibrating: Option<CalibrationWindow>,
    /// When the running recalibration started, so its window measures time from
    /// its own open rather than from the start of the run.
    recalibration_started_at: Option<Duration>,
    mismatch_since: Option<Duration>,
}

impl StreamPipeline {
    /// Builds a pipeline over an unopened sink.
    ///
    /// The sink is opened when the **calibration window** establishes the
    /// expected frame size, because that is the earliest moment at which its
    /// size can be known.
    #[must_use]
    pub fn new(
        sink: Box<dyn FrameSink>,
        stats: Arc<StreamStats>,
        config: StreamPipelineConfig,
    ) -> Self {
        Self {
            sink,
            stats,
            calibration: CalibrationWindow::new(config.calibration),
            recalibration_policy: config.recalibration,
            mismatch_grace: config.mismatch_grace,
            resize_to: config.resize_to,
            expected_frame_size: None,
            sink_frame_size: None,
            recalibrating: None,
            recalibration_started_at: None,
            mismatch_since: None,
        }
    }

    /// The **expected frame size**, once the calibration window has established
    /// it.
    #[must_use]
    pub fn expected_frame_size(&self) -> Option<FrameSize> {
        self.expected_frame_size
    }

    /// The size the sink is open at, if it is open.
    #[must_use]
    pub fn sink_frame_size(&self) -> Option<FrameSize> {
        self.sink_frame_size
    }

    /// Offers one captured frame, exactly as the camera delivered it.
    ///
    /// # Errors
    ///
    /// Returns [`StreamError::Calibration`] if the opening window establishes no
    /// frame size, and [`StreamError::Sink`] if the sink fails.
    pub fn handle(&mut self, jpeg: &[u8], elapsed: Duration) -> Result<PipelineEvent, StreamError> {
        self.stats.record_received();

        let Ok(frame_size) = parse_frame_size(jpeg) else {
            self.stats.record_malformed();
            return Ok(PipelineEvent::DroppedMalformed);
        };

        match self.expected_frame_size {
            None => self.establish(frame_size, jpeg, elapsed),
            Some(expected) if frame_size == expected => {
                self.recalibrating = None;
                self.mismatch_since = None;
                self.deliver(jpeg)
            }
            Some(_) => self.mismatch(frame_size, elapsed),
        }
    }

    /// Closes the sink. Harmless when it is already closed.
    ///
    /// # Errors
    ///
    /// Returns [`StreamError::Sink`] if the platform backend cannot release the
    /// device.
    pub fn shutdown(&mut self) -> Result<(), StreamError> {
        let closed = self.sink.close();
        self.sink_frame_size = None;
        closed?;
        Ok(())
    }

    fn establish(
        &mut self,
        frame_size: FrameSize,
        _jpeg: &[u8],
        elapsed: Duration,
    ) -> Result<PipelineEvent, StreamError> {
        self.stats.record_calibration_observation();
        let outcome = self.calibration.observe(frame_size, elapsed);

        match outcome {
            CalibrationOutcome::Establishing => Ok(PipelineEvent::Calibrating),
            CalibrationOutcome::Failed(error) => Err(StreamError::Calibration(error)),
            CalibrationOutcome::Established {
                expected_frame_size,
                ..
            } => {
                self.expected_frame_size = Some(expected_frame_size);
                let open_at = self.resize_to.unwrap_or(expected_frame_size);
                self.sink.open(open_at, PixelFormat::Rgb24)?;
                self.stats.record_sink_open();
                self.sink_frame_size = Some(open_at);
                // The frame that closed the window is not delivered: the sink did
                // not exist when it arrived, and delivering it would make the
                // frame accounting count one frame twice.
                Ok(PipelineEvent::SinkOpened {
                    frame_size: open_at,
                })
            }
        }
    }

    fn mismatch(
        &mut self,
        frame_size: FrameSize,
        elapsed: Duration,
    ) -> Result<PipelineEvent, StreamError> {
        // A recalibration already running takes every mismatched frame.
        if let Some(window) = self.recalibrating.as_mut() {
            self.stats.record_calibration_observation();
            let since = self
                .recalibration_started_at
                .map_or(Duration::ZERO, |started| elapsed.saturating_sub(started));
            let outcome = window.observe(frame_size, since);
            return self.settle(outcome);
        }

        let first = *self.mismatch_since.get_or_insert(elapsed);
        if elapsed.saturating_sub(first) < self.mismatch_grace {
            self.stats.record_size_mismatch();
            return Ok(PipelineEvent::DroppedSizeMismatch);
        }

        // The grace period is over: this is a size change until it proves
        // otherwise, so start a short recalibration and let this frame be its
        // first observation.
        self.stats.record_recalibration();
        let mut window = CalibrationWindow::new(self.recalibration_policy.clone());
        self.stats.record_calibration_observation();
        // The window measures its own time, so a long run does not close it on
        // the frame that opened it.
        let outcome = window.observe(frame_size, Duration::ZERO);
        self.recalibrating = Some(window);
        self.recalibration_started_at = Some(elapsed);
        match self.settle(outcome)? {
            // This frame both started the window and is its first observation.
            PipelineEvent::Recalibrating => Ok(PipelineEvent::RecalibrationStarted),
            settled => Ok(settled),
        }
    }

    fn settle(&mut self, outcome: CalibrationOutcome) -> Result<PipelineEvent, StreamError> {
        match outcome {
            CalibrationOutcome::Establishing => Ok(PipelineEvent::Recalibrating),
            CalibrationOutcome::Failed(_) => {
                // Nothing agreed on a size. Retrying at once would spin, so the
                // trigger is cleared and the caller decides what the camera is
                // doing.
                self.recalibrating = None;
                self.recalibration_started_at = None;
                self.mismatch_since = None;
                Ok(PipelineEvent::Unsettled)
            }
            CalibrationOutcome::Established {
                expected_frame_size,
                ..
            } => {
                self.recalibrating = None;
                self.recalibration_started_at = None;
                self.mismatch_since = None;

                if self.expected_frame_size == Some(expected_frame_size) {
                    // The camera came back to the size the sink is already open
                    // at, so nothing needs re-opening.
                    return Ok(PipelineEvent::DroppedSizeMismatch);
                }

                // Sink re-open and nothing else: an open sink is never handed a
                // new geometry. The close comes first and the bookkeeping with
                // it, so a failed re-open leaves a pipeline that knows it holds
                // nothing — and whose `Drop` therefore does not claim a device it
                // already released.
                self.sink.close()?;
                self.sink_frame_size = None;
                let open_at = self.resize_to.unwrap_or(expected_frame_size);
                self.sink.open(open_at, PixelFormat::Rgb24)?;
                self.stats.record_sink_open();
                self.stats.record_sink_reopen();
                self.expected_frame_size = Some(expected_frame_size);
                self.sink_frame_size = Some(open_at);

                Ok(PipelineEvent::SinkReopened {
                    frame_size: open_at,
                })
            }
        }
    }

    fn deliver(&mut self, jpeg: &[u8]) -> Result<PipelineEvent, StreamError> {
        let frame = match LiveViewFrame::from_jpeg(jpeg) {
            Ok(frame) => frame,
            Err(_) => {
                // The marker claimed the expected size but the image behind it
                // will not decode. There is no frame to forward.
                self.stats.record_malformed();
                return Ok(PipelineEvent::DroppedMalformed);
            }
        };
        let frame = match self.resize_to {
            Some(target) => frame.resize_to(target),
            None => frame,
        };

        self.sink.accept(&frame)?;
        self.stats.record_delivered(frame.pixels().len());
        Ok(PipelineEvent::Delivered)
    }
}

impl Drop for StreamPipeline {
    fn drop(&mut self) {
        // The safety net behind [`StreamPipeline::shutdown`]: a pipeline dropped
        // on a path that never reached the explicit close — a panic in the
        // capture thread, a caller that returned early — must not leave the
        // device held for the next consumer application.
        if self.sink_frame_size.is_some() {
            let _ = self.sink.close();
            self.sink_frame_size = None;
        }
    }
}
