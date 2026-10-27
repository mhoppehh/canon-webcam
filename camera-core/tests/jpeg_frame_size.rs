//! Frame size is learned from frame data only: the JPEG start-of-frame marker.
//!
//! These tests are the seam for `camera_core::jpeg::parse_frame_size`. They use
//! real JPEG files under `assets/live-view/` so the expected values come from an
//! independent source of truth (the encoder), not from the parser itself. The
//! same files are embedded by `camera_core::synthetic` as a frame source for the
//! tests and demonstrations that must run without a camera.

use camera_core::frame_size::FrameSize;
use camera_core::jpeg::{parse_frame_size, JpegError};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/live-view")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path:?}: {error}"))
}

#[test]
fn reads_the_frame_size_of_a_full_size_live_view_frame() {
    let frame = fixture("frame-1024x576.jpg");

    assert_eq!(
        parse_frame_size(&frame).expect("full size frame must parse"),
        FrameSize::new(1024, 576)
    );
}

#[test]
fn reads_the_frame_size_of_a_reduced_size_live_view_frame() {
    let frame = fixture("frame-512x288.jpg");

    assert_eq!(
        parse_frame_size(&frame).expect("reduced size frame must parse"),
        FrameSize::new(512, 288)
    );
}

#[test]
fn skips_segments_that_carry_no_frame_size() {
    // The real fixture opens with APP0 and quantisation/huffman tables before the
    // start-of-frame marker, so this proves the marker walk rather than a
    // byte-offset shortcut.
    let frame = fixture("frame-1024x576.jpg");

    assert_eq!(
        parse_frame_size(&frame).expect("frame must parse"),
        FrameSize::new(1024, 576)
    );
}

#[test]
fn tolerates_fill_bytes_before_a_marker() {
    // 0xFF fill bytes are legal between segments and are common in camera output.
    let mut frame = vec![0xFF, 0xD8];
    frame.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
    frame.extend_from_slice(&sof0(640, 360, 2));

    assert_eq!(
        parse_frame_size(&frame).expect("fill bytes must not defeat the marker walk"),
        FrameSize::new(640, 360)
    );
}

#[test]
fn reads_a_progressive_frame() {
    let mut frame = vec![0xFF, 0xD8];
    frame.extend_from_slice(&sof(0xC2, 720, 480));

    assert_eq!(
        parse_frame_size(&frame).expect("SOF2 must be read"),
        FrameSize::new(720, 480)
    );
}

#[test]
fn does_not_mistake_a_huffman_table_marker_for_a_start_of_frame() {
    // 0xC4 (DHT) shares the 0xC_ range with the start-of-frame markers. Its
    // payload here would decode to a frame size if it were misread as SOF0.
    let mut frame = vec![0xFF, 0xD8];
    frame.extend_from_slice(&[0xFF, 0xC4]);
    frame.extend_from_slice(&[0x00, 0x08, 0x08, 0x02, 0x00, 0x01, 0x00, 0x01]);
    frame.extend_from_slice(&sof0(320, 240, 1));

    assert_eq!(
        parse_frame_size(&frame).expect("DHT must be skipped"),
        FrameSize::new(320, 240)
    );
}

#[test]
fn rejects_data_that_is_not_a_jpeg() {
    let error = parse_frame_size(b"not a jpeg at all").expect_err("must be rejected");

    assert_eq!(error, JpegError::NotJpeg);
}

#[test]
fn rejects_a_truncated_segment() {
    // An APP0 segment that claims 32 payload bytes but supplies 3.
    let frame = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x20, 0x00, 0x00, 0x00];

    assert_eq!(
        parse_frame_size(&frame).expect_err("must be rejected"),
        JpegError::Truncated
    );
}

#[test]
fn rejects_a_stream_without_a_start_of_frame_marker() {
    let mut frame = vec![0xFF, 0xD8];
    frame.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
    frame.extend_from_slice(&[0xFF, 0xD9]);

    assert_eq!(
        parse_frame_size(&frame).expect_err("must be rejected"),
        JpegError::NoStartOfFrame
    );
}

fn sof(marker: u8, width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, marker, 0x00, 0x11, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0x03; 12]);
    bytes
}

fn sof0(width: u16, height: u16, components: u8) -> Vec<u8> {
    let length = 8 + 3 * u16::from(components);
    let mut bytes = vec![0xFF, 0xC0];
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.push(0x08);
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.push(components);
    bytes.extend(std::iter::repeat_n(0x00, usize::from(components) * 3));
    bytes
}
