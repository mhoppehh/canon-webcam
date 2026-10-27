//! The run report as a value: which sink published, and the platform sink's own
//! counters.
//!
//! The platform sink cannot be opened without OBS in `/Applications`, so the
//! report is exercised by handing it the counters that sink produces rather than
//! by publishing. Everything else in the report comes from a real
//! [`RecoveredStreamRun`] shape, so the rendering is the code a run uses.

use std::time::Duration;

use camera_cli::report::render_stream;
use camera_cli::sink::{SinkReport, SinkSelection};
use camera_core::calibration::CalibrationPolicy;
use camera_core::config::{ConfigOverrides, StreamingConfig};
use camera_core::frame_size::FrameSize;
use camera_core::mechanism::Platform;
use camera_core::output_setting::OutputSetting;
use camera_core::recovery::RecoveryLog;
use camera_core::runner::{RecoveredStreamRun, StreamEnd, StreamPlan};
use camera_core::stats::StatsSnapshot;

fn config() -> StreamingConfig {
    StreamingConfig::resolve(None, ConfigOverrides::default(), Platform::MacOs)
        .expect("the macOS defaults must resolve")
}

fn plan(config: &StreamingConfig) -> StreamPlan {
    StreamPlan::new(
        OutputSetting::from_raw(3).expect("3 is TFT + PC"),
        config.pipeline_config(CalibrationPolicy::default()),
    )
}

fn run() -> RecoveredStreamRun {
    RecoveredStreamRun {
        end: StreamEnd::DurationElapsed,
        measured_frame_size: Some(FrameSize::new(1024, 576)),
        stats: StatsSnapshot {
            frames_received: 121,
            frames_delivered: 120,
            frames_dropped_size_mismatch: 0,
            frames_dropped_malformed: 0,
            frames_dropped_by_queue: 1,
            frames_refused_by_sink: 0,
            frames_observed_for_calibration: 1,
            bytes_delivered: 82_000_000,
            sink_opens: 1,
            sink_reopens: 0,
            recalibrations: 0,
            peak_queue_depth: 2,
            elapsed: Duration::from_secs(5),
            measured_frame_rate: 24.0,
            data_rate: 16_400_000.0,
        },
        recovery: RecoveryLog::default(),
        sessions_opened: 1,
    }
}

#[test]
fn the_report_names_the_platform_sink_and_its_own_counters() {
    let config = config();
    let plan = plan(&config);
    let sink = SinkReport {
        selection: SinkSelection::VirtualCamera,
        consumer_page_seen: true,
        consumer_pages_served: 1,
        frames_published: 118,
        frames_dropped: 2,
        notes: vec!["OBS is left running".to_owned()],
    };

    let report = render_stream(&config, &plan, &run(), &sink);

    assert!(
        report.contains(
            "sink: obs-virtual-camera (the platform backend published the live view feed)"
        ),
        "the report must name the sink that published, was {report}"
    );
    assert!(
        report.contains("sink consumer page connected: yes"),
        "the consumer page is what proves the feed arrived, was {report}"
    );
    assert!(
        report.contains("sink pages served: 1"),
        "a page count of zero is the difference between a broken transport and a \\\n         browser source that never loaded, was {report}"
    );
    assert!(
        report.contains("sink frames published: 118"),
        "the sink's own counter must be reported, was {report}"
    );
    assert!(
        report.contains("sink frames dropped: 2"),
        "the sink's own drops must be reported, was {report}"
    );
    assert!(
        report.contains("sink note: OBS is left running"),
        "the sink's one-off remarks must be printed once, was {report}"
    );
    assert!(
        report.contains("frames delivered: 120"),
        "the pipeline's counters must still be there, was {report}"
    );
}

#[test]
fn the_counting_sink_reports_no_platform_counters_at_all() {
    let config = config();
    let plan = plan(&config);
    let sink = SinkReport {
        selection: SinkSelection::Stats,
        ..SinkReport::default()
    };

    let report = render_stream(&config, &plan, &run(), &sink);

    assert!(
        report.contains("sink: stats (the counting sink; nothing is published"),
        "the counting sink must be named as itself, was {report}"
    );
    assert!(
        !report.contains("sink frames published") && !report.contains("sink consumer page"),
        "zeros would read as a measurement the counting sink never made, was {report}"
    );
}

#[test]
fn a_platform_sink_that_published_nothing_does_not_read_as_a_publication() {
    let config = config();
    let plan = plan(&config);
    let sink = SinkReport {
        selection: SinkSelection::VirtualCamera,
        ..SinkReport::default()
    };

    let report = render_stream(&config, &plan, &run(), &sink);

    assert!(
        report.contains("obs-virtual-camera (the platform backend, which published no frame"),
        "a run that never reached the page must not read as published, was {report}"
    );
    assert!(
        !report.contains("published the live view feed"),
        "nothing was published, was {report}"
    );
}
