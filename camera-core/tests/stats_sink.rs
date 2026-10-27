//! The sink contract and the counters behind the run report.
//!
//! The stats sink stands in for ticket 04's platform backend: it is a real
//! `FrameSink`, so the rule "no frame handed downstream differs from the expected
//! frame size" is tested against the same trait a backend implements.

use std::sync::Arc;
use std::time::Duration;

use camera_core::frame_size::FrameSize;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::sink::{FrameSink, SinkError};
use camera_core::stats::{StatsSink, StreamStats};

const FULL: FrameSize = FrameSize::new(1024, 576);

fn frame(frame_size: FrameSize) -> LiveViewFrame {
    let pixels = (frame_size.width() * frame_size.height()) as usize * 3;
    LiveViewFrame::from_rgb24(frame_size, vec![7; pixels]).expect("valid frame")
}

#[test]
fn an_open_sink_accepts_a_frame_at_the_size_it_was_opened_at() {
    let stats = Arc::new(StreamStats::new());
    let mut sink = StatsSink::new(Arc::clone(&stats));

    sink.open(FULL, PixelFormat::Rgb24).expect("must open");
    sink.accept(&frame(FULL)).expect("must accept");

    assert_eq!(sink.opened_at(), Some(FULL));
    assert_eq!(sink.pixel_format(), PixelFormat::Rgb24);
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_refused_by_sink,
        0,
        "a frame at the expected size must not be refused"
    );
}

#[test]
fn a_sink_refuses_a_frame_of_another_size_rather_than_resizing_itself() {
    let stats = Arc::new(StreamStats::new());
    let mut sink = StatsSink::new(Arc::clone(&stats));
    sink.open(FULL, PixelFormat::Rgb24).expect("must open");

    let error = sink
        .accept(&frame(FrameSize::new(512, 288)))
        .expect_err("a size change must close and re-open the sink, never reinterpret it");

    assert_eq!(
        error,
        SinkError::FrameSizeMismatch {
            expected: FULL,
            actual: FrameSize::new(512, 288),
        }
    );
    assert!(
        error.to_string().contains("closed and re-opened"),
        "message was {error}"
    );
    let snapshot = stats.snapshot(Duration::from_secs(1));
    assert_eq!(snapshot.frames_delivered, 0);
    assert_eq!(snapshot.frames_refused_by_sink, 1);
}

#[test]
fn a_closed_sink_refuses_frames() {
    let stats = Arc::new(StreamStats::new());
    let mut sink = StatsSink::new(Arc::clone(&stats));

    let error = sink
        .accept(&frame(FULL))
        .expect_err("a frame must not be accepted before opening");

    assert_eq!(error, SinkError::NotOpen);
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_refused_by_sink,
        1
    );
}

#[test]
fn closing_a_sink_is_idempotent_because_every_exit_path_closes_it() {
    let stats = Arc::new(StreamStats::new());
    let mut sink = StatsSink::new(stats);

    sink.open(FULL, PixelFormat::Rgb24).expect("must open");

    assert_eq!(sink.close(), Ok(()));
    assert_eq!(
        sink.close(),
        Ok(()),
        "close must succeed when already closed"
    );
    assert_eq!(sink.opened_at(), None);
}

#[test]
fn re_opening_a_sink_at_a_new_size_accepts_only_that_size() {
    let stats = Arc::new(StreamStats::new());
    let mut sink = StatsSink::new(Arc::clone(&stats));
    let reduced = FrameSize::new(512, 288);

    sink.open(FULL, PixelFormat::Rgb24).expect("must open");
    sink.close().expect("must close");
    stats.record_sink_reopen();
    sink.open(reduced, PixelFormat::Rgb24)
        .expect("must re-open");

    assert_eq!(sink.opened_at(), Some(reduced));
    sink.accept(&frame(reduced))
        .expect("new size must be accepted");
    assert!(sink.accept(&frame(FULL)).is_err());

    let snapshot = stats.snapshot(Duration::from_secs(1));
    assert_eq!(snapshot.sink_opens, 2);
    assert_eq!(snapshot.sink_reopens, 1);
    assert_eq!(
        snapshot.frames_refused_by_sink, 1,
        "the old size is now refused"
    );
}

#[test]
fn the_report_computes_the_measured_rate_and_the_data_rate_from_what_was_delivered() {
    let stats = StreamStats::new();

    for _ in 0..10 {
        stats.record_delivered(1024 * 576 * 3);
    }

    let snapshot = stats.snapshot(Duration::from_millis(500));

    assert_eq!(snapshot.frames_delivered, 10);
    assert_eq!(snapshot.measured_frame_rate, 20.0);
    assert_eq!(
        snapshot.data_rate,
        10.0 * (1024.0 * 576.0 * 3.0) / 0.5,
        "the data rate is the delivered pixel bytes per second"
    );
}

#[test]
fn the_accounting_is_exact_so_no_frame_disappears() {
    let stats = StreamStats::new();

    for _ in 0..5 {
        stats.record_received();
        stats.record_calibration_observation();
    }
    for _ in 0..10 {
        stats.record_received();
        stats.record_delivered(3);
    }
    for _ in 0..2 {
        stats.record_received();
        stats.record_size_mismatch();
    }

    let snapshot = stats.snapshot(Duration::from_secs(1));

    assert_eq!(snapshot.frames_received, 17);
    assert_eq!(snapshot.frames_accounted(), snapshot.frames_received);
}

#[test]
fn the_report_counts_every_kind_of_drop_separately_and_sees_the_queue_pressure() {
    let stats = StreamStats::new();

    stats.record_received();
    stats.record_received();
    stats.record_received();
    stats.record_size_mismatch();
    stats.record_malformed();
    stats.record_queue_drops(5);
    stats.observe_queue_depth(2);
    stats.observe_queue_depth(4);
    stats.observe_queue_depth(1);

    let snapshot = stats.snapshot(Duration::from_secs(2));

    assert_eq!(snapshot.frames_received, 3);
    assert_eq!(snapshot.frames_captured(), 8);
    assert_eq!(snapshot.frames_dropped_size_mismatch, 1);
    assert_eq!(snapshot.frames_dropped_malformed, 1);
    assert_eq!(snapshot.frames_dropped_by_queue, 5);
    assert_eq!(snapshot.frames_dropped(), 2);
    assert_eq!(snapshot.frames_dropped_total(), 7);
    assert_eq!(
        snapshot.peak_queue_depth, 4,
        "the peak depth is what bounds latency"
    );
}

#[test]
fn a_zero_length_run_reports_zero_rates_rather_than_dividing_by_zero() {
    let snapshot = StreamStats::new().snapshot(Duration::ZERO);

    assert_eq!(snapshot.measured_frame_rate, 0.0);
    assert_eq!(snapshot.data_rate, 0.0);
}
