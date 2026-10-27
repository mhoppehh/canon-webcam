//! The statistics that make "frames delivered, frames dropped, measured rate,
//! data rate" a measurement.
//!
//! Every counter here is incremented exactly where the event happens: the queue
//! counts what it evicted, the pipeline counts what it refused, and the
//! [`StatsSink`] counts what actually reached the sink. Nothing is inferred from
//! anything else, so a report cannot quietly disagree with what ran.
//!
//! Rates are computed from delivered frames and elapsed time, never from the
//! configured `fps`: the configured value is a preference the camera never
//! agreed to.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::frame_size::FrameSize;
use crate::pixel_format::{LiveViewFrame, PixelFormat};
use crate::sink::{FrameSink, SinkError};

/// Counters shared by the queue, the pipeline and the sink.
///
/// Shared rather than merged afterwards, so the numbers a report prints are the
/// numbers the run produced, without a reconciliation step that could be wrong.
#[derive(Debug, Default)]
pub struct StreamStats {
    frames_received: AtomicU64,
    frames_delivered: AtomicU64,
    frames_dropped_size_mismatch: AtomicU64,
    frames_dropped_malformed: AtomicU64,
    frames_dropped_by_queue: AtomicU64,
    frames_refused_by_sink: AtomicU64,
    frames_observed_for_calibration: AtomicU64,
    bytes_delivered: AtomicU64,
    sink_opens: AtomicU64,
    sink_reopens: AtomicU64,
    recalibrations: AtomicU64,
    peak_queue_depth: AtomicUsize,
}

impl StreamStats {
    /// A fresh set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A frame came out of the capture queue and was offered to the pipeline.
    pub fn record_received(&self) {
        self.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    /// A frame reached the sink.
    pub fn record_delivered(&self, bytes: usize) {
        self.frames_delivered.fetch_add(1, Ordering::Relaxed);
        self.bytes_delivered
            .fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// A frame was dropped because its frame size was not the expected one.
    pub fn record_size_mismatch(&self) {
        self.frames_dropped_size_mismatch
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A frame was dropped because it carried no readable frame size.
    pub fn record_malformed(&self) {
        self.frames_dropped_malformed
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A frame went into a **calibration window** instead of being delivered.
    ///
    /// The **leading frames** and the opening calibration window land here, and
    /// so does every frame a short recalibration observes. Counting them keeps
    /// the accounting exact: every received frame is delivered, dropped, or
    /// observed by a calibration window.
    pub fn record_calibration_observation(&self) {
        self.frames_observed_for_calibration
            .fetch_add(1, Ordering::Relaxed);
    }

    /// The capture queue evicted `dropped` frames because the pipeline was
    /// behind.
    pub fn record_queue_drops(&self, dropped: u64) {
        self.frames_dropped_by_queue
            .fetch_add(dropped, Ordering::Relaxed);
    }

    /// A sink refused a frame the pipeline believed was deliverable. Always a
    /// bug, and always visible.
    pub fn record_sink_refusal(&self) {
        self.frames_refused_by_sink.fetch_add(1, Ordering::Relaxed);
    }

    /// The sink was opened.
    pub fn record_sink_open(&self) {
        self.sink_opens.fetch_add(1, Ordering::Relaxed);
    }

    /// The sink was closed and re-opened at a newly measured frame size.
    pub fn record_sink_reopen(&self) {
        self.sink_reopens.fetch_add(1, Ordering::Relaxed);
    }

    /// A short recalibration was started because the camera stopped producing
    /// the expected frame size.
    pub fn record_recalibration(&self) {
        self.recalibrations.fetch_add(1, Ordering::Relaxed);
    }

    /// Records how deep the capture queue currently is, keeping the maximum.
    pub fn observe_queue_depth(&self, depth: usize) {
        self.peak_queue_depth.fetch_max(depth, Ordering::Relaxed);
    }

    /// A consistent-enough snapshot for a report.
    ///
    /// `elapsed` is how long the run lasted, and the two rates are computed from
    /// it.
    #[must_use]
    pub fn snapshot(&self, elapsed: std::time::Duration) -> StatsSnapshot {
        let seconds = elapsed.as_secs_f64();
        let frames_delivered = self.frames_delivered.load(Ordering::Relaxed);
        let bytes_delivered = self.bytes_delivered.load(Ordering::Relaxed);

        StatsSnapshot {
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_delivered,
            frames_dropped_size_mismatch: self.frames_dropped_size_mismatch.load(Ordering::Relaxed),
            frames_dropped_malformed: self.frames_dropped_malformed.load(Ordering::Relaxed),
            frames_dropped_by_queue: self.frames_dropped_by_queue.load(Ordering::Relaxed),
            frames_refused_by_sink: self.frames_refused_by_sink.load(Ordering::Relaxed),
            frames_observed_for_calibration: self
                .frames_observed_for_calibration
                .load(Ordering::Relaxed),
            bytes_delivered,
            sink_opens: self.sink_opens.load(Ordering::Relaxed),
            sink_reopens: self.sink_reopens.load(Ordering::Relaxed),
            recalibrations: self.recalibrations.load(Ordering::Relaxed),
            peak_queue_depth: self.peak_queue_depth.load(Ordering::Relaxed),
            elapsed,
            measured_frame_rate: if seconds > 0.0 {
                frames_delivered as f64 / seconds
            } else {
                0.0
            },
            data_rate: if seconds > 0.0 {
                bytes_delivered as f64 / seconds
            } else {
                0.0
            },
        }
    }
}

/// What a run actually did, as a value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatsSnapshot {
    /// Frames the pipeline took from the capture queue.
    pub frames_received: u64,
    /// Frames the sink accepted.
    pub frames_delivered: u64,
    /// Frames dropped because they were not the expected frame size.
    pub frames_dropped_size_mismatch: u64,
    /// Frames dropped because they carried no readable frame size.
    pub frames_dropped_malformed: u64,
    /// Frames the bounded queue evicted because the pipeline was behind.
    pub frames_dropped_by_queue: u64,
    /// Frames a sink refused. Always a bug; always reported.
    pub frames_refused_by_sink: u64,
    /// Frames a calibration window observed instead of the sink receiving them:
    /// the **leading frames** of a run, and what a short recalibration saw.
    pub frames_observed_for_calibration: u64,
    /// Pixel bytes the sink accepted.
    pub bytes_delivered: u64,
    /// How many times a sink was opened.
    pub sink_opens: u64,
    /// How many of those opens were **sink re-opens** after a size change.
    pub sink_reopens: u64,
    /// How many short recalibrations were started.
    pub recalibrations: u64,
    /// The deepest the capture queue was ever seen to be.
    pub peak_queue_depth: usize,
    /// How long the run lasted.
    pub elapsed: std::time::Duration,
    /// Delivered frames per second across the run.
    pub measured_frame_rate: f64,
    /// Delivered pixel bytes per second across the run.
    pub data_rate: f64,
}

impl StatsSnapshot {
    /// Frames the pipeline received and refused: the wrong frame size, or no
    /// readable frame size.
    ///
    /// Frames the capture queue dropped never reached the pipeline at all, so
    /// they are counted separately by
    /// [`StatsSnapshot::frames_dropped_by_queue`].
    #[must_use]
    pub fn frames_dropped(&self) -> u64 {
        self.frames_dropped_size_mismatch
            .saturating_add(self.frames_dropped_malformed)
    }

    /// Every frame that never reached the sink, wherever it was lost.
    #[must_use]
    pub fn frames_dropped_total(&self) -> u64 {
        self.frames_dropped()
            .saturating_add(self.frames_dropped_by_queue)
    }

    /// Every frame the camera produced, as far as this run can tell.
    #[must_use]
    pub fn frames_captured(&self) -> u64 {
        self.frames_received
            .saturating_add(self.frames_dropped_by_queue)
    }

    /// Every frame this snapshot accounts for.
    ///
    /// This must equal [`StatsSnapshot::frames_received`]: a frame the pipeline
    /// received is delivered, dropped, or observed by a calibration window, and
    /// never anything else.
    #[must_use]
    pub fn frames_accounted(&self) -> u64 {
        self.frames_delivered
            .saturating_add(self.frames_dropped())
            .saturating_add(self.frames_observed_for_calibration)
    }
}

/// A sink that accepts frames and counts them.
///
/// It is a real [`FrameSink`], so it exercises the same trait a platform
/// backend implements, and it enforces the same rule a real sink does:
/// a frame whose frame size is not the one it was opened at is refused.
#[derive(Debug)]
pub struct StatsSink {
    stats: std::sync::Arc<StreamStats>,
    opened_at: Option<FrameSize>,
    pixel_format: PixelFormat,
}

impl StatsSink {
    /// Builds a sink sharing `stats` with the rest of the pipeline.
    #[must_use]
    pub fn new(stats: std::sync::Arc<StreamStats>) -> Self {
        Self {
            stats,
            opened_at: None,
            pixel_format: PixelFormat::Rgb24,
        }
    }

    /// The frame size the sink is open at, if it is open.
    #[must_use]
    pub fn opened_at(&self) -> Option<FrameSize> {
        self.opened_at
    }

    /// The format it expects, which is always [`PixelFormat::Rgb24`].
    #[must_use]
    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }
}

impl FrameSink for StatsSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        self.opened_at = Some(expected_frame_size);
        self.pixel_format = pixel_format;
        self.stats.record_sink_open();
        Ok(())
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        let Some(expected) = self.opened_at else {
            self.stats.record_sink_refusal();
            return Err(SinkError::NotOpen);
        };
        if frame.frame_size() != expected {
            self.stats.record_sink_refusal();
            return Err(SinkError::FrameSizeMismatch {
                expected,
                actual: frame.frame_size(),
            });
        }
        // Delivered frames and their bytes are counted by the pipeline, which is
        // the one place that knows exactly what it handed over. Counting here
        // too would double every frame.
        Ok(())
    }

    fn close(&mut self) -> Result<(), SinkError> {
        self.opened_at = None;
        Ok(())
    }
}
