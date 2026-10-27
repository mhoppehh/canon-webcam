//! One PTP session per run, enforced by ownership.
//!
//! The camera session type is not `Clone` and its only constructor takes the
//! process-wide claim, so a second live session cannot be created by accident:
//! the second attempt is an error rather than a second handle on the camera.
//! These tests exercise that claim directly, without a camera.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use camera_core::session::{SessionError, SessionExclusivity};

#[test]
fn a_second_claim_is_rejected_while_the_first_is_alive() {
    let exclusivity = SessionExclusivity::new();

    let first = exclusivity.claim().expect("the first claim must succeed");
    let second = exclusivity
        .claim()
        .expect_err("a second concurrent session is a bug, not a fallback");

    assert_eq!(second, SessionError::AlreadyOpen);
    drop(first);
}

#[test]
fn a_claim_can_be_taken_again_once_the_previous_one_ends() {
    let exclusivity = SessionExclusivity::new();

    let first = exclusivity.claim().expect("the first claim must succeed");
    drop(first);
    let second = exclusivity.claim().expect("the claim must be released");

    drop(second);
}

#[test]
fn the_claim_holds_while_its_owner_lives() {
    let exclusivity = SessionExclusivity::new();
    let claim = exclusivity.claim().expect("the first claim must succeed");

    for _ in 0..3 {
        assert_eq!(exclusivity.claim().err(), Some(SessionError::AlreadyOpen));
    }

    drop(claim);
    assert!(exclusivity.claim().is_ok());
}

#[test]
fn the_process_wide_claim_is_a_single_instance() {
    // The claim the camera session takes is process wide, so two calls must
    // observe the same value. This is the property that makes the session
    // exclusive across the whole run rather than per caller.
    let first = camera_core::session::process_session_exclusivity();
    let second = camera_core::session::process_session_exclusivity();

    assert!(std::ptr::eq(first, second));
}

#[test]
fn claims_can_be_shared_across_threads() {
    let exclusivity = Arc::new(SessionExclusivity::new());
    let successes = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    for _ in 0..8 {
        let exclusivity = Arc::clone(&exclusivity);
        let successes = Arc::clone(&successes);
        handles.push(std::thread::spawn(move || {
            if let Ok(claim) = exclusivity.claim() {
                successes.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(20));
                drop(claim);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread must not panic");
    }

    assert_eq!(
        successes.load(Ordering::SeqCst),
        1,
        "exactly one thread may hold the session at a time"
    );
}
