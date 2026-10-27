# 05: PTP failure detection and recovery

**What to build:** The capture layer survives a camera that has stopped answering. A rejected property write can drop the camera into a state where it stays on the USB bus and is still auto-detected, but every operation returns `PTP Timeout`. Today that wedges the process silently; after this ticket it either recovers or fails loudly.

Recovery is foundational and must not wait for the sink: there is no point building output on a capture layer known to wedge. Sink-side behaviour on disconnect is ticket 06.

**Blocked by:** 02

**Status:** resolved (hardware verification outstanding)

- [x] A `PTP Timeout` or rejected-write condition is detected and reported, rather than surfacing as a bare timeout or a hang.
- [x] A recovery reset destroys the wedged PTP session; it does not open a second session beside it. This is the one place where starting a new process is correct, and the code says so.
- [x] Recovery attempts a bounded number of `gphoto2 --reset` calls, then a full session reopen — bounded, so a permanently wedged camera cannot produce an infinite retry loop.
- [x] After a successful recovery the stream resumes without restarting the process.
- [x] Recovery failure ends the process with a non-zero exit and a message that tells the user to remove and reconnect the USB cable.
- [ ] The recovery path is exercised by deliberately triggering the wedged state, not only by reasoning about it.
- [x] The dangerous `output` values are never written by the recovery path, so recovery cannot re-wedge the camera.
- [x] The number of reset attempts and their outcome appear in the log, so a user report can be diagnosed without a debugger.

## Comments

### What was verified, and how

`camera-core/tests/ptp_recovery.rs` runs the recovery loop against a synthetic
session that fails on demand, and `camera-cli/tests/binary.rs` runs the same
through the real binary:

- **Detection before recovery.** `the_wedged_state_is_reported_as_wedged_before_any_recovery_is_attempted`
  asserts the condition is named as wedged before anything is retried.
- **The reset is `gphoto2 --reset`.** `the_recovery_reset_is_gphoto2_dash_dash_reset`
  asserts the exact command, its exit-zero success and its failure reporting,
  including the case where the command cannot start at all.
- **The bound is real.** `the_reset_budget_is_bounded_and_the_bound_is_what_runs`,
  `a_budget_of_one_attempts_exactly_one_reset` and
  `a_reset_that_fails_every_time_ends_the_run_after_the_bound` assert that a
  permanently wedged camera stops instead of retrying forever, and
  `recovery_stops_at_the_first_reset_that_works` asserts it does not overshoot.
- **Resume in the same process.** `a_wedged_camera_is_recovered_and_the_stream_resumes_in_the_same_process`
  (core) and its CLI counterpart assert the run continues rather than restarting.
- **The remedy is the cable.** `the_recovery_failure_remedy_is_remove_and_reconnect_the_usb_cable`
  and `a_permanently_wedged_camera_ends_the_command_with_the_cable_instruction`
  assert the non-zero exit and the exact instruction.
- **Recovery cannot re-wedge the camera.** `the_output_setting_recovery_re_applies_is_never_a_dangerous_value`
  asserts the `output` values 6, 7, 10 and 11 are never written back.
- **The log is diagnosable.** `the_log_says_how_many_reset_attempts_ran_and_how_they_ended`
  asserts the attempt count and each outcome appear.

Also asserted: a disconnected camera is never retried, because a reset cannot
help a cable that is not connected.

### Unverified (no camera attached)

- **Triggering the wedged state for real.** The tests make the session fail; they
  do not wedge an M50. The evidence would be a run that writes a rejected
  property, observes `PTP Timeout`, and then recovers — with the camera present.
- **The reset working on a wedged camera.** `gphoto2 --reset` is only exercised
  against a scripted command here. Whether it clears the real Canon state, and
  whether the `--shell` session must be killed first, needs the camera.
