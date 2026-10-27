//! Shared support for the start/stop control tests.
//!
//! Two things are needed to prove that a stop releases the camera and a start
//! takes it again, and neither may need hardware:
//!
//! - a **counting** session opener, which knows how many **PTP sessions** have
//!   been opened and how many are still alive. A session that is dropped without
//!   the count falling is exactly the leak this ticket is about, so the count is
//!   decremented in `Drop` rather than by a bookkeeping call a caller could
//!   forget;
//! - a recording **sink**, which records every open, every accepted frame size and
//!   every close in one shared log, so the log spans several starts.
//!
//! The sessions themselves are the real `SyntheticSession`, wrapped rather than
//! reimplemented: the frames are the committed JPEGs and the **leading frames**
//! and **frame sizes** are the measured ones.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camera_core::frame_size::FrameSize;
use camera_core::output_setting::OutputSetting;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::recovery::PtpSessionOpener;
use camera_core::session::{PtpSession, SessionError};
use camera_core::sink::{FrameSink, SinkError};
use camera_core::synthetic::{SharedLog, SyntheticSession, FULL_FRAME_SIZE};

/// How many **PTP sessions** were opened, and how many are alive.
#[derive(Clone, Debug, Default)]
pub struct SessionCounts {
    opened: Arc<AtomicUsize>,
    live: Arc<AtomicUsize>,
}

impl SessionCounts {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many sessions were handed out.
    #[must_use]
    pub fn opened(&self) -> usize {
        self.opened.load(Ordering::SeqCst)
    }

    /// How many are alive right now. A stop must make this zero.
    #[must_use]
    pub fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    fn note_opened(&self) -> Arc<AtomicUsize> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        self.live.fetch_add(1, Ordering::SeqCst);
        Arc::clone(&self.live)
    }
}

/// An opener that hands out counted synthetic sessions.
///
/// Every session produces the committed full-size JPEG for ever, at `frame_rate`,
/// after `leading_frames` reduced-size **leading frames**, and writes its
/// operations into `log`. Those are the same behaviour `SyntheticSession` has; the
/// only addition is the live count.
#[derive(Clone, Debug)]
pub struct CountingOpener {
    counts: SessionCounts,
    log: SharedLog,
    leading_frames: usize,
    frame_rate: f64,
}

impl CountingOpener {
    #[must_use]
    pub fn new(counts: SessionCounts, log: SharedLog) -> Self {
        Self {
            counts,
            log,
            leading_frames: 0,
            frame_rate: 250.0,
        }
    }

    /// Emits `count` **leading frames** at the reduced frame size first.
    #[must_use]
    pub fn with_leading_frames(mut self, count: usize) -> Self {
        self.leading_frames = count;
        self
    }

    /// Paces the sessions at `frames_per_second`.
    #[must_use]
    pub fn at_rate(mut self, frames_per_second: f64) -> Self {
        self.frame_rate = frames_per_second;
        self
    }
}

impl PtpSessionOpener for CountingOpener {
    fn open_session(&mut self) -> Result<Box<dyn PtpSession + Send>, SessionError> {
        let session = SyntheticSession::new(FULL_FRAME_SIZE)
            .with_shared_log(Arc::clone(&self.log))
            .with_leading_frames(self.leading_frames)
            .at_rate(self.frame_rate);

        Ok(Box::new(CountedSession {
            inner: session,
            live: self.counts.note_opened(),
        }))
    }
}

/// A synthetic session that leaves the live count when it is dropped.
struct CountedSession {
    inner: SyntheticSession,
    live: Arc<AtomicUsize>,
}

impl Drop for CountedSession {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PtpSession for CountedSession {
    fn model(&self) -> &str {
        self.inner.model()
    }

    fn set_output_setting(&mut self, setting: OutputSetting) -> Result<(), SessionError> {
        self.inner.set_output_setting(setting)
    }

    fn activate_viewfinder_action(&mut self) -> Result<(), SessionError> {
        self.inner.activate_viewfinder_action()
    }

    fn capture_live_view_frame(&mut self) -> Result<Vec<u8>, SessionError> {
        self.inner.capture_live_view_frame()
    }
}

/// What a [`RecordingSink`] was asked to do, across every sink it has built.
#[derive(Debug, Default)]
pub struct SinkLog {
    pub opens: Vec<FrameSize>,
    pub accepted: Vec<FrameSize>,
    pub closes: usize,
    /// How many sinks are open right now. A stop must leave this at zero.
    pub open_sinks: usize,
}

/// A sink that records every open, frame and close into a shared log.
///
/// Each `open_sink()` call hands out a fresh `RecordingSink` over the same log, so
/// the log spans several starts — which is how "a start opens a sink and a stop
/// closes it" is checked cycle after cycle.
#[derive(Clone, Debug, Default)]
pub struct RecordingSink {
    log: Arc<Mutex<SinkLog>>,
    open: bool,
}

impl RecordingSink {
    #[must_use]
    pub fn new(log: Arc<Mutex<SinkLog>>) -> Self {
        Self { log, open: false }
    }

    /// A factory, in the shape a `FeedRun` wants.
    #[must_use]
    pub fn factory(
        log: Arc<Mutex<SinkLog>>,
    ) -> Box<dyn FnMut() -> Box<dyn FrameSink + Send> + Send> {
        Box::new(move || Box::new(Self::new(Arc::clone(&log))))
    }

    /// Reads the log. Panics if another test thread panicked while holding it.
    pub fn read(log: &Arc<Mutex<SinkLog>>) -> std::sync::MutexGuard<'_, SinkLog> {
        log.lock().expect("the sink log must not be poisoned")
    }
}

impl FrameSink for RecordingSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        _pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        let mut log = self.log.lock().expect("the sink log must not be poisoned");
        log.opens.push(expected_frame_size);
        log.open_sinks += 1;
        drop(log);
        self.open = true;
        Ok(())
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        if !self.open {
            return Err(SinkError::NotOpen);
        }
        self.log
            .lock()
            .expect("the sink log must not be poisoned")
            .accepted
            .push(frame.frame_size());
        Ok(())
    }

    fn close(&mut self) -> Result<(), SinkError> {
        self.open = false;
        // Closing is idempotent, so a close on a sink that was never opened —
        // which is what a run that established no frame size does — must not
        // make the count of open sinks negative.
        let mut log = self.log.lock().expect("the sink log must not be poisoned");
        log.closes += 1;
        log.open_sinks = log.open_sinks.saturating_sub(1);
        Ok(())
    }
}

/// Waits until `condition` holds, failing the test rather than hanging if it does
/// not hold within `timeout`.
///
/// Every wait in these tests goes through here, because a test that blocks for
/// ever is worse than a test that fails: it hides which fact was never true.
pub fn wait_until(timeout: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out after {timeout:?} waiting for {what}");
}
