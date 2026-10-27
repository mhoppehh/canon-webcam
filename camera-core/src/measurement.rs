//! **Measurement**: learning the frame size and the frame rate from frame data.
//!
//! The M50 honours no live view format it is asked for, so the only honest way
//! to learn the **measured frame size** and **measured frame rate** is to capture
//! frames and read them. Nothing here reads a size from a property: the
//! `liveviewsize` property accepts a write on Canon and changes nothing, so
//! reporting it would produce a confidently wrong answer (see
//! `docs/adr/0004-no-live-view-size-selection.md`).
//!
//! One run does, in order, on one borrowed **PTP session**:
//!
//! 1. write the **output setting**,
//! 2. press the **viewfinder action** (it does not persist, so it is pressed on
//!    every run, in the same session as the capture),
//! 3. capture through a **calibration window** to establish the **expected frame
//!    size**, then capture a sampling window to measure the rate.

use std::error;
use std::fmt;
use std::time::{Duration, Instant};

use crate::calibration::{
    CalibrationError, CalibrationOutcome, CalibrationPolicy, CalibrationWindow,
};
use crate::frame_size::FrameSize;
use crate::jpeg::{parse_frame_size, JpegError};
use crate::output_setting::OutputSetting;
use crate::session::{PtpSession, SessionError};

/// The full size this camera produced in `TEST-RESULTS.md` §4.4.
///
/// A **sanity check only**. It is never the source of a measured frame size.
pub const SANITY_CHECK_FRAME_SIZE: FrameSize = FrameSize::new(1024, 576);

/// The rate this camera produced in `TEST-RESULTS.md` §4.6, roughly.
///
/// A **sanity check only**. It is never the source of a measured frame rate.
pub const SANITY_CHECK_FRAME_RATE: f64 = 24.0;

/// The default number of frames in the sampling window.
pub const DEFAULT_SAMPLING_FRAMES: usize = 48;

/// Measures elapsed time.
///
/// Injected so a measurement can be tested without waiting, and so the frame
/// rate is measured rather than assumed.
pub trait Clock {
    /// Time since the run started.
    fn elapsed(&self) -> Duration;
}

/// The real clock: monotonic, started when the run starts.
#[derive(Debug)]
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    /// Starts the clock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// A sampling window that cannot produce a rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementPlanError {
    /// One frame contains no interval, so no rate can be derived from it.
    SamplingFramesTooSmall {
        /// The sampling window that was requested.
        sampling_frames: usize,
    },
}

impl fmt::Display for MeasurementPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SamplingFramesTooSmall { sampling_frames } => write!(
                formatter,
                "a sampling window of {sampling_frames} frames contains no interval; use at \
                 least two"
            ),
        }
    }
}

impl error::Error for MeasurementPlanError {}

/// What one measurement run is asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementPlan {
    output_setting: OutputSetting,
    calibration: CalibrationPolicy,
    sampling_frames: usize,
}

impl MeasurementPlan {
    /// Validates and builds a plan.
    ///
    /// # Errors
    ///
    /// Returns [`MeasurementPlanError`] if the sampling window cannot produce a
    /// rate.
    pub fn new(
        output_setting: OutputSetting,
        calibration: CalibrationPolicy,
        sampling_frames: usize,
    ) -> Result<Self, MeasurementPlanError> {
        if sampling_frames < 2 {
            return Err(MeasurementPlanError::SamplingFramesTooSmall { sampling_frames });
        }
        Ok(Self {
            output_setting,
            calibration,
            sampling_frames,
        })
    }

    /// The **output setting** to write at session start.
    #[must_use]
    pub fn output_setting(&self) -> OutputSetting {
        self.output_setting
    }

    /// The **calibration window** policy.
    #[must_use]
    pub fn calibration(&self) -> &CalibrationPolicy {
        &self.calibration
    }

    /// How many frames the sampling window observes.
    #[must_use]
    pub fn sampling_frames(&self) -> usize {
        self.sampling_frames
    }
}

impl Default for MeasurementPlan {
    fn default() -> Self {
        Self {
            output_setting: OutputSetting::tft_plus_pc(),
            calibration: CalibrationPolicy::default(),
            sampling_frames: DEFAULT_SAMPLING_FRAMES,
        }
    }
}

/// What a measurement learned, measured from frame data.
#[derive(Clone, Debug)]
pub struct Measurement {
    /// The measured frame size: the expected frame size the calibration window
    /// established.
    pub measured_frame_size: FrameSize,
    /// The measured frame rate, in frames per second, across the sampling window.
    pub measured_frame_rate: f64,
    /// Every frame read from the camera, including the calibration window.
    pub frames_captured: usize,
    /// Frames of another size that preceded the expected frame size, the
    /// **leading frames**.
    pub leading_frames: usize,
    /// Frames captured during the sampling window that were not at the expected
    /// frame size. A non-zero value means the camera was not steady.
    pub unexpected_frames: usize,
    /// Frames captured during the sampling window, including the establishment
    /// frame.
    pub sampling_frames: usize,
    /// The span of the sampling window.
    pub sampling_elapsed: Duration,
    /// The span of the whole run.
    pub elapsed: Duration,
}

/// Why a run produced no usable measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeasureError {
    /// The session refused an operation.
    Session(SessionError),
    /// A captured frame carried no readable frame size.
    Frame(JpegError),
    /// The **calibration window** established nothing.
    Calibration(CalibrationError),
    /// The **expected frame size** was established and then never produced again
    /// inside the sampling window. The camera changed size, which no measurement
    /// can paper over; a **sink re-open** belongs to the sink, not here.
    ExpectedFrameSizeNotRecurring {
        /// The frame size the calibration window established.
        expected: FrameSize,
    },
    /// The clock did not advance across the sampling window.
    NoElapsedTime,
}

impl fmt::Display for MeasureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(error) => write!(formatter, "{error}"),
            Self::Frame(error) => write!(formatter, "a captured frame was rejected: {error}"),
            Self::Calibration(error) => write!(formatter, "{error}"),
            Self::ExpectedFrameSizeNotRecurring { expected } => write!(
                formatter,
                "the expected frame size {expected} was established and then never produced again \
                 inside the sampling window, so the camera changed frame size"
            ),
            Self::NoElapsedTime => {
                formatter.write_str("no time elapsed across the sampling window, so no frame rate")
            }
        }
    }
}

impl error::Error for MeasureError {}

impl From<SessionError> for MeasureError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<JpegError> for MeasureError {
    fn from(error: JpegError) -> Self {
        Self::Frame(error)
    }
}

impl From<CalibrationError> for MeasureError {
    fn from(error: CalibrationError) -> Self {
        Self::Calibration(error)
    }
}

/// Captures through a **calibration window** and a sampling window on one
/// borrowed **PTP session**, and reports what the frames themselves said.
///
/// The session is borrowed, never opened here: a measurement cannot create a
/// second session, which is what makes "one PTP session per run" structural.
///
/// # Errors
///
/// Returns [`MeasureError`] if the session refuses an operation, a frame cannot
/// be read, the calibration window establishes nothing, the camera stops
/// producing the expected frame size, or no time elapsed.
pub fn measure(
    session: &mut dyn PtpSession,
    plan: &MeasurementPlan,
    clock: &dyn Clock,
) -> Result<Measurement, MeasureError> {
    session.set_output_setting(plan.output_setting())?;
    session.activate_viewfinder_action()?;

    let mut window = CalibrationWindow::new(plan.calibration().clone());
    let mut frames_captured = 0_usize;
    // Set when the calibration window closes, with the start of the sampling
    // window, so the rate is measured over frames and not over a guess.
    let mut established: Option<(FrameSize, Duration)> = None;
    let mut leading_frames = 0_usize;
    let mut sampling_frames = 0_usize;
    let mut unexpected_frames = 0_usize;
    let mut expected_after_establishment = 0_usize;

    let (expected_frame_size, sampling_start, sampling_end) = loop {
        let frame = session.capture_live_view_frame()?;
        frames_captured += 1;
        let frame_size = parse_frame_size(&frame)?;
        let elapsed = clock.elapsed();

        match established {
            None => match window.observe(frame_size, elapsed) {
                CalibrationOutcome::Establishing => continue,
                CalibrationOutcome::Failed(error) => return Err(MeasureError::Calibration(error)),
                CalibrationOutcome::Established {
                    expected_frame_size,
                    leading_frames: leading,
                } => {
                    established = Some((expected_frame_size, elapsed));
                    leading_frames = leading;
                    sampling_frames = 1;
                    if frame_size != expected_frame_size {
                        unexpected_frames += 1;
                    }
                }
            },
            Some((expected_frame_size, _)) => {
                sampling_frames += 1;
                if frame_size == expected_frame_size {
                    expected_after_establishment += 1;
                } else {
                    unexpected_frames += 1;
                }
            }
        }

        if let Some((expected_frame_size, sampling_start)) = established {
            if sampling_frames >= plan.sampling_frames() {
                break (expected_frame_size, sampling_start, elapsed);
            }
        }
    };

    if expected_after_establishment == 0 {
        return Err(MeasureError::ExpectedFrameSizeNotRecurring {
            expected: expected_frame_size,
        });
    }

    let sampling_elapsed = sampling_end.saturating_sub(sampling_start);
    if sampling_elapsed.is_zero() {
        return Err(MeasureError::NoElapsedTime);
    }

    // `sampling_frames` frames span `sampling_frames - 1` intervals, and the
    // sampling window is at least two frames, so the denominator is non-zero.
    let measured_frame_rate = (sampling_frames - 1) as f64 / sampling_elapsed.as_secs_f64();

    Ok(Measurement {
        measured_frame_size: expected_frame_size,
        measured_frame_rate,
        frames_captured,
        leading_frames,
        unexpected_frames,
        sampling_frames,
        sampling_elapsed,
        elapsed: sampling_end,
    })
}
