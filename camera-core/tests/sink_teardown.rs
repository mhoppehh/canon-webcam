//! The sink letting go when the camera does.
//!
//! ADR-0002's mechanism leaves the consumer application holding whatever the
//! sink last published, so a sink left open on a dead capture layer is worse than
//! a crash: the call keeps showing a frozen frame and nobody can tell it is dead.
//! These tests drive a **disconnected camera** through the runner and watch the
//! sink, which is the only place the difference between "closed" and "held" is
//! observable without OBS and a consumer application.
//!
//! What that proves: the sink closes as the cable goes rather than when the run
//! gives up; nothing reaches it after it closes; a camera that comes back is
//! recalibrated and the sink **re-opened** at whatever size that segment measures,
//! with no frame from before the reconnect crossing into it; a camera that does
//! not come back ends the run as disconnected; and the sink is closed on the error
//! paths too, including a capture thread that dies without reporting why.
//!
//! What it cannot prove: that OBS's own device entry disappears for a consumer
//! application within a few seconds. That needs OBS and Discord/Teams/Meet.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camera_core::calibration::CalibrationPolicy;
use camera_core::frame_size::FrameSize;
use camera_core::measurement::MonotonicClock;
use camera_core::output_setting::OutputSetting;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::recovery::{Recovery, RecoveryReset, RecoveryResetError};
use camera_core::runner::{run, run_recovering, StreamEnd, StreamPlan};
use camera_core::session::{PtpSession, SessionError};
use camera_core::sink::{FrameSink, SinkError};
use camera_core::stats::StreamStats;
use camera_core::stream::{StreamError, StreamPipeline, StreamPipelineConfig};
use camera_core::synthetic::{
    live_view_jpeg, ScriptedSessionOpener, SyntheticSession, FULL_FRAME_SIZE, REDUCED_FRAME_SIZE,
};

// ---------------------------------------------------------------------------
// A sink that records what it was asked to do
// ---------------------------------------------------------------------------

/// One thing that happened to the sink, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Opened(FrameSize),
    Accepted(FrameSize),
    Closed,
}

/// One entry of the sink's log, with the moment it happened.
///
/// The moment is what lets a test tell "closed when the cable went" from "closed
/// when the run finally gave up", which is the whole point of the ticket.
#[derive(Clone, Debug)]
struct Entry {
    event: Event,
    at: Instant,
}

/// A sink that writes every call into a shared log, so a test can ask what the
/// consumer application would have seen, and when.
///
/// Cloning shares the log and nothing else, which is what the runner's `open_sink`
/// closure needs: a fresh sink object per segment, one history for the whole run.
#[derive(Clone, Default)]
struct WatchingSink {
    log: Arc<Mutex<Vec<Entry>>>,
    open_at: Option<FrameSize>,
    /// Whether accepting a frame fails, which is a platform backend failure.
    refuse_frames: bool,
}

impl WatchingSink {
    fn new() -> Self {
        Self::default()
    }

    fn refusing_frames() -> Self {
        Self {
            refuse_frames: true,
            ..Self::default()
        }
    }

    fn entries(&self) -> Vec<Entry> {
        self.log.lock().expect("the event log").clone()
    }

    fn events(&self) -> Vec<Event> {
        self.entries()
            .into_iter()
            .map(|entry| entry.event)
            .collect()
    }

    fn record(&self, event: Event) {
        self.log.lock().expect("the event log").push(Entry {
            event,
            at: Instant::now(),
        });
    }

    /// The frame sizes the sink was opened at, in order.
    fn opens(&self) -> Vec<FrameSize> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Opened(size) => Some(size),
                _ => None,
            })
            .collect()
    }

    /// The frame size of every accepted frame, grouped by the segment that
    /// accepted it: one inner vector per open.
    ///
    /// A frame in the wrong group is a frame from before a reconnect that
    /// survived into the segment after it, which is exactly the stale frame this
    /// ticket forbids.
    fn accepted_by_segment(&self) -> Vec<Vec<FrameSize>> {
        let mut segments: Vec<Vec<FrameSize>> = Vec::new();
        for entry in self.entries() {
            match entry.event {
                Event::Opened(_) => segments.push(Vec::new()),
                Event::Accepted(size) => {
                    if let Some(segment) = segments.last_mut() {
                        segment.push(size);
                    }
                }
                Event::Closed => {}
            }
        }
        segments
    }

    /// How many frames were accepted in total.
    fn accepted(&self) -> usize {
        self.events()
            .into_iter()
            .filter(|event| matches!(event, Event::Accepted(_)))
            .count()
    }

    /// Whether a frame was accepted after the sink was last closed. A non-zero
    /// answer is a consumer application being fed by a camera that has gone.
    fn frames_after_close(&self) -> usize {
        let mut closed = false;
        let mut after = 0;
        for entry in self.entries() {
            match entry.event {
                Event::Closed => closed = true,
                Event::Opened(_) => closed = false,
                Event::Accepted(_) if closed => after += 1,
                Event::Accepted(_) => {}
            }
        }
        after
    }

    /// When the sink was last closed, if it ever was.
    fn closed_at(&self) -> Option<Instant> {
        self.entries()
            .into_iter()
            .rfind(|entry| matches!(entry.event, Event::Closed))
            .map(|entry| entry.at)
    }

    /// Whether the sink is closed at the end of the run: the last thing that
    /// happened was a close, or it never opened at all.
    fn ends_closed(&self) -> bool {
        match self.events().last() {
            Some(Event::Closed) => true,
            Some(_) => false,
            // A sink that never opened holds no device.
            None => true,
        }
    }
}

impl FrameSink for WatchingSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        _pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        self.record(Event::Opened(expected_frame_size));
        self.open_at = Some(expected_frame_size);
        Ok(())
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        let Some(expected) = self.open_at else {
            return Err(SinkError::NotOpen);
        };
        if expected != frame.frame_size() {
            return Err(SinkError::FrameSizeMismatch {
                expected,
                actual: frame.frame_size(),
            });
        }
        self.record(Event::Accepted(frame.frame_size()));
        if self.refuse_frames {
            return Err(SinkError::Backend {
                detail: "OBS stopped answering on its control channel".to_owned(),
            });
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), SinkError> {
        self.record(Event::Closed);
        self.open_at = None;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A **recovery reset** a disconnect test never expects to see.
struct UnusedReset {
    calls: Arc<AtomicUsize>,
}

impl RecoveryReset for UnusedReset {
    fn reset(&mut self) -> Result<(), RecoveryResetError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn command_line(&self) -> String {
        "gphoto2 --reset".to_owned()
    }
}

/// A **calibration window** that closes after eight frames, with two recurrences
/// enough to establish the **expected frame size**, so a test's arithmetic about
/// what the sink should have received stays readable.
fn calibration() -> CalibrationPolicy {
    CalibrationPolicy::new(8, Duration::from_secs(30), 2).expect("valid policy")
}

fn plan(run_for: Duration, reconnect_grace: Duration) -> StreamPlan {
    StreamPlan::new(
        OutputSetting::tft_plus_pc(),
        StreamPipelineConfig::new(calibration()),
    )
    .with_run_for(run_for)
    .with_reconnect_grace(reconnect_grace)
}

/// A session that produces `frame_size` and then goes away, as a pulled cable
/// does.
fn session_that_disconnects_after(frames: usize, frame_size: FrameSize) -> SyntheticSession {
    SyntheticSession::new(frame_size)
        .with_frame_period(Duration::from_millis(1))
        .failing_after(frames, SyntheticSession::disconnected())
}

fn session(frame_size: FrameSize) -> SyntheticSession {
    SyntheticSession::new(frame_size).with_frame_period(Duration::from_millis(1))
}

/// A **PTP session** that delivers `frames` frames and then unwinds.
///
/// It stands in for a capture thread that dies without reporting why, which is the
/// one way a capture thread can stop without closing the capture queue.
struct CollapsingSession {
    frame_size: FrameSize,
    frames: usize,
    captured: usize,
}

impl CollapsingSession {
    fn after(frames: usize, frame_size: FrameSize) -> Self {
        Self {
            frame_size,
            frames,
            captured: 0,
        }
    }
}

impl PtpSession for CollapsingSession {
    fn model(&self) -> &str {
        "Collapsing M50"
    }

    fn set_output_setting(&mut self, _setting: OutputSetting) -> Result<(), SessionError> {
        Ok(())
    }

    fn activate_viewfinder_action(&mut self) -> Result<(), SessionError> {
        Ok(())
    }

    fn capture_live_view_frame(&mut self) -> Result<Vec<u8>, SessionError> {
        if self.captured >= self.frames {
            panic!("the capture thread collapsed without reporting why");
        }
        self.captured += 1;
        let jpeg = live_view_jpeg(self.frame_size).expect("a measured frame at this size");
        Ok(jpeg.to_vec())
    }
}

// ---------------------------------------------------------------------------
// The sink lets go
// ---------------------------------------------------------------------------

#[test]
fn a_disconnect_closes_the_sink_before_the_run_reports_it() {
    let sink = WatchingSink::new();
    let mut opener =
        ScriptedSessionOpener::new([session_that_disconnects_after(12, REDUCED_FRAME_SIZE)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut recovery = Recovery::new(Box::new(UnusedReset {
        calls: Arc::clone(&calls),
    }));

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5), Duration::from_millis(40)),
        &mut || Box::new(sink.clone()),
        &MonotonicClock::new(),
        &mut recovery,
    )
    .expect("the run must complete");

    assert!(
        matches!(run.end, StreamEnd::DisconnectedCamera { .. }),
        "end was {:?}",
        run.end
    );
    assert!(
        sink.ends_closed(),
        "the last thing the sink must do is close; it did {:?}",
        sink.events()
    );
    assert_eq!(
        sink.frames_after_close(),
        0,
        "no frame may be delivered after the sink is closed"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a disconnected camera must not get a recovery reset"
    );
    assert_eq!(run.sessions_opened, 1);
}

#[test]
fn the_sink_closes_when_the_cable_goes_and_not_when_the_run_gives_up() {
    // A grace long enough that a sink closed only at the end of the run would be
    // plainly late. The consumer application must lose the device when the cable
    // is pulled, not up to a reconnect grace later: that wait is exactly the
    // frozen last frame this ticket exists to remove.
    let grace = Duration::from_millis(300);
    let sink = WatchingSink::new();
    let mut opener =
        ScriptedSessionOpener::new([session_that_disconnects_after(12, REDUCED_FRAME_SIZE)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut recovery = Recovery::new(Box::new(UnusedReset {
        calls: Arc::clone(&calls),
    }));

    let started = Instant::now();
    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5), grace),
        &mut || Box::new(sink.clone()),
        &MonotonicClock::new(),
        &mut recovery,
    )
    .expect("the run must complete");

    assert!(
        matches!(run.end, StreamEnd::DisconnectedCamera { .. }),
        "end was {:?}",
        run.end
    );

    let closed_at = sink.closed_at().expect("the sink must be closed");
    let closed_after = closed_at.duration_since(started);
    assert!(
        closed_after < grace,
        "the sink must close as the cable goes, not when the run gives up: it closed after \
         {closed_after:?}, with a grace of {grace:?}"
    );
}

#[test]
fn a_reconnecting_camera_is_recalibrated_and_the_sink_reopened_at_its_new_size() {
    let sink = WatchingSink::new();
    // The cable is pulled on the reduced size and pushed back in on the full one:
    // the second session measures a different size, so the re-open must take the
    // size that segment measured rather than the one the first segment used.
    let mut opener = ScriptedSessionOpener::new([
        session_that_disconnects_after(12, REDUCED_FRAME_SIZE),
        session(FULL_FRAME_SIZE),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut recovery = Recovery::new(Box::new(UnusedReset {
        calls: Arc::clone(&calls),
    }));

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_millis(300), Duration::from_secs(2)),
        &mut || Box::new(sink.clone()),
        &MonotonicClock::new(),
        &mut recovery,
    )
    .expect("the run must complete");

    assert_eq!(
        run.end,
        StreamEnd::DurationElapsed,
        "the run must resume and end on its own duration"
    );
    assert_eq!(
        sink.opens(),
        vec![REDUCED_FRAME_SIZE, FULL_FRAME_SIZE],
        "each segment opens the sink at the size it measured"
    );
    assert_eq!(
        run.sessions_opened, 2,
        "a reconnect is one more session, not a new process"
    );
    assert_eq!(
        run.measured_frame_size,
        Some(FULL_FRAME_SIZE),
        "the run reports the size it was delivering when it ended"
    );
    assert!(
        sink.ends_closed(),
        "the run ends with the sink closed; it did {:?}",
        sink.events()
    );
    assert_eq!(sink.frames_after_close(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // The frame size is the proof that nothing stale survived the reconnect: a
    // frame captured before the cable went could only reach the second segment at
    // the reduced size, which that segment never produced.
    let segments = sink.accepted_by_segment();
    assert_eq!(segments.len(), 2, "one sink open per segment");
    assert!(
        segments[0].iter().all(|size| *size == REDUCED_FRAME_SIZE),
        "the first segment accepted {:?}",
        segments[0]
    );
    assert!(
        !segments[1].is_empty(),
        "the live view feed must resume after the reconnect"
    );
    assert!(
        segments[1].iter().all(|size| *size == FULL_FRAME_SIZE),
        "the second segment accepted {:?}, which means a stale frame from before the reconnect \
         crossed into it",
        segments[1]
    );
}

#[test]
fn a_camera_that_never_comes_back_ends_the_run_as_disconnected() {
    let sink = WatchingSink::new();
    let mut opener =
        ScriptedSessionOpener::new([session_that_disconnects_after(12, REDUCED_FRAME_SIZE)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut recovery = Recovery::new(Box::new(UnusedReset {
        calls: Arc::clone(&calls),
    }));

    let grace = Duration::from_millis(80);
    let started = Instant::now();
    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5), grace),
        &mut || Box::new(sink.clone()),
        &MonotonicClock::new(),
        &mut recovery,
    )
    .expect("the run must complete");

    assert!(
        matches!(run.end, StreamEnd::DisconnectedCamera { .. }),
        "end was {:?}",
        run.end
    );
    assert!(
        started.elapsed() >= grace,
        "the run must wait the grace before giving up; it waited {:?}",
        started.elapsed()
    );
    assert!(
        sink.ends_closed(),
        "a run that gives up must still close the sink; it did {:?}",
        sink.events()
    );
    assert_eq!(run.sessions_opened, 1, "waiting is not a session");
}

// ---------------------------------------------------------------------------
// Nothing is delivered after the capture layer stops producing
// ---------------------------------------------------------------------------

#[test]
fn no_frame_is_delivered_after_the_capture_layer_stops_producing() {
    let sink = WatchingSink::new();
    let mut opener =
        ScriptedSessionOpener::new([session_that_disconnects_after(12, REDUCED_FRAME_SIZE)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut recovery = Recovery::new(Box::new(UnusedReset {
        calls: Arc::clone(&calls),
    }));

    let run = run_recovering(
        &mut opener,
        &plan(Duration::from_secs(5), Duration::from_millis(40)),
        &mut || Box::new(sink.clone()),
        &MonotonicClock::new(),
        &mut recovery,
    )
    .expect("the run must complete");

    assert!(matches!(run.end, StreamEnd::DisconnectedCamera { .. }));

    // Every accepted frame is a frame the run counted as delivered: the sink is
    // never handed a frame the pipeline did not account for, and no delivered
    // frame is handed over twice.
    let accepted = sink.accepted();
    assert!(accepted > 0, "the run never delivered anything to prove");
    assert_eq!(
        u64::try_from(accepted).expect("a small count"),
        run.stats.frames_delivered,
        "the sink accepted {accepted} frames and the run counted {} delivered",
        run.stats.frames_delivered
    );

    // Nothing was invented either: the sink never received more frames than the
    // camera produced.
    assert!(
        u64::try_from(accepted).expect("a small count") <= run.stats.frames_captured(),
        "{accepted} frames reached the sink but the camera produced only {}",
        run.stats.frames_captured()
    );

    // The accounting the run reports is exact, so a repeated frame would have to
    // show up here.
    assert_eq!(
        run.stats.frames_accounted(),
        run.stats.frames_received,
        "every frame the pipeline took is delivered, dropped, or observed by a calibration window"
    );

    // And the delivery stopped: the sink was closed after the last frame and never
    // handed another.
    assert_eq!(
        sink.frames_after_close(),
        0,
        "a frame reached the sink after it was closed: {:?}",
        sink.events()
    );
    assert!(sink.ends_closed());
}

// ---------------------------------------------------------------------------
// Every exit path closes the sink
// ---------------------------------------------------------------------------

#[test]
fn a_platform_backend_failure_still_closes_the_sink() {
    let sink = WatchingSink::refusing_frames();
    let mut session = session(REDUCED_FRAME_SIZE);

    let error = run(
        &mut session,
        &plan(Duration::from_secs(5), Duration::from_secs(1)),
        Box::new(sink.clone()),
        &MonotonicClock::new(),
    )
    .expect_err("a sink that refuses frames is a stream error");

    assert!(
        matches!(error, StreamError::Sink(SinkError::Backend { .. })),
        "error was {error:?}"
    );
    assert!(
        sink.ends_closed(),
        "an error path must still close the sink; it did {:?}",
        sink.events()
    );
    assert_eq!(sink.frames_after_close(), 0);
}

#[test]
fn a_capture_thread_that_dies_without_a_word_still_closes_the_sink() {
    // The queue is the only thing that carries news from the capture thread, and a
    // thread that unwinds never writes to it. Without the finished flag the
    // pipeline waits on a queue that can never fill again, holding the device for
    // ever; run with no duration, this test would hang rather than fail if that
    // protection were missing.
    let sink = WatchingSink::new();
    let mut session = CollapsingSession::after(12, REDUCED_FRAME_SIZE);

    let run = run(
        &mut session,
        &StreamPlan::new(
            OutputSetting::tft_plus_pc(),
            StreamPipelineConfig::new(calibration()),
        ),
        Box::new(sink.clone()),
        &MonotonicClock::new(),
    )
    .expect("a dead capture thread is reported, not raised");

    assert!(
        matches!(run.end, StreamEnd::CaptureFailed { .. }),
        "end was {:?}",
        run.end
    );
    assert!(
        sink.ends_closed(),
        "a crashed run must not leave the device held; the sink did {:?}",
        sink.events()
    );
    assert_eq!(sink.frames_after_close(), 0);
}

#[test]
fn a_pipeline_dropped_without_shutdown_closes_the_sink() {
    // The safety net behind the explicit close: a pipeline dropped on a path that
    // never reached `shutdown` — a panic between them, a caller that returned
    // early — must not leave the device held.
    let sink = WatchingSink::new();
    let jpeg = live_view_jpeg(REDUCED_FRAME_SIZE).expect("a measured frame");
    let mut pipeline = StreamPipeline::new(
        Box::new(sink.clone()),
        Arc::new(StreamStats::new()),
        StreamPipelineConfig::new(calibration()),
    );

    for _ in 0..calibration().window_frames() {
        pipeline
            .handle(jpeg, Duration::ZERO)
            .expect("the calibration window must settle");
    }
    assert!(
        pipeline.sink_frame_size().is_some(),
        "the pipeline never opened the sink"
    );

    drop(pipeline);

    assert_eq!(
        sink.events().last(),
        Some(&Event::Closed),
        "dropping an open pipeline must close the sink; it did {:?}",
        sink.events()
    );
}

#[test]
fn a_platform_backend_failure_reads_differently_from_either_camera_state() {
    let disconnected = StreamEnd::DisconnectedCamera {
        detail: "the camera is absent from the USB bus".to_owned(),
    };
    let wedged = StreamEnd::WedgedCamera {
        detail: "PTP Timeout".to_owned(),
    };
    let backend = StreamError::Sink(SinkError::Backend {
        detail: "OBS stopped answering on its control channel".to_owned(),
    });

    // Three states, three sentences: a user must be able to tell which of them
    // they are looking at, because the remedies differ.
    let messages = [
        disconnected.to_string(),
        wedged.to_string(),
        backend.to_string(),
    ];
    assert!(messages[0].contains("disconnected"), "{}", messages[0]);
    assert!(messages[1].contains("wedged"), "{}", messages[1]);
    assert!(messages[2].contains("sink failed"), "{}", messages[2]);
    for (index, message) in messages.iter().enumerate() {
        assert!(
            !messages
                .iter()
                .enumerate()
                .any(|(other, text)| other != index && text.contains(message.as_str())),
            "the messages must stay distinguishable: {message}"
        );
    }

    // The remedies differ too, which is the point of keeping the states apart.
    assert!(disconnected
        .remedy()
        .is_some_and(|text| text.contains("reconnect")));
    assert!(wedged.remedy().is_some_and(|text| text.contains("--reset")));
    assert_eq!(
        StreamError::Sink(SinkError::NotOpen).to_string(),
        "the virtual camera sink is not open"
    );
}
