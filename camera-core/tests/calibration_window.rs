//! The calibration window establishes the expected frame size.
//!
//! The seam under test is `CalibrationWindow::observe`, which takes one frame
//! size at a time plus the elapsed time, and returns what the window concluded.
//! Time is supplied by the caller, so no test waits.

use std::time::Duration;

use camera_core::calibration::{
    CalibrationError, CalibrationOutcome, CalibrationPolicy, CalibrationWindow,
};
use camera_core::frame_size::FrameSize;

const FULL: FrameSize = FrameSize::new(1024, 576);
const REDUCED: FrameSize = FrameSize::new(512, 288);

fn policy(window_frames: usize, min_recurrences: usize) -> CalibrationPolicy {
    CalibrationPolicy::new(window_frames, Duration::from_secs(3), min_recurrences)
        .expect("test policy must be valid")
}

/// Feeds frames of one size and returns the last outcome.
fn feed(
    window: &mut CalibrationWindow,
    frame_size: FrameSize,
    count: usize,
    elapsed: &mut Duration,
) -> CalibrationOutcome {
    let mut outcome = CalibrationOutcome::Establishing;
    for _ in 0..count {
        *elapsed += Duration::from_millis(33);
        outcome = window.observe(frame_size, *elapsed);
    }
    outcome
}

#[test]
fn a_five_frame_leading_transient_does_not_win_over_the_recurring_full_size() {
    let mut window = CalibrationWindow::new(policy(40, 3));
    let mut elapsed = Duration::from_millis(33);

    let after_transient = feed(&mut window, REDUCED, 5, &mut elapsed);

    assert_eq!(
        after_transient,
        CalibrationOutcome::Establishing,
        "five leading frames must not settle the window, because the reduced size is exactly \
         what the leading frames are"
    );

    let outcome = feed(&mut window, FULL, 35, &mut elapsed);

    assert_eq!(
        outcome,
        CalibrationOutcome::Established {
            expected_frame_size: FULL,
            leading_frames: 5,
        },
        "the recurring full size must win even though the reduced size also recurs often enough"
    );
}

#[test]
fn the_largest_recurring_size_wins_when_more_than_one_recurs() {
    let mut window = CalibrationWindow::new(policy(20, 3));
    let mut elapsed = Duration::from_millis(33);

    let _ = feed(&mut window, REDUCED, 10, &mut elapsed);
    let outcome = feed(&mut window, FULL, 10, &mut elapsed);

    assert_eq!(
        outcome,
        CalibrationOutcome::Established {
            expected_frame_size: FULL,
            leading_frames: 10,
        }
    );
}

#[test]
fn a_size_that_does_not_recur_enough_never_becomes_expected() {
    let mut window = CalibrationWindow::new(policy(20, 8));
    let mut elapsed = Duration::from_millis(33);

    let _ = feed(&mut window, REDUCED, 5, &mut elapsed);
    let outcome = feed(&mut window, FULL, 15, &mut elapsed);

    assert_eq!(
        outcome,
        CalibrationOutcome::Established {
            expected_frame_size: FULL,
            leading_frames: 5,
        },
        "the window must pick the full size, which recurs 15 times, not the reduced size, \
         which recurs only 5 times"
    );
}

#[test]
fn a_window_without_a_recurring_size_fails_loudly() {
    let mut window = CalibrationWindow::new(policy(10, 3));
    let mut elapsed = Duration::from_millis(33);
    let mut outcome = CalibrationOutcome::Establishing;

    for frame in 1..=10 {
        elapsed += Duration::from_millis(33);
        outcome = window.observe(FrameSize::new(100 + frame, 100), elapsed);
    }

    match outcome {
        CalibrationOutcome::Failed(CalibrationError::NoRecurringFrameSize {
            window_frames,
            min_recurrences,
            observed,
        }) => {
            assert_eq!(window_frames, 10);
            assert_eq!(min_recurrences, 3);
            assert_eq!(observed.len(), 10, "the failure must report what was seen");
        }
        other => panic!("expected a loud failure, got {other:?}"),
    }
}

#[test]
fn the_window_closes_on_elapsed_time_as_well_as_on_frame_count() {
    let policy = CalibrationPolicy::new(10_000, Duration::from_secs(1), 2)
        .expect("test policy must be valid");
    let mut window = CalibrationWindow::new(policy);

    assert_eq!(
        window.observe(FULL, Duration::from_millis(0)),
        CalibrationOutcome::Establishing
    );
    assert_eq!(
        window.observe(FULL, Duration::from_millis(700)),
        CalibrationOutcome::Establishing
    );
    assert_eq!(
        window.observe(FULL, Duration::from_millis(1_000)),
        CalibrationOutcome::Established {
            expected_frame_size: FULL,
            leading_frames: 0,
        },
        "the window must close at one second even though only three frames arrived"
    );
}

#[test]
fn the_outcome_is_stable_after_the_window_closes() {
    let mut window = CalibrationWindow::new(policy(3, 2));
    let mut elapsed = Duration::from_millis(33);

    let _ = feed(&mut window, FULL, 3, &mut elapsed);
    let after_close = window.observe(REDUCED, elapsed + Duration::from_millis(33));

    assert_eq!(
        after_close,
        CalibrationOutcome::Established {
            expected_frame_size: FULL,
            leading_frames: 0,
        },
        "a late frame must not reopen a closed window"
    );
}

#[test]
fn a_threshold_larger_than_the_window_is_rejected() {
    assert!(
        CalibrationPolicy::new(3, Duration::from_secs(1), 5).is_err(),
        "a threshold that can never be reached makes the window meaningless"
    );
}

#[test]
fn a_zero_length_window_is_rejected() {
    assert!(CalibrationPolicy::new(0, Duration::from_secs(1), 1).is_err());
    assert!(CalibrationPolicy::new(10, Duration::ZERO, 2).is_err());
}

#[test]
fn a_zero_threshold_is_rejected() {
    assert!(CalibrationPolicy::new(10, Duration::from_secs(1), 0).is_err());
}
