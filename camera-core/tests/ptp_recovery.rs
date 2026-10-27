//! Detection of a **wedged camera** and recovery from it, exercised rather than
//! argued.
//!
//! No M50 is attached, so the wedged state is produced deliberately instead: the
//! synthetic **PTP session** stops answering with `PTP Timeout` after N frames,
//! which is the observed state of `TEST-RESULTS.md` §4.8, and the **recovery
//! reset** is a fake that records every call. What that proves:
//!
//! - the wedged condition is detected and reported rather than hanging;
//! - the reset runs a bounded number of times, then a full session reopen, and
//!   the stream resumes in the same process;
//! - exhaustion ends the run with a cable reconnect instruction;
//! - the budget is a bound for the whole run, so a permanently wedged camera
//!   cannot loop;
//! - recovery writes no **output setting** of its own, so it cannot re-wedge the
//!   camera with a dangerous `output` value.
//!
//! What it cannot prove: that an M50's `gphoto2 --reset` clears a real wedged
//! camera. That was measured by hand (`TEST-RESULTS.md` §4.8) and is recorded as
//! unverified in the ticket.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camera_core::calibration::CalibrationPolicy;
use camera_core::frame_size::FrameSize;
use camera_core::measurement::MonotonicClock;
use camera_core::output_setting::OutputSetting;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::recovery::{
    CommandRecoveryReset, PtpSessionOpener, Recovery, RecoveryReset, RecoveryResetError,
    MAX_RECOVERY_RESET_ATTEMPTS,
};
use camera_core::runner::{run_recovering, StreamEnd, StreamPlan};
use camera_core::session::{PtpSession, SessionError};
use camera_core::sink::{FrameSink, SinkError};
use camera_core::synthetic::{
    ScriptedSessionOpener, SharedLog, SyntheticSession, REDUCED_FRAME_SIZE,
};

/// A **recovery reset** that records its calls and answers from a script.
struct FakeReset {
    calls: Arc<AtomicUsize>,
    /// What each successive call returns; the last entry repeats for ever.
    outcomes: Vec<Result<(), RecoveryResetError>>,
    next: usize,
}

impl FakeReset {
    /// Returns the fake and a handle that counts its calls.
    fn new(outcomes: Vec<Result<(), RecoveryResetError>>) -> (Self, Arc<AtomicUsize>) {
        assert!(!outcomes.is_empty());
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                calls: Arc::clone(&calls),
                outcomes,
                next: 0,
            },
            calls,
        )
    }

    fn succeeding() -> (Self, Arc<AtomicUsize>) {
        Self::new(vec![Ok(())])
    }

    fn failing() -> (Self, Arc<AtomicUsize>) {
        Self::new(vec![Err(RecoveryResetError::new(
            "`gphoto2 --reset` exited with exit status: 1",
        ))])
    }
}

impl RecoveryReset for FakeReset {
    fn reset(&mut self) -> Result<(), RecoveryResetError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let outcome = self.outcomes[self.next.min(self.outcomes.len() - 1)].clone();
        self.next += 1;
        outcome
    }

    fn command_line(&self) -> String {
        "gphoto2 --reset".to_owned()
    }
}

/// A sink that records the frame size of every open, across every segment.
#[derive(Clone, Default)]
struct RecordingSink {
    opens: Arc<Mutex<Vec<FrameSize>>>,
    opened_at: Option<FrameSize>,
}

impl RecordingSink {
    fn opens(&self) -> Vec<FrameSize> {
        self.opens
            .lock()
            .expect("the sink log must not be poisoned")
            .clone()
    }
}

impl FrameSink for RecordingSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        _pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        self.opens
            .lock()
            .expect("the sink log must not be poisoned")
            .push(expected_frame_size);
        self.opened_at = Some(expected_frame_size);
        Ok(())
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        match self.opened_at {
            Some(expected) if expected == frame.frame_size() => Ok(()),
            _ => Err(SinkError::FrameSizeMismatch {
                expected: self.opened_at.unwrap_or(frame.frame_size()),
                actual: frame.frame_size(),
            }),
        }
    }

    fn close(&mut self) -> Result<(), SinkError> {
        self.opened_at = None;
        Ok(())
    }
}

fn plan(run_for: Duration) -> StreamPlan {
    StreamPlan::new(
        OutputSetting::tft_plus_pc(),
        camera_core::stream::StreamPipelineConfig::new(
            CalibrationPolicy::new(8, Duration::from_secs(30), 2).expect("valid policy"),
        ),
    )
    .with_run_for(run_for)
}

fn session(log: SharedLog) -> SyntheticSession {
    SyntheticSession::new(REDUCED_FRAME_SIZE)
        .with_shared_log(log)
        .with_frame_period(Duration::from_millis(2))
}

/// An opener that hands out a wedged session for ever: every reset "succeeds" and
/// the camera wedges again, which is a permanently wedged camera.
struct EndlesslyWedgedOpener {
    opened: usize,
}

impl PtpSessionOpener for EndlesslyWedgedOpener {
    fn open_session(&mut self) -> Result<Box<dyn PtpSession + Send>, SessionError> {
        self.opened += 1;
        Ok(Box::new(
            SyntheticSession::new(REDUCED_FRAME_SIZE)
                .with_frame_period(Duration::from_millis(1))
                .failing_after(3, SyntheticSession::wedged()),
        ))
    }
}

#[test]
fn the_recovery_reset_is_gphoto2_dash_dash_reset() {
    let reset = CommandRecoveryReset::new();

    assert_eq!(reset.command_line(), "gphoto2 --reset");
    assert_eq!(
        reset.invocation(),
        ("gphoto2", ["--reset".to_owned()].as_slice())
    );
}

#[test]
fn a_reset_that_exits_zero_is_a_successful_recovery_reset() {
    let mut reset = CommandRecoveryReset::with_program("true");

    assert_eq!(reset.reset(), Ok(()));
}

#[test]
fn a_reset_that_exits_non_zero_reports_its_command_and_status() {
    let mut reset = CommandRecoveryReset::with_program("false");

    let error = reset.reset().expect_err("a non-zero exit is a failure");
    let message = error.to_string();
    assert!(message.contains("`false --reset`"), "message was {message}");
    assert!(message.contains("exited with"), "message was {message}");
}

#[test]
fn a_reset_that_cannot_start_names_the_command_it_could_not_run() {
    let mut reset = CommandRecoveryReset::with_program("gphoto2-not-installed-here");

    let error = reset.reset().expect_err("a missing program is a failure");
    assert!(
        error
            .to_string()
            .contains("could not run `gphoto2-not-installed-here --reset`"),
        "message was {error}"
    );
}

#[test]
fn a_wedged_camera_is_recovered_and_the_stream_resumes_in_the_same_process() {
    let log: SharedLog = SharedLog::default();
    let opener = ScriptedSessionOpener::new([
        session(Arc::clone(&log)).failing_after(30, SyntheticSession::wedged()),
        session(Arc::clone(&log)),
    ]);
    let mut opener = opener;
    let (reset, resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset));
    let sink = RecordingSink::default();
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_millis(400)),
        &mut || Box::new(sink.clone()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(
        run.end,
        StreamEnd::DurationElapsed,
        "a recovered run continues to its planned duration"
    );
    assert_eq!(
        run.sessions_opened, 2,
        "the wedged session is replaced, not doubled: one session at a time"
    );
    assert_eq!(opener.opened(), 2);
    assert_eq!(resets.load(Ordering::SeqCst), 1, "one reset cleared it");
    assert_eq!(run.recovery.attempts_used(), 1);
    assert_eq!(run.recovery.recoveries(), 1);
    assert_eq!(run.recovery.episodes(), 1);
    assert!(
        run.stats.frames_delivered > 0,
        "the feed must continue after the recovery"
    );
    assert_eq!(
        sink.opens(),
        vec![REDUCED_FRAME_SIZE, REDUCED_FRAME_SIZE],
        "the sink is closed with the wedged session and re-opened with the new one"
    );
}

#[test]
fn the_wedged_state_is_reported_as_wedged_before_any_recovery_is_attempted() {
    let log: SharedLog = SharedLog::default();
    let mut opener =
        ScriptedSessionOpener::new([session(log).failing_after(6, SyntheticSession::wedged())]);
    let (reset, resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset));
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_millis(400)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    // The scripted opener runs out of sessions, so the reset "succeeds" but no
    // session can be reopened until the run's budget is gone.
    assert!(
        matches!(run.end, StreamEnd::RecoveryFailed { .. }),
        "end was {:?}",
        run.end
    );
    assert_eq!(
        resets.load(Ordering::SeqCst),
        MAX_RECOVERY_RESET_ATTEMPTS,
        "every allowed reset ran"
    );
    let message = run.end.to_string();
    assert!(
        message.contains("wedged"),
        "the report must still name the state, message was {message}"
    );
}

#[test]
fn the_reset_budget_is_bounded_and_the_bound_is_what_runs() {
    let mut opener = EndlesslyWedgedOpener { opened: 0 };
    let (reset, resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset));
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(
        resets.load(Ordering::SeqCst),
        MAX_RECOVERY_RESET_ATTEMPTS,
        "a permanently wedged camera gets exactly the budget, no more"
    );
    assert_eq!(run.recovery.attempts_used(), MAX_RECOVERY_RESET_ATTEMPTS);
    assert_eq!(
        run.sessions_opened,
        MAX_RECOVERY_RESET_ATTEMPTS + 1,
        "the run opened its first session plus one per reset, and then stopped"
    );
    let StreamEnd::RecoveryFailed { attempts, .. } = run.end else {
        panic!(
            "a permanently wedged camera must end the run, end was {:?}",
            run.end
        );
    };
    assert_eq!(attempts, MAX_RECOVERY_RESET_ATTEMPTS);
    assert!(
        run.end.is_failure(),
        "exhaustion is a failure, not a successful run"
    );
}

#[test]
fn a_budget_of_one_attempts_exactly_one_reset() {
    let mut opener = EndlesslyWedgedOpener { opened: 0 };
    let (reset, resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset)).with_max_attempts(1);
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(resets.load(Ordering::SeqCst), 1);
    assert_eq!(run.recovery.attempts_used(), 1);
}

#[test]
fn a_reset_that_fails_every_time_ends_the_run_after_the_bound() {
    let log: SharedLog = SharedLog::default();
    let mut opener = ScriptedSessionOpener::new([
        session(Arc::clone(&log)).failing_after(4, SyntheticSession::wedged()),
        session(Arc::clone(&log)),
    ]);
    let (reset, resets) = FakeReset::failing();
    let mut recovery = Recovery::new(Box::new(reset));
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(resets.load(Ordering::SeqCst), MAX_RECOVERY_RESET_ATTEMPTS);
    assert_eq!(
        run.sessions_opened, 1,
        "a reset that never succeeds reopens nothing"
    );
    assert!(
        run.recovery.attempts().iter().all(|attempt| matches!(
            attempt.outcome,
            camera_core::recovery::ResetAttemptOutcome::ResetFailed { .. }
        )),
        "every attempt must say the reset command failed: {:?}",
        run.recovery.attempts()
    );
    assert!(matches!(run.end, StreamEnd::RecoveryFailed { .. }));
}

#[test]
fn a_reset_that_succeeds_but_reopens_nothing_is_counted_as_a_failed_attempt() {
    let log: SharedLog = SharedLog::default();
    let mut opener =
        ScriptedSessionOpener::new([session(log).failing_after(4, SyntheticSession::wedged())]);
    let (reset, resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset)).with_max_attempts(2);
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(resets.load(Ordering::SeqCst), 2);
    assert!(run.recovery.attempts().iter().all(|attempt| matches!(
        attempt.outcome,
        camera_core::recovery::ResetAttemptOutcome::ReopenFailed { .. }
    )));
    assert!(matches!(run.end, StreamEnd::RecoveryFailed { .. }));
}

#[test]
fn recovery_stops_at_the_first_reset_that_works() {
    let log: SharedLog = SharedLog::default();
    let mut opener = ScriptedSessionOpener::new([
        session(Arc::clone(&log)).failing_after(4, SyntheticSession::wedged()),
        session(Arc::clone(&log)),
    ]);
    let (reset, resets) = FakeReset::new(vec![
        Err(RecoveryResetError::new("the first reset did not help")),
        Err(RecoveryResetError::new("the second reset did not help")),
        Ok(()),
    ]);
    let mut recovery = Recovery::new(Box::new(reset));
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_millis(300)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(run.end, StreamEnd::DurationElapsed);
    assert_eq!(
        resets.load(Ordering::SeqCst),
        3,
        "no reset after the one that worked"
    );
    assert_eq!(run.recovery.recoveries(), 1);
    let last = run.recovery.last().expect("there were attempts");
    assert_eq!(
        last.outcome,
        camera_core::recovery::ResetAttemptOutcome::Recovered
    );
    assert_eq!(last.attempt, 3);
}

#[test]
fn a_disconnected_camera_is_never_retried_because_a_reset_cannot_help() {
    let mut opener = ScriptedSessionOpener::new(Vec::new());
    let (reset, resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset));
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_millis(100)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    assert!(
        matches!(run.end, StreamEnd::DisconnectedCamera { .. }),
        "end was {:?}",
        run.end
    );
    assert_eq!(
        resets.load(Ordering::SeqCst),
        0,
        "a camera that is not on the bus must not get a reset"
    );
    assert_eq!(run.recovery.attempts_used(), 0);
    assert_eq!(run.sessions_opened, 0);
}

#[test]
fn the_recovery_failure_remedy_is_remove_and_reconnect_the_usb_cable() {
    let failed = StreamEnd::RecoveryFailed {
        attempts: 3,
        detail: "PTP Timeout".to_owned(),
    };

    let remedy = failed.remedy().expect("a failed run has a remedy");
    assert!(
        remedy.contains("remove and reconnect the USB cable"),
        "remedy was {remedy}"
    );
    assert!(
        failed.to_string().contains("3 recovery resets"),
        "the message must say how many resets ran: {failed}"
    );
    assert_ne!(
        failed.to_string(),
        attempts_note().to_string(),
        "recovery exhaustion is not the same end as a fresh wedge"
    );
}

#[test]
fn the_output_setting_recovery_re_applies_is_never_a_dangerous_value() {
    let log: SharedLog = SharedLog::default();
    let mut opener = ScriptedSessionOpener::new([
        session(Arc::clone(&log)).failing_after(6, SyntheticSession::wedged()),
        session(Arc::clone(&log)),
    ]);
    let (reset, _resets) = FakeReset::succeeding();
    let mut recovery = Recovery::new(Box::new(reset));
    let clock = MonotonicClock::new();

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_millis(300)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    let written: Vec<String> = log
        .lock()
        .expect("the session log must not be poisoned")
        .iter()
        .filter(|entry| entry.starts_with("set-output:"))
        .cloned()
        .collect();

    assert!(
        !written.is_empty(),
        "the reopened session must re-apply the output setting"
    );
    assert!(
        written.iter().all(|entry| entry == "set-output:3"),
        "only the allowlisted full-size value may be written, recovery wrote {written:?}"
    );
    assert_eq!(
        written.len(),
        run.sessions_opened,
        "the setting is written once per session and never by recovery itself"
    );
}

#[test]
fn the_log_says_how_many_reset_attempts_ran_and_how_they_ended() {
    let log: SharedLog = SharedLog::default();
    let mut opener = ScriptedSessionOpener::new([
        session(Arc::clone(&log)).failing_after(4, SyntheticSession::wedged()),
        session(Arc::clone(&log)),
    ]);
    let (reset, _resets) = FakeReset::failing();
    let mut recovery = Recovery::new(Box::new(reset)).with_max_attempts(2);
    let clock = MonotonicClock::new();

    let _ = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5)),
        &mut || Box::new(RecordingSink::default()),
        &clock,
        &mut recovery,
    )
    .expect("the run must complete");

    let rendered = run_log(&recovery);
    assert!(
        rendered.contains("recovery resets: 2 attempts"),
        "the count must be in the log: {rendered}"
    );
    assert!(
        rendered.contains("recovery reset attempt 1 of episode 1"),
        "each attempt must be numbered: {rendered}"
    );
    assert!(
        rendered.contains("recovery reset attempt 2 of episode 1"),
        "each attempt must be numbered: {rendered}"
    );
    assert!(
        rendered.contains("the reset command failed"),
        "the outcome of each attempt must be in the log: {rendered}"
    );
}

/// A `StreamEnd::WedgedCamera` to compare against, so the two messages are
/// visibly distinct.
fn attempts_note() -> StreamEnd {
    StreamEnd::WedgedCamera {
        detail: "PTP Timeout".to_owned(),
    }
}

fn run_log(recovery: &Recovery) -> String {
    recovery.log().to_string()
}
