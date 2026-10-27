//! One normalised pixel format, decoded once, for every sink.
//!
//! These tests use the real JPEGs under `assets/live-view/`, so the decoder is
//! exercised against frames an encoder actually produced rather than against
//! hand-built bytes.

use camera_core::frame_size::FrameSize;
use camera_core::jpeg::parse_frame_size;
use camera_core::pixel_format::{FrameError, LiveViewFrame, PixelFormat};

const FULL: FrameSize = FrameSize::new(1024, 576);
const REDUCED: FrameSize = FrameSize::new(512, 288);

fn asset(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/live-view")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path:?}: {error}"))
}

#[test]
fn a_full_size_live_view_frame_decodes_to_rgb24() {
    let frame = LiveViewFrame::from_jpeg(&asset("frame-1024x576.jpg")).expect("must decode");

    assert_eq!(frame.frame_size(), FULL);
    assert_eq!(frame.pixel_format(), PixelFormat::Rgb24);
    assert_eq!(
        frame.pixels().len(),
        1024 * 576 * 3,
        "rgb24 is three bytes per pixel with no padding"
    );
}

#[test]
fn a_reduced_size_live_view_frame_decodes_to_the_same_format() {
    let frame = LiveViewFrame::from_jpeg(&asset("frame-512x288.jpg")).expect("must decode");

    assert_eq!(frame.frame_size(), REDUCED);
    assert_eq!(frame.pixel_format(), PixelFormat::Rgb24);
    assert_eq!(frame.pixels().len(), 512 * 288 * 3);
}

#[test]
fn the_decoded_frame_size_agrees_with_the_start_of_frame_marker() {
    let jpeg = asset("frame-1024x576.jpg");
    let frame = LiveViewFrame::from_jpeg(&jpeg).expect("must decode");

    assert_eq!(
        parse_frame_size(&jpeg).expect("must parse"),
        frame.frame_size(),
        "the marker walk and the decoder must read the same frame size"
    );
}

#[test]
fn an_unresized_frame_keeps_the_camera_jpeg_for_passthrough() {
    // ADR-0002 publishes the camera's JPEG unchanged when no resizing is
    // requested, so the sink must be able to reach those bytes.
    let jpeg = asset("frame-1024x576.jpg");
    let frame = LiveViewFrame::from_jpeg(&jpeg).expect("must decode");

    assert_eq!(frame.source_jpeg(), Some(jpeg.as_slice()));
}

#[test]
fn something_that_is_not_a_jpeg_is_refused() {
    let error = LiveViewFrame::from_jpeg(b"not a jpeg at all").expect_err("must be refused");

    assert!(
        error.to_string().contains("not a JPEG"),
        "message was {error}"
    );
}

#[test]
fn a_truncated_jpeg_is_refused_rather_than_half_decoded() {
    let truncated = &asset("frame-1024x576.jpg")[..200];

    let error = LiveViewFrame::from_jpeg(truncated).expect_err("must be refused");

    assert!(
        error.to_string().contains("could not be decoded")
            || error.to_string().contains("truncated"),
        "message was {error}"
    );
}

#[test]
fn a_pixel_buffer_of_the_wrong_length_cannot_become_a_frame() {
    let error = LiveViewFrame::from_rgb24(REDUCED, vec![0; 10]).expect_err("must be refused");

    assert_eq!(
        error,
        FrameError::PixelCountMismatch {
            frame_size: REDUCED,
            expected: 512 * 288 * 3,
            actual: 10,
        }
    );
    assert!(error.to_string().contains("512x288"), "message was {error}");
}

#[test]
fn resizing_changes_the_frame_size_and_drops_the_original_jpeg() {
    let frame = LiveViewFrame::from_jpeg(&asset("frame-1024x576.jpg")).expect("must decode");

    let resized = frame.resize_to(FrameSize::new(1920, 1080));

    assert_eq!(resized.frame_size(), FrameSize::new(1920, 1080));
    assert_eq!(resized.pixels().len(), 1920 * 1080 * 3);
    assert_eq!(
        resized.source_jpeg(),
        None,
        "the camera's JPEG describes the old geometry, so it cannot be published to a sink \
         opened at the new one"
    );
}

#[test]
fn resizing_replicates_pixels_because_it_adds_no_detail() {
    // Two pixels, red then blue, stretched to four.
    let frame = LiveViewFrame::from_rgb24(FrameSize::new(2, 1), vec![255, 0, 0, 0, 0, 255])
        .expect("valid frame");

    let stretched = frame.resize_to(FrameSize::new(4, 1));

    assert_eq!(
        stretched.pixels(),
        &[255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255]
    );
}

#[test]
fn the_pixel_format_round_trips_through_its_configuration_key() {
    assert_eq!("rgb24", PixelFormat::Rgb24.as_str());
    assert_eq!("rgb24", PixelFormat::Rgb24.to_string());
    assert_eq!(
        "rgb24".parse::<PixelFormat>(),
        Ok(PixelFormat::Rgb24),
        "the configuration key must parse back"
    );
}

#[test]
fn an_unknown_pixel_format_names_the_key_it_came_from() {
    let error = "yuv420p"
        .parse::<PixelFormat>()
        .expect_err("must be refused");

    assert_eq!(error.value(), "yuv420p");
    assert!(
        error.to_string().contains("pixel_format"),
        "the refusal must name the offending key, message was {error}"
    );
}
