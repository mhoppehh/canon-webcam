# 06: Sink teardown on camera disconnect

**What to build:** When the camera goes away, the virtual camera goes away with it. Split out from 05 because it is a different failure surface: 05 is the capture layer noticing, this is the sink layer letting go. A sink left open on a dead capture layer is worse than a crash, because the consumer application keeps showing a frozen last frame and the user cannot tell the feed is dead.

**Blocked by:** 04, 05

**Status:** resolved (hardware verification outstanding)

- [x] Removing the USB cable causes the sink to close rather than hold the last frame indefinitely.
- [ ] The consumer application releases the device within a few seconds and reports the camera as unavailable.
- [x] No stale or repeated frame is delivered after the capture layer stops producing.
- [x] Reconnecting the cable recalibrates the frame size, then re-opens the sink at that size, and the feed resumes without restarting the process.
- [x] The user-facing message distinguishes "camera disconnected" from "camera wedged" (05) and from "platform backend failed" (04), because the remedies differ.
- [x] The sink is closed on every exit path, including error paths, so a crashed run does not leave the device held.

## Comments

### What was verified, and how

No camera is attached, so a **disconnected camera** is driven through
`camera_core::synthetic::SyntheticSession`, and the sink is a `WatchingSink` in
`camera-core/tests/sink_teardown.rs` that records every `open`, `accept` and
`close` into one timestamped log shared by every segment of a run. That log is the
only place the difference between "closed" and "held" is observable without OBS
and a consumer application.

Covered by tests in `camera-core/tests/sink_teardown.rs`:

- `a_disconnect_closes_the_sink_before_the_run_reports_it` — a session that stops
  with `SessionError::NoCamera` ends the run as a **disconnected camera**, the
  last thing the sink did was close, nothing was accepted after that close, and no
  **recovery reset** ran, because a reset cannot put a camera back on the USB bus.
- `the_sink_closes_when_the_cable_goes_and_not_when_the_run_gives_up` — the sink
  closes well inside the **reconnect grace**, not when the run gives up. A sink
  closed only at the end of the run would leave the consumer holding the last
  frame for the whole grace, which is the frozen picture this ticket removes.
- `a_camera_that_never_comes_back_ends_the_run_as_disconnected` — a camera that
  stays away ends the run as disconnected after the grace, still with the sink
  closed, and the wait is not counted as an extra **PTP session**.
- `a_reconnecting_camera_is_recalibrated_and_the_sink_reopened_at_its_new_size` —
  the cable comes back on a different **frame size**: one process, one clock and
  one set of counters, `sessions_opened` is two, and the sink is opened once per
  segment at the size that segment measured (`512x288` then `1024x576`), which is
  a **sink re-open** and never a resize of an open sink.
- `no_frame_is_delivered_after_the_capture_layer_stops_producing` — the frames the
  sink accepted equal the frames the run counted as delivered exactly, never
  exceed the frames the camera produced, and stop at the close. The reconnect test
  proves the rest of it: every frame accepted in the second segment is the size
  that segment measured, so a frame captured before the cable went cannot have
  survived into it.
- `a_platform_backend_failure_still_closes_the_sink` — a **platform backend** that
  refuses frames is a `StreamError`, and the sink is still closed on that error
  path.
- `a_capture_thread_that_dies_without_a_word_still_closes_the_sink` — a capture
  thread that unwinds never writes to the queue that carries news from it, so the
  pipeline used to wait on that queue for ever with the device held. `run_into`
  now marks the capture thread finished on its way out, unwinding included, and
  the pipeline ends the run as `CaptureFailed` with the sink closed. The test runs
  with no duration on purpose: without that protection it would hang rather than
  fail.
- `a_pipeline_dropped_without_shutdown_closes_the_sink` — the `Drop` safety net
  behind the explicit close: a `StreamPipeline` dropped on a path that never
  reached `shutdown()` still closes the sink.
- `a_platform_backend_failure_reads_differently_from_either_camera_state` — the
  three user-facing messages are pairwise distinguishable and the two camera
  states keep their own remedies (`reconnect the camera` versus
  `gphoto2 --reset`). `camera-cli` prints a `StreamEnd` with its remedy, so a
  disconnect and a **wedged camera** reach the user as different instructions.

Two corrections to the draft while verifying it: the pipeline's own bookkeeping of
the open sink is now cleared as the sink closes during a **sink re-open**, so a
failed re-open cannot leave a pipeline claiming a device it already released; and
a camera that comes back in a state a fresh session cannot fix is reported as what
it is instead of being waited out as a disconnection.

Clean build: `cargo test --workspace` passes 196 tests with none failing,
`cargo clippy --workspace --all-targets` is silent and `cargo fmt --check` is
clean.

### Unverified (no camera attached)

- **The consumer application releasing the device within a few seconds.** Requires
  OBS in `/Applications` with its camera extension approved, plus Discord, Teams
  or Meet. Closing evidence: pull the USB cable during a call and see the device
  disappear from the consumer application's camera list at the same moment
  `stream` reports the camera as disconnected. What is proved here is the reachable
  half: the sink is closed as the cable goes, and ADR-0002's close is what
  releases the device (`StopVirtualCam`, then the scene item and the input).
- **That a real reconnect resumes the feed end to end.** The synthetic run proves
  the run shape — fresh **PTP session**, recalibration, **sink re-open** at the
  measured size, same process. Whether libgphoto2 lists a reconnected M50 again
  without a `gphoto2 --reset`, and how long that takes, is hardware.
- **The grace a cable reconnect actually needs**, which would tune
  `DEFAULT_RECONNECT_GRACE`. Ten seconds is a guess: long enough for a hand to
  plug a cable back in, short enough that an unplugged camera ends the run with a
  remedy instead of leaving a silent process.

### Notes for later tickets

- Three endings a user must not confuse: `StreamEnd::DisconnectedCamera`,
  `StreamEnd::WedgedCamera` / `StreamEnd::RecoveryFailed`, and
  `StreamEnd::CaptureFailed`. A **platform backend** failure is not a `StreamEnd`
  at all: it is `StreamError::Sink`, which ticket 04's actionable messages hang
  off.
- `StreamPlan::with_reconnect_grace` is the knob for how long a run waits for a
  **disconnected camera**; the CLI uses the default because nothing measured a
  better number.
- `camera-cli`'s `stats_sink()` builds a fresh `Arc<StreamStats>` per segment, so
  the sink's own counters restart after a reconnect. The numbers the report prints
  come from `RecoveredStreamRun::stats`, which spans the run, so nothing is wrong
  today — but that closure is where to fix it if the sink ever needs run-spanning
  counters of its own.
- A capture thread blocked inside `capture_live_view_frame` cannot be interrupted:
  the queue close is only noticed between captures, so `run_into`'s join waits for
  whichever libgphoto2 timeout applies. That bound belongs to the gphoto2 binding,
  not to this layer.

