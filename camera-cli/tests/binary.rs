//! The command as a user runs it: argument rejection happens before any camera
//! is opened, so these tests are safe on a machine with no camera attached.

use std::process::{Command, Output};

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_camera-cli"))
        .args(arguments)
        .output()
        .expect("the binary must run")
}

#[test]
fn the_output_guard_rejects_mobile2_at_the_command_line() {
    let output = run(&["measure", "--output", "8"]);

    assert!(!output.status.success(), "value 8 must not be accepted");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MOBILE2"),
        "the refusal must name the value, stderr was {stderr:?}"
    );
}

#[test]
fn a_window_that_can_never_establish_a_size_never_reaches_the_camera() {
    let output = run(&["measure", "--window-frames", "3", "--min-recurrences", "5"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("recur"),
        "the refusal must explain the threshold, stderr was {stderr:?}"
    );
}

#[test]
fn help_names_the_measure_subcommand() {
    let output = run(&["--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("measure"), "stdout was {stdout:?}");
    assert!(
        stdout.contains("live view"),
        "help must say what is measured, stdout was {stdout:?}"
    );
}

#[test]
fn help_states_that_liveviewsize_is_not_used() {
    let output = run(&["--help"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("liveviewsize"),
        "the help text must record that liveviewsize is a no-op on Canon, stdout was {stdout:?}"
    );
}

#[test]
fn an_unknown_command_fails_without_a_usage_panic() {
    let output = run(&["capture"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("capture"));
}

#[test]
fn a_mechanism_that_does_not_exist_on_this_platform_never_reaches_the_camera() {
    // `v4l2` is a Linux mechanism: naming it on macOS is a configuration error,
    // never a silent fallback.
    let output = run(&["stream", "--mechanism", "v4l2"]);

    assert!(
        !output.status.success(),
        "v4l2 must not be accepted on macOS"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mechanism"),
        "the refusal must name the offending key, stderr was {stderr:?}"
    );
}

#[test]
fn a_stream_run_reports_the_measured_frame_size_and_not_the_requested_one() {
    // The calibration window is shortened so this test stays quick; the
    // synthetic source is used because no camera is attached.
    let output = run(&[
        "stream",
        "--synthetic",
        "--resolution",
        "1080p",
        "--window-frames",
        "8",
        "--window-duration-ms",
        "500",
        "--min-recurrences",
        "2",
        "--duration-secs",
        "1",
    ]);

    assert!(output.status.success(), "the synthetic run must succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("measured frame size: 1024x576"),
        "the measured frame size must come from the frames, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("requested size: 1920x1080"),
        "the request must still be reported, labelled as a preference, stdout was {stdout:?}"
    );
    assert!(
        !stdout.contains("measured frame size: 1920x1080"),
        "the request must never be echoed as the result, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("virtual camera mechanism: obs-virtual-camera"),
        "the mechanism in use must be printed, stdout was {stdout:?}"
    );
}

/// The options that keep a wedged-camera demonstration quick and hardware-free.
fn wedged_stream(arguments: &[&str]) -> Output {
    let mut full = vec![
        "stream",
        "--synthetic",
        "--synthetic-fps",
        "500",
        "--synthetic-wedge-after",
        "30",
        "--window-frames",
        "8",
        "--window-duration-ms",
        "500",
        "--min-recurrences",
        "2",
        "--duration-secs",
        "1",
    ];
    full.extend_from_slice(arguments);
    run(&full)
}

#[test]
fn a_wedged_camera_is_recovered_and_the_stream_resumes_in_the_same_process() {
    // The synthetic source stops answering with PTP Timeout after 30 frames,
    // which is the wedged camera of TEST-RESULTS.md §4.8. The run must recover
    // and continue to its planned duration rather than restarting or exiting.
    let output = wedged_stream(&[]);

    assert!(
        output.status.success(),
        "a recovered run must succeed, stderr was {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PTP sessions opened: 2"),
        "the wedged session must be replaced by one reopened session, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("recovery resets: 1 attempts, 1 wedged episodes, 1 recovered"),
        "the count and the outcome of the resets must be in the log, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("outcome: the live view stream ran for the planned duration"),
        "the stream must resume after the recovery, stdout was {stdout:?}"
    );
}

#[test]
fn a_permanently_wedged_camera_ends_the_command_with_the_cable_instruction() {
    // Every simulated recovery reset fails, so only a cable reconnect is left.
    let output = wedged_stream(&["--synthetic-reset-fails"]);

    assert!(
        !output.status.success(),
        "a camera that does not recover must end with a non-zero exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("remove and reconnect the USB cable"),
        "the failure must name the remedy, stderr was {stderr:?}"
    );
    assert!(
        stderr.contains("wedged"),
        "the failure must name the state, stderr was {stderr:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("recovery resets: 3 attempts"),
        "the logged attempt count must be the bounded number, stdout was {stdout:?}"
    );
    assert!(
        stdout.contains("the reset command failed"),
        "the log must say each attempt failed, stdout was {stdout:?}"
    );
}
