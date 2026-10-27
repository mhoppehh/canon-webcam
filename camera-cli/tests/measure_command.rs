//! The `measure` subcommand as a user sees it: arguments and the printed report.
//!
//! These tests use the `camera_cli` library so they can inspect the parsed plan
//! and the rendered report without touching a camera.

use std::time::Duration;

use camera_cli::args::{parse, ArgError, Command};
use camera_cli::report::render;
use camera_core::calibration::CalibrationPolicy;
use camera_core::frame_size::FrameSize;
use camera_core::measurement::{Measurement, MeasurementPlan};
use camera_core::output_setting::OutputSetting;

fn parse_args(arguments: &[&str]) -> Result<Command, ArgError> {
    parse(arguments.iter().map(|argument| (*argument).to_owned()))
}

#[test]
fn the_default_measure_plan_sets_the_full_size_output_setting() {
    let Command::Measure(args) = parse_args(&["measure"]).expect("measure must parse") else {
        panic!("expected the measure subcommand");
    };

    assert_eq!(args.plan().output_setting(), OutputSetting::tft_plus_pc());
}

#[test]
fn the_output_option_accepts_an_allowlisted_value() {
    let Command::Measure(args) = parse_args(&["measure", "--output", "1"]).expect("must parse")
    else {
        panic!("expected the measure subcommand");
    };

    assert_eq!(args.plan().output_setting().raw(), 1);
}

#[test]
fn the_output_option_rejects_mobile2_through_the_same_guard_as_the_session() {
    let error = parse_args(&["measure", "--output", "8"]).expect_err("value 8 must be rejected");

    assert!(
        matches!(error, ArgError::OutputRejected(_)),
        "error was {error:?}"
    );
    assert!(
        error.to_string().contains("MOBILE2"),
        "the user must be told which value was refused, message was {error}"
    );
}

#[test]
fn the_window_options_are_parsed() {
    let Command::Measure(args) = parse_args(&[
        "measure",
        "--window-frames",
        "40",
        "--window-duration-ms",
        "2500",
        "--min-recurrences",
        "6",
        "--sampling-frames",
        "24",
    ])
    .expect("must parse") else {
        panic!("expected the measure subcommand");
    };

    let calibration = args.plan().calibration();
    assert_eq!(calibration.window_frames(), 40);
    assert_eq!(calibration.window_duration(), Duration::from_millis(2_500));
    assert_eq!(calibration.min_recurrences(), 6);
    assert_eq!(args.plan().sampling_frames(), 24);
}

#[test]
fn help_is_available_at_the_top_level_and_per_subcommand() {
    assert_eq!(parse_args(&["--help"]).expect("must parse"), Command::Help);
    assert_eq!(
        parse_args(&["measure", "--help"]).expect("must parse"),
        Command::Help
    );
}

#[test]
fn a_missing_command_is_reported() {
    assert_eq!(
        parse_args(&[]).expect_err("must be rejected"),
        ArgError::MissingCommand
    );
}

#[test]
fn an_unknown_command_is_reported() {
    let error = parse_args(&["record"]).expect_err("must be rejected");

    assert!(
        matches!(error, ArgError::UnknownCommand { .. }),
        "error was {error:?}"
    );
}

#[test]
fn an_unknown_option_is_reported() {
    let error = parse_args(&["measure", "--resolution", "1080p"]).expect_err("must be rejected");

    assert!(
        matches!(error, ArgError::UnknownOption { .. }),
        "error was {error:?}"
    );
}

#[test]
fn a_missing_option_value_is_reported() {
    let error = parse_args(&["measure", "--output"]).expect_err("must be rejected");

    assert!(
        matches!(error, ArgError::MissingValue { .. }),
        "error was {error:?}"
    );
}

#[test]
fn a_window_that_can_never_establish_a_size_is_rejected_before_any_camera_is_opened() {
    let error = parse_args(&["measure", "--window-frames", "3", "--min-recurrences", "5"])
        .expect_err("must be rejected");

    assert!(
        matches!(error, ArgError::PlanRejected(_)),
        "error was {error:?}"
    );
}

#[test]
fn a_single_sampling_frame_is_rejected_before_any_camera_is_opened() {
    let error = parse_args(&["measure", "--sampling-frames", "1"]).expect_err("must be rejected");

    assert!(
        matches!(error, ArgError::PlanRejected(_)),
        "error was {error:?}"
    );
}

#[test]
fn the_report_is_diffable_plain_text() {
    let report = render(
        "Canon EOS M50",
        &MeasurementPlan::default(),
        &sample_measurement(),
    );

    for expected in [
        "camera model: Canon EOS M50",
        "output setting: TFT + PC (3)",
        "measured frame size: 1024x576",
        "measured frame rate: 24.02 fps",
        "leading frames: 5",
        "frames captured: 61",
    ] {
        assert!(
            report.contains(expected),
            "the report must contain {expected:?}, report was:\n{report}"
        );
    }
}

#[test]
fn the_sanity_check_is_labelled_as_a_check_and_not_as_the_source() {
    let report = render(
        "Canon EOS M50",
        &MeasurementPlan::default(),
        &sample_measurement(),
    );

    let sanity_line = report
        .lines()
        .find(|line| line.starts_with("sanity check"))
        .expect("the report must carry a sanity check line");

    assert!(sanity_line.contains("1024x576"), "line was {sanity_line:?}");
    assert!(
        sanity_line.contains("roughly 24 fps"),
        "line was {sanity_line:?}"
    );
    assert!(
        sanity_line.contains("never the source"),
        "the sanity check must never read as the source, line was {sanity_line:?}"
    );
}

#[test]
fn the_sanity_check_reports_a_mismatch_rather_than_hiding_it() {
    let mut measurement = sample_measurement();
    measurement.measured_frame_size = FrameSize::new(640, 360);
    measurement.measured_frame_rate = 12.5;

    let report = render("Canon EOS M50", &MeasurementPlan::default(), &measurement);

    let sanity_line = report
        .lines()
        .find(|line| line.starts_with("sanity check"))
        .expect("the report must carry a sanity check line");
    assert!(
        sanity_line.contains("does not match"),
        "line was {sanity_line:?}"
    );
}

#[test]
fn the_report_states_the_calibration_window_it_used() {
    let plan = MeasurementPlan::new(
        OutputSetting::tft_plus_pc(),
        CalibrationPolicy::new(40, Duration::from_millis(2_500), 6).expect("valid"),
        24,
    )
    .expect("valid");

    let report = render("Canon EOS M50", &plan, &sample_measurement());

    assert!(
        report.contains("calibration window:"),
        "report was:\n{report}"
    );
    assert!(report.contains("40 frames"), "report was:\n{report}");
    assert!(report.contains("2.500 s"), "report was:\n{report}");
    assert!(report.contains("6 times"), "report was:\n{report}");
}

#[test]
fn the_report_names_unexpected_frames_when_the_size_changed_mid_capture() {
    let mut measurement = sample_measurement();
    measurement.unexpected_frames = 3;

    let report = render("Canon EOS M50", &MeasurementPlan::default(), &measurement);

    assert!(
        report.contains("unexpected frames: 3"),
        "report was:\n{report}"
    );
}

fn sample_measurement() -> Measurement {
    Measurement {
        measured_frame_size: FrameSize::new(1024, 576),
        measured_frame_rate: 24.02,
        frames_captured: 61,
        leading_frames: 5,
        unexpected_frames: 0,
        sampling_frames: 48,
        sampling_elapsed: Duration::from_millis(1_957),
        elapsed: Duration::from_millis(2_680),
    }
}
