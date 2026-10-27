//! One normalised pixel format for everything downstream of the pipeline.
//!
//! The head of the pipeline sees JPEG, because that is what the M50 sends. The
//! sink layer must not: a platform backend that each decoded its own way would
//! make "one normalised pixel format" a wish. So decoding happens once, here, and
//! produces a [`LiveViewFrame`] in [`PixelFormat::Rgb24`].
//!
//! `Rgb24` is eight bits per channel, packed, row-major, with no padding and no
//! alpha: three bytes per pixel, `width * height * 3` in total. It was chosen
//! because it is the smallest representation every candidate sink can consume
//! without a further conversion, and because two of the three platform backends
//! expect exactly it.
//!
//! A frame also keeps the JPEG it was decoded from while it has not been
//! resized. `docs/adr/0002-macos-virtual-camera-mechanism.md` requires the macOS
//! mechanism to publish the camera's JPEG unchanged when no resizing is
//! requested, so that the live view feed is never re-encoded on the way to the
//! screen. Keeping the original bytes is what lets the sink do that; a resized
//! frame has no such bytes and carries `None`.

use std::error;
use std::fmt;
use std::str::FromStr;

use crate::frame_size::FrameSize;
use crate::jpeg::{parse_frame_size, JpegError};

/// The pixel format every frame handed downstream is in.
///
/// One variant on purpose: this is a decision, not a menu. A second variant
/// means a second conversion somewhere, and nothing has asked for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Packed eight-bit RGB, three bytes per pixel, row-major, no padding.
    Rgb24,
}

impl PixelFormat {
    /// The bytes one pixel occupies.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb24 => 3,
        }
    }

    /// The key used in the TOML configuration file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The pixel format named in the configuration could not be understood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPixelFormat {
    value: String,
}

impl UnknownPixelFormat {
    /// The value as typed.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for UnknownPixelFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "`pixel_format` is `{}`, which is not a pixel format this program produces. It \
             produces only `rgb24`: the pipeline decodes each JPEG once and hands every sink the \
             same packed eight-bit RGB",
            self.value
        )
    }
}

impl error::Error for UnknownPixelFormat {}

impl FromStr for PixelFormat {
    type Err = UnknownPixelFormat;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "rgb24" | "rgb" => Ok(Self::Rgb24),
            _ => Err(UnknownPixelFormat {
                value: value.to_owned(),
            }),
        }
    }
}

/// A frame cannot be built from the pixels it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The pixel buffer is not the size the frame size requires.
    PixelCountMismatch {
        /// The frame size the pixels were meant to fill.
        frame_size: FrameSize,
        /// `width * height * bytes_per_pixel`.
        expected: usize,
        /// What was supplied.
        actual: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PixelCountMismatch {
                frame_size,
                expected,
                actual,
            } => write!(
                formatter,
                "a {frame_size} frame in rgb24 needs {expected} bytes, but {actual} were given"
            ),
        }
    }
}

impl error::Error for FrameError {}

/// One decoded live view frame, normalised for the sink layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveViewFrame {
    frame_size: FrameSize,
    pixel_format: PixelFormat,
    pixels: Vec<u8>,
    source_jpeg: Option<Vec<u8>>,
}

impl LiveViewFrame {
    /// Decodes a JPEG exactly as the camera delivered it.
    ///
    /// The **frame size** comes from the same start-of-frame marker the
    /// calibration window uses, so a frame cannot be delivered at a size the
    /// pipeline did not expect.
    ///
    /// # Errors
    ///
    /// Returns [`JpegError`] if the frame is not a JPEG, is truncated, carries no
    /// start-of-frame marker, or the decoder refuses it.
    pub fn from_jpeg(jpeg: &[u8]) -> Result<Self, JpegError> {
        let frame_size = parse_frame_size(jpeg)?;

        let mut decoder = jpeg_decoder::Decoder::new(jpeg);
        let pixels = decoder.decode().map_err(|error| JpegError::Decode {
            detail: error.to_string(),
        })?;
        let info = decoder.info().ok_or_else(|| JpegError::Decode {
            detail: "the decoder reported no frame information".to_owned(),
        })?;

        // The marker walk and the decoder read the same start-of-frame segment.
        // Disagreement means one of them misread the frame, and forwarding it
        // under the other's frame size would hand a sink the wrong geometry.
        let decoded_size = FrameSize::new(u32::from(info.width), u32::from(info.height));
        if decoded_size != frame_size {
            return Err(JpegError::Decode {
                detail: format!(
                    "the start-of-frame marker says {frame_size} but the decoder produced \
                     {decoded_size}"
                ),
            });
        }
        if info.pixel_format != jpeg_decoder::PixelFormat::RGB24 {
            return Err(JpegError::Decode {
                detail: format!(
                    "the decoder produced {:?}, which is not the rgb24 the pipeline normalises to",
                    info.pixel_format
                ),
            });
        }

        Self::from_rgb24(frame_size, pixels)
            .map(|mut frame| {
                frame.source_jpeg = Some(jpeg.to_vec());
                frame
            })
            .map_err(|error| JpegError::Decode {
                detail: error.to_string(),
            })
    }

    /// Builds a frame from packed RGB pixels.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError`] if the buffer is not `width * height * 3` bytes.
    pub fn from_rgb24(frame_size: FrameSize, pixels: Vec<u8>) -> Result<Self, FrameError> {
        let expected = pixel_bytes(frame_size, PixelFormat::Rgb24);
        if pixels.len() != expected {
            return Err(FrameError::PixelCountMismatch {
                frame_size,
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            frame_size,
            pixel_format: PixelFormat::Rgb24,
            pixels,
            source_jpeg: None,
        })
    }

    /// The frame's pixel dimensions.
    #[must_use]
    pub fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// The format [`LiveViewFrame::pixels`] is in.
    #[must_use]
    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// The packed pixel bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The JPEG this frame was decoded from, if it was not resized.
    ///
    /// The macOS mechanism publishes these bytes unchanged when no resizing was
    /// requested, so the live view feed is not re-encoded on the way to the
    /// screen. A resized frame has no original geometry to publish and returns
    /// `None`.
    #[must_use]
    pub fn source_jpeg(&self) -> Option<&[u8]> {
        self.source_jpeg.as_deref()
    }

    /// Resizes the frame to `target` by nearest neighbour.
    ///
    /// This is **resizing**, not resolution: it interpolates nothing and reveals
    /// no detail the camera did not send. It exists only because a consumer
    /// application may refuse a 1024x576 device and accept a larger geometry.
    #[must_use]
    pub fn resize_to(&self, target: FrameSize) -> Self {
        Self {
            frame_size: target,
            pixel_format: self.pixel_format,
            pixels: nearest_neighbour(self.frame_size, &self.pixels, target),
            // The original bytes describe the old geometry, so publishing them
            // to a sink opened at the new one would be a lie.
            source_jpeg: None,
        }
    }
}

fn pixel_bytes(frame_size: FrameSize, pixel_format: PixelFormat) -> usize {
    let pixels = usize::try_from(u64::from(frame_size.width()) * u64::from(frame_size.height()))
        .unwrap_or(usize::MAX);
    pixels.saturating_mul(pixel_format.bytes_per_pixel())
}

fn nearest_neighbour(from: FrameSize, pixels: &[u8], to: FrameSize) -> Vec<u8> {
    let channels = PixelFormat::Rgb24.bytes_per_pixel();
    let source_width = from.width() as usize;
    let source_height = from.height() as usize;
    let target_width = to.width() as usize;
    let target_height = to.height() as usize;

    let mut out = vec![0_u8; target_width * target_height * channels];
    if source_width == 0 || source_height == 0 {
        return out;
    }

    for y in 0..target_height {
        let source_y = y * source_height / target_height;
        for x in 0..target_width {
            let source_x = x * source_width / target_width;
            let source = (source_y * source_width + source_x) * channels;
            let target = (y * target_width + x) * channels;
            out[target..target + channels].copy_from_slice(&pixels[source..source + channels]);
        }
    }
    out
}
