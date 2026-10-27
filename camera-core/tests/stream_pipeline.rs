//! The pipeline state machine: calibration window, one expected frame size, and
//! what happens when the camera stops producing it.
//!
//! Every test here drives the pipeline with frame bytes and a scripted elapsed
//! time, and observes a recording sink. Nothing waits and nothing needs a camera.
//!
//! Two rules are load bearing and are asserted on the sink side rather than by
//! trusting the pipeline:
//!
//! - a frame is handed to the sink only at the frame size the sink was opened
//!   at, so no frame handed downstream ever differs from the expected frame size;
//! - a size change closes the sink and re-opens it. It never reinterprets the
//!   sink that is already open.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camera_core::calibration::CalibrationPolicy;
use camera_core::frame_size::FrameSize;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::sink::{FrameSink, SinkError};
use camera_core::stats::StreamStats;
use camera_core::stream::{PipelineEvent, StreamPipeline, StreamPipelineConfig};

const FULL: FrameSize = FrameSize::new(1024, 576);
const REDUCED: FrameSize = FrameSize::new(512, 288);

fn asset(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/live-view")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path:?}: {error}"))
}

fn real_jpeg(frame_size: FrameSize) -> Vec<u8> {
    match frame_size {
        FULL => asset("frame-1024x576.jpg"),
        REDUCED => asset("frame-512x288.jpg"),
        other => panic!("no real fixture for {other}"),
    }
}

/// A JPEG that carries a start-of-frame marker and nothing decodable.
///
/// A frame of an unexpected size is dropped before it is ever decoded, so this is
/// all such a frame needs to be.
fn marker_only(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

#[derive(Debug, Default)]
struct SinkLog {
    opens: Vec<FrameSize>,
    accepted: Vec<FrameSize>,
    closes: usize,
    open_at: Option<FrameSize>,
    refusals: usize,
}

impl SinkLog {
    fn accepted_bytes(&self) -> usize {
        self.accepted.len()
    }
}

#[derive(Clone, Debug, Default)]
struct RecordingSink {
    log: Arc<Mutex<SinkLog>>,
}

impl RecordingSink {
    fn new() -> Self {
        Self::default()
    }

    fn log(&self) -> std::sync::MutexGuard<'_, SinkLog> {
        self.log.lock().expect("the sink log must not be poisoned")
    }
}

impl FrameSink for RecordingSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        _pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        let mut log = self.log();
        log.opens.push(expected_frame_size);
        log.open_at = Some(expected_frame_size);
        Ok(())
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        let mut log = self.log();
        let Some(expected) = log.open_at else {
            log.refusals += 1;
            return Err(SinkError::NotOpen);
        };
        if frame.frame_size() != expected {
            log.refusals += 1;
            return Err(SinkError::FrameSizeMismatch {
                expected,
                actual: frame.frame_size(),
            });
        }
        log.accepted.push(frame.frame_size());
        Ok(())
    }

    fn close(&mut self) -> Result<(), SinkError> {
        let mut log = self.log();
        log.closes += 1;
        log.open_at = None;
        Ok(())
    }
}

fn config() -> StreamPipelineConfig {
    StreamPipelineConfig::new(
        // Twenty frames, eight recurrences: the leading run of five reduced
        // frames cannot outvote the steady size.
        CalibrationPolicy::new(20, Duration::from_secs(3), 8).expect("valid policy"),
    )
}

fn pipeline(config: StreamPipelineConfig) -> (StreamPipeline, RecordingSink, Arc<StreamStats>) {
    let sink = RecordingSink::new();
    let stats = Arc::new(StreamStats::new());
    let pipeline = StreamPipeline::new(Box::new(sink.clone()), Arc::clone(&stats), config);
    (pipeline, sink, stats)
}

/// Twenty milliseconds a frame, so a hundred milliseconds is five frames.
fn at_frame(frame: u32) -> Duration {
    Duration::from_millis(u64::from(frame) * 20)
}

#[test]
fn leading_frames_never_reach_the_sink() {
    let (mut pipeline, sink, _stats) = pipeline(config());

    // The five reduced frames a viewfinder action can produce, then the steady
    // size. The calibration window holds the leading run; there is no discard
    // count anywhere.
    let mut frame = 0;
    for _ in 0..5 {
        pipeline
            .handle(&real_jpeg(REDUCED), at_frame(frame))
            .expect("must not fail");
        frame += 1;
    }
    for _ in 0..20 {
        pipeline
            .handle(&real_jpeg(FULL), at_frame(frame))
            .expect("must not fail");
        frame += 1;
    }

    let log = sink.log();
    assert_eq!(
        log.opens,
        vec![FULL],
        "the sink is opened once, at the steady size"
    );
    assert!(
        log.accepted.iter().all(|size| *size == FULL),
        "a leading frame must never reach the sink, accepted {:?}",
        log.accepted
    );
    assert!(!log.accepted.is_empty(), "steady frames must be delivered");
}

#[test]
fn the_sink_is_opened_at_the_frame_size_the_calibration_window_established() {
    let (mut pipeline, sink, stats) = pipeline(config());

    let mut events = Vec::new();
    for frame in 0..20 {
        events.push(
            pipeline
                .handle(&real_jpeg(FULL), at_frame(frame))
                .expect("must not fail"),
        );
    }

    assert!(
        events.contains(&PipelineEvent::SinkOpened { frame_size: FULL }),
        "events were {events:?}"
    );
    assert_eq!(sink.log().opens, vec![FULL]);
    assert_eq!(pipeline.expected_frame_size(), Some(FULL));
    assert_eq!(stats.snapshot(Duration::from_secs(1)).sink_opens, 1);
}

#[test]
fn a_frame_of_another_size_is_dropped_and_never_forwarded() {
    let (mut pipeline, sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    let event = pipeline
        .handle(&real_jpeg(REDUCED), at_frame(20))
        .expect("must not fail");

    assert_eq!(event, PipelineEvent::DroppedSizeMismatch);
    let log = sink.log();
    assert!(
        log.accepted.iter().all(|size| *size == FULL),
        "accepted {:?}",
        log.accepted
    );
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_dropped_size_mismatch,
        1
    );
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_refused_by_sink,
        0
    );
}

#[test]
fn less_than_a_second_of_mismatches_does_not_start_a_recalibration() {
    let (mut pipeline, _sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    // Twenty frames of another size, 20 ms apart: 380 ms, under the grace.
    for frame in 0..20 {
        let event = pipeline
            .handle(&real_jpeg(REDUCED), at_frame(frame))
            .expect("must not fail");
        assert_eq!(event, PipelineEvent::DroppedSizeMismatch);
    }

    assert_eq!(
        stats.snapshot(Duration::from_secs(1)).recalibrations,
        0,
        "a transient of the reduced size is not a size change"
    );
}

#[test]
fn about_a_second_of_consecutive_mismatches_starts_a_recalibration() {
    let (mut pipeline, _sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    let mut events = Vec::new();
    // 1.2 s of another size at 20 ms a frame.
    for frame in 0..60 {
        events.push(
            pipeline
                .handle(&marker_only(512, 288), at_frame(frame))
                .expect("must not fail"),
        );
    }

    assert!(
        events.contains(&PipelineEvent::Recalibrating),
        "a second of mismatches must start a recalibration, events were {events:?}"
    );
    assert!(stats.snapshot(Duration::from_secs(1)).recalibrations >= 1);
}

#[test]
fn a_recalibration_that_settles_on_a_new_size_closes_and_reopens_the_sink() {
    let (mut pipeline, sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    // A full second of the reduced size triggers the recalibration; the
    // recalibration window then establishes the reduced size.
    let mut events = Vec::new();
    for frame in 0..200 {
        events.push(
            pipeline
                .handle(&real_jpeg(REDUCED), at_frame(frame))
                .expect("must not fail"),
        );
    }

    let log = sink.log();
    assert_eq!(
        log.opens,
        vec![FULL, REDUCED],
        "a size change is a sink re-open at the new size, never a reinterpretation"
    );
    assert_eq!(log.closes, 1, "the old sink must be closed first");
    assert_eq!(pipeline.expected_frame_size(), Some(REDUCED));
    assert!(
        log.accepted
            .iter()
            .all(|size| *size == FULL || *size == REDUCED),
        "accepted {:?}",
        log.accepted
    );
    assert!(
        log.accepted.iter().filter(|size| **size == REDUCED).count() > 0,
        "frames at the new expected size must be delivered after the re-open"
    );
    assert!(
        events.contains(&PipelineEvent::SinkReopened {
            frame_size: REDUCED
        }),
        "events were {events:?}"
    );
    let snapshot = stats.snapshot(Duration::from_secs(1));
    assert_eq!(snapshot.sink_opens, 2);
    assert_eq!(snapshot.sink_reopens, 1);
    assert_eq!(snapshot.frames_refused_by_sink, 0);
}

#[test]
fn a_recalibration_that_does_not_settle_is_reported_rather_than_guessed_at() {
    let mut config = config();
    config = config.with_recalibration(
        CalibrationPolicy::new(6, Duration::from_secs(1), 4).expect("valid policy"),
    );
    let (mut pipeline, sink, stats) = pipeline(config);
    establish(&mut pipeline, FULL);

    // Never the same size twice, so nothing can settle.
    let sizes = [
        (640_u16, 360_u16),
        (320, 180),
        (800, 450),
        (111, 222),
        (333, 444),
        (555, 666),
        (777, 888),
        (999, 111),
    ];
    let mut events = Vec::new();
    for frame in 0..(20 * sizes.len()) {
        let (width, height) = sizes[frame % sizes.len()];
        events.push(
            pipeline
                .handle(&marker_only(width, height), at_frame(frame as u32))
                .expect("must not fail"),
        );
    }

    assert!(
        events.contains(&PipelineEvent::Unsettled),
        "an unsettled recalibration must be escalated, events were {events:?}"
    );
    assert_eq!(
        sink.log().opens,
        vec![FULL],
        "an unsettled recalibration must not re-open the sink at a size nothing agreed on"
    );
    assert_eq!(stats.snapshot(Duration::from_secs(1)).sink_reopens, 0);
}

#[test]
fn a_return_to_the_expected_size_cancels_the_recalibration() {
    let (mut pipeline, sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    // Trigger the recalibration with a second of mismatches...
    let mut frame = 0;
    for _ in 0..60 {
        pipeline
            .handle(&marker_only(800, 450), at_frame(frame))
            .expect("must not fail");
        frame += 1;
    }
    assert!(stats.snapshot(Duration::from_secs(1)).recalibrations >= 1);

    // ...then the camera comes back at the expected size, which settles it
    // without touching the open sink.
    let event = pipeline
        .handle(&real_jpeg(FULL), at_frame(frame))
        .expect("must not fail");

    assert_eq!(event, PipelineEvent::Delivered);
    assert_eq!(sink.log().opens, vec![FULL], "no re-open was needed");
    assert_eq!(stats.snapshot(Duration::from_secs(1)).sink_reopens, 0);
}

#[test]
fn a_frame_with_no_readable_frame_size_is_dropped_and_counted() {
    let (mut pipeline, _sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    let event = pipeline
        .handle(b"\xFF\xD8 not really a jpeg", at_frame(20))
        .expect("a malformed frame must not end the run");

    assert_eq!(event, PipelineEvent::DroppedMalformed);
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_dropped_malformed,
        1
    );
}

#[test]
fn a_frame_that_carries_the_expected_size_but_cannot_be_decoded_is_dropped() {
    let (mut pipeline, sink, stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    // The marker says 1024x576 but there is no image data behind it.
    let event = pipeline
        .handle(&marker_only(1024, 576), at_frame(20))
        .expect("a frame that will not decode must not end the run");

    assert_eq!(event, PipelineEvent::DroppedMalformed);
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_dropped_malformed,
        1
    );
    assert_eq!(sink.log().accepted_bytes(), 0);
    assert_eq!(
        stats
            .snapshot(Duration::from_secs(1))
            .frames_refused_by_sink,
        0
    );
}

#[test]
fn with_upscaling_the_sink_is_opened_at_the_requested_size_and_every_delivered_frame_matches_it() {
    let requested = FrameSize::new(1920, 1080);
    let (mut pipeline, sink, _stats) = pipeline(config().with_resize_to(requested));

    establish(&mut pipeline, FULL);
    pipeline
        .handle(&real_jpeg(FULL), at_frame(20))
        .expect("must not fail");

    let log = sink.log();
    assert_eq!(
        log.opens,
        vec![requested],
        "with resizing requested the sink is opened at the requested size"
    );
    assert_eq!(
        pipeline.expected_frame_size(),
        Some(FULL),
        "the measured size is unchanged"
    );
    assert_eq!(
        log.accepted,
        vec![requested],
        "every delivered frame is at the size the sink was opened at"
    );
    assert_eq!(log.refusals, 0);
}

#[test]
fn shutting_down_closes_the_sink_and_is_harmless_when_already_closed() {
    let (mut pipeline, sink, _stats) = pipeline(config());
    establish(&mut pipeline, FULL);

    pipeline.shutdown().expect("must close");
    pipeline.shutdown().expect("closing twice must be harmless");

    assert_eq!(sink.log().closes, 2);
    assert_eq!(sink.log().open_at, None);
}

#[test]
fn the_counters_agree_that_every_received_frame_was_delivered_or_dropped() {
    let (mut pipeline, _sink, stats) = pipeline(config());

    let mut frame = 0;
    let mut fed = 0;
    for _ in 0..5 {
        pipeline
            .handle(&real_jpeg(REDUCED), at_frame(frame))
            .expect("must not fail");
        frame += 1;
        fed += 1;
    }
    for _ in 0..40 {
        pipeline
            .handle(&real_jpeg(FULL), at_frame(frame))
            .expect("must not fail");
        frame += 1;
        fed += 1;
    }
    pipeline
        .handle(&marker_only(700, 400), at_frame(frame))
        .expect("must not fail");
    fed += 1;

    let snapshot = stats.snapshot(Duration::from_secs(1));
    assert_eq!(
        snapshot.frames_received, fed,
        "every frame is counted on arrival"
    );
    assert_eq!(
        snapshot.frames_accounted(),
        snapshot.frames_received,
        "every frame is delivered, dropped, or observed by a calibration window"
    );
    assert!(
        snapshot.frames_dropped() > 0,
        "the frame of an unexpected size must be counted as dropped"
    );
    assert!(
        snapshot.frames_observed_for_calibration >= 20,
        "the opening calibration window is where the leading frames go"
    );
}

fn establish(pipeline: &mut StreamPipeline, frame_size: FrameSize) {
    for frame in 0..20 {
        let event = pipeline
            .handle(&real_jpeg(frame_size), at_frame(frame))
            .expect("calibration must not fail");
        assert!(
            matches!(
                event,
                PipelineEvent::Calibrating
                    | PipelineEvent::SinkOpened { .. }
                    | PipelineEvent::Delivered
            ),
            "unexpected event while establishing: {event:?}"
        );
    }
    assert_eq!(pipeline.expected_frame_size(), Some(frame_size));
}
