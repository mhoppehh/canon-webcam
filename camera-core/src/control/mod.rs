//! Turning the live view feed off and on while the process keeps running.
//!
//! `docs/adr/0005-start-stop-control.md` is the authority for the shape of this
//! module. It decided two things:
//!
//! - the state machine is here, in [`FeedController`], and it owns the **PTP
//!   session** and the **virtual camera sink** for the life of the process;
//! - the front end is a local Unix socket ([`socket`]), with a menu bar item or a
//!   tray left as a second front end over the same controller.
//!
//! The split is what makes the acceptance criteria hold for every front end
//! rather than for one of them. A front end can only start, stop, and ask what
//! the feed is doing; it cannot reach a session or a sink, so it cannot leave
//! either behind.
//!
//! # What a stop releases, and when
//!
//! `docs/adr/0001-single-ptp-session.md` says there is one **PTP session** at a
//! time, so a stop must drop it rather than park it: another application, or
//! another instance of this one, is allowed to take the camera the moment the
//! feed stops. The controller therefore runs each start on a worker thread and,
//! on a stop, waits for that thread to report — and the thread drops the opener
//! (and with it the session), the sink factory's sink and the recovery state
//! *before* it reports. [`FeedController::state`] reads the same flag, so a
//! worker that has released the camera is never reported as still running.
//!
//! # What a start does
//!
//! Nothing is reused. A start builds a fresh [`FeedRun`]: a fresh opener, a fresh
//! sink, a fresh [`Recovery`] and a fresh clock, and then runs
//! [`run_recovering`](crate::runner::run_recovering_until) from the beginning.
//! That is deliberate — the **output setting** and the **viewfinder action** are
//! re-applied by the same code that applies them on any run, and the **leading
//! frames** are handled by a fresh **calibration window** rather than by a count
//! carried over from the previous run.

// Unix domain sockets are the front end on macOS and Linux. A Windows front end
// would be a named pipe; the controller above is unaffected by the absence of
// one.
#[cfg(unix)]
pub mod socket;

use std::error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::frame_size::FrameSize;
use crate::measurement::MonotonicClock;
use crate::recovery::{PtpSessionOpener, Recovery};
use crate::runner::{self, RecoveredStreamRun, StopToken, StreamEnd, StreamPlan};
use crate::sink::FrameSink;
use crate::stream::StreamError;

/// How long a stop waits for the run to give the camera back before it reports
/// that it could not.
///
/// A stop cannot be instant: the capture thread is inside a capture and the
/// pipeline is inside one tick of its loop. What it must never be is unbounded,
/// because the thread it waits for may be inside a call to a camera that has
/// stopped answering. Two seconds is the cost of the longest capture this
/// software expects plus a tick, and it is short enough that a script can wait.
pub const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether the live view feed is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedState {
    /// No **PTP session** and no **sink** are held.
    Stopped,
    /// A run holds the session and the sink.
    Running,
}

impl FeedState {
    /// The word a front end prints, so a script has one thing to compare.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
        }
    }

    /// Whether the feed is running.
    #[must_use]
    pub fn is_running(self) -> bool {
        self == Self::Running
    }
}

impl fmt::Display for FeedState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a start or a stop did.
///
/// The two "already" answers are separate from the two that changed something,
/// because they are not failures: starting a running feed and stopping a stopped
/// one are both harmless, and a caller that wanted to know whether it changed
/// anything can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedChange {
    /// The feed was stopped and is now running.
    Started,
    /// The feed was already running; nothing was taken a second time.
    AlreadyRunning,
    /// The feed was running and is now stopped, with both the **PTP session** and
    /// the **sink** released.
    Stopped,
    /// The feed was already stopped; nothing was released a second time.
    AlreadyStopped,
}

/// Everything one run of the feed needs, built fresh for every start.
///
/// The three parts are exactly the arguments
/// [`run_recovering_until`](crate::runner::run_recovering_until) takes, grouped
/// so that a front end cannot supply two of them and forget the third. The opener
/// owns the **PTP session**, the sink factory is called once per sink the run
/// opens (which is once per segment, and again for every **sink re-open**), and
/// the recovery is what a **wedged camera** is answered with.
pub struct FeedRun {
    /// Opens the **PTP session**, and a fresh one after a recovery reset.
    pub opener: Box<dyn PtpSessionOpener + Send>,
    /// Opens a new **sink**. Called per segment, so it must not return a sink that
    /// was closed before.
    pub open_sink: Box<dyn FnMut() -> Box<dyn FrameSink + Send> + Send>,
    /// The **recovery reset** state for this run.
    pub recovery: Recovery,
}

impl FeedRun {
    /// Groups one run's three parts.
    #[must_use]
    pub fn new(
        opener: Box<dyn PtpSessionOpener + Send>,
        open_sink: Box<dyn FnMut() -> Box<dyn FrameSink + Send> + Send>,
        recovery: Recovery,
    ) -> Self {
        Self {
            opener,
            open_sink,
            recovery,
        }
    }
}

/// Builds a fresh [`FeedRun`] for every start.
///
/// A factory rather than a stored `FeedRun` for one reason: a start must build a
/// new opener, because the session the previous start held is gone, and reusing
/// the old one would mean asking a destroyed session for a new handle. The
/// practical form is a closure that captures the counters a test wants to see, or
/// the camera and sink configuration a real run needs.
pub type FeedFactory = Arc<dyn Fn() -> FeedRun + Send + Sync>;

/// What the feed is doing, and what the last completed run did.
///
/// A front end reports this. It is deliberately not a `Result`: "the feed is
/// stopped because the camera was disconnected" is an answer, not a failure of
/// the question.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedStatus {
    /// Whether the feed is running now.
    pub state: FeedState,
    /// How the last completed run ended, if one has completed.
    pub last_end: Option<StreamEnd>,
    /// The last **measured frame size** that run established, if any.
    pub measured_frame_size: Option<FrameSize>,
    /// How many **PTP sessions** that run used: one, plus one per recovery.
    pub sessions_opened: usize,
    /// Why the last run could not report an ending of its own: a
    /// [`StreamError`] (the **calibration window** established no frame size, or
    /// the sink failed), or a run that panicked.
    pub error: Option<String>,
}

/// Why the controller could not do what it was asked.
///
/// Neither variant can leave the camera held: a timed-out stop keeps the worker
/// (which still owns the session) reachable so a later stop can try again, and it
/// says so rather than pretending the camera is free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedError {
    /// The run did not give the camera back within the stop timeout. The sink may
    /// still be open and the **PTP session** still held until it does.
    StopTimedOut {
        /// How long the stop waited.
        timeout: Duration,
    },
    /// The worker thread could not be created.
    Spawn {
        /// What the operating system said.
        detail: String,
    },
}

impl fmt::Display for FeedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StopTimedOut { timeout } => write!(
                formatter,
                "the feed did not release the camera and the sink within {:.1} s, so it may still \
                 hold both; the stop can be retried",
                timeout.as_secs_f64()
            ),
            Self::Spawn { detail } => {
                write!(formatter, "the feed could not be started ({detail})")
            }
        }
    }
}

impl error::Error for FeedError {}

/// The start/stop state machine. See the module documentation and
/// `docs/adr/0005-start-stop-control.md`.
///
/// It is not `Sync` and does not need to be: one front end owns it, and a front
/// end that serves several callers serialises them. That is what keeps "exactly
/// one **PTP session**" true without a lock around the controller.
pub struct FeedController {
    plan: StreamPlan,
    factory: FeedFactory,
    stop_timeout: Duration,
    worker: Option<Worker>,
    last: Option<RunReport>,
}

/// What one completed run did, in the terms [`FeedStatus`] reports.
#[derive(Clone, Debug, Default, PartialEq)]
struct RunReport {
    end: Option<StreamEnd>,
    measured_frame_size: Option<FrameSize>,
    sessions_opened: usize,
    error: Option<String>,
}

impl RunReport {
    fn from_outcome(outcome: Result<RecoveredStreamRun, StreamError>) -> Self {
        match outcome {
            Ok(run) => Self {
                end: Some(run.end),
                measured_frame_size: run.measured_frame_size,
                sessions_opened: run.sessions_opened,
                error: None,
            },
            Err(error) => Self {
                error: Some(error.to_string()),
                ..Self::default()
            },
        }
    }

    /// A run that died without reporting, which only a panic does.
    fn panicked() -> Self {
        Self {
            error: Some(
                "the run ended without reporting how, which a panicking run does; the PTP session \
                 and the sink went with it"
                    .to_owned(),
            ),
            ..Self::default()
        }
    }
}

/// One run, on its own thread, so the control surface answers while frames
/// arrive.
struct Worker {
    stop: StopToken,
    finished: Arc<AtomicBool>,
    outcome: Receiver<Result<RecoveredStreamRun, StreamError>>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            // The thread has reported, so this join is a formality; a panicking
            // run is already accounted for by the disconnected channel.
            let _ = handle.join();
        }
    }
}

/// Marks the worker as finished on every way out, including the way out that
/// unwinds. It is set after the session and the sink are dropped, so "finished"
/// and "the camera is free" are the same fact.
struct Finished(Arc<AtomicBool>);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl FeedController {
    /// A controller that has not started the feed.
    #[must_use]
    pub fn new(plan: StreamPlan, factory: FeedFactory) -> Self {
        Self {
            plan,
            factory,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
            worker: None,
            last: None,
        }
    }

    /// Replaces the bounded wait a stop performs.
    #[must_use]
    pub fn with_stop_timeout(mut self, timeout: Duration) -> Self {
        self.stop_timeout = timeout;
        self
    }

    /// Whether the feed is running.
    ///
    /// It has no side effects: it reads a flag the worker sets once it has
    /// released the **PTP session** and the **sink**, so a script may ask as often
    /// as it likes without taking anything.
    #[must_use]
    pub fn state(&self) -> FeedState {
        match &self.worker {
            Some(worker) if !worker.finished.load(Ordering::SeqCst) => FeedState::Running,
            _ => FeedState::Stopped,
        }
    }

    /// Records what a run that ended on its own did, so a status query can say
    /// why the feed is not running.
    ///
    /// A run that fails at the **sink** — a missing prerequisite, a lost OBS —
    /// ends by itself, and its outcome only reaches [`FeedStatus`] when a
    /// finished worker is reaped. A front end that only ever asks for the state
    /// would otherwise report `stopped` for ever and lose the actionable reason.
    /// This takes nothing: a run that is still running, and a feed that was
    /// never started, are left exactly as they were.
    pub fn record_finished_run(&mut self) {
        self.reap();
    }

    /// What the feed is doing, and what the last completed run did.
    #[must_use]
    pub fn status(&self) -> FeedStatus {
        let last = self.last.clone().unwrap_or_default();

        FeedStatus {
            state: self.state(),
            last_end: last.end,
            measured_frame_size: last.measured_frame_size,
            sessions_opened: last.sessions_opened,
            error: last.error,
        }
    }

    /// Starts the feed, taking the **PTP session** and opening the **sink**.
    ///
    /// Starting a feed that is already running changes nothing and starts no
    /// second session; it answers [`FeedChange::AlreadyRunning`].
    ///
    /// # Errors
    ///
    /// Returns [`FeedError::Spawn`] if the run could not be given a thread.
    pub fn start(&mut self) -> Result<FeedChange, FeedError> {
        self.reap();
        if self.worker.is_some() {
            return Ok(FeedChange::AlreadyRunning);
        }

        let stop = StopToken::new();
        let finished = Arc::new(AtomicBool::new(false));
        let (report, outcome) = mpsc::channel();

        let plan = self.plan.clone();
        let factory = Arc::clone(&self.factory);
        let worker_stop = stop.clone();
        let worker_finished = Arc::clone(&finished);

        let handle = thread::Builder::new()
            .name("canon-webcam-feed".to_owned())
            .spawn(move || {
                let _finished = Finished(worker_finished);

                let FeedRun {
                    mut opener,
                    mut open_sink,
                    mut recovery,
                } = (factory)();

                // A clock per run, because every number this run reports is
                // about this run.
                let clock = MonotonicClock::new();
                let result = runner::run_recovering_until(
                    &mut *opener,
                    &plan,
                    &mut *open_sink,
                    &clock,
                    &mut recovery,
                    &worker_stop,
                );

                // The **PTP session** and the **sink** are released before the
                // feed is reported stopped, so another application may take the
                // camera as soon as the stop is acknowledged.
                drop(opener);
                drop(open_sink);
                drop(recovery);

                let _ = report.send(result);
            })
            .map_err(|error| FeedError::Spawn {
                detail: error.to_string(),
            })?;

        self.worker = Some(Worker {
            stop,
            finished,
            outcome,
            handle: Some(handle),
        });
        self.last = None;

        Ok(FeedChange::Started)
    }

    /// Stops the feed, releasing the **PTP session** and closing the **sink**.
    ///
    /// Stopping a feed that is already stopped changes nothing and releases
    /// nothing twice; it answers [`FeedChange::AlreadyStopped`].
    ///
    /// # Errors
    ///
    /// Returns [`FeedError::StopTimedOut`] if the run has not given the camera
    /// back within the stop timeout. The wait is bounded on purpose: the run may
    /// be inside a call to a camera that no longer answers.
    pub fn stop(&mut self) -> Result<FeedChange, FeedError> {
        if self.state() == FeedState::Stopped {
            self.reap();
            return Ok(FeedChange::AlreadyStopped);
        }

        let Some(worker) = self.worker.take() else {
            return Ok(FeedChange::AlreadyStopped);
        };

        worker.stop.stop();
        match worker.outcome.recv_timeout(self.stop_timeout) {
            Ok(result) => {
                worker.join();
                self.last = Some(RunReport::from_outcome(result));
                Ok(FeedChange::Stopped)
            }
            Err(RecvTimeoutError::Timeout) => {
                // The run still holds the camera. It is kept reachable so a
                // second stop can wait for it again, rather than being abandoned
                // with no way to ask about it.
                self.worker = Some(worker);
                Err(FeedError::StopTimedOut {
                    timeout: self.stop_timeout,
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                // The run died without reporting — a panic. Its session and sink
                // went with it, so the feed is stopped.
                worker.join();
                self.last = Some(RunReport::panicked());
                Ok(FeedChange::Stopped)
            }
        }
    }

    /// Joins a worker that has already reported, so its outcome is not lost.
    fn reap(&mut self) {
        let reported = self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.finished.load(Ordering::SeqCst));
        if !reported {
            return;
        }
        if let Some(worker) = self.worker.take() {
            let outcome = worker.outcome.try_recv();
            worker.join();
            if let Ok(result) = outcome {
                self.last = Some(RunReport::from_outcome(result));
            }
        }
    }
}

impl Drop for FeedController {
    fn drop(&mut self) {
        // Bounded, and nothing is lost if it is not honoured: the worker owns the
        // session and the sink, so dropping this controller releases them either
        // way. A program that exits with a feed running must not leave the
        // virtual camera device held for the next run.
        let _ = self.stop();
    }
}

/// One line of the control protocol.
///
/// The protocol is deliberately one word in and one word out, so `socat`, `nc -U`
/// and a shell script can all speak it without a parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlRequest {
    /// Start the feed.
    Start,
    /// Stop the feed.
    Stop,
    /// Ask whether the feed is running.
    Status,
    /// Stop the feed and end the control surface.
    Shutdown,
}

impl ControlRequest {
    /// Parses one command line.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "status" => Some(Self::Status),
            "shutdown" => Some(Self::Shutdown),
            _ => None,
        }
    }

    /// The word this request is sent as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Status => "status",
            Self::Shutdown => "shutdown",
        }
    }
}

/// One line of the control protocol's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlReply {
    /// The feed is now running because this request started it.
    Started,
    /// The feed was already running; nothing was taken twice.
    AlreadyRunning,
    /// The feed is now stopped, with the session and the sink released.
    Stopped,
    /// The feed was already stopped; nothing was released twice.
    AlreadyStopped,
    /// The answer to `status`: the feed is running.
    Running,
    /// The control surface is stopping the feed and closing.
    ShuttingDown,
    /// The request could not be carried out. The detail is the reason, in the
    /// same words a user would otherwise read on a terminal.
    Failed(String),
}

impl ControlReply {
    /// The reply for a feed state, which is what `status` answers.
    #[must_use]
    pub fn state(state: FeedState) -> Self {
        match state {
            FeedState::Running => Self::Running,
            FeedState::Stopped => Self::Stopped,
        }
    }

    /// Parses one reply line.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        match line {
            "started" => Some(Self::Started),
            "already-running" => Some(Self::AlreadyRunning),
            "stopped" => Some(Self::Stopped),
            "already-stopped" => Some(Self::AlreadyStopped),
            "running" => Some(Self::Running),
            "shutting-down" => Some(Self::ShuttingDown),
            other => other
                .strip_prefix("error: ")
                .map(|detail| Self::Failed(detail.to_owned())),
        }
    }
}

impl fmt::Display for ControlReply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started => formatter.write_str("started"),
            Self::AlreadyRunning => formatter.write_str("already-running"),
            Self::Stopped => formatter.write_str("stopped"),
            Self::AlreadyStopped => formatter.write_str("already-stopped"),
            Self::Running => formatter.write_str("running"),
            Self::ShuttingDown => formatter.write_str("shutting-down"),
            Self::Failed(detail) => write!(formatter, "error: {detail}"),
        }
    }
}

impl From<FeedChange> for ControlReply {
    fn from(change: FeedChange) -> Self {
        match change {
            FeedChange::Started => Self::Started,
            FeedChange::AlreadyRunning => Self::AlreadyRunning,
            FeedChange::Stopped => Self::Stopped,
            FeedChange::AlreadyStopped => Self::AlreadyStopped,
        }
    }
}
