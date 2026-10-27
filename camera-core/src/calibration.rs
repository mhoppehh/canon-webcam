//! The **calibration window**: how the **expected frame size** is established.
//!
//! The window takes the largest **frame size** that recurs at least a threshold
//! number of times inside the window. It does not discard a fixed number of
//! frames: a fixed count locks onto the reduced size, because the **leading
//! frames** are exactly the reduced size. Evidence: `TEST-RESULTS.md` §4.4
//! reported five `512x288` leading frames in a six second capture, and the same
//! five in a thirty second capture, so a leading run is short but its length is
//! not guaranteed.
//!
//! Both limits are armed and whichever is reached first closes the window, so a
//! camera that sends very few frames cannot hold the window open for ever.

use std::collections::BTreeMap;
use std::error;
use std::fmt;
use std::time::Duration;

use crate::frame_size::FrameSize;

/// The default window: 72 frames inside three seconds, a size recurring eight
/// times wins.
///
/// Eight is deliberately above the five leading frames reported in
/// `TEST-RESULTS.md`, with margin, so a leading run of the reduced size cannot
/// outvote the steady full size.
pub const DEFAULT_WINDOW_FRAMES: usize = 72;
/// See [`DEFAULT_WINDOW_FRAMES`].
pub const DEFAULT_WINDOW_DURATION: Duration = Duration::from_secs(3);
/// See [`DEFAULT_WINDOW_FRAMES`].
pub const DEFAULT_MIN_RECURRENCES: usize = 8;

/// A reason the window limits are meaningless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationPolicyError {
    /// The frame limit, the time limit or the threshold is zero.
    ZeroLimit,
    /// The threshold cannot be reached inside the frame limit.
    ThresholdAboveWindow {
        /// The threshold that was requested.
        min_recurrences: usize,
        /// The frame limit that was requested.
        window_frames: usize,
    },
}

impl fmt::Display for CalibrationPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLimit => formatter.write_str(
                "the calibration window needs a positive frame limit, time limit and threshold",
            ),
            Self::ThresholdAboveWindow {
                min_recurrences,
                window_frames,
            } => write!(
                formatter,
                "a size cannot recur {min_recurrences} times inside a {window_frames} frame \
                 window, so no size could ever become expected"
            ),
        }
    }
}

impl error::Error for CalibrationPolicyError {}

/// How long the window stays open and how often a size must recur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationPolicy {
    window_frames: usize,
    window_duration: Duration,
    min_recurrences: usize,
}

impl CalibrationPolicy {
    /// Validates and builds a policy.
    ///
    /// # Errors
    ///
    /// Returns [`CalibrationPolicyError`] for limits that cannot establish a
    /// **frame size**.
    pub fn new(
        window_frames: usize,
        window_duration: Duration,
        min_recurrences: usize,
    ) -> Result<Self, CalibrationPolicyError> {
        if window_frames == 0 || window_duration.is_zero() || min_recurrences == 0 {
            return Err(CalibrationPolicyError::ZeroLimit);
        }
        if min_recurrences > window_frames {
            return Err(CalibrationPolicyError::ThresholdAboveWindow {
                min_recurrences,
                window_frames,
            });
        }
        Ok(Self {
            window_frames,
            window_duration,
            min_recurrences,
        })
    }

    /// The largest number of frames the window observes.
    #[must_use]
    pub fn window_frames(&self) -> usize {
        self.window_frames
    }

    /// The longest the window stays open.
    #[must_use]
    pub fn window_duration(&self) -> Duration {
        self.window_duration
    }

    /// How often a **frame size** must recur to become expected.
    #[must_use]
    pub fn min_recurrences(&self) -> usize {
        self.min_recurrences
    }
}

impl Default for CalibrationPolicy {
    fn default() -> Self {
        Self {
            window_frames: DEFAULT_WINDOW_FRAMES,
            window_duration: DEFAULT_WINDOW_DURATION,
            min_recurrences: DEFAULT_MIN_RECURRENCES,
        }
    }
}

/// Why a window could not establish an **expected frame size**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibrationError {
    /// No **frame size** recurred often enough inside the window.
    NoRecurringFrameSize {
        /// How many frames the window observed.
        window_frames: usize,
        /// The threshold that was applied.
        min_recurrences: usize,
        /// Every frame size seen, with how often it was seen, so the failure
        /// explains itself.
        observed: Vec<(FrameSize, usize)>,
    },
}

impl fmt::Display for CalibrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRecurringFrameSize {
                window_frames,
                min_recurrences,
                observed,
            } => {
                write!(
                    formatter,
                    "no frame size recurred {min_recurrences} times in the calibration window of \
                     {window_frames} frames, so no expected frame size could be established. \
                     Observed:"
                )?;
                for (frame_size, count) in observed {
                    write!(formatter, " {frame_size} x{count};")?;
                }
                Ok(())
            }
        }
    }
}

impl error::Error for CalibrationError {}

/// What the window has concluded so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalibrationOutcome {
    /// The window is still open.
    Establishing,
    /// The window closed and found the **expected frame size**.
    Established {
        /// The steady frame size the sink is opened at.
        expected_frame_size: FrameSize,
        /// How many frames of another size preceded it, the **leading frames**.
        leading_frames: usize,
    },
    /// The window closed and found nothing to expect.
    Failed(CalibrationError),
}

/// Accumulates **frame sizes** until the window closes.
#[derive(Debug)]
pub struct CalibrationWindow {
    policy: CalibrationPolicy,
    counts: BTreeMap<FrameSize, usize>,
    frames: usize,
    outcome: Option<CalibrationOutcome>,
}

impl CalibrationWindow {
    /// Opens a window with a policy.
    #[must_use]
    pub fn new(policy: CalibrationPolicy) -> Self {
        Self {
            policy,
            counts: BTreeMap::new(),
            frames: 0,
            outcome: None,
        }
    }

    /// The policy in force.
    #[must_use]
    pub fn policy(&self) -> &CalibrationPolicy {
        &self.policy
    }

    /// Adds one frame's **frame size** at the given elapsed time, and reports
    /// what the window has concluded.
    ///
    /// Once the window has closed the outcome is stable: a late frame cannot
    /// reopen it or change the established size.
    pub fn observe(&mut self, frame_size: FrameSize, elapsed: Duration) -> CalibrationOutcome {
        if let Some(outcome) = &self.outcome {
            return outcome.clone();
        }

        self.frames += 1;
        *self.counts.entry(frame_size).or_insert(0) += 1;

        let closed =
            self.frames >= self.policy.window_frames || elapsed >= self.policy.window_duration;
        if !closed {
            return CalibrationOutcome::Establishing;
        }

        let outcome = self.establish();
        self.outcome = Some(outcome.clone());
        outcome
    }

    fn establish(&self) -> CalibrationOutcome {
        let expected = self
            .counts
            .iter()
            .filter(|(_, count)| **count >= self.policy.min_recurrences)
            .map(|(frame_size, _)| *frame_size)
            .max_by_key(|frame_size| (frame_size.area(), frame_size.width(), frame_size.height()));

        match expected {
            Some(expected_frame_size) => {
                let recurring = self.counts.get(&expected_frame_size).copied().unwrap_or(0);
                CalibrationOutcome::Established {
                    expected_frame_size,
                    leading_frames: self.frames - recurring,
                }
            }
            None => CalibrationOutcome::Failed(CalibrationError::NoRecurringFrameSize {
                window_frames: self.frames,
                min_recurrences: self.policy.min_recurrences,
                observed: self
                    .counts
                    .iter()
                    .map(|(frame_size, count)| (*frame_size, *count))
                    .collect(),
            }),
        }
    }
}

impl Default for CalibrationWindow {
    fn default() -> Self {
        Self::new(CalibrationPolicy::default())
    }
}
