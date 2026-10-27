//! The capture thread, the bounded queue, and the pipeline loop.
//!
//! Three stages, and the shape is the guarantee:
//!
//! 1. a **capture thread** that owns the one **PTP session** and does nothing but
//!    press the **viewfinder action**, capture, and push the frame into a
//!    [`BoundedQueue`]. It never decodes and never touches a sink, so a slow
//!    pipeline can cost frames but can never stall capture;
//! 2. a bounded queue that drops the **oldest** frame when full, so a slow
//!    pipeline loses frames instead of growing latency;
//! 3. the [`StreamPipeline`], which decodes, drops frames of the wrong frame
//!    size, and delivers the rest to the sink.
//!
//! The only thing the two threads say to each other in the other direction is a
//! [`CaptureCommand`], and the capture thread only ever checks for one between
//! captures, so the answer to "does the capture path block on decode or output?"
//! is no by construction rather than by timing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::bounded_queue::{BoundedQueue, PopOutcome, DEFAULT_QUEUE_CAPACITY};
use crate::frame_size::FrameSize;
use crate::measurement::Clock;
use crate::output_setting::OutputSetting;
use crate::recovery::{PtpSessionOpener, Recovery, RecoveryLog};
use crate::session::{PtpSession, SessionError};
use crate::sink::FrameSink;
use crate::stats::{StatsSnapshot, StreamStats};
use crate::stream::{PipelineEvent, StreamError, StreamPipeline, StreamPipelineConfig};

/// How often the pipeline looks up from the queue to check the run deadline.
const PIPELINE_TICK: Duration = Duration::from_millis(20);

/// How long a run waits for a **disconnected camera** to come back by default.
///
/// A cable pulled and pushed back costs a few seconds; a camera left unplugged is
/// not worth waiting for, and the run ends as a disconnected camera.
const DEFAULT_RECONNECT_GRACE: Duration = Duration::from_secs(10);

/// How often a waiting run asks whether the camera is back.
const RECONNECT_POLL: Duration = Duration::from_millis(50);

/// One capture attempt, handed to the pipeline.
type CapturedFrame = Result<Vec<u8>, SessionError>;

/// Tells a running feed to wind down and give the camera back.
///
/// A start/stop control surface cannot interrupt a thread that is inside a
/// capture, and it must not: a capture that is abandoned mid-flight is a session
/// left in an unknown state. So a stop is a request the pipeline reads on its own
/// turn of its loop, and the answer to "how soon does a stop take effect?" is
/// therefore one [`PIPELINE_TICK`], not zero.
///
/// It is shared rather than borrowed because the control surface lives on one
/// thread and the run on another; a shared flag is the smallest thing that can
/// cross that boundary.
#[derive(Clone, Debug, Default)]
pub struct StopToken {
    stopped: Arc<AtomicBool>,
}

impl StopToken {
    /// A token nothing has stopped.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the run to stop. Calling it more than once is harmless.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// Whether a stop has been asked for.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Something the pipeline needs the capture thread to do, and the only traffic
/// that flows back towards the camera.
enum CaptureCommand {
    /// Re-apply the **output setting** and the **viewfinder action**, which is
    /// how a **recalibration** asks the camera for the full frame size again.
    Reapply {
        reply: Sender<Result<(), SessionError>>,
    },
    /// Capture one frame without using it, to find out whether the camera
    /// answers at all.
    Probe {
        reply: Sender<Result<(), SessionError>>,
    },
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEnd {
    /// The planned duration elapsed.
    DurationElapsed,
    /// The camera is on the USB bus but every operation fails: a **wedged
    /// camera**, which a **recovery reset** clears.
    WedgedCamera {
        /// What the camera said.
        detail: String,
    },
    /// The camera is absent from the USB bus: a **disconnected camera**, which a
    /// recovery reset cannot help.
    DisconnectedCamera {
        /// What libgphoto2 said.
        detail: String,
    },
    /// A **wedged camera** survived every bounded **recovery reset** attempt, so
    /// only a cable reconnect is left.
    RecoveryFailed {
        /// How many recovery resets ran.
        attempts: usize,
        /// What the last attempt said.
        detail: String,
    },
    /// Capture stopped for a reason that is neither of those.
    CaptureFailed {
        /// What happened.
        detail: String,
    },
    /// The run was asked to stop through its [`StopToken`], which is what the
    /// start/stop control surface does. The **PTP session** and the **sink** are
    /// released before this is reported; it is not a failure.
    Stopped,
}

impl StreamEnd {
    /// Whether the run ended badly.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        !matches!(self, Self::DurationElapsed | Self::Stopped)
    }

    /// What the user should do about it, if anything.
    ///
    /// The two camera states keep distinct remedies on purpose: a recovery reset
    /// clears a wedged camera and does nothing for a disconnected one.
    #[must_use]
    pub fn remedy(&self) -> Option<&'static str> {
        match self {
            Self::DurationElapsed => None,
            Self::WedgedCamera { .. } => Some(
                "the camera is still on the USB bus, so run `gphoto2 --reset`, or disconnect and \
                 reconnect the cable, and start again",
            ),
            Self::DisconnectedCamera { .. } => Some(
                "reconnect the camera; it is not on the USB bus at all, so a reset cannot help",
            ),
            Self::RecoveryFailed { .. } => Some(
                "remove and reconnect the USB cable, then start again; the recovery reset has \
                 already been tried, so it will not help a second time",
            ),
            Self::CaptureFailed { .. } => None,
            Self::Stopped => None,
        }
    }
}

impl std::fmt::Display for StreamEnd {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DurationElapsed => {
                formatter.write_str("the live view stream ran for the planned duration")
            }
            Self::WedgedCamera { detail } => write!(
                formatter,
                "the camera is wedged: it is on the USB bus but every operation fails ({detail})"
            ),
            Self::DisconnectedCamera { detail } => write!(
                formatter,
                "the camera is disconnected: it is absent from the USB bus ({detail})"
            ),
            Self::RecoveryFailed { attempts, detail } => write!(
                formatter,
                "the camera did not recover: {attempts} recovery resets did not clear the \
                 wedged camera ({detail})"
            ),
            Self::CaptureFailed { detail } => write!(formatter, "capture stopped: {detail}"),
            Self::Stopped => formatter.write_str(
                "the live view feed was stopped, and the PTP session and the sink were released",
            ),
        }
    }
}

/// What one run is asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamPlan {
    output_setting: OutputSetting,
    pipeline: StreamPipelineConfig,
    run_for: Option<Duration>,
    queue_capacity: usize,
    reconnect_grace: Duration,
}

impl StreamPlan {
    /// Builds a plan that runs until the camera stops producing frames.
    #[must_use]
    pub fn new(output_setting: OutputSetting, pipeline: StreamPipelineConfig) -> Self {
        Self {
            output_setting,
            pipeline,
            run_for: None,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            reconnect_grace: DEFAULT_RECONNECT_GRACE,
        }
    }

    /// Stops after `duration`, which is what a timed soak uses.
    #[must_use]
    pub fn with_run_for(mut self, duration: Duration) -> Self {
        self.run_for = Some(duration);
        self
    }

    /// Sets how long a run waits for a **disconnected camera** to come back
    /// before it gives up. A cable that is pulled for longer ends the run.
    #[must_use]
    pub fn with_reconnect_grace(mut self, grace: Duration) -> Self {
        self.reconnect_grace = grace;
        self
    }

    /// Sets how many frames the capture queue may hold.
    #[must_use]
    pub fn with_queue_capacity(mut self, capacity: usize) -> Self {
        self.queue_capacity = capacity;
        self
    }

    /// The **output setting** written at session start and on every
    /// recalibration.
    #[must_use]
    pub fn output_setting(&self) -> OutputSetting {
        self.output_setting
    }

    /// The pipeline configuration, including the **calibration window** policy.
    #[must_use]
    pub fn pipeline(&self) -> &StreamPipelineConfig {
        &self.pipeline
    }

    /// How long the run lasts, or `None` for as long as frames arrive.
    #[must_use]
    pub fn run_for(&self) -> Option<Duration> {
        self.run_for
    }

    /// How many frames the capture queue may hold.
    #[must_use]
    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    /// How long a run waits for a **disconnected camera** to come back.
    #[must_use]
    pub fn reconnect_grace(&self) -> Duration {
        self.reconnect_grace
    }
}

/// The result of one run.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamRun {
    /// How the run ended.
    pub end: StreamEnd,
    /// The **measured frame size**, once a calibration window established one.
    pub measured_frame_size: Option<FrameSize>,
    /// What the run actually did.
    pub stats: StatsSnapshot,
}

/// The result of a run that may have survived a **wedged camera**.
///
/// It is a [`StreamRun`] with the recovery history attached: the counters span
/// every segment, and [`RecoveredStreamRun::recovery`] says how many **recovery
/// resets** ran and how each one ended, so a user report is diagnosable without a
/// debugger.
#[derive(Clone, Debug)]
pub struct RecoveredStreamRun {
    /// How the run ended.
    pub end: StreamEnd,
    /// The last **measured frame size** the run established, if any.
    pub measured_frame_size: Option<FrameSize>,
    /// What the run did, across every segment.
    pub stats: StatsSnapshot,
    /// Every recovery reset the run performed, and its outcome.
    pub recovery: RecoveryLog,
    /// How many **PTP sessions** the run used: one, plus one per recovery.
    pub sessions_opened: usize,
}

/// Runs the whole pipeline until the plan's duration elapses or capture stops.
///
/// # Errors
///
/// Returns [`StreamError`] if the **calibration window** establishes no frame
/// size or the sink fails. A camera that stops answering is not an error here: it
/// is reported in [`StreamRun::end`] as a **wedged camera** or a **disconnected
/// camera**.
pub fn run(
    session: &mut (dyn PtpSession + Send),
    plan: &StreamPlan,
    sink: Box<dyn FrameSink + Send>,
    clock: &dyn Clock,
) -> Result<StreamRun, StreamError> {
    run_into(
        session,
        plan,
        sink,
        clock,
        None,
        &Arc::new(StreamStats::new()),
    )
}

/// Runs one segment of the pipeline over a caller-supplied set of counters.
///
/// A run that survives a **recovery reset** is several segments over one camera
/// and one duration, so the counters have to outlive a segment; that is the only
/// difference from [`run`]. Everything else, including the reason a segment
/// ended, is per segment. A segment that is handed a [`StopToken`] leaves early
/// with [`StreamEnd::Stopped`], which is how the start/stop control surface gives
/// the camera back without ending the process.
fn run_into(
    session: &mut (dyn PtpSession + Send),
    plan: &StreamPlan,
    sink: Box<dyn FrameSink + Send>,
    clock: &dyn Clock,
    stop: Option<&StopToken>,
    stats: &Arc<StreamStats>,
) -> Result<StreamRun, StreamError> {
    let queue: Arc<BoundedQueue<CapturedFrame>> =
        Arc::new(BoundedQueue::new(plan.queue_capacity()));
    let (commands, command_rx) = mpsc::channel::<CaptureCommand>();

    let mut pipeline = StreamPipeline::new(sink, Arc::clone(stats), plan.pipeline().clone());

    // Whether the capture thread has finished, however it finished. A capture
    // thread that reports a failure closes the queue on its way out, so the
    // pipeline learns about it from the queue; this flag exists for the one case
    // that closes nothing — a capture thread that dies without a word — because
    // waiting on a queue that nobody will ever fill again would wait for ever and
    // leave the device held.
    let capture_finished = Arc::new(AtomicBool::new(false));

    let end = thread::scope(|scope| -> Result<StreamEnd, StreamError> {
        let capture_queue = Arc::clone(&queue);
        let output_setting = plan.output_setting();
        let finished = Arc::clone(&capture_finished);
        let capture = scope.spawn(move || {
            // Set on the way out of the closure, including the way out that
            // unwinds.
            let _finished = CaptureFinished(finished);
            capture_loop(session, output_setting, &command_rx, &capture_queue);
        });

        let outcome = pipeline_loop(
            &queue,
            &commands,
            &mut pipeline,
            stats,
            &SegmentLimits {
                run_for: plan.run_for(),
                clock,
                stop,
            },
            &capture_finished,
        );

        // Closing the queue is what releases a capture thread that is about to
        // push its last frame, so it happens before the join on every path. The
        // capture thread notices the close between captures, which is why a
        // scoped thread never has to be abandoned.
        queue.close();
        let panicked = capture.join().is_err();

        let end = outcome?;
        if panicked {
            // The pipeline's own counters still describe everything that reached
            // it, so the run is reported rather than thrown away.
            return Ok(StreamEnd::CaptureFailed {
                detail: "the capture thread panicked".to_owned(),
            });
        }
        Ok(end)
    })?;

    stats.record_queue_drops(queue.stats().dropped());

    Ok(StreamRun {
        end,
        measured_frame_size: pipeline.expected_frame_size(),
        stats: stats.snapshot(clock.elapsed()),
    })
}

/// Runs the pipeline, surviving a **wedged camera** with a bounded **recovery
/// reset** sequence, and resumes in the same process afterwards.
///
/// This is the run the CLI performs. The shape is the guarantee:
///
/// - one **PTP session** at a time, obtained from `opener`, so a recovery never
///   sits beside the session it is replacing;
/// - a segment that ends as a [`StreamEnd::WedgedCamera`] has its session
///   released *before* the reset runs, because a recovery reset destroys that
///   session rather than adding one (`docs/adr/0001-single-ptp-session.md`). The
///   reset is therefore the one place where starting a new process is correct;
/// - the reset is tried a bounded number of times and the session is reopened in
///   this process, so success means the same run continues with its counters and
///   its clock intact;
/// - exhaustion is a [`StreamEnd::RecoveryFailed`], which is a non-zero exit with
///   a cable reconnect instruction, not a retry loop.
///
/// A **disconnected camera** is the other failure surface, and it takes the other
/// answer. A reset cannot put a camera back on the USB bus, so nothing is reset:
/// the segment that noticed the disconnection has already closed the sink on its
/// way out, so no consumer application is left holding a frozen last frame, and
/// this run then waits up to the plan's [`StreamPlan::reconnect_grace`] for
/// libgphoto2 to list the camera again. A camera that comes back is a fresh
/// **PTP session** and a fresh segment, which recalibrates the frame size and
/// **re-opens** the sink at whatever size it measures — the same process, the
/// same counters, the same clock. A camera that stays away ends the run as a
/// [`StreamEnd::DisconnectedCamera`].
///
/// # Errors
///
/// Returns [`StreamError`] if the **calibration window** establishes no frame
/// size or a sink fails, including on a segment after a recovery.
pub fn run_recovering(
    opener: &mut dyn PtpSessionOpener,
    plan: &StreamPlan,
    open_sink: &mut dyn FnMut() -> Box<dyn FrameSink + Send>,
    clock: &dyn Clock,
    recovery: &mut Recovery,
) -> Result<RecoveredStreamRun, StreamError> {
    // A run with no control surface has nothing that could stop it.
    let never = StopToken::new();
    run_recovering_until(opener, plan, open_sink, clock, recovery, &never)
}

/// [`run_recovering`], with a [`StopToken`] a start/stop control surface can set.
///
/// The token is checked by every segment, so a stop is honoured by whatever
/// segment is running and a recovery in progress is not a special case: the run
/// leaves after the segment it is in finishes, which is the only point at which
/// the sink is closed and the session released by the code that always closes and
/// releases them.
///
/// # Errors
///
/// Returns [`StreamError`] for the same reasons [`run_recovering`] does.
pub fn run_recovering_until(
    opener: &mut dyn PtpSessionOpener,
    plan: &StreamPlan,
    open_sink: &mut dyn FnMut() -> Box<dyn FrameSink + Send>,
    clock: &dyn Clock,
    recovery: &mut Recovery,
    stop: &StopToken,
) -> Result<RecoveredStreamRun, StreamError> {
    // One set of counters for the whole run, so a recovered run reports as one
    // run rather than as the last segment of one.
    let stats = Arc::new(StreamStats::new());
    let mut sessions_opened = 0;
    let mut measured_frame_size = None;

    let mut session = match opener.open_session() {
        Ok(session) => {
            sessions_opened += 1;
            session
        }
        Err(error) => {
            return Ok(RecoveredStreamRun {
                end: classify_session_failure(&error),
                measured_frame_size,
                stats: stats.snapshot(clock.elapsed()),
                recovery: recovery.log().clone(),
                sessions_opened,
            });
        }
    };

    let end = loop {
        let segment = run_into(&mut *session, plan, open_sink(), clock, Some(stop), &stats)?;
        if segment.measured_frame_size.is_some() {
            measured_frame_size = segment.measured_frame_size;
        }

        match segment.end {
            StreamEnd::WedgedCamera { detail } => {
                // The recovery reset destroys the wedged session, so its handle is
                // released before the reset runs: a second session beside this one
                // is what wedges a Canon body in the first place (ADR-0001).
                drop(session);

                match recovery.recover(opener) {
                    Ok(reopened) => {
                        sessions_opened += 1;
                        session = reopened;
                    }
                    Err(failure) => {
                        break StreamEnd::RecoveryFailed {
                            attempts: recovery.log().attempts_used(),
                            detail: format!("{detail}; {failure}"),
                        };
                    }
                }
            }
            StreamEnd::DisconnectedCamera { detail } => {
                // The segment closed the sink on its way out, so no consumer is
                // left holding a frozen last frame while the camera is away. The
                // run keeps its clock and its counters: a camera that comes back
                // resumes this run rather than starting a second one, and the
                // fresh segment recalibrates the frame size before it re-opens
                // the sink at whatever size it measures.
                drop(session);

                match wait_for_reconnect(opener, plan, clock) {
                    Reconnect::Reopened(reopened) => {
                        sessions_opened += 1;
                        session = reopened;
                    }
                    Reconnect::DurationElapsed => break StreamEnd::DurationElapsed,
                    Reconnect::NotBack { detail: why } => {
                        break StreamEnd::DisconnectedCamera {
                            detail: format!("{detail}; {why}"),
                        };
                    }
                    Reconnect::Failed(end) => break end,
                }
            }
            end => break end,
        }
    };

    Ok(RecoveredStreamRun {
        end,
        measured_frame_size,
        stats: stats.snapshot(clock.elapsed()),
        recovery: recovery.log().clone(),
        sessions_opened,
    })
}

/// What happened while a run waited for a **disconnected camera** to come back.
enum Reconnect {
    /// The camera is back, on a fresh **PTP session**.
    Reopened(Box<dyn PtpSession + Send>),
    /// The plan's duration elapsed first.
    DurationElapsed,
    /// The camera did not come back within the plan's grace.
    NotBack {
        /// What libgphoto2 said the last time it looked.
        detail: String,
    },
    /// The camera came back in a state a fresh session cannot fix.
    Failed(StreamEnd),
}

/// Waits for a **disconnected camera** to come back, a bounded number of looks.
///
/// A reset cannot put a camera back on the USB bus, so the only thing that can
/// help a disconnected camera is time: this waits for libgphoto2 to list it
/// again, and gives up when the plan's grace runs out.
fn wait_for_reconnect(
    opener: &mut dyn PtpSessionOpener,
    plan: &StreamPlan,
    clock: &dyn Clock,
) -> Reconnect {
    let waited_from = clock.elapsed();

    loop {
        let why = match opener.open_session() {
            Ok(session) => return Reconnect::Reopened(session),
            // Absent from the bus, which is the one condition waiting can fix.
            Err(SessionError::NoCamera { detail }) => detail,
            // The camera never left the bus, so it is not coming back because it
            // never went away: that is a state to report, not one to wait out.
            Err(error) => return Reconnect::Failed(classify_session_failure(&error)),
        };

        if plan.run_for().is_some_and(|limit| clock.elapsed() >= limit) {
            return Reconnect::DurationElapsed;
        }
        if clock.elapsed() - waited_from >= plan.reconnect_grace() {
            return Reconnect::NotBack { detail: why };
        }

        thread::sleep(RECONNECT_POLL.min(plan.reconnect_grace()));
    }
}

/// Marks a capture thread as finished, on the way out and on the way out that
/// unwinds.
///
/// It exists so that "the capture thread is gone" is a fact the pipeline can read
/// rather than a state it has to infer from a queue that has gone quiet.
struct CaptureFinished(Arc<AtomicBool>);

impl Drop for CaptureFinished {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn capture_loop(
    session: &mut (dyn PtpSession + Send),
    output_setting: OutputSetting,
    commands: &Receiver<CaptureCommand>,
    queue: &BoundedQueue<CapturedFrame>,
) {
    let applied = session
        .set_output_setting(output_setting)
        .and_then(|()| session.activate_viewfinder_action());

    if let Err(error) = applied {
        queue.push(Err(error));
        queue.close();
        return;
    }

    loop {
        match session.capture_live_view_frame() {
            Ok(frame) => {
                // The only thing this thread does with a frame: hand it over.
                // No decoding, no sink, no waiting for either.
                queue.push(Ok(frame));
            }
            Err(error) => {
                queue.push(Err(error));
                queue.close();
                return;
            }
        }

        // Between captures only, and never waiting for a command.
        while let Ok(command) = commands.try_recv() {
            let result = match command {
                CaptureCommand::Reapply { reply } => {
                    let result = session
                        .set_output_setting(output_setting)
                        .and_then(|()| session.activate_viewfinder_action());
                    let _ = reply.send(result.clone());
                    result
                }
                CaptureCommand::Probe { reply } => {
                    let result = session.capture_live_view_frame().map(|_| ());
                    let _ = reply.send(result.clone());
                    result
                }
            };
            if let Err(error) = result {
                queue.push(Err(error));
                queue.close();
                return;
            }
        }

        if queue.is_closed() {
            return;
        }
    }
}

/// How long one segment of a run lasts, and what can end it early.
///
/// The three are one thing — "the conditions this segment runs under" — and they
/// travel together through the pipeline loop, which is otherwise all about the
/// queue and the sink.
struct SegmentLimits<'a> {
    /// The planned duration, or `None` for as long as frames arrive.
    run_for: Option<Duration>,
    /// The clock the duration is measured on.
    clock: &'a dyn Clock,
    /// The start/stop control surface's request to stop, when there is one.
    stop: Option<&'a StopToken>,
}

fn pipeline_loop(
    queue: &BoundedQueue<CapturedFrame>,
    commands: &Sender<CaptureCommand>,
    pipeline: &mut StreamPipeline,
    stats: &StreamStats,
    limits: &SegmentLimits<'_>,
    capture_finished: &AtomicBool,
) -> Result<StreamEnd, StreamError> {
    let outcome = run_pipeline_loop(queue, commands, pipeline, stats, limits, capture_finished);

    // Every exit path closes the sink, the error path included: a run that failed
    // on a sink or calibration error must not leave the device held, and the
    // consumer must not be left showing the last frame it was handed. Closing
    // happens before the outcome is unwrapped, which is what makes that true.
    let closed = pipeline.shutdown();
    let end = outcome?;
    closed?;

    Ok(end)
}

fn run_pipeline_loop(
    queue: &BoundedQueue<CapturedFrame>,
    commands: &Sender<CaptureCommand>,
    pipeline: &mut StreamPipeline,
    stats: &StreamStats,
    limits: &SegmentLimits<'_>,
    capture_finished: &AtomicBool,
) -> Result<StreamEnd, StreamError> {
    let end = loop {
        // A stop is checked first, so a feed that is asked to stop stops before
        // it spends another frame on the sink. The wait below is one
        // [`PIPELINE_TICK`], so this is the most a stop can wait for frames that
        // are still arriving.
        if limits.stop.is_some_and(StopToken::is_stopped) {
            break StreamEnd::Stopped;
        }

        if let Some(limit) = limits.run_for {
            if limits.clock.elapsed() >= limit {
                break StreamEnd::DurationElapsed;
            }
        }

        stats.observe_queue_depth(queue.depth());

        match queue.pop_timeout(PIPELINE_TICK) {
            PopOutcome::Frame(Ok(frame)) => {
                let event = pipeline.handle(&frame, limits.clock.elapsed())?;
                match event {
                    PipelineEvent::RecalibrationStarted => {
                        // Ask the camera for the full frame size again, once.
                        if let Err(end) = reapply(commands) {
                            break end;
                        }
                    }
                    PipelineEvent::Unsettled => break probe(commands),
                    _ => {}
                }
            }
            PopOutcome::Frame(Err(error)) => break classify_session_failure(&error),
            PopOutcome::TimedOut | PopOutcome::Empty => {
                // Nothing to pop and nothing closed the queue, so the only thing
                // that can be wrong is that no capture will ever arrive again.
                // Waiting on it would wait for ever with the device still held.
                if capture_finished.load(Ordering::SeqCst) {
                    break StreamEnd::CaptureFailed {
                        detail: "the capture thread stopped producing frames without reporting \
                                 why"
                        .to_owned(),
                    };
                }
            }
            // The capture thread closes the queue only after pushing the failure
            // that closed it, and that failure is popped above, so this is the
            // run being wound down rather than a camera condition.
            PopOutcome::Closed => {
                break StreamEnd::CaptureFailed {
                    detail: "the capture thread stopped producing frames".to_owned(),
                }
            }
        }
    };

    Ok(end)
}

fn reapply(commands: &Sender<CaptureCommand>) -> Result<(), StreamEnd> {
    let (reply, answer) = mpsc::channel();
    if commands.send(CaptureCommand::Reapply { reply }).is_err() {
        return Err(StreamEnd::CaptureFailed {
            detail: "the capture thread is gone, so the acquisition settings cannot be re-applied"
                .to_owned(),
        });
    }
    match answer.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(classify_session_failure(&error)),
        Err(_) => Err(StreamEnd::CaptureFailed {
            detail: "the capture thread did not answer a request to re-apply the acquisition \
                     settings"
                .to_owned(),
        }),
    }
}

fn probe(commands: &Sender<CaptureCommand>) -> StreamEnd {
    let (reply, answer) = mpsc::channel();
    if commands.send(CaptureCommand::Probe { reply }).is_err() {
        return StreamEnd::CaptureFailed {
            detail: "the capture thread is gone; capture stopped producing frames".to_owned(),
        };
    }
    match answer.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => StreamEnd::WedgedCamera {
            detail: "the camera answers captures but agrees on no frame size, so it is on the USB \
                     bus and is not behaving"
                .to_owned(),
        },
        Ok(Err(error)) => classify_session_failure(&error),
        Err(_) => StreamEnd::WedgedCamera {
            detail: "the camera did not answer a probe capture".to_owned(),
        },
    }
}

/// Maps a session failure onto the two camera states the glossary keeps apart.
#[must_use]
pub fn classify_session_failure(error: &SessionError) -> StreamEnd {
    match error {
        SessionError::NoCamera { detail } => StreamEnd::DisconnectedCamera {
            detail: detail.clone(),
        },
        SessionError::Camera { detail, .. } => StreamEnd::WedgedCamera {
            detail: detail.clone(),
        },
        SessionError::AlreadyOpen => StreamEnd::CaptureFailed {
            detail: error.to_string(),
        },
    }
}
