//! Command line parsing for the `measure` subcommand.
//!
//! Everything is validated here, before a **PTP session** is opened: a value that
//! the **output setting** guard refuses, or a **calibration window** that could
//! never establish a size, is a usage error rather than something to discover
//! halfway through a capture.

use std::error;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use camera_core::calibration::{CalibrationPolicy, CalibrationPolicyError};
use camera_core::config::{ConfigError, ConfigOverrides, StreamingConfig};
use camera_core::control::ControlRequest;
use camera_core::measurement::{MeasurementPlan, MeasurementPlanError};
use camera_core::mechanism::Platform;
use camera_core::output_setting::{OutputSetting, OutputSettingError};

use crate::sink::{SinkSelection, DEFAULT_SINK};

/// The default frame rate the synthetic frame source runs at, so a run without a
/// camera behaves like a run at the rate an M50 produces.
pub const DEFAULT_SYNTHETIC_FRAME_RATE: f64 = 24.0;

/// The help text. It records the decisions a reader would otherwise have to
/// rediscover, including that `liveviewsize` is never used.
pub const USAGE: &str = "\
camera-cli — Canon EOS M50 live view as an operating-system camera

USAGE:
    camera-cli <command> [options]

COMMANDS:
    measure    Capture live view, establish the expected frame size with a
               calibration window, and report the measured frame size and the
               measured frame rate.
    stream     Run the sustained live view pipeline: capture, bounded queue,
               decode to one pixel format, and a sink. Prints the measured frame
               size, the measured frame rate and the mechanism in use, then what
               the run actually did.
    serve      Run the same pipeline behind a local control socket, so the feed
               can be stopped and started without restarting the process. A stop
               releases the PTP session and closes the sink, and a start takes
               both again. Runs until `control ... shutdown` is sent.
    control    Send one command to a running `serve`: start, stop, status or
               shutdown.
    help       Print this text.

OPTIONS FOR measure:
    --output <raw>             output setting to write. Allowlist: 0, 1, 2, 3.
                               Default 3 (TFT + PC), which is the full frame size.
    --window-frames <n>        Calibration window frame limit. Default 72.
    --window-duration-ms <ms>  Calibration window time limit. Default 3000.
    --min-recurrences <n>      How often a frame size must recur inside the
                               window to become the expected frame size. Default 8.
    --sampling-frames <n>      Frames in the sampling window that measures the
                               rate. Default 48.

OPTIONS FOR stream:
    --config <path>            TOML configuration file. Keys: resolution, fps,
                               pixel_format, upscale, mechanism. Command line
                               options override the file. On macOS the `[macos]`
                               table carries the mechanism's prerequisites:
                               obs_app_path, websocket_endpoint,
                               websocket_password, obs_log_directory and
                               launch_obs.
    --resolution <size>        A requested size, such as 1080p or 1024x576. It is
                               a preference: the M50 honours no size request, so
                               the report always states the measured frame size.
    --fps <n>                  A frame rate preference for the sink to pace at.
    --pixel-format <format>    The one normalized format the sink accepts. rgb24.
    --upscale                  Resize delivered frames to --resolution. This is
                               resizing: it adds no detail.
    --no-upscale               Turn resizing off (the default).
    --mechanism <name>         Virtual camera mechanism. On macOS the only one is
                               obs-virtual-camera; naming another is an error,
                               not a fallback.
    --sink <name>              Where the live view feed is published. `stats`
                               (the default) counts it and publishes nothing, so
                               no consumer application sees anything;
                               `virtual-camera` publishes it through the platform
                               backend, which is what makes the camera appear.
                               The platform backend is built from `mechanism` and
                               the `[macos]` table of the configuration; asking
                               for it where it cannot be built is an error, never
                               a fallback to `stats`.
    --duration-secs <n>        Stop after this many seconds.
    --queue-capacity <n>       Frames the bounded capture queue holds. Default 4.
    --synthetic                Use a synthetic frame source instead of a camera,
                               for demonstrating the pipeline with no M50
                               attached.
    --synthetic-fps <n>        Frame rate of the synthetic source. Default 24.
    --synthetic-wedge-after <n>
                               Make the synthetic source stop answering with
                               PTP Timeout after n frames, which is the wedged
                               camera the recovery path exists for. The first
                               recovery reset then clears it, and the run
                               resumes in the same process.
    --synthetic-reset-fails    Make every recovery reset fail, so a synthetic
                               run shows the exit path a permanently wedged
                               camera produces.
    --output <raw>             output setting to write before capturing.
    --window-frames <n>        Calibration window frame limit. Default 72.
    --window-duration-ms <ms>  Calibration window time limit. Default 3000.
    --min-recurrences <n>      How often a frame size must recur inside the
                               window to become the expected frame size. Default 8.

OPTIONS FOR serve:
    --socket <path>            Required. The local Unix socket the control
                               commands are sent to.
    Every option of stream is accepted, and applies to each start: the output
    setting, the calibration window, the queue capacity, the configuration file,
    the sink and the synthetic source are resolved once, before the socket is
    bound. Each start takes a new sink from the same construction path a
    `stream` run uses, so a stop releases the virtual camera and a start takes
    it again.

OPTIONS FOR control:
    --socket <path>            Required. The local Unix socket of a running serve.

CONTROL COMMANDS:
    start      Take the PTP session and open the sink. Answers `started`, or
               `already-running` when the feed is already running.
    stop       Release the PTP session and close the sink. Answers `stopped`, or
               `already-stopped` when the feed is already stopped.
    status     Answers `running` or `stopped` and changes nothing. When the feed
               is stopped because the last run ended in a failure, it answers
               `error: ...` with the reason, so a script learns why the feed is
               not running rather than only that it is stopped.
    shutdown   Stop the feed and close the control socket.

EXIT CODES FOR control status:
    0          the feed is running
    3          the feed is stopped
    1          the command failed

NOTES:
    One PTP session is held for the whole run: the output setting, the viewfinder
    action and every capture share it. A second session is a bug, not a fallback.

    A wedged camera (on the USB bus, every operation a PTP Timeout) is detected
    and recovered: the wedged session is destroyed by a bounded number of
    `gphoto2 --reset` runs and a fresh PTP session is opened in this process, so
    a successful recovery resumes the stream without restarting it. `gphoto2
    --reset` is the one place where starting a process is correct, because it
    destroys the session rather than adding one. If every attempt fails the
    command exits non-zero and tells you to remove and reconnect the USB cable.

    The viewfinder action does not persist between invocations, so it is pressed
    on every run, in the same session as the capture.

    The output setting is written on every run, because persistence across
    process boundaries is not proven.

    liveviewsize is a no-op on Canon: it accepts the write and changes nothing.
    This command never uses it and never reports it, because reporting it would
    produce a confidently wrong answer.

    The measured frame size and the measured frame rate come from frame data
    only. 1024x576 at roughly 24 fps is a sanity check on them, never their
    source.

    The capture thread never decodes and never touches a sink: it pushes frames
    into a bounded queue that drops the oldest frame when full, so a slow
    pipeline costs frames and not latency.

    serve starts stopped. Stopping releases the PTP session and closes the sink,
    so another application, or another instance of this one, can take the camera
    between runs. Starting again re-applies the output setting and the viewfinder
    action and handles the leading frames with a fresh calibration window, which
    is what a fresh run does. Stopping when stopped and starting when running
    change nothing.

    A stop is a request the pipeline reads on its next turn, not a kill, so the
    capture that is in flight finishes. It is bounded: if the camera does not
    give the session back within two seconds, serve says so and the camera may
    still be held.

    --duration-secs with serve makes each start stop by itself when the duration
    elapses, rather than limiting the whole process.
";

/// What the user asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Print [`USAGE`].
    Help,
    /// Measure live view.
    Measure(MeasureArgs),
    /// Run the sustained pipeline.
    Stream(StreamArgs),
    /// Run the feed behind a control socket, turned off and on by `control`.
    Serve(ServeArgs),
    /// Ask a running `serve` to start, stop or report the feed.
    Control(ControlArgs),
}

/// A validated `measure` plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasureArgs {
    plan: MeasurementPlan,
}

impl MeasureArgs {
    /// The plan to run.
    #[must_use]
    pub fn plan(&self) -> &MeasurementPlan {
        &self.plan
    }
}

/// A validated `stream` plan.
///
/// The **streaming configuration** is resolved here, during argument parsing,
/// which is what makes "a rejected configuration fails before any camera
/// connection" true: `run` has not opened a session yet.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamArgs {
    config: StreamingConfig,
    calibration: CalibrationPolicy,
    output: OutputSetting,
    duration: Option<Duration>,
    sink: SinkSelection,
    synthetic_frame_rate: Option<f64>,
    synthetic_wedge_after: Option<usize>,
    synthetic_reset_fails: bool,
    queue_capacity: usize,
    config_path: Option<PathBuf>,
}

impl StreamArgs {
    /// The resolved configuration.
    #[must_use]
    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }

    /// The **calibration window** policy.
    #[must_use]
    pub fn calibration(&self) -> &CalibrationPolicy {
        &self.calibration
    }

    /// The **output setting** written at session start.
    #[must_use]
    pub fn output_setting(&self) -> OutputSetting {
        self.output
    }

    /// How long to run, or `None` to run until the camera stops producing.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// The sink the live view feed is published through.
    #[must_use]
    pub fn sink(&self) -> SinkSelection {
        self.sink
    }

    /// The synthetic source's frame rate, or `None` to use the camera.
    #[must_use]
    pub fn synthetic_frame_rate(&self) -> Option<f64> {
        self.synthetic_frame_rate
    }

    /// How many frames the synthetic source answers before it stops answering
    /// with `PTP Timeout`, or `None` for a source that never stops.
    #[must_use]
    pub fn synthetic_wedge_after(&self) -> Option<usize> {
        self.synthetic_wedge_after
    }

    /// Whether every simulated **recovery reset** fails.
    #[must_use]
    pub fn synthetic_reset_fails(&self) -> bool {
        self.synthetic_reset_fails
    }

    /// Frames the bounded capture queue holds.
    #[must_use]
    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    /// The configuration file that was read, if any.
    #[must_use]
    pub fn config_path(&self) -> Option<&std::path::Path> {
        self.config_path.as_deref()
    }
}

/// A validated `serve` invocation: where the control socket goes, and the plan
/// the feed runs.
///
/// The **streaming configuration** is resolved during parsing here too, so a
/// rejected configuration fails before a socket is bound and before any camera
/// connection.
#[derive(Clone, Debug, PartialEq)]
pub struct ServeArgs {
    socket: PathBuf,
    stream: StreamArgs,
}

impl ServeArgs {
    /// The **control socket** path, named by `--socket`.
    #[must_use]
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// The plan the feed runs on every start.
    #[must_use]
    pub fn stream(&self) -> &StreamArgs {
        &self.stream
    }
}

/// A validated `control` invocation: one command, sent to one socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlArgs {
    socket: PathBuf,
    request: ControlRequest,
}

impl ControlArgs {
    /// The **control socket** path, named by `--socket`.
    #[must_use]
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// The command to send.
    #[must_use]
    pub fn request(&self) -> ControlRequest {
        self.request
    }
}

/// A plan that cannot establish a frame size or a frame rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanRejection {
    /// The calibration window policy is meaningless.
    Calibration(CalibrationPolicyError),
    /// The measurement plan is meaningless.
    Measurement(MeasurementPlanError),
}

impl fmt::Display for PlanRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Calibration(error) => write!(formatter, "{error}"),
            Self::Measurement(error) => write!(formatter, "{error}"),
        }
    }
}

/// Why the arguments were refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    /// No subcommand was given.
    MissingCommand,
    /// The subcommand is not one this program has.
    UnknownCommand {
        /// The subcommand as typed.
        command: String,
    },
    /// The option is not one this program has.
    UnknownOption {
        /// The option as typed.
        option: String,
    },
    /// The option needs a value.
    MissingValue {
        /// The option as typed.
        option: String,
    },
    /// The value is not a number of the right kind.
    InvalidValue {
        /// The option as typed.
        option: String,
        /// The value as typed.
        value: String,
    },
    /// The value is outside the **output setting** allowlist.
    OutputRejected(OutputSettingError),
    /// The plan could not be built.
    PlanRejected(PlanRejection),
    /// The configuration file or an override could not be used. The message
    /// names the offending key.
    Configuration(ConfigError),
    /// A subcommand that needs it was given no `--socket`.
    MissingSocket,
    /// `control` was given no command word.
    MissingControlCommand,
    /// `control` was given more than one command word.
    TooManyControlCommands,
    /// The `control` subcommand was given a word that is not a control command.
    UnknownControl {
        /// The word as typed.
        command: String,
    },
    /// `--sink` was given a value that is not a sink.
    UnknownSink {
        /// The value as typed.
        value: String,
    },
}

impl fmt::Display for ArgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommand => {
                formatter.write_str("no command given; try `camera-cli --help`")
            }
            Self::UnknownCommand { command } => {
                write!(
                    formatter,
                    "unknown command `{command}`; try `camera-cli --help`"
                )
            }
            Self::UnknownOption { option } => {
                write!(
                    formatter,
                    "unknown option `{option}`; try `camera-cli --help`"
                )
            }
            Self::MissingValue { option } => write!(formatter, "`{option}` needs a value"),
            Self::InvalidValue { option, value } => {
                write!(formatter, "`{option}` does not accept `{value}`")
            }
            Self::OutputRejected(error) => write!(formatter, "{error}"),
            Self::PlanRejected(error) => write!(formatter, "{error}"),
            Self::Configuration(error) => write!(formatter, "{error}"),
            Self::MissingSocket => formatter.write_str(
                "`--socket <path>` is required: it is the local socket a `control` command \
                 connects to",
            ),
            Self::UnknownControl { command } => write!(
                formatter,
                "`{command}` is not a control command; use `start`, `stop`, `status` or \
                 `shutdown`"
            ),
            Self::MissingControlCommand => formatter
                .write_str("`control` needs a command: `start`, `stop`, `status` or `shutdown`"),
            Self::TooManyControlCommands => formatter
                .write_str("`control` takes one command; one connection carries one command"),
            Self::UnknownSink { value } => write!(
                formatter,
                "`--sink` is `{value}`, which is not a sink; use `{}` (the counting sink, which \
                 publishes nothing) or `{}` (the platform backend)",
                SinkSelection::Stats,
                SinkSelection::VirtualCamera
            ),
        }
    }
}

impl error::Error for ArgError {}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// Returns [`ArgError`] for an unknown command or option, a missing or unparsable
/// value, a refused **output setting**, or a plan that could not work.
pub fn parse<I>(arguments: I) -> Result<Command, ArgError>
where
    I: IntoIterator<Item = String>,
{
    parse_for_platform(arguments, Platform::current())
}

/// Parses the arguments against a named platform.
///
/// The platform is a parameter so the mechanism check can be tested for more than
/// the machine the tests happen to run on.
///
/// # Errors
///
/// Returns [`ArgError`] for anything [`parse`] rejects.
pub fn parse_for_platform<I>(arguments: I, platform: Platform) -> Result<Command, ArgError>
where
    I: IntoIterator<Item = String>,
{
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Err(ArgError::MissingCommand);
    };

    match command.as_str() {
        "--help" | "-h" | "help" => Ok(Command::Help),
        "measure" => parse_measure(&mut arguments),
        "stream" => parse_stream(&mut arguments, platform),
        "serve" => parse_serve(&mut arguments, platform),
        "control" => parse_control(&mut arguments),
        other => Err(ArgError::UnknownCommand {
            command: other.to_owned(),
        }),
    }
}

fn parse_measure<I>(arguments: &mut I) -> Result<Command, ArgError>
where
    I: Iterator<Item = String>,
{
    let defaults = MeasurementPlan::default();
    let mut output = defaults.output_setting();
    let mut window_frames = defaults.calibration().window_frames();
    let mut window_duration = defaults.calibration().window_duration();
    let mut min_recurrences = defaults.calibration().min_recurrences();
    let mut sampling_frames = defaults.sampling_frames();

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--output" => {
                let value = take_value(arguments, &option)?;
                let raw = number::<u8>(&option, &value)?;
                output = OutputSetting::from_raw(raw).map_err(ArgError::OutputRejected)?;
            }
            "--window-frames" => {
                let value = take_value(arguments, &option)?;
                window_frames = number::<usize>(&option, &value)?;
            }
            "--window-duration-ms" => {
                let value = take_value(arguments, &option)?;
                window_duration = Duration::from_millis(number::<u64>(&option, &value)?);
            }
            "--min-recurrences" => {
                let value = take_value(arguments, &option)?;
                min_recurrences = number::<usize>(&option, &value)?;
            }
            "--sampling-frames" => {
                let value = take_value(arguments, &option)?;
                sampling_frames = number::<usize>(&option, &value)?;
            }
            other => {
                return Err(ArgError::UnknownOption {
                    option: other.to_owned(),
                })
            }
        }
    }

    let calibration = CalibrationPolicy::new(window_frames, window_duration, min_recurrences)
        .map_err(|error| ArgError::PlanRejected(PlanRejection::Calibration(error)))?;
    let plan = MeasurementPlan::new(output, calibration, sampling_frames)
        .map_err(|error| ArgError::PlanRejected(PlanRejection::Measurement(error)))?;

    Ok(Command::Measure(MeasureArgs { plan }))
}

fn take_value<I>(arguments: &mut I, option: &str) -> Result<String, ArgError>
where
    I: Iterator<Item = String>,
{
    arguments.next().ok_or_else(|| ArgError::MissingValue {
        option: option.to_owned(),
    })
}

/// Parses the `stream` options and resolves the **streaming configuration**.
///
/// Resolution happens here, not in `run`, so a rejected configuration never
/// reaches the camera.
fn parse_stream<I>(arguments: &mut I, platform: Platform) -> Result<Command, ArgError>
where
    I: Iterator<Item = String>,
{
    match parse_stream_options(arguments, platform)? {
        Some(stream) => Ok(Command::Stream(stream)),
        None => Ok(Command::Help),
    }
}

/// Parses the options `stream` and `serve` share.
///
/// `None` is `--help`, which both subcommands accept.
fn parse_stream_options<I>(
    arguments: &mut I,
    platform: Platform,
) -> Result<Option<StreamArgs>, ArgError>
where
    I: Iterator<Item = String>,
{
    let defaults = MeasurementPlan::default();
    let calibration_defaults = defaults.calibration().clone();

    let mut overrides = ConfigOverrides::default();
    let mut config_path: Option<PathBuf> = None;
    let mut output = defaults.output_setting();
    let mut window_frames = calibration_defaults.window_frames();
    let mut window_duration = calibration_defaults.window_duration();
    let mut min_recurrences = calibration_defaults.min_recurrences();
    let mut duration = None;
    let mut queue_capacity = camera_core::bounded_queue::DEFAULT_QUEUE_CAPACITY;
    let mut sink = DEFAULT_SINK;
    let mut synthetic_frame_rate = None;
    let mut synthetic_wedge_after = None;
    let mut synthetic_reset_fails = false;

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--help" | "-h" => return Ok(None),
            "--config" => {
                config_path = Some(PathBuf::from(take_value(arguments, &option)?));
            }
            "--resolution" => overrides.resolution = Some(take_value(arguments, &option)?),
            "--fps" => overrides.fps = Some(take_value(arguments, &option)?),
            "--pixel-format" => overrides.pixel_format = Some(take_value(arguments, &option)?),
            "--upscale" => overrides.upscale = Some(true),
            "--no-upscale" => overrides.upscale = Some(false),
            "--mechanism" => overrides.mechanism = Some(take_value(arguments, &option)?),
            "--sink" => {
                let value = take_value(arguments, &option)?;
                sink = SinkSelection::parse(&value).ok_or(ArgError::UnknownSink { value })?;
            }
            "--duration-secs" => {
                let value = take_value(arguments, &option)?;
                duration = Some(Duration::from_secs(number::<u64>(&option, &value)?));
            }
            "--queue-capacity" => {
                let value = take_value(arguments, &option)?;
                queue_capacity = number::<usize>(&option, &value)?;
            }
            "--synthetic" => {
                synthetic_frame_rate.get_or_insert(DEFAULT_SYNTHETIC_FRAME_RATE);
            }
            "--synthetic-fps" => {
                let value = take_value(arguments, &option)?;
                let rate = number::<f64>(&option, &value)?;
                if rate <= 0.0 {
                    return Err(ArgError::InvalidValue { option, value });
                }
                synthetic_frame_rate = Some(rate);
            }
            "--synthetic-wedge-after" => {
                let value = take_value(arguments, &option)?;
                let frames = number::<usize>(&option, &value)?;
                if frames == 0 {
                    return Err(ArgError::InvalidValue { option, value });
                }
                synthetic_wedge_after = Some(frames);
                synthetic_frame_rate.get_or_insert(DEFAULT_SYNTHETIC_FRAME_RATE);
            }
            "--synthetic-reset-fails" => {
                synthetic_reset_fails = true;
                synthetic_frame_rate.get_or_insert(DEFAULT_SYNTHETIC_FRAME_RATE);
            }
            "--output" => {
                let value = take_value(arguments, &option)?;
                let raw = number::<u8>(&option, &value)?;
                output = OutputSetting::from_raw(raw).map_err(ArgError::OutputRejected)?;
            }
            "--window-frames" => {
                let value = take_value(arguments, &option)?;
                window_frames = number::<usize>(&option, &value)?;
            }
            "--window-duration-ms" => {
                let value = take_value(arguments, &option)?;
                window_duration = Duration::from_millis(number::<u64>(&option, &value)?);
            }
            "--min-recurrences" => {
                let value = take_value(arguments, &option)?;
                min_recurrences = number::<usize>(&option, &value)?;
            }
            other => {
                return Err(ArgError::UnknownOption {
                    option: other.to_owned(),
                })
            }
        }
    }

    let calibration = CalibrationPolicy::new(window_frames, window_duration, min_recurrences)
        .map_err(|error| ArgError::PlanRejected(PlanRejection::Calibration(error)))?;

    let config = match &config_path {
        Some(path) => StreamingConfig::load(path, overrides, platform),
        None => StreamingConfig::resolve(None, overrides, platform),
    }
    .map_err(ArgError::Configuration)?;

    Ok(Some(StreamArgs {
        config,
        calibration,
        output,
        duration,
        sink,
        synthetic_frame_rate,
        synthetic_wedge_after,
        synthetic_reset_fails,
        queue_capacity,
        config_path,
    }))
}

/// Parses `serve`, which is `stream`'s options plus the **control socket** path.
///
/// `--socket` is removed from the argument list before the shared options are
/// parsed, because it is the one option the two subcommands do not share.
fn parse_serve<I>(arguments: &mut I, platform: Platform) -> Result<Command, ArgError>
where
    I: Iterator<Item = String>,
{
    let mut socket: Option<PathBuf> = None;
    let mut remaining = Vec::new();

    while let Some(option) = arguments.next() {
        if option == "--socket" {
            socket = Some(PathBuf::from(take_value(arguments, &option)?));
        } else {
            remaining.push(option);
        }
    }

    let mut remaining = remaining.into_iter();
    let Some(stream) = parse_stream_options(&mut remaining, platform)? else {
        return Ok(Command::Help);
    };
    let socket = socket.ok_or(ArgError::MissingSocket)?;

    Ok(Command::Serve(ServeArgs { socket, stream }))
}

/// Parses `control`, which is one command word and the socket to send it to.
fn parse_control<I>(arguments: &mut I) -> Result<Command, ArgError>
where
    I: Iterator<Item = String>,
{
    let mut socket: Option<PathBuf> = None;
    let mut requests: Vec<ControlRequest> = Vec::new();

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--socket" => socket = Some(PathBuf::from(take_value(arguments, &option)?)),
            other => {
                let parsed =
                    ControlRequest::parse(other).ok_or_else(|| ArgError::UnknownControl {
                        command: other.to_owned(),
                    })?;
                requests.push(parsed);
            }
        }
    }

    let socket = socket.ok_or(ArgError::MissingSocket)?;
    let mut requests = requests.into_iter();
    let request = requests.next().ok_or(ArgError::MissingControlCommand)?;
    if requests.next().is_some() {
        // One connection carries one command, so this is a usage error rather
        // than a queue.
        return Err(ArgError::TooManyControlCommands);
    }

    Ok(Command::Control(ControlArgs { socket, request }))
}

fn number<T>(option: &str, value: &str) -> Result<T, ArgError>
where
    T: FromStr,
{
    value.parse().map_err(|_| ArgError::InvalidValue {
        option: option.to_owned(),
        value: value.to_owned(),
    })
}
