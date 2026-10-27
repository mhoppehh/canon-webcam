//! The **virtual camera sink** and the honest statistics about what reached it.
//!
//! The sink is the seam a platform backend implements: `open`, `accept` a frame,
//! `close`, and nothing else. It is handed normalised frames and is never asked
//! to interpret a JPEG or to know where a frame came from.
//!
//! The stats sink is the sink this build uses when no virtual camera is wanted.
//! It accepts frames and counts them, which is what makes "frames delivered,
//! frames dropped, measured rate, data rate" a measurement rather than a claim.

use std::error;
use std::fmt;

use crate::frame_size::FrameSize;
use crate::pixel_format::{LiveViewFrame, PixelFormat};

/// Why a sink refused to do something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SinkError {
    /// A frame was offered before the sink was opened, or after it was closed.
    NotOpen,
    /// A frame arrived whose frame size is not the one the sink was opened at.
    ///
    /// This is not a recoverable condition and it is not expected to happen: the
    /// pipeline drops mismatched frames before they reach a sink. It is checked
    /// here so that "no frame handed downstream differs from the expected frame
    /// size" holds by construction even if a caller forgets.
    FrameSizeMismatch {
        /// The frame size the sink was opened at.
        expected: FrameSize,
        /// The frame size that arrived.
        actual: FrameSize,
    },
    /// The platform backend failed. The detail is the backend's own explanation,
    /// which is written to be actionable.
    Backend {
        /// What the backend said.
        detail: String,
    },
}

impl fmt::Display for SinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOpen => formatter.write_str("the virtual camera sink is not open"),
            Self::FrameSizeMismatch { expected, actual } => write!(
                formatter,
                "the virtual camera sink was opened at {expected} and was handed a {actual} frame; \
                 a sink is never resized while it is open, it is closed and re-opened"
            ),
            Self::Backend { detail } => {
                write!(formatter, "the virtual camera sink failed: {detail}")
            }
        }
    }
}

impl error::Error for SinkError {}

/// Everything downstream of the pipeline implements this.
///
/// One sink and one implementation per platform; the platform backend is fixed at
/// build time rather than chosen by a user, so this trait has no notion of a
/// mechanism at all.
///
/// Frames arrive already decoded, in [`PixelFormat::Rgb24`]. A frame is handed
/// over only when its frame size equals the size the sink was opened at.
pub trait FrameSink {
    /// Opens the sink at the **expected frame size**.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError`] if the platform backend cannot open. Those failures
    /// name the OBS setting or command to apply.
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        pixel_format: PixelFormat,
    ) -> Result<(), SinkError>;

    /// Hands one frame to the sink.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError::NotOpen`] if the sink is not open, or
    /// [`SinkError::FrameSizeMismatch`] if the frame is not at the expected frame
    /// size.
    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError>;

    /// Closes the sink, releasing the device.
    ///
    /// Closing is idempotent and must succeed when the sink is already closed,
    /// because every exit path closes it and a crashed run must not leave the
    /// device held.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError`] if the platform backend cannot release the device.
    fn close(&mut self) -> Result<(), SinkError>;
}
