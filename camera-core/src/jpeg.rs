//! Reads the **frame size** out of a live view frame.
//!
//! The M50 sends JPEG frames, so the frame size is the dimensions in the JPEG
//! start-of-frame marker. Reading it from the frame data is the only honest way
//! to learn it: the camera honours no live view format it is asked for, and the
//! `liveviewsize` property is a no-op on Canon (see
//! `docs/adr/0004-no-live-view-size-selection.md`).
//!
//! Only the marker walk is implemented, so no image decoding crate is needed.

use std::error;
use std::fmt;

use crate::frame_size::FrameSize;

/// The start of image marker, the first two bytes of every JPEG frame.
const START_OF_IMAGE: [u8; 2] = [0xFF, 0xD8];

/// The marker prefix byte that introduces every segment.
const MARKER_PREFIX: u8 = 0xFF;

/// End of image. Reaching it without a start-of-frame marker means the frame
/// carries no frame size.
const END_OF_IMAGE: u8 = 0xD9;

/// The marker code of the start of image, used to skip a stray one.
const START_OF_IMAGE_CODE: u8 = 0xD8;

/// A number that could not be read as a JPEG frame size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JpegError {
    /// The data does not begin with the JPEG start-of-image marker.
    NotJpeg,
    /// A segment ran past the end of the data.
    Truncated,
    /// The frame carries no start-of-frame marker, so it carries no frame size.
    NoStartOfFrame,
    /// The marker walk found a frame size but the decoder refused the frame, or
    /// produced a size the marker did not claim.
    Decode {
        /// What the decoder said.
        detail: String,
    },
}

impl fmt::Display for JpegError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotJpeg => formatter.write_str("the frame is not a JPEG"),
            Self::Truncated => formatter.write_str("the frame is truncated mid-segment"),
            Self::NoStartOfFrame => {
                formatter.write_str("the frame has no JPEG start-of-frame marker")
            }
            Self::Decode { detail } => {
                write!(formatter, "the frame could not be decoded: {detail}")
            }
        }
    }
}

impl error::Error for JpegError {}

/// Returns the **frame size** recorded in the JPEG start-of-frame marker.
///
/// # Errors
///
/// Returns [`JpegError`] if the data is not a JPEG, is truncated, or contains no
/// start-of-frame marker.
pub fn parse_frame_size(frame: &[u8]) -> Result<FrameSize, JpegError> {
    if frame.len() < START_OF_IMAGE.len() || frame[..START_OF_IMAGE.len()] != START_OF_IMAGE {
        return Err(JpegError::NotJpeg);
    }

    let mut cursor = START_OF_IMAGE.len();

    loop {
        if cursor >= frame.len() {
            return Err(JpegError::NoStartOfFrame);
        }
        if frame[cursor] != MARKER_PREFIX {
            return Err(JpegError::NotJpeg);
        }
        // Any number of 0xFF fill bytes may precede the marker code.
        while cursor < frame.len() && frame[cursor] == MARKER_PREFIX {
            cursor += 1;
        }
        let Some(&marker) = frame.get(cursor) else {
            return Err(JpegError::Truncated);
        };
        cursor += 1;

        match marker {
            END_OF_IMAGE => return Err(JpegError::NoStartOfFrame),
            // Markers without a length field, and a stray start of image.
            START_OF_IMAGE_CODE | 0x01 | 0xD0..=0xD7 => {}
            // Start-of-frame markers, except the three in this range that are
            // not frames: DHT (0xC4), JPG (0xC8) and DAC (0xCC).
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                // Segment length (2 bytes), precision (1 byte), height (2
                // bytes), width (2 bytes).
                let fields = frame.get(cursor..cursor + 7).ok_or(JpegError::Truncated)?;
                let height = u32::from(u16::from_be_bytes([fields[3], fields[4]]));
                let width = u32::from(u16::from_be_bytes([fields[5], fields[6]]));
                return Ok(FrameSize::new(width, height));
            }
            _ => {
                let length_bytes = frame.get(cursor..cursor + 2).ok_or(JpegError::Truncated)?;
                let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
                if length < 2 {
                    return Err(JpegError::Truncated);
                }
                cursor = cursor.checked_add(length).ok_or(JpegError::Truncated)?;
                if cursor > frame.len() {
                    return Err(JpegError::Truncated);
                }
            }
        }
    }
}
