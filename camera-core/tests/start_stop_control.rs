//! The start/stop control surface, driven through its controller.
//!
//! The camera is `SyntheticSession` behind a counting opener, so "a stop releases
//! the **PTP session**" is a number going to zero and "a start takes it again" is
//! a number going to one, not an inference from behaviour. The **sink** is a
//! recording sink, so "a stop closes the sink" is a recorded close rather than a
//! closed-looking object.
//!
//! What these tests prove is the state machine: what a start and a stop do to the
//! session and the sink, that starting again re-applies the **output setting** and
//! the **viewfinder action** with a fresh **calibration window**, that the
//! running/stopped state can be read without side effects, and that a stop is
//! bounded while frames are still arriving. What they cannot prove, because no M50
//! and no OBS are attached, is that an M50 releases and re-takes its session this
//! way: that is recorded in the ticket.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camera_core::calibration::CalibrationPolicy;
use camera_core::control::{
    FeedChange, FeedController, FeedError, FeedFactory, FeedRun, FeedState,
};
use camera_core::output_setting::OutputSetting;
use camera_core::recovery::{Recovery, RecoveryReset, RecoveryResetError};
use camera_core::runner::{StreamEnd, StreamPlan};
use camera_core::stream::StreamPipelineConfig;
use camera_core::synthetic::{SharedLog, FULL_FRAME_SIZE, REDUCED_FRAME_SIZE};
use support::control::{wait_until, CountingOpener, RecordingSink, SessionCounts, SinkLog};

/// The **calibration window** every run here uses.
///
/// Twenty frames and eight recurrences settles well inside a hundred frames, and
/// still needs more full-size frames than the five **leading frames** these
/// sessions emit, so a leading frame never becomes the **expected frame size**.
fn pipeline_config() -> StreamPipelineConfig {
    StreamPipelineConfig::new(
        CalibrationPolicy::new(20, Duration::from_secs(30), 8).expect("valid policy"),
    )
}

fn plan() -> StreamPlan {
    StreamPlan::new(OutputSetting::tft_plus_pc(), pipeline_config())
}

/// The **recovery reset** a synthetic feed must never need.
struct NoRecoveryReset;

impl RecoveryReset for NoRecoveryReset {
    fn reset(&mut self) -> Result<(), RecoveryResetError> {
        panic!("a synthetic feed must never be asked for a recovery reset")
    }

    fn command_line(&self) -> String {
        "none (a synthetic feed has no wedged camera to clear)".to_owned()
    }
}

/// One feed's worth of recording pieces, and the controller over them.
struct Feeder {
    controller: FeedController,
    counts: SessionCounts,
    sessions: SharedLog,
    sink_log: Arc<Mutex<SinkLog>>,
}

impl Feeder {
    fn new(leading_frames: usize, frame_rate: f64) -> Self {
        let counts = SessionCounts::new();
        let sessions = SharedLog::default();
        let sink_log = Arc::new(Mutex::new(SinkLog::default()));

        let opener = CountingOpener::new(counts.clone(), Arc::clone(&sessions))
            .with_leading_frames(leading_frames)
            .at_rate(frame_rate);
        let sink_factory = Arc::clone(&sink_log);
        let factory: FeedFactory = Arc::new(move || {
            FeedRun::new(
                Box::new(opener.clone()),
                RecordingSink::factory(Arc::clone(&sink_factory)),
                Recovery::new(Box::new(NoRecoveryReset)),
            )
        });

        Self {
            controller: FeedController::new(plan(), factory),
            counts,
            sessions,
            sink_log,
        }
    }

    /// Waits for `more` frames beyond the ones already delivered.
    fn wait_for_frames(&self, more: usize) {
        let already = RecordingSink::read(&self.sink_log).accepted.len();
        wait_until(Duration::from_secs(5), "frames to reach the sink", || {
            RecordingSink::read(&self.sink_log).accepted.len() >= already + more
        });
    }

    fn sink_opens(&self) -> usize {
        RecordingSink::read(&self.sink_log).opens.len()
    }

    fn sink_closes(&self) -> usize {
        RecordingSink::read(&self.sink_log).closes
    }

    fn open_sinks(&self) -> usize {
        RecordingSink::read(&self.sink_log).open_sinks
    }

    /// The session operation log split at every start, one entry per session.
    fn per_session_log(&self) -> Vec<Vec<String>> {
        let log = self
            .sessions
            .lock()
            .expect("the synthetic session log must not be poisoned");
        let mut sessions: Vec<Vec<String>> = Vec::new();
        for entry in log.iter() {
            if entry.starts_with("set-output:") {
                sessions.push(Vec::new());
            }
            if let Some(current) = sessions.last_mut() {
                current.push(entry.clone());
            }
        }
        sessions
    }
}

#[test]
fn a_start_stop_start_cycle_releases_and_retakes_the_session_and_the_sink() {
    let mut feeder = Feeder::new(0, 250.0);

    assert_eq!(feeder.controller.state(), FeedState::Stopped);

    // First start: one PTP session, one sink.
    assert_eq!(
        feeder
            .controller
            .start()
            .expect("the first start must work"),
        FeedChange::Started
    );
    feeder.wait_for_frames(3);
    assert_eq!(feeder.counts.live(), 1, "a start takes exactly one session");
    assert_eq!(feeder.counts.opened(), 1, "and opens it once");
    assert_eq!(feeder.sink_opens(), 1, "and opens one sink");

    // Stop: both released before the stop is acknowledged.
    assert_eq!(
        feeder.controller.stop().expect("the stop must work"),
        FeedChange::Stopped
    );
    assert_eq!(
        feeder.counts.live(),
        0,
        "a stop must release the PTP session so another application can take the camera"
    );
    assert_eq!(
        feeder.sink_closes(),
        feeder.sink_opens(),
        "every sink that was opened was closed by the stop"
    );
    assert_eq!(feeder.open_sinks(), 0, "and none is left open");
    assert_eq!(feeder.controller.state(), FeedState::Stopped);

    // Second start: a new session and a new sink, not the closed ones.
    assert_eq!(
        feeder
            .controller
            .start()
            .expect("the second start must work"),
        FeedChange::Started
    );
    feeder.wait_for_frames(3);
    assert_eq!(
        feeder.counts.opened(),
        2,
        "the second start opened a session"
    );
    assert_eq!(feeder.counts.live(), 1, "and exactly one is alive");
    assert_eq!(feeder.sink_opens(), 2, "and the sink was opened again");

    assert_eq!(
        feeder.controller.stop().expect("the second stop must work"),
        FeedChange::Stopped
    );
    assert_eq!(feeder.counts.live(), 0);
    assert_eq!(feeder.sink_closes(), 2);
    assert_eq!(feeder.open_sinks(), 0);
}

#[test]
fn starting_again_re_applies_the_output_setting_and_the_viewfinder_action() {
    let mut feeder = Feeder::new(5, 250.0);

    feeder
        .controller
        .start()
        .expect("the first start must work");
    feeder.wait_for_frames(3);
    feeder.controller.stop().expect("the stop must work");

    feeder
        .controller
        .start()
        .expect("the second start must work");
    feeder.wait_for_frames(3);
    feeder.controller.stop().expect("the second stop must work");

    let sessions = feeder.per_session_log();
    assert_eq!(sessions.len(), 2, "one session per start");

    for (index, session) in sessions.iter().enumerate() {
        assert_eq!(
            session[0],
            "set-output:3",
            "session {} must re-apply the output setting before capturing: {session:?}",
            index + 1
        );
        assert_eq!(
            session[1],
            "viewfinder",
            "session {} must re-apply the viewfinder action, which does not persist: {session:?}",
            index + 1
        );
        assert!(
            session[2..].iter().all(|entry| entry == "capture"),
            "session {} must not re-apply anything in the middle of a run: {session:?}",
            index + 1
        );
    }

    // The leading frames of both runs went to a calibration window, not to the
    // sink: the expected frame size is established again on every start.
    let accepted = RecordingSink::read(&feeder.sink_log).accepted.clone();
    assert!(!accepted.is_empty());
    assert!(
        accepted.iter().all(|size| *size == FULL_FRAME_SIZE),
        "a leading {REDUCED_FRAME_SIZE} frame reached the sink: {accepted:?}"
    );
}

#[test]
fn starting_when_running_and_stopping_when_stopped_are_harmless() {
    let mut feeder = Feeder::new(0, 250.0);

    assert_eq!(
        feeder
            .controller
            .stop()
            .expect("stopping nothing must work"),
        FeedChange::AlreadyStopped
    );
    assert_eq!(
        feeder
            .controller
            .stop()
            .expect("stopping nothing twice must work"),
        FeedChange::AlreadyStopped
    );
    assert_eq!(feeder.counts.opened(), 0, "no session was taken");
    assert_eq!(feeder.sink_opens(), 0, "no sink was opened");

    assert_eq!(
        feeder.controller.start().expect("the start must work"),
        FeedChange::Started
    );
    feeder.wait_for_frames(3);

    assert_eq!(
        feeder
            .controller
            .start()
            .expect("a second start must be harmless"),
        FeedChange::AlreadyRunning
    );
    assert_eq!(
        feeder.counts.opened(),
        1,
        "a start while running must not take a second PTP session"
    );
    assert_eq!(feeder.counts.live(), 1);
    assert_eq!(
        feeder.sink_opens(),
        1,
        "nor must it open a second sink over the open one"
    );
    assert_eq!(feeder.controller.state(), FeedState::Running);

    assert_eq!(
        feeder.controller.stop().expect("the stop must work"),
        FeedChange::Stopped
    );
    assert_eq!(feeder.counts.live(), 0);
}

#[test]
fn stopping_while_frames_are_arriving_is_bounded() {
    // A feed producing frames as fast as the pipeline will take them, so the
    // stop cannot be quick by luck: the capture thread is always mid-frame.
    let mut feeder = Feeder::new(0, 500.0);
    feeder.controller.start().expect("the start must work");
    feeder.wait_for_frames(10);

    let began = Instant::now();
    assert_eq!(
        feeder.controller.stop().expect("the stop must work"),
        FeedChange::Stopped
    );
    let elapsed = began.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "a stop must complete within a couple of seconds while frames arrive, took {elapsed:?}"
    );
    assert_eq!(
        feeder.counts.live(),
        0,
        "and must still release the session"
    );
}

#[test]
fn the_state_can_be_queried_without_side_effects() {
    let mut feeder = Feeder::new(0, 250.0);

    assert_eq!(feeder.controller.state(), FeedState::Stopped);
    assert_eq!(feeder.controller.status().state, FeedState::Stopped);
    for _ in 0..50 {
        assert_eq!(feeder.controller.state(), FeedState::Stopped);
    }
    assert_eq!(feeder.counts.opened(), 0, "asking must not take a session");
    assert_eq!(feeder.sink_opens(), 0, "asking must not open a sink");

    feeder.controller.start().expect("the start must work");
    feeder.wait_for_frames(3);
    for _ in 0..50 {
        assert_eq!(feeder.controller.state(), FeedState::Running);
    }
    assert_eq!(
        feeder.counts.opened(),
        1,
        "asking must not take a second session"
    );
    assert_eq!(feeder.sink_opens(), 1, "asking must not re-open the sink");

    let while_running = feeder.controller.status();
    assert_eq!(while_running.state, FeedState::Running);
    assert_eq!(while_running.last_end, None, "no run has completed yet");

    feeder.controller.stop().expect("the stop must work");
    let after = feeder.controller.status();
    assert_eq!(after.state, FeedState::Stopped);
    assert_eq!(after.last_end, Some(StreamEnd::Stopped));
    assert_eq!(after.sessions_opened, 1);
    assert_eq!(after.measured_frame_size, Some(FULL_FRAME_SIZE));
}

#[test]
fn a_stop_timeout_says_the_camera_may_still_be_held() {
    // A capture that has not returned is the one thing a stop cannot get past.
    // One frame every two seconds against a stop timeout of fifty milliseconds is
    // exactly that case: the answer must be an error that says the camera may
    // still be held, never a wait that does not end.
    let counts = SessionCounts::new();
    let sessions = SharedLog::default();
    let sink_log = Arc::new(Mutex::new(SinkLog::default()));
    let opener = CountingOpener::new(counts.clone(), Arc::clone(&sessions)).at_rate(0.5);
    let sink_factory = Arc::clone(&sink_log);
    let factory: FeedFactory = Arc::new(move || {
        FeedRun::new(
            Box::new(opener.clone()),
            RecordingSink::factory(Arc::clone(&sink_factory)),
            Recovery::new(Box::new(NoRecoveryReset)),
        )
    });
    let mut controller =
        FeedController::new(plan(), factory).with_stop_timeout(Duration::from_millis(50));

    controller.start().expect("the start must work");
    // The session records a capture before it sleeps, so this waits until the
    // capture thread is inside the two-second capture.
    wait_until(
        Duration::from_secs(2),
        "the capture thread to enter a capture",
        || {
            sessions
                .lock()
                .expect("the synthetic session log must not be poisoned")
                .iter()
                .any(|entry| entry == "capture")
        },
    );

    let error = controller
        .stop()
        .expect_err("a capture that has not returned must be reported, not waited on");
    assert!(
        matches!(error, FeedError::StopTimedOut { .. }),
        "the error must say the stop timed out, was {error:?}"
    );
    assert!(
        error.to_string().contains("may still hold"),
        "the message must say the camera may still be held: {error}"
    );

    // Still running, because the capture really is still in flight.
    assert_eq!(controller.state(), FeedState::Running);
    assert_eq!(counts.live(), 1);

    // Once the capture returns, the run notices the stop it was already given,
    // releases the session and closes the sink without another stop being needed.
    wait_until(
        Duration::from_secs(10),
        "the feed to release the camera",
        || controller.state() == FeedState::Stopped,
    );
    assert_eq!(counts.live(), 0, "the session was released");
    let sink = RecordingSink::read(&sink_log);
    assert_eq!(
        sink.open_sinks, 0,
        "no sink may be left open when the run ends, even one that was never opened"
    );
    drop(sink);
    assert_eq!(
        controller
            .stop()
            .expect("the eventual stop must be harmless"),
        FeedChange::AlreadyStopped
    );
}
