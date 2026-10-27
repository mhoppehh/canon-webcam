//! `serve` and `control` as a user and a script run them.
//!
//! The feed is the synthetic source, so no camera is needed: the point of these
//! tests is the contract a script sees — the word printed, the exit status, and
//! that the socket outlives many commands and goes away on `shutdown`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use camera_core::control::socket;
use camera_core::control::ControlRequest;

/// `control status` exits with this when the feed is stopped.
const EXIT_CODE_STOPPED: i32 = 3;

fn socket_path(name: &str) -> PathBuf {
    // `/tmp` rather than the per-user temporary directory: a Unix socket path is
    // limited to about a hundred bytes, and the macOS temporary path is long.
    PathBuf::from("/tmp").join(format!("cw-cli-control-{}-{name}.sock", std::process::id()))
}

fn control(socket: &Path, command: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_camera-cli"))
        .args([
            "control",
            "--socket",
            &socket.display().to_string(),
            command,
        ])
        .output()
        .expect("the binary must run")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

/// A `serve` process, killed when the test ends however it ends.
struct Serve {
    child: Child,
    socket: PathBuf,
}

impl Serve {
    fn start(name: &str) -> Self {
        Self::start_with(name, &["--synthetic", "--synthetic-fps", "60"])
    }

    /// Starts `serve` with `extra` on top of the socket path.
    fn start_with(name: &str, extra: &[&str]) -> Self {
        let socket = socket_path(name);
        let _ = std::fs::remove_file(&socket);

        let mut arguments: Vec<String> = vec![
            "serve".to_owned(),
            "--socket".to_owned(),
            socket.display().to_string(),
        ];
        arguments.extend(extra.iter().map(|argument| (*argument).to_owned()));

        let child = Command::new(env!("CARGO_BIN_EXE_camera-cli"))
            .args(&arguments)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("serve must start");

        let serve = Self { child, socket };
        serve.wait_until_answering();
        serve
    }

    /// Waits until the control socket answers, failing rather than hanging.
    fn wait_until_answering(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if socket::send_command(&self.socket, ControlRequest::Status, Duration::from_secs(1))
                .is_ok()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("`serve` never answered on {}", self.socket.display());
    }

    /// Waits for the process to exit, and reports its status.
    fn wait_for_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait().expect("the child must be waitable") {
                Some(status) => return Some(status),
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        None
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[test]
fn a_script_can_check_start_and_stop_the_feed() {
    let serve = Serve::start("cycle");
    let socket = serve.socket.clone();

    let status = control(&socket, "status");
    assert_eq!(stdout_of(&status), "stopped");
    assert_eq!(
        status.status.code(),
        Some(EXIT_CODE_STOPPED),
        "a stopped feed must be reportable in the exit status"
    );

    let started = control(&socket, "start");
    assert!(
        started.status.success(),
        "start failed: {}",
        stderr_of(&started)
    );
    assert_eq!(stdout_of(&started), "started");

    let status = control(&socket, "status");
    assert_eq!(stdout_of(&status), "running");
    assert_eq!(status.status.code(), Some(0));

    // Harmless: starting a running feed changes nothing and says so.
    let again = control(&socket, "start");
    assert!(again.status.success());
    assert_eq!(stdout_of(&again), "already-running");

    let stopped = control(&socket, "stop");
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        stderr_of(&stopped)
    );
    assert_eq!(stdout_of(&stopped), "stopped");

    let status = control(&socket, "status");
    assert_eq!(stdout_of(&status), "stopped");
    assert_eq!(status.status.code(), Some(EXIT_CODE_STOPPED));

    // Harmless the other way round.
    let stopped_again = control(&socket, "stop");
    assert!(stopped_again.status.success());
    assert_eq!(stdout_of(&stopped_again), "already-stopped");

    // A second start after a stop works, without a new process.
    assert_eq!(stdout_of(&control(&socket, "start")), "started");
    assert_eq!(stdout_of(&control(&socket, "status")), "running");
    assert_eq!(stdout_of(&control(&socket, "stop")), "stopped");

    let mut serve = serve;
    assert_eq!(stdout_of(&control(&socket, "shutdown")), "shutting-down");
    let status = serve
        .wait_for_exit(Duration::from_secs(10))
        .expect("`serve` must exit after `shutdown`");
    assert!(status.success(), "`serve` exited with {status}");
    assert!(
        !socket.exists(),
        "the socket file must be removed on shutdown"
    );
}

#[test]
fn a_start_builds_the_platform_sink_through_the_same_path_a_stream_run_uses() {
    // The configuration points the macOS mechanism at an OBS that is not there,
    // which is the one platform-sink failure this machine can produce. A start
    // must reach it: if `serve` built a different sink, or fell back to the
    // counting sink, the feed would run and say nothing.
    let config =
        PathBuf::from("/tmp").join(format!("cw-cli-control-macos-{}.toml", std::process::id()));
    let missing = "/tmp/cw-cli-there-is-no-obs-here/OBS.app";
    std::fs::write(
        &config,
        format!("[macos]\nobs_app_path = \"{missing}\"\nlaunch_obs = false\n"),
    )
    .expect("the configuration file must be writable");

    let serve = Serve::start_with(
        "platform-sink",
        &[
            "--sink",
            "virtual-camera",
            "--config",
            &config.display().to_string(),
            "--synthetic",
            "--synthetic-fps",
            "60",
            "--window-frames",
            "8",
            "--window-duration-ms",
            "500",
            "--min-recurrences",
            "2",
        ],
    );
    let socket = serve.socket.clone();

    let started = control(&socket, "start");
    assert_eq!(stdout_of(&started), "started");

    // The run ends on its own. The surface must say why, not only that it is not
    // running, which is what makes the actionable message reachable through the
    // control socket.
    let detail = wait_for_failure(&socket);
    assert!(
        detail.contains("Install OBS Studio"),
        "the reason must be the OBS prerequisite's own message, was {detail:?}"
    );
    assert!(
        detail.contains(missing),
        "the configured path must be the path that was checked, was {detail:?}"
    );

    // A failed run released everything, so a stop has nothing left to release.
    assert_eq!(stdout_of(&control(&socket, "stop")), "already-stopped");

    let _ = std::fs::remove_file(&config);
}

/// Polls `control status` until the feed reports why it is not running.
///
/// Bounded: a feed that never reports a reason fails the test rather than
/// waiting for ever.
fn wait_for_failure(socket: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let output = control(socket, "status");
        if !output.status.success() {
            let detail = stderr_of(&output);
            if detail.contains("error: ") {
                return detail;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the feed never reported why it stopped");
}

#[test]
fn an_unknown_control_command_is_refused_without_a_connection() {
    let output = control(Path::new("/tmp/does-not-exist.sock"), "restart");

    assert!(!output.status.success());
    assert!(
        stderr_of(&output).contains("restart"),
        "the refusal must name the command, stderr was {:?}",
        stderr_of(&output)
    );
    assert!(stdout_of(&output).is_empty());
}

#[test]
fn serve_without_a_socket_path_says_which_option_is_missing() {
    let output = Command::new(env!("CARGO_BIN_EXE_camera-cli"))
        .args(["serve", "--synthetic"])
        .output()
        .expect("the binary must run");

    assert!(!output.status.success());
    assert!(
        stderr_of(&output).contains("--socket"),
        "the refusal must name the option, stderr was {:?}",
        stderr_of(&output)
    );
}

#[test]
fn a_control_command_that_cannot_connect_fails_rather_than_hanging() {
    let began = Instant::now();
    let output = control(Path::new("/tmp/does-not-exist-either.sock"), "status");
    let elapsed = began.elapsed();

    assert!(!output.status.success());
    assert!(
        elapsed < Duration::from_secs(5),
        "a missing socket must fail at once, took {elapsed:?}"
    );
    assert!(
        stderr_of(&output).contains("control socket"),
        "the error must say what failed, stderr was {:?}",
        stderr_of(&output)
    );
}
