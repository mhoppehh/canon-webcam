//! Ten start/stop cycles in a row, and nothing is left behind.
//!
//! This is the leak test the ticket asks for, and it is the only test in this
//! binary: the thread count is read from `ps`, which counts the threads of the
//! whole process, so a second test running beside this one would be counted too.
//! One test per binary is what makes the measurement mean what it says.
//!
//! Three things are asserted: no **PTP session** is alive afterwards, every
//! **sink** that was opened was closed, and neither the open file descriptors
//! (`/dev/fd`) nor the thread count grew. The first two are exact counters; the
//! last two are read from the operating system, because a leaked fd or thread is
//! exactly the kind of thing an in-process counter cannot see.

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camera_core::calibration::CalibrationPolicy;
use camera_core::control::{FeedChange, FeedController, FeedFactory, FeedRun};
use camera_core::output_setting::OutputSetting;
use camera_core::recovery::{Recovery, RecoveryReset, RecoveryResetError};
use camera_core::runner::StreamPlan;
use camera_core::stream::StreamPipelineConfig;
use camera_core::synthetic::SharedLog;
use support::control::{wait_until, CountingOpener, RecordingSink, SessionCounts, SinkLog};

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

/// How many file descriptors this process has open.
///
/// `/dev/fd` on macOS is the file descriptor table, so counting its entries counts
/// what the process holds. It is read twice — once before and once after the
/// cycles — and the test only ever asserts that it did not grow.
fn open_file_descriptors() -> Option<usize> {
    let path = Path::new("/dev/fd");
    if !path.exists() {
        return None;
    }
    std::fs::read_dir(path).ok().map(|entries| entries.count())
}

/// How many threads this process has.
///
/// `ps -M` prints one line per thread plus a header. It is read through a child
/// process rather than through `libc`, because the workspace denies unsafe code.
/// `None` when `ps` is not the shape this expects, so a different `ps` makes the
/// test skip the assertion rather than fail for the wrong reason.
fn open_threads() -> Option<usize> {
    let output = Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let lines = text.lines().count();
    lines.checked_sub(1)
}

#[test]
fn ten_start_stop_cycles_leak_no_session_thread_or_file_descriptor() {
    let counts = SessionCounts::new();
    let sessions = SharedLog::default();
    let sink_log = Arc::new(Mutex::new(SinkLog::default()));

    // Fast enough that ten cycles take well under a second, and with the
    // calibrated frame size established inside the first few dozen frames.
    let opener = CountingOpener::new(counts.clone(), Arc::clone(&sessions)).at_rate(400.0);
    let sink_factory = Arc::clone(&sink_log);
    let factory: FeedFactory = Arc::new(move || {
        FeedRun::new(
            Box::new(opener.clone()),
            RecordingSink::factory(Arc::clone(&sink_factory)),
            Recovery::new(Box::new(NoRecoveryReset)),
        )
    });

    let plan = StreamPlan::new(
        OutputSetting::tft_plus_pc(),
        StreamPipelineConfig::new(
            CalibrationPolicy::new(20, Duration::from_secs(30), 8).expect("valid policy"),
        ),
    );
    let mut controller = FeedController::new(plan, factory);

    // One warm-up cycle first. Whatever the first start allocates once — a
    // thread-local, a lazily initialised allocator arena — is allocated before the
    // measurement rather than counted as a leak.
    controller.start().expect("the warm-up start must work");
    wait_until(Duration::from_secs(5), "a warm-up frame", || {
        !RecordingSink::read(&sink_log).accepted.is_empty()
    });
    controller.stop().expect("the warm-up stop must work");

    let opened_before = counts.opened();
    let sink_opens_before = RecordingSink::read(&sink_log).opens.len();
    let sink_closes_before = RecordingSink::read(&sink_log).closes;
    let descriptors_before = open_file_descriptors();
    let threads_before = open_threads();

    for cycle in 1..=10 {
        assert_eq!(
            controller.start().expect("each start must work"),
            FeedChange::Started,
            "cycle {cycle} did not start"
        );

        let accepted_before = RecordingSink::read(&sink_log).accepted.len();
        wait_until(
            Duration::from_secs(5),
            &format!("a frame in cycle {cycle}"),
            || RecordingSink::read(&sink_log).accepted.len() > accepted_before,
        );

        assert_eq!(
            counts.opened(),
            opened_before + cycle,
            "cycle {cycle} must take exactly one PTP session"
        );
        assert_eq!(counts.live(), 1, "cycle {cycle} must hold one session");

        assert_eq!(
            controller.stop().expect("each stop must work"),
            FeedChange::Stopped,
            "cycle {cycle} did not stop"
        );
        assert_eq!(
            counts.live(),
            0,
            "cycle {cycle} left a PTP session alive, so the camera is still held"
        );
    }

    let sink = RecordingSink::read(&sink_log);
    assert_eq!(
        sink.opens.len(),
        sink_opens_before + 10,
        "every cycle opens a sink"
    );
    assert_eq!(
        sink.closes,
        sink_closes_before + 10,
        "and every sink that was opened was closed"
    );
    assert_eq!(sink.open_sinks, 0, "no sink was left open");

    assert_eq!(counts.opened(), opened_before + 10);
    assert_eq!(counts.live(), 0, "no PTP session survived the cycles");
    assert_eq!(controller.state(), camera_core::control::FeedState::Stopped);

    if let (Some(before), Some(after)) = (descriptors_before, open_file_descriptors()) {
        assert!(
            after <= before,
            "ten cycles leaked file descriptors: {before} before, {after} after"
        );
    }
    if let (Some(before), Some(after)) = (threads_before, open_threads()) {
        assert!(
            after <= before,
            "ten cycles leaked threads: {before} before, {after} after"
        );
    }
}
