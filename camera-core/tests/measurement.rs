//! Measurement learns the frame size and the frame rate from frame data.
//!
//! The seam is `measure(&mut dyn PtpSession, &MeasurementPlan, &dyn Clock)`.
//! Frame bytes come from a fake session and time comes from a scripted clock, so
//! no test needs a camera and none waits.

mod support;

use std::time::Duration;

use camera_core::calibration::{CalibrationError, CalibrationPolicy};
use camera_core::frame_size::FrameSize;
use camera_core::jpeg::JpegError;
use camera_core::measurement::{measure, MeasureError, MeasurementPlan, MeasurementPlanError};
use camera_core::output_setting::OutputSetting;
use camera_core::session::SessionError;

use support::{frame_of_size, FakeSession, ScriptedClock};

const FULL: FrameSize = FrameSize::new(1024, 576);
const REDUCED: FrameSize = FrameSize::new(512, 288);

fn plan(window_frames: usize, min_recurrences: usize, sampling_frames: usize) -> MeasurementPlan {
    MeasurementPlan::new(
        OutputSetting::tft_plus_pc(),
        CalibrationPolicy::new(window_frames, Duration::from_secs(3), min_recurrences)
            .expect("test policy must be valid"),
        sampling_frames,
    )
    .expect("test plan must be valid")
}

/// Frames captured before the sampling window ends: the calibration window plus
/// the sampling frames, minus the establishment frame that both count.
fn frame_budget(window_frames: usize, sampling_frames: usize) -> usize {
    window_frames + sampling_frames - 1
}

#[test]
fn the_measured_frame_size_and_rate_come_from_the_frames() {
    let session_frames = std::iter::repeat_n(frame_of_size(REDUCED), 5)
        .chain(std::iter::repeat_n(frame_of_size(FULL), 60))
        .collect();
    let mut session = FakeSession::with_frames(session_frames);
    let clock = ScriptedClock::stepping(200, Duration::from_millis(40));

    let measurement = measure(&mut session, &plan(20, 3, 20), &clock).expect("measurement");

    assert_eq!(measurement.measured_frame_size, FULL);
    assert_eq!(measurement.leading_frames, 5);
    assert_eq!(measurement.unexpected_frames, 0);
    assert_eq!(
        measurement.measured_frame_rate, 25.0,
        "20 sampling frames span 19 intervals of 40 ms"
    );
    assert_eq!(
        measurement.frames_captured,
        frame_budget(20, 20),
        "every capture is counted, including the calibration window"
    );
    // The clock is scripted, so the last read is the total elapsed time.
    assert_eq!(measurement.elapsed, Duration::from_millis(40 * 39));
}

#[test]
fn a_steady_first_frame_reports_no_leading_frames() {
    let frames = std::iter::repeat_n(frame_of_size(FULL), 20).collect();
    let mut session = FakeSession::with_frames(frames);
    let clock = ScriptedClock::stepping(20, Duration::from_millis(40));

    let measurement = measure(&mut session, &plan(5, 3, 5), &clock).expect("measurement");

    assert_eq!(measurement.measured_frame_size, FULL);
    assert_eq!(measurement.leading_frames, 0);
    assert_eq!(measurement.measured_frame_rate, 25.0);
}

#[test]
fn the_output_setting_and_the_viewfinder_action_precede_every_capture() {
    let mut session = FakeSession::with_frames(vec![
        frame_of_size(FULL),
        frame_of_size(FULL),
        frame_of_size(FULL),
        frame_of_size(FULL),
    ]);
    let clock = ScriptedClock::stepping(10, Duration::from_millis(40));

    let _ = measure(&mut session, &plan(2, 2, 3), &clock).expect("measurement");

    let log = session.log();
    assert_eq!(log[0], "set-output:3", "the output setting comes first");
    assert_eq!(
        log[1], "viewfinder",
        "the viewfinder action must be pressed in the same session as the capture"
    );
    assert!(
        log[2..].iter().all(|entry| entry == "capture"),
        "nothing may be re-applied between captures, log was {log:?}"
    );
}

#[test]
fn the_viewfinder_action_is_applied_on_every_run() {
    let make_session = || {
        FakeSession::with_frames(vec![
            frame_of_size(FULL),
            frame_of_size(FULL),
            frame_of_size(FULL),
            frame_of_size(FULL),
            frame_of_size(FULL),
        ])
    };

    let mut first = make_session();
    let mut second = make_session();
    let clock = ScriptedClock::stepping(20, Duration::from_millis(40));
    let clock_two = ScriptedClock::stepping(20, Duration::from_millis(40));

    let _ = measure(&mut first, &plan(2, 2, 3), &clock).expect("first run");
    let _ = measure(&mut second, &plan(2, 2, 3), &clock_two).expect("second run");

    assert_eq!(
        first
            .log()
            .iter()
            .filter(|entry| *entry == "viewfinder")
            .count(),
        1
    );
    assert_eq!(
        second
            .log()
            .iter()
            .filter(|entry| *entry == "viewfinder")
            .count(),
        1,
        "the action does not persist between runs, so it must be applied again"
    );
}

#[test]
fn the_plan_chooses_which_allowlisted_output_value_is_set() {
    let mut session = FakeSession::with_frames(vec![
        frame_of_size(FULL),
        frame_of_size(FULL),
        frame_of_size(FULL),
    ]);
    let clock = ScriptedClock::stepping(10, Duration::from_millis(40));
    let plan = MeasurementPlan::new(
        OutputSetting::from_raw(1).expect("allowlisted"),
        CalibrationPolicy::new(2, Duration::from_secs(3), 2).expect("valid"),
        2,
    )
    .expect("valid plan");

    let _ = measure(&mut session, &plan, &clock).expect("measurement");

    assert_eq!(session.log()[0], "set-output:1");
}

#[test]
fn a_window_without_a_recurring_size_fails_loudly() {
    let frames = (0..10)
        .map(|frame| frame_of_size(FrameSize::new(100 + frame, 100)))
        .collect();
    let mut session = FakeSession::with_frames(frames);
    let clock = ScriptedClock::stepping(40, Duration::from_millis(40));

    let error = measure(&mut session, &plan(10, 3, 5), &clock)
        .expect_err("a window with no recurring size must fail");

    assert!(
        matches!(
            error,
            MeasureError::Calibration(CalibrationError::NoRecurringFrameSize { .. })
        ),
        "error was {error:?}"
    );
}

#[test]
fn a_capture_whose_expected_size_never_returns_fails_loudly() {
    let frames = vec![
        frame_of_size(FULL),
        frame_of_size(FULL),
        frame_of_size(REDUCED),
        frame_of_size(REDUCED),
        frame_of_size(REDUCED),
        frame_of_size(REDUCED),
    ];
    let mut session = FakeSession::with_frames(frames);
    let clock = ScriptedClock::stepping(40, Duration::from_millis(40));

    let error = measure(&mut session, &plan(3, 2, 4), &clock)
        .expect_err("a camera that stops producing the expected size must fail");

    assert_eq!(
        error,
        MeasureError::ExpectedFrameSizeNotRecurring { expected: FULL }
    );
}

#[test]
fn a_frame_that_is_not_a_jpeg_fails_loudly() {
    let mut session = FakeSession::new("Canon EOS M50", vec![Ok(b"not a frame".to_vec())]);
    let clock = ScriptedClock::stepping(10, Duration::from_millis(40));

    let error = measure(&mut session, &plan(2, 2, 2), &clock).expect_err("must fail");

    assert_eq!(error, MeasureError::Frame(JpegError::NotJpeg));
}

#[test]
fn a_rejected_output_setting_stops_before_any_capture() {
    let mut session = FakeSession::with_frames(vec![frame_of_size(FULL)])
        .failing_output_setting(SessionError::AlreadyOpen);
    let clock = ScriptedClock::stepping(10, Duration::from_millis(40));

    let error = measure(&mut session, &plan(2, 2, 2), &clock).expect_err("must fail");

    assert_eq!(error, MeasureError::Session(SessionError::AlreadyOpen));
    assert_eq!(
        session.log(),
        &vec!["set-output:3".to_owned()],
        "a rejected setting must not be followed by a capture"
    );
}

#[test]
fn a_rejected_viewfinder_action_stops_before_any_capture() {
    let mut session = FakeSession::with_frames(vec![frame_of_size(FULL)]).failing_viewfinder(
        SessionError::Camera {
            operation: camera_core::session::SessionOperation::ActivateViewfinderAction,
            detail: "not supported".to_owned(),
        },
    );
    let clock = ScriptedClock::stepping(10, Duration::from_millis(40));

    let error = measure(&mut session, &plan(2, 2, 2), &clock).expect_err("must fail");

    assert!(
        matches!(error, MeasureError::Session(_)),
        "error was {error:?}"
    );
}

#[test]
fn a_clock_that_does_not_advance_cannot_produce_a_rate() {
    let frames = std::iter::repeat_n(frame_of_size(FULL), 10).collect();
    let mut session = FakeSession::with_frames(frames);
    let clock = ScriptedClock::from_milliseconds([40, 40, 40, 40, 40, 40]);

    let error = measure(&mut session, &plan(2, 2, 3), &clock).expect_err("must fail");

    assert_eq!(error, MeasureError::NoElapsedTime);
}

#[test]
fn a_plan_with_a_single_sampling_frame_is_rejected() {
    let error = MeasurementPlan::new(
        OutputSetting::tft_plus_pc(),
        CalibrationPolicy::default(),
        1,
    )
    .expect_err("one sampling frame contains no interval");

    assert_eq!(
        error,
        MeasurementPlanError::SamplingFramesTooSmall { sampling_frames: 1 }
    );
}

#[test]
fn the_default_plan_reproduces_the_verified_acquisition_settings() {
    let plan = MeasurementPlan::default();

    assert_eq!(plan.output_setting(), OutputSetting::tft_plus_pc());
    assert!(plan.calibration().min_recurrences() >= 6);
    assert!(plan.sampling_frames() >= 2);
}
