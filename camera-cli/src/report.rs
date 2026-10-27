//! Renders a measurement for a human to read and to diff between runs.
//!
//! One `key: value` per line, in a fixed order, with no timestamps and no
//! decoration, so `diff` between two runs shows only what changed.
//!
//! The sanity check line is labelled as a check and never as the source: the
//! measured frame size and the measured frame rate come from frame data, and
//! `1024x576` at roughly 24 frames per second is only what a working M50 has
//! produced before.

use std::fmt::Write as _;

use camera_core::config::StreamingConfig;
use camera_core::frame_size::FrameSize;
use camera_core::measurement::{
    Measurement, MeasurementPlan, SANITY_CHECK_FRAME_RATE, SANITY_CHECK_FRAME_SIZE,
};
use camera_core::runner::{RecoveredStreamRun, StreamPlan};

use crate::sink::{SinkReport, SinkSelection};

/// Renders one measurement.
#[must_use]
pub fn render(model: &str, plan: &MeasurementPlan, measurement: &Measurement) -> String {
    let calibration = plan.calibration();
    let mut report = String::new();

    push(&mut report, &format!("camera model: {model}"));
    push(
        &mut report,
        &format!("output setting: {}", plan.output_setting()),
    );
    push(
        &mut report,
        &format!(
            "calibration window: {} frames or {:.3} s, a size must recur {} times",
            calibration.window_frames(),
            calibration.window_duration().as_secs_f64(),
            calibration.min_recurrences()
        ),
    );
    push(
        &mut report,
        &format!("sampling frames: {}", measurement.sampling_frames),
    );
    push(
        &mut report,
        &format!("measured frame size: {}", measurement.measured_frame_size),
    );
    if let Some(note) = frame_size_note(Some(measurement.measured_frame_size)) {
        push(&mut report, &format!("frame size note: {note}"));
    }
    push(
        &mut report,
        &format!(
            "measured frame rate: {:.2} fps",
            measurement.measured_frame_rate
        ),
    );
    push(
        &mut report,
        &format!("frames captured: {}", measurement.frames_captured),
    );
    push(
        &mut report,
        &format!("leading frames: {}", measurement.leading_frames),
    );
    push(
        &mut report,
        &format!("unexpected frames: {}", measurement.unexpected_frames),
    );
    push(
        &mut report,
        &format!(
            "sampling span: {:.3} s",
            measurement.sampling_elapsed.as_secs_f64()
        ),
    );
    push(
        &mut report,
        &format!("elapsed: {:.3} s", measurement.elapsed.as_secs_f64()),
    );
    push(&mut report, &sanity_check(measurement));

    report
}

/// Renders what one stream run actually did, including what recovery from a
/// **wedged camera** cost and which sink published.
///
/// The **measured frame size** and the **measured frame rate** come from
/// delivered frame data. The requested size is printed once, labelled as a
/// preference, so it can never be mistaken for the result. The recovery lines say
/// how many **recovery resets** ran and how each ended, so a user report is
/// diagnosable without a debugger. When the **platform backend** published, its
/// own counters follow the pipeline's: what reached the sink is not the same fact
/// as what the sink did with it.
#[must_use]
pub fn render_stream(
    config: &StreamingConfig,
    plan: &StreamPlan,
    run: &RecoveredStreamRun,
    sink: &SinkReport,
) -> String {
    let stats = &run.stats;
    let calibration = plan.pipeline().calibration();
    let mut report = String::new();

    push(
        &mut report,
        &format!("virtual camera mechanism: {}", config.mechanism()),
    );
    push(&mut report, &format!("sink: {}", sink_line(config, sink)));
    push(
        &mut report,
        &format!(
            "requested size: {} (a preference; this camera honours no size request)",
            config
                .requested_size()
                .map_or_else(|| "none".to_owned(), |size| size.to_string())
        ),
    );
    push(
        &mut report,
        &format!("pixel format: {}", config.pixel_format()),
    );
    if let Some(note) = config.upscale_note() {
        push(&mut report, &format!("upscaling: {note}"));
    } else {
        push(&mut report, "upscaling: off");
    }
    push(
        &mut report,
        &format!(
            "calibration window: {} frames or {:.3} s, a size must recur {} times",
            calibration.window_frames(),
            calibration.window_duration().as_secs_f64(),
            calibration.min_recurrences()
        ),
    );
    push(
        &mut report,
        &format!(
            "measured frame size: {}",
            run.measured_frame_size
                .map_or_else(|| "unestablished".to_owned(), |size| size.to_string())
        ),
    );
    if let Some(note) = frame_size_note(run.measured_frame_size) {
        push(&mut report, &format!("frame size note: {note}"));
    }
    push(
        &mut report,
        &format!("measured frame rate: {:.2} fps", stats.measured_frame_rate),
    );
    push(
        &mut report,
        &format!("data rate: {:.2} MB/s", stats.data_rate / 1_000_000.0),
    );
    push(
        &mut report,
        &format!("frames delivered: {}", stats.frames_delivered),
    );
    push(
        &mut report,
        &format!("frames dropped: {}", stats.frames_dropped_total()),
    );
    push(
        &mut report,
        &format!(
            "  dropped by the queue: {}, wrong frame size: {}, unreadable: {}",
            stats.frames_dropped_by_queue,
            stats.frames_dropped_size_mismatch,
            stats.frames_dropped_malformed
        ),
    );
    push(
        &mut report,
        &format!(
            "sink opens: {} (re-opens: {}), recalibrations: {}",
            stats.sink_opens, stats.sink_reopens, stats.recalibrations
        ),
    );
    for line in sink_counters(sink) {
        push(&mut report, &line);
    }
    push(
        &mut report,
        &format!("peak queue depth: {}", stats.peak_queue_depth),
    );
    push(
        &mut report,
        &format!("PTP sessions opened: {}", run.sessions_opened),
    );
    for line in run.recovery.to_string().lines() {
        push(&mut report, line);
    }
    push(
        &mut report,
        &format!("elapsed: {:.3} s", stats.elapsed.as_secs_f64()),
    );
    push(&mut report, &format!("outcome: {}", run.end));

    report
}

/// The line that says which sink published the live view feed.
///
/// The platform sink is named by its mechanism, which is the word the
/// configuration uses and the word a user reads in OBS. It only says it
/// published once a frame actually reached the **consumer page**: a run that ends
/// before the sink opens must not read as a successful publication.
fn sink_line(config: &StreamingConfig, sink: &SinkReport) -> String {
    match sink.selection {
        SinkSelection::Stats => {
            "stats (the counting sink; nothing is published to a consumer application)".to_owned()
        }
        SinkSelection::VirtualCamera => {
            let mechanism = config.mechanism();
            if sink.frames_published > 0 {
                format!("{mechanism} (the platform backend published the live view feed)")
            } else {
                format!(
                    "{mechanism} (the platform backend, which published no frame to a consumer \
                     page)"
                )
            }
        }
    }
}

/// The platform sink's own counters, which only it can report.
///
/// They are printed apart from the pipeline's counters because they answer a
/// different question: the pipeline says what reached the sink, and these say
/// what the sink handed to the **consumer page**. The counting sink has nothing
/// to add, and printing zeros for it would read as a measurement.
fn sink_counters(sink: &SinkReport) -> Vec<String> {
    if sink.selection != SinkSelection::VirtualCamera {
        return Vec::new();
    }

    let mut lines = vec![
        format!(
            "sink consumer page connected: {}",
            yes_no(sink.consumer_page_seen)
        ),
        format!("sink pages served: {}", sink.consumer_pages_served),
        format!("sink frames published: {}", sink.frames_published),
        format!("sink frames dropped: {}", sink.frames_dropped),
    ];
    for note in &sink.notes {
        lines.push(format!("sink note: {note}"));
    }
    lines
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn sanity_check(measurement: &Measurement) -> String {
    let verdict = if measurement.measured_frame_size == SANITY_CHECK_FRAME_SIZE {
        "matches"
    } else {
        "does not match"
    };

    format!(
        "sanity check: {SANITY_CHECK_FRAME_SIZE} at roughly {SANITY_CHECK_FRAME_RATE:.0} fps \
         {verdict} (sanity check only; never the source of the measured frame size or rate)"
    )
}

/// A note when the measured frame size is the photo-mode one.
///
/// Measured on an M50: the movie position gives 1024x576 at about 24 fps, and the
/// photo positions give 960x640 at about 30 fps. In a photo position the body
/// also keeps its own screen dark while the PC takes the live view, which reads
/// as a fault and is only the mode. The frame size is the evidence for the mode,
/// so the note is keyed on it rather than on a mode the program cannot read.
fn frame_size_note(frame_size: Option<FrameSize>) -> Option<&'static str> {
    const PHOTO_MODE_FRAME_SIZE: FrameSize = FrameSize::new(960, 640);
    match frame_size {
        Some(size) if size == PHOTO_MODE_FRAME_SIZE => Some(
            "960x640 at about 30 fps is the photo-mode live view; the movie position gives \
             1024x576 at about 24 fps and keeps the camera's own screen on",
        ),
        _ => None,
    }
}

fn push(report: &mut String, line: &str) {
    writeln!(report, "{line}").expect("writing to a String cannot fail");
}

#[cfg(test)]
mod tests {
    use super::frame_size_note;
    use camera_core::frame_size::FrameSize;

    #[test]
    fn the_photo_mode_frame_size_is_named_as_the_mode() {
        let note = frame_size_note(Some(FrameSize::new(960, 640))).expect("a note");
        assert!(note.contains("movie position"), "note was {note}");
        assert!(
            frame_size_note(Some(FrameSize::new(1024, 576))).is_none(),
            "the movie-mode size is what the docs promise and needs no note"
        );
        assert!(frame_size_note(None).is_none());
    }
}
