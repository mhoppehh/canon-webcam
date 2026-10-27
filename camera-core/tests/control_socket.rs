//! The local socket front end, driven the way a script drives it.
//!
//! These tests are about the surface, not the state machine: one word in, one
//! word out, `already-*` answers for harmless requests, a shutdown that stops the
//! feed and takes the socket with it, and a client that connects and says nothing
//! not being able to stall everything else. That last one is the trap the ADR
//! records — an accepted socket on macOS inherits `O_NONBLOCK`, so a blocking read
//! needs `set_nonblocking(false)` and every wait needs a bound — and the test
//! proves the surface survives a silent client rather than assuming it does.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use camera_core::calibration::CalibrationPolicy;
use camera_core::control::socket::{self, ControlServer};
use camera_core::control::{ControlReply, ControlRequest, FeedController, FeedFactory, FeedRun};
use camera_core::output_setting::OutputSetting;
use camera_core::recovery::{Recovery, RecoveryReset, RecoveryResetError};
use camera_core::runner::StreamPlan;
use camera_core::stream::StreamPipelineConfig;
use camera_core::synthetic::SharedLog;
use support::control::{wait_until, CountingOpener, RecordingSink, SessionCounts, SinkLog};

/// A path under `/tmp` short enough for the socket address limit.
///
/// The macOS temporary directory is per-user and long, and a Unix socket path is
/// limited to about a hundred bytes, so the path is built here rather than taken
/// from `temp_dir`.
fn socket_path(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
    PathBuf::from("/tmp").join(format!(
        "cw-control-{}-{name}-{unique}.sock",
        std::process::id()
    ))
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

fn plan() -> StreamPlan {
    StreamPlan::new(
        OutputSetting::tft_plus_pc(),
        StreamPipelineConfig::new(
            CalibrationPolicy::new(20, Duration::from_secs(30), 8).expect("valid policy"),
        ),
    )
}

/// A control surface on its own thread, with the counters behind it.
struct Fixture {
    path: PathBuf,
    counts: SessionCounts,
    closed: Receiver<bool>,
    handle: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start(client_timeout: Duration) -> Self {
        let counts = SessionCounts::new();
        let sessions = SharedLog::default();
        let sink_log = Arc::new(Mutex::new(SinkLog::default()));

        let opener = CountingOpener::new(counts.clone(), sessions).at_rate(200.0);
        let sink_factory = Arc::clone(&sink_log);
        let factory: FeedFactory = Arc::new(move || {
            FeedRun::new(
                Box::new(opener.clone()),
                RecordingSink::factory(Arc::clone(&sink_factory)),
                Recovery::new(Box::new(NoRecoveryReset)),
            )
        });

        let path = socket_path("control");
        let server = ControlServer::bind(&path, FeedController::new(plan(), factory))
            .expect("the control socket must bind")
            .with_client_timeout(client_timeout);

        let (closed, closed_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let result = server.serve_until_shutdown();
            let _ = closed.send(result.is_ok());
        });

        Self {
            path,
            counts,
            closed: closed_rx,
            handle: Some(handle),
        }
    }

    fn send(&self, request: ControlRequest) -> ControlReply {
        socket::send_command(&self.path, request, Duration::from_secs(3))
            .expect("the control surface must answer")
    }

    /// Waits for `count` sessions to be alive.
    ///
    /// A start answers as soon as the run has its own thread, so the session is
    /// taken a moment later; this is the wait, not an assertion that the thread
    /// has already got there.
    fn wait_for_live_sessions(&self, count: usize) {
        wait_until(
            Duration::from_secs(5),
            &format!("{count} live sessions"),
            || self.counts.live() == count,
        );
    }

    /// Sends one raw line and returns the raw answer, for the malformed cases.
    fn send_raw(&self, line: &str) -> String {
        let mut stream = UnixStream::connect(&self.path).expect("the surface must be there");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("the timeout must be settable");
        stream
            .write_all(format!("{line}\n").as_bytes())
            .expect("the command must be sent");

        let mut answer = String::new();
        BufReader::new(stream)
            .read_line(&mut answer)
            .expect("the surface must answer");
        answer.trim().to_owned()
    }

    /// Waits for the surface to close, and reports whether it closed cleanly.
    fn wait_for_close(&mut self, timeout: Duration) -> bool {
        let Ok(clean) = self.closed.recv_timeout(timeout) else {
            return false;
        };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        clean
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Never leave a thread and a socket file behind, even when an assertion
        // above failed.
        let _ = socket::send_command(&self.path, ControlRequest::Shutdown, Duration::from_secs(3));
        let _ = self.wait_for_close(Duration::from_secs(5));
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn a_status_query_reports_the_feed_and_changes_nothing() {
    let fixture = Fixture::start(Duration::from_secs(1));

    assert_eq!(fixture.send(ControlRequest::Status), ControlReply::Stopped);
    assert_eq!(fixture.counts.opened(), 0, "asking must not take a session");

    assert_eq!(fixture.send(ControlRequest::Start), ControlReply::Started);
    fixture.wait_for_live_sessions(1);
    assert_eq!(fixture.send(ControlRequest::Status), ControlReply::Running);

    assert_eq!(
        fixture.send(ControlRequest::Start),
        ControlReply::AlreadyRunning,
        "starting a running feed is harmless"
    );
    assert_eq!(
        fixture.counts.opened(),
        1,
        "and takes no second PTP session"
    );

    assert_eq!(fixture.send(ControlRequest::Stop), ControlReply::Stopped);
    assert_eq!(fixture.counts.live(), 0, "a stop releases the session");
    assert_eq!(fixture.send(ControlRequest::Status), ControlReply::Stopped);
    assert_eq!(
        fixture.send(ControlRequest::Stop),
        ControlReply::AlreadyStopped,
        "stopping a stopped feed is harmless"
    );

    // A word that is not part of the protocol is answered, not ignored, so a
    // script that spells a command wrong learns why instead of waiting.
    let answer = fixture.send_raw("restart");
    assert!(
        answer.starts_with("error: ") && answer.contains("restart"),
        "the answer must name the command that was not understood, was {answer:?}"
    );
}

#[test]
fn shutdown_stops_the_feed_and_takes_the_socket_with_it() {
    let mut fixture = Fixture::start(Duration::from_secs(1));

    assert_eq!(fixture.send(ControlRequest::Start), ControlReply::Started);
    fixture.wait_for_live_sessions(1);

    assert_eq!(
        fixture.send(ControlRequest::Shutdown),
        ControlReply::ShuttingDown
    );
    assert!(
        fixture.wait_for_close(Duration::from_secs(5)),
        "the surface must close when it is told to"
    );
    assert_eq!(
        fixture.counts.live(),
        0,
        "closing the surface must stop the feed and release the session"
    );
    assert!(
        !fixture.path.exists(),
        "the socket file must be removed, was {}",
        fixture.path.display()
    );
    assert!(
        UnixStream::connect(&fixture.path).is_err(),
        "nothing must answer on the path afterwards"
    );
}

#[test]
fn a_client_that_connects_and_says_nothing_cannot_stall_the_surface() {
    // A three-hundred-millisecond client timeout, so the test proves the bound
    // rather than waiting for the default second.
    let fixture = Fixture::start(Duration::from_millis(300));

    let silent = UnixStream::connect(&fixture.path).expect("the surface must accept");
    silent
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("the timeout must be settable");

    // The surface is now blocked reading from a client that will say nothing, and
    // the accepted socket must still have a working read timeout rather than
    // spinning on WouldBlock (the `O_NONBLOCK` inheritance trap).
    thread::sleep(Duration::from_millis(50));

    let began = Instant::now();
    let reply = socket::send_command(
        &fixture.path,
        ControlRequest::Status,
        Duration::from_secs(3),
    )
    .expect("a second caller must still be answered");
    let waited = began.elapsed();

    assert_eq!(reply, ControlReply::Stopped);
    assert!(
        waited < Duration::from_secs(2),
        "a silent client must be dropped after the client timeout, not held for ever; \
         the second caller waited {waited:?}"
    );

    // And the silent client is told why, rather than left hanging.
    let mut answer = String::new();
    BufReader::new(silent)
        .read_line(&mut answer)
        .expect("the silent client must be answered");
    assert!(
        answer.starts_with("error: "),
        "the silent client must be told no command arrived, was {answer:?}"
    );

    assert_eq!(fixture.counts.live(), 0);
    assert_eq!(fixture.send(ControlRequest::Status), ControlReply::Stopped);
}

#[test]
fn the_state_it_reports_matches_the_controller_it_serves() {
    let fixture = Fixture::start(Duration::from_secs(1));

    // `started` then `stopped` is what a script sees; the controller agrees.
    assert_eq!(fixture.send(ControlRequest::Start), ControlReply::Started);
    assert_eq!(fixture.send(ControlRequest::Status), ControlReply::Running);
    assert_eq!(fixture.send(ControlRequest::Stop), ControlReply::Stopped);
    assert_eq!(fixture.send(ControlRequest::Status), ControlReply::Stopped);

    // Two starts in a row over the socket take one session each, not two.
    assert_eq!(fixture.send(ControlRequest::Start), ControlReply::Started);
    assert_eq!(fixture.send(ControlRequest::Stop), ControlReply::Stopped);
    assert_eq!(
        fixture.counts.opened(),
        2,
        "two starts take two sessions, one after the other"
    );
    assert_eq!(fixture.counts.live(), 0);
}
