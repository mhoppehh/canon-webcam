//! The whole pipeline with real threads: capture, bounded queue, decode, sink.
//!
//! No camera is attached, so the camera is `SyntheticSession`, which produces
//! committed JPEGs at the two frame sizes this camera has ever been measured at.
//! Every run here is bounded by the run duration or by the synthetic camera
//! failing, so the suite stays fast.
//!
//! What these tests can prove: the stages are wired the way they are described,
//! drops are counted, a size change re-opens the sink, and a camera that stops
//! answering is reported as a **wedged camera** or a **disconnected camera** with
//! distinct messages. What they cannot prove, because no M50 is attached, is that
//! an M50 does any of it: that is recorded in the ticket.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camera_core::calibration::CalibrationPolicy;
use camera_core::frame_size::FrameSize;
use camera_core::measurement::MonotonicClock;
use camera_core::output_setting::OutputSetting;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::runner::{classify_session_failure, run, StreamEnd, StreamPlan};
use camera_core::session::SessionError;
use camera_core::sink::{FrameSink, SinkError};
use camera_core::stats::StatsSink;
use camera_core::stream::{StreamError, StreamPipelineConfig};
use camera_core::synthetic::{
    live_view_jpeg, SyntheticSession, FULL_FRAME_SIZE, REDUCED_FRAME_SIZE,
};

#[derive(Debug, Default)]
struct SinkLog {
    opens: Vec<FrameSize>,
    accepted: Vec<FrameSize>,
    closes: usize,
}

#[derive(Clone, Debug, Default)]
struct LoggedSink {
    log: Arc<Mutex<SinkLog>>,
    fail_open: bool,
}

impl LoggedSink {
    fn new() -> Self {
        Self::default()
    }

    fn failing() -> Self {
        Self {
            fail_open: true,
            ..Self::default()
        }
    }

    fn log(&self) -> std::sync::MutexGuard<'_, SinkLog> {
        self.log.lock().expect("the sink log must not be poisoned")
    }
}

impl FrameSink for LoggedSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        _pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        if self.fail_open {
            return Err(SinkError::Backend {
                detail: "OBS is not installed in /Applications".to_owned(),
            });
        }
        self.log().opens.push(expected_frame_size);
        Ok(())
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        self.log().accepted.push(frame.frame_size());
        Ok(())
    }

    fn close(&mut self) -> Result<(), SinkError> {
        self.log().closes += 1;
        Ok(())
    }
}

fn pipeline_config() -> StreamPipelineConfig {
    StreamPipelineConfig::new(
        CalibrationPolicy::new(20, Duration::from_secs(30), 8).expect("valid policy"),
    )
}

fn plan() -> StreamPlan {
    StreamPlan::new(OutputSetting::tft_plus_pc(), pipeline_config())
}

fn fast_session() -> SyntheticSession {
    SyntheticSession::new(FULL_FRAME_SIZE).with_frame_period(Duration::from_millis(3))
}

#[test]
fn a_timed_run_delivers_frames_and_reports_what_it_did() {
    let mut session = fast_session();
    let clock = MonotonicClock::new();

    let run = run(
        &mut session,
        &plan().with_run_for(Duration::from_millis(300)),
        Box::new(StatsSink::new(Arc::new(
            camera_core::stats::StreamStats::new(),
        ))),
        &clock,
    )
    .expect("the run must complete");

    assert_eq!(run.end, StreamEnd::DurationElapsed);
    assert_eq!(
        run.measured_frame_size,
        Some(FULL_FRAME_SIZE),
        "the measured frame size comes from the frames"
    );
    assert!(run.stats.frames_delivered > 0, "no frame reached the sink");
    assert!(
        run.stats.measured_frame_rate > 0.0,
        "the measured rate comes from delivered frames"
    );
    assert_eq!(
        run.stats.frames_accounted(),
        run.stats.frames_received,
        "every received frame is delivered, dropped, or observed while calibrating"
    );
    assert!(
        run.stats.peak_queue_depth <= 4,
        "the queue is bounded, so latency is bounded, peak depth was {}",
        run.stats.peak_queue_depth
    );
}

#[test]
fn the_capture_path_reports_its_operations_and_only_hands_frames_over() {
    let mut session = fast_session();
    let clock = MonotonicClock::new();

    let _ = run(
        &mut session,
        &plan().with_run_for(Duration::from_millis(100)),
        Box::new(StatsSink::new(Arc::new(
            camera_core::stats::StreamStats::new(),
        ))),
        &clock,
    )
    .expect("the run must complete");

    let log = session.log();
    assert_eq!(log[0], "set-output:3", "the output setting comes first");
    assert_eq!(log[1], "viewfinder", "then the viewfinder action");
    assert!(
        log[2..].iter().all(|entry| entry == "capture"),
        "nothing may be re-applied between captures in a steady run"
    );
}

#[test]
fn leading_frames_never_reach_the_sink() {
    let mut session = fast_session().with_leading_frames(5);
    let sink = LoggedSink::new();
    let clock = MonotonicClock::new();

    let run = run(
        &mut session,
        &plan().with_run_for(Duration::from_millis(300)),
        Box::new(sink.clone()),
        &clock,
    )
    .expect("the run must complete");

    let log = sink.log();
    assert_eq!(
        log.opens,
        vec![FULL_FRAME_SIZE],
        "the sink is opened only once the calibration window has established the steady size"
    );
    assert!(
        log.accepted.iter().all(|size| *size == FULL_FRAME_SIZE),
        "a leading frame reached the sink: {:?}",
        log.accepted
    );
    assert!(!log.accepted.is_empty());
    assert!(
        run.stats.frames_observed_for_calibration >= 20,
        "the calibration window is where the leading frames go"
    );
}

#[test]
fn a_size_change_re_opens_the_sink_and_the_feed_continues() {
    // Forty frames at the full size, so the calibration window settles first, and
    // then the reduced size, which only a sink re-open can serve.
    let mut session = fast_session().changing_size_after(40, REDUCED_FRAME_SIZE);
    let sink = LoggedSink::new();
    let clock = MonotonicClock::new();

    let run = run(
        &mut session,
        &plan().with_run_for(Duration::from_millis(2_000)),
        Box::new(sink.clone()),
        &clock,
    )
    .expect("the run must complete");

    assert_eq!(run.end, StreamEnd::DurationElapsed);
    let log = sink.log();
    assert_eq!(
        log.opens,
        vec![FULL_FRAME_SIZE, REDUCED_FRAME_SIZE],
        "a size change is a sink re-open, never a reinterpretation of the open sink"
    );
    assert_eq!(run.stats.sink_reopens, 1);
    assert!(
        log.accepted.contains(&REDUCED_FRAME_SIZE),
        "the feed must continue at the new size"
    );
    assert!(
        session
            .log()
            .iter()
            .filter(|entry| *entry == "set-output:3")
            .count()
            >= 2,
        "a recalibration re-applies the output setting rather than assuming it held"
    );
    assert!(
        session
            .log()
            .iter()
            .filter(|entry| *entry == "viewfinder")
            .count()
            >= 2,
        "a recalibration re-applies the viewfinder action, which does not persist"
    );
}

#[test]
fn a_camera_that_disappears_is_reported_as_disconnected() {
    let mut session = fast_session().failing_after(30, SyntheticSession::disconnected());
    let sink = LoggedSink::new();
    let clock = MonotonicClock::new();

    let run = run(&mut session, &plan(), Box::new(sink.clone()), &clock)
        .expect("a camera failure is reported, not returned as an error");

    assert!(
        matches!(run.end, StreamEnd::DisconnectedCamera { .. }),
        "end was {:?}",
        run.end
    );
    assert!(run.end.is_failure());
    assert!(
        run.end.to_string().contains("disconnected"),
        "message was {}",
        run.end
    );
    assert!(
        run.end.to_string().contains("absent from the USB bus"),
        "message was {}",
        run.end
    );
    assert!(
        run.end.remedy().unwrap_or_default().contains("reconnect"),
        "the remedy must say to reconnect"
    );
    assert!(sink.log().closes >= 1, "every exit path closes the sink");
}

#[test]
fn a_camera_that_times_out_is_reported_as_wedged_with_its_own_remedy() {
    let mut session = fast_session().failing_after(30, SyntheticSession::wedged());
    let clock = MonotonicClock::new();

    let run = run(
        &mut session,
        &plan(),
        Box::new(StatsSink::new(Arc::new(
            camera_core::stats::StreamStats::new(),
        ))),
        &clock,
    )
    .expect("a camera failure is reported, not returned as an error");

    assert!(
        matches!(run.end, StreamEnd::WedgedCamera { .. }),
        "end was {:?}",
        run.end
    );
    let message = run.end.to_string();
    assert!(message.contains("wedged"), "message was {message}");
    assert!(
        message.contains("on the USB bus"),
        "a wedged camera is still detected, message was {message}"
    );
    let remedy = run.end.remedy().unwrap_or_default();
    assert!(remedy.contains("gphoto2 --reset"), "remedy was {remedy}");
}

#[test]
fn the_two_camera_states_are_never_reported_as_one() {
    let wedged = classify_session_failure(&SyntheticSession::wedged());
    let disconnected = classify_session_failure(&SyntheticSession::disconnected());

    assert_ne!(wedged, disconnected);
    assert_ne!(
        wedged.to_string(),
        disconnected.to_string(),
        "the states need distinct messages because the remedies differ"
    );
    assert_ne!(wedged.remedy(), disconnected.remedy());
}

#[test]
fn a_second_ptp_session_is_a_programming_error_not_a_camera_state() {
    let end = classify_session_failure(&SessionError::AlreadyOpen);

    assert!(
        matches!(end, StreamEnd::CaptureFailed { .. }),
        "end was {end:?}"
    );
    assert_eq!(end.remedy(), None);
}

#[test]
fn a_sink_that_cannot_open_ends_the_run_with_its_own_explanation() {
    let mut session = fast_session();
    let clock = MonotonicClock::new();

    let error = run(
        &mut session,
        &plan(),
        Box::new(LoggedSink::failing()),
        &clock,
    )
    .expect_err("a sink that cannot open must be an error, not a run");

    assert!(matches!(error, StreamError::Sink(_)), "error was {error:?}");
    assert!(
        error.to_string().contains("/Applications"),
        "the backend's own explanation must survive, message was {error}"
    );
}

#[test]
fn the_synthetic_source_only_produces_the_two_frame_sizes_anyone_measured() {
    assert!(live_view_jpeg(FULL_FRAME_SIZE).is_some());
    assert!(live_view_jpeg(REDUCED_FRAME_SIZE).is_some());
    assert_eq!(
        live_view_jpeg(FrameSize::new(1920, 1080)),
        None,
        "no frame at 1080p has ever been measured, so none may be invented"
    );
}

#[test]
#[should_panic(expected = "no measured live view frame exists")]
fn asking_the_synthetic_source_for_an_unmeasured_size_is_loud() {
    let _ = SyntheticSession::new(FrameSize::new(1280, 720));
}
