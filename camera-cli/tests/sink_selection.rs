//! Choosing the sink: the `--sink` flag, its default, and what a run does when
//! the **platform backend** cannot publish.
//!
//! No OBS and no camera are needed for any of this. The platform sink cannot be
//! opened on a machine without OBS in `/Applications`, so what is asserted is the
//! selection surface, the configuration plumbing and the refusal — never a
//! successful open.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use camera_cli::args::{parse, Command as Invocation};
use camera_cli::sink::{SinkPlan, SinkSelection, DEFAULT_SINK};
use camera_core::config::{ConfigOverrides, StreamingConfig};
use camera_core::mechanism::Platform;

/// How long any spawned run may take before the test kills it and fails.
///
/// A test that blocks for ever is worse than one that fails: it hides which fact
/// was never true.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

fn run_bounded(arguments: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_camera-cli"))
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary must run");

    let deadline = Instant::now() + RUN_TIMEOUT;
    loop {
        match child.try_wait().expect("the child must be waitable") {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the binary did not finish within {RUN_TIMEOUT:?}: {arguments:?}");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    child
        .wait_with_output()
        .expect("the finished child must yield its output")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The options that make a synthetic run calibrate and finish quickly.
fn quick_synthetic(arguments: &mut Vec<String>) {
    for option in [
        "--synthetic",
        "--window-frames",
        "8",
        "--window-duration-ms",
        "500",
        "--min-recurrences",
        "2",
    ] {
        arguments.push(option.to_owned());
    }
}

fn stream_sink(arguments: &[&str]) -> SinkSelection {
    let mut full = vec!["stream".to_owned()];
    full.extend(arguments.iter().map(|argument| (*argument).to_owned()));

    match parse(full).expect("the arguments must parse") {
        Invocation::Stream(stream) => stream.sink(),
        other => panic!("expected a stream plan, got {other:?}"),
    }
}

#[test]
fn the_flag_selects_the_platform_sink_and_the_default_is_the_counting_sink() {
    assert_eq!(
        stream_sink(&[]),
        DEFAULT_SINK,
        "a run that asks for nothing must not depend on OBS being installed"
    );
    assert_eq!(DEFAULT_SINK, SinkSelection::Stats);
    assert_eq!(stream_sink(&["--sink", "stats"]), SinkSelection::Stats);
    assert_eq!(
        stream_sink(&["--sink", "virtual-camera"]),
        SinkSelection::VirtualCamera,
        "the flag is the only way to ask for the platform sink"
    );

    // A value that is not a sink is refused, naming both that are.
    let mut full = vec!["stream".to_owned(), "--sink".to_owned()];
    full.push("obs-virtual-camera".to_owned());
    let error = parse(full).expect_err("a mechanism name is not a sink name");
    let message = error.to_string();
    assert!(message.contains("obs-virtual-camera"), "was {message}");
    assert!(message.contains("stats"), "was {message}");
    assert!(message.contains("virtual-camera"), "was {message}");
}

#[test]
fn the_help_text_states_the_default_sink() {
    let output = run_bounded(&["--help"]);

    assert!(output.status.success());
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("--sink <name>"),
        "the flag must be discoverable, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("(the default)"),
        "the default must be stated, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("`virtual-camera` publishes it through the platform"),
        "the platform sink must be described, stdout was {stdout:?}"
    );
}

#[test]
fn selecting_the_platform_sink_where_it_is_not_compiled_in_does_not_fall_back() {
    // The configuration resolves for macOS, so the sink choice is what carries
    // the refusal: on a platform with no backend the platform sink is an error,
    // never a quiet substitution of the counting sink.
    let config = StreamingConfig::resolve(None, ConfigOverrides::default(), Platform::MacOs)
        .expect("the macOS defaults must resolve");

    let error = SinkPlan::select_for(SinkSelection::VirtualCamera, &config, Platform::Linux)
        .expect_err("there is no Linux platform backend in this build");

    let message = error.to_string();
    assert!(message.contains("Linux"), "was {message}");
    assert!(
        message.contains("nowhere to go"),
        "the refusal must say the feed has nowhere to publish, was {message}"
    );

    assert!(
        SinkPlan::select_for(SinkSelection::Stats, &config, Platform::Linux).is_ok(),
        "the counting sink is available on every platform"
    );
}

#[test]
fn a_missing_obs_is_reported_with_the_configured_path_and_the_real_instruction() {
    // The configuration points the macOS mechanism at an OBS that is not there,
    // which is what makes the failure testable on this machine. If the path were
    // not read from the configuration the message would name the default.
    let directory = PathBuf::from("/tmp");
    let missing = directory.join(format!("cw-cli-no-obs-{}", std::process::id()));
    let config_path = directory.join(format!("cw-cli-sink-{}.toml", std::process::id()));
    std::fs::write(
        &config_path,
        format!(
            "[macos]\nobs_app_path = \"{}\"\nlaunch_obs = false\n",
            missing.join("OBS.app").display()
        ),
    )
    .expect("the configuration file must be writable");

    let mut arguments: Vec<String> = vec![
        "stream".to_owned(),
        "--sink".to_owned(),
        "virtual-camera".to_owned(),
        "--config".to_owned(),
        config_path.display().to_string(),
        "--duration-secs".to_owned(),
        "10".to_owned(),
    ];
    quick_synthetic(&mut arguments);
    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let output = run_bounded(&borrowed);
    let _ = std::fs::remove_file(&config_path);

    assert!(
        !output.status.success(),
        "the run must not pretend to publish, stdout was {:?}",
        stdout_of(&output)
    );
    let stderr = stderr_of(&output);
    assert!(
        stderr.contains(&missing.join("OBS.app").display().to_string()),
        "the refusal must name the configured path, stderr was {stderr:?}"
    );
    assert!(
        stderr.contains("Install OBS Studio"),
        "the refusal must be the prerequisite's own message, stderr was {stderr:?}"
    );
    assert!(
        stderr.contains("virtual camera sink failed"),
        "the failure must be reported as the sink's, stderr was {stderr:?}"
    );
    assert!(
        !stdout_of(&output).contains("sink: stats"),
        "the run must not report the counting sink it did not use, stdout was {:?}",
        stdout_of(&output)
    );
}

#[test]
fn a_run_report_names_the_counting_sink_that_published() {
    let mut arguments: Vec<String> = vec![
        "stream".to_owned(),
        "--duration-secs".to_owned(),
        "1".to_owned(),
    ];
    quick_synthetic(&mut arguments);
    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let output = run_bounded(&borrowed);

    assert!(
        output.status.success(),
        "the synthetic run must succeed, stderr was {:?}",
        stderr_of(&output)
    );
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("sink: stats (the counting sink; nothing is published"),
        "the report must name the sink that published, stdout was {stdout:?}"
    );
    assert!(
        !stdout.contains("sink frames published"),
        "the counting sink has no platform counters to report, stdout was {stdout:?}"
    );
}
