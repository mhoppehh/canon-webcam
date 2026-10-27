//! The `camera-cli` binary.
//!
//! `measure` is the first slice: it holds one **PTP session** for the whole run,
//! applies the acquisition settings, captures through a **calibration window**,
//! and reports the **measured frame size** and **measured frame rate**.
//!
//! `stream` runs the sustained pipeline: the same one session, a bounded queue
//! that drops the **oldest** frame, and the sink layer. It reports what the run
//! actually did rather than what was asked for.

use std::error;
use std::fmt;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use camera_cli::args::{self, ArgError, Command};
use camera_cli::report;
use camera_cli::sink::{SinkPlan, SinkSelectionError};
use camera_core::control::{
    socket, ControlReply, ControlRequest, FeedController, FeedFactory, FeedRun,
};
use camera_core::measurement::{measure, MeasureError, MonotonicClock};
use camera_core::recovery::{CommandRecoveryReset, Recovery, RecoveryReset, RecoveryResetError};
use camera_core::runner::{self as stream_runner, RecoveredStreamRun, StreamEnd, StreamPlan};
use camera_core::session::{PtpSession, SessionError};
use camera_core::stream::StreamError;
use camera_core::synthetic::{self, ScriptedSessionOpener, SyntheticSession};

/// How long a `control` command waits for its answer.
///
/// It is longer than the control surface's own two-second bound on a stop, so a
/// slow stop is reported as a stop that failed rather than as a client that gave
/// up first.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// What `control status` exits with when the feed is stopped, so a script can
/// branch on it without parsing the word.
const EXIT_CODE_STOPPED: u8 = 3;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, AppError> {
    match args::parse(std::env::args().skip(1))? {
        Command::Help => {
            print!("{}", args::USAGE);
            Ok(ExitCode::SUCCESS)
        }
        Command::Measure(measure_args) => {
            let plan = measure_args.plan();

            // The one session of the run. Everything below borrows it; nothing
            // below can open another.
            let mut session = camera_core::gphoto_session::CameraSession::open()?;
            let model = session.model().to_owned();

            // Started before the first capture, so the rate spans real time.
            let clock = MonotonicClock::new();
            let measurement = measure(&mut session, plan, &clock)?;

            print!("{}", report::render(&model, plan, &measurement));
            Ok(ExitCode::SUCCESS)
        }
        Command::Stream(stream_args) => {
            // The configuration was resolved during parsing, so an invalid
            // `mechanism`, `pixel_format` or `resolution` has already failed.
            // Nothing below has opened a PTP session yet.
            let config = stream_args.config();
            let plan = stream_plan(&stream_args);

            // Which sink publishes is resolved here too, so a sink this build
            // cannot publish through is refused before the camera is touched.
            let sink_plan = SinkPlan::select(stream_args.sink(), config)?;

            let clock = MonotonicClock::new();

            // One sink per segment: a wedged camera closes the sink on its way
            // out, so a resumed segment opens it again rather than reusing a
            // closed one.
            let mut open_sink = sink_plan.factory();

            let run: RecoveredStreamRun = match stream_args.synthetic_frame_rate() {
                // No camera attached: the synthetic source is real JPEG data at
                // the two frame sizes this camera has produced, so the pipeline
                // and the recovery path are exercised rather than simulated.
                Some(frame_rate) => {
                    let mut opener = synthetic_opener(&stream_args, frame_rate);
                    let reset: Box<dyn RecoveryReset> = Box::new(SimulatedRecoveryReset::new(
                        stream_args.synthetic_reset_fails(),
                    ));
                    let mut recovery = Recovery::new(reset);
                    stream_runner::run_recovering(
                        &mut opener,
                        &plan,
                        &mut *open_sink,
                        &clock,
                        &mut recovery,
                    )?
                }
                None => {
                    // The one session of the run, exactly as `measure` holds it,
                    // reopened only after a recovery reset destroyed a wedged one.
                    let mut opener = camera_core::gphoto_session::CameraSessionOpener::new();
                    let mut recovery = Recovery::new(Box::new(CommandRecoveryReset::new()));
                    stream_runner::run_recovering(
                        &mut opener,
                        &plan,
                        &mut *open_sink,
                        &clock,
                        &mut recovery,
                    )?
                }
            };

            print!(
                "{}",
                report::render_stream(config, &plan, &run, &sink_plan.report())
            );

            if run.end.is_failure() {
                return Err(AppError::StreamEnded(run.end));
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Serve(serve_args) => {
            // The same construction path `stream` uses, so a `control start`
            // takes the sink that a `stream` run would have built.
            let sink_plan =
                SinkPlan::select(serve_args.stream().sink(), serve_args.stream().config())?;
            let controller = FeedController::new(
                stream_plan(serve_args.stream()),
                feed_factory(serve_args.stream(), sink_plan.clone()),
            );
            let server = socket::ControlServer::bind(serve_args.socket(), controller)?;

            // Printed before serving, so whoever started the process can see the
            // path it is actually listening on and what a start will publish
            // through.
            println!("control socket: {}", server.path().display());
            println!("sink: {}", sink_plan.selection());
            println!("feed: {}", server_state(&server));

            server.serve_until_shutdown()?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Control(control_args) => {
            let reply = socket::send_command(
                control_args.socket(),
                control_args.request(),
                CONTROL_TIMEOUT,
            )?;

            if let ControlReply::Failed(detail) = &reply {
                return Err(AppError::Control(detail.clone()));
            }

            println!("{reply}");

            // `status` answers the question a script asks, so it also answers it
            // in the exit status: running is success, stopped is 3.
            Ok(match (&reply, control_args.request()) {
                (ControlReply::Stopped, ControlRequest::Status) => {
                    ExitCode::from(EXIT_CODE_STOPPED)
                }
                _ => ExitCode::SUCCESS,
            })
        }
    }
}

/// The plan the pipeline runs, whether it runs once (`stream`) or behind a
/// control socket (`serve`).
fn stream_plan(arguments: &args::StreamArgs) -> StreamPlan {
    let mut plan = StreamPlan::new(
        arguments.output_setting(),
        arguments
            .config()
            .pipeline_config(arguments.calibration().clone()),
    )
    .with_queue_capacity(arguments.queue_capacity());
    if let Some(duration) = arguments.duration() {
        plan = plan.with_run_for(duration);
    }
    plan
}

/// Builds everything one start of a served feed needs.
///
/// A factory rather than a single set of parts, because a start after a stop
/// needs a fresh **PTP session**: the one the previous start held is gone. The
/// sink comes from the same [`SinkPlan`] a `stream` run uses, and a fresh one is
/// built for every start — which is what makes a stop release the virtual camera
/// and a start take it again. The synthetic source stands in for the camera when
/// none is attached, exactly as it does for `stream`, so a start/stop cycle is
/// exercised without hardware.
fn feed_factory(arguments: &args::StreamArgs, sink: SinkPlan) -> FeedFactory {
    let arguments = arguments.clone();
    Arc::new(move || {
        let open_sink = sink.factory();
        let Some(frame_rate) = arguments.synthetic_frame_rate() else {
            return FeedRun::new(
                Box::new(camera_core::gphoto_session::CameraSessionOpener::new()),
                open_sink,
                Recovery::new(Box::new(CommandRecoveryReset::new())),
            );
        };

        FeedRun::new(
            Box::new(synthetic_opener(&arguments, frame_rate)),
            open_sink,
            Recovery::new(Box::new(SimulatedRecoveryReset::new(
                arguments.synthetic_reset_fails(),
            ))),
        )
    })
}

/// What a freshly bound control surface is doing, which is nothing yet.
fn server_state(server: &socket::ControlServer) -> &'static str {
    server.state().as_str()
}

/// The synthetic **PTP sessions** a demonstration run uses: the first is the one
/// that wedges, when `--synthetic-wedge-after` asks for that, and the second is
/// the one a **recovery reset** must reopen.
fn synthetic_opener(arguments: &args::StreamArgs, frame_rate: f64) -> ScriptedSessionOpener {
    let mut first = SyntheticSession::new(synthetic::FULL_FRAME_SIZE)
        .with_leading_frames(synthetic::OBSERVED_LEADING_FRAMES)
        .at_rate(frame_rate);
    if let Some(frames) = arguments.synthetic_wedge_after() {
        first = first.failing_after(frames, SyntheticSession::wedged());
    }
    let reopened = SyntheticSession::new(synthetic::FULL_FRAME_SIZE).at_rate(frame_rate);

    ScriptedSessionOpener::new([first, reopened])
}

/// The **recovery reset** a synthetic run performs.
///
/// Nothing is shelled out: there is no camera and no `gphoto2` session to
/// destroy, so the synthetic source stands in for the reset tool as well as for
/// the camera. The real run uses [`CommandRecoveryReset`], which runs
/// `gphoto2 --reset`, and the attempt counting is the same code either way.
struct SimulatedRecoveryReset {
    fails: bool,
}

impl SimulatedRecoveryReset {
    fn new(fails: bool) -> Self {
        Self { fails }
    }
}

impl RecoveryReset for SimulatedRecoveryReset {
    fn reset(&mut self) -> Result<(), RecoveryResetError> {
        if self.fails {
            Err(RecoveryResetError::new(
                "the simulated recovery reset was told to fail",
            ))
        } else {
            Ok(())
        }
    }

    fn command_line(&self) -> String {
        "gphoto2 --reset (simulated; no camera is attached)".to_owned()
    }
}

/// Anything that stopped the run.
#[derive(Debug)]
enum AppError {
    Arguments(ArgError),
    Session(SessionError),
    Measure(MeasureError),
    Stream(StreamError),
    StreamEnded(StreamEnd),
    Sink(SinkSelectionError),
    ControlSocket(std::io::Error),
    Control(String),
}

impl From<ArgError> for AppError {
    fn from(error: ArgError) -> Self {
        Self::Arguments(error)
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::ControlSocket(error)
    }
}

impl From<SessionError> for AppError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<MeasureError> for AppError {
    fn from(error: MeasureError) -> Self {
        Self::Measure(error)
    }
}

impl From<StreamError> for AppError {
    fn from(error: StreamError) -> Self {
        Self::Stream(error)
    }
}

impl From<SinkSelectionError> for AppError {
    fn from(error: SinkSelectionError) -> Self {
        Self::Sink(error)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(error) => write!(formatter, "{error}"),
            Self::Session(error) => write!(formatter, "{error}"),
            Self::Measure(error) => write!(formatter, "{error}"),
            Self::Stream(error) => write!(formatter, "{error}"),
            Self::StreamEnded(end) => match end.remedy() {
                Some(remedy) => write!(formatter, "{end}; {remedy}"),
                None => write!(formatter, "{end}"),
            },
            Self::Sink(error) => write!(formatter, "{error}"),
            Self::ControlSocket(error) => write!(formatter, "the control socket failed: {error}"),
            Self::Control(detail) => formatter.write_str(detail),
        }
    }
}

impl error::Error for AppError {}
