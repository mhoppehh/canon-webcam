# 07: Start and stop the feed without restarting the process

**What to build:** A control surface that turns the feed off and on while the process keeps running, releasing both the camera session and the virtual camera device between runs, and taking them again cleanly.

The control surface is a local socket or a tray menu — whichever the sink decision in 03 makes practical. A tray menu on macOS and a local socket on Linux need not be the same mechanism; the shared contract is what matters.

**Blocked by:** 04

**Status:** resolved (hardware verification outstanding)

- [x] Stopping releases the PTP session and closes the sink, so another application or another instance can use the camera.
- [x] Starting re-takes both, re-applies `output` and `viewfinder`, and discards the leading frames again.
- [x] Repeated start/stop cycles (at least 10 in a row) do not leak sessions, threads, or file descriptors.
- [x] Stop completes within a couple of seconds even when the feed is active.
- [x] The command reports whether the feed is currently running, so a script can check before acting.
- [x] Stopping when already stopped, and starting when already running, are both harmless.

## Comments

### The mechanism, and why

`docs/adr/0005-start-stop-control.md`. The front end is a **local Unix socket**
(`camera_core::control::socket`), served by `camera-cli serve --socket <path>` and
driven by `camera-cli control --socket <path> <start|stop|status|shutdown>`. A
menu bar item was rejected for this ticket: nothing in the workspace links a GUI
toolkit, the process already owns sockets (the loopback frame transport, the
obs-websocket client), and a socket is the same mechanism on Linux where a tray
does not exist in a headless session. The tray stays possible because the state
machine (`camera_core::control::FeedController`) is the shared contract and a
front end holds no state — it can only start, stop and ask.

`status` answers `running` or `stopped`, and `camera-cli control status` also
answers in its exit status (0 running, 3 stopped), which is what makes it usable
as a check in a script.

A stop is a `StopToken` the pipeline reads on its next tick (20 ms), not a kill:
the capture in flight finishes, so a session is never abandoned mid-operation.
The sink is closed by the pipeline's existing exit path, which every ending
already takes, and the **PTP session** is dropped when the run's opener goes out
of scope — before the feed is reported stopped, so a stop that has been
acknowledged means the camera is free. That order is what the tests assert.

### What was verified, and how

No camera and no OBS are attached, so the feed is the committed-JPEG
`SyntheticSession` behind a **counting opener** (it decrements a live count in
`Drop`, so a leaked session cannot hide) and a **recording sink** (one shared log
of opens, accepted frame sizes, closes and currently-open sinks across every
start). Both are in `camera-core/tests/support/control.rs`.

- `camera-core/tests/start_stop_control.rs`
  - `a_start_stop_start_cycle_releases_and_retakes_the_session_and_the_sink` —
    start takes exactly one session and opens one sink; stop takes the live count
    to zero and closes every sink it opened; the second start opens a *new*
    session and a *new* sink.
  - `starting_again_re_applies_the_output_setting_and_the_viewfinder_action` —
    the session log is split at every start: each session begins
    `set-output:3`, `viewfinder`, then captures only, so a start is a fresh run
    and not a resume. The five **leading frames** of each run never reach the
    sink (every accepted frame size is `1024x576`), so they went through a fresh
    **calibration window**.
  - `starting_when_running_and_stopping_when_stopped_are_harmless` —
    `already-running` and `already-stopped`, with the session and sink counts
    unchanged: no second session, no second sink, no panic.
  - `stopping_while_frames_are_arriving_is_bounded` — frames at 500 fps, stop
    measured with `Instant`; asserted under two seconds and the session released.
  - `the_state_can_be_queried_without_side_effects` — 50 queries while stopped and
    50 while running change neither the session count nor the sink count; the
    status after a stop reports `StreamEnd::Stopped`, one session and the measured
    frame size.
  - `a_stop_timeout_says_the_camera_may_still_be_held` — a two-second capture
    against a 50 ms stop timeout returns `FeedError::StopTimedOut` saying the
    camera may still be held, rather than waiting; when the capture returns the
    feed releases the session and closes the sink on its own, and the eventual
    stop is `already-stopped`.
- `camera-core/tests/start_stop_cycles.rs` (alone in its binary, because it counts
  the process's threads) — ten start/stop cycles after one warm-up: no live
  session, every sink opened was closed, no sink left open, and neither `/dev/fd`
  nor `ps -M` thread count grew.
- `camera-core/tests/control_socket.rs` — the surface over a real socket: status
  changes nothing, `started`/`already-running`/`stopped`/`already-stopped` as
  above, an unknown word answered with `error: …` rather than ignored, `shutdown`
  stopping the feed, closing the surface and removing the socket file, and a
  client that connects and says nothing not stalling it (the accepted socket is
  put back into blocking mode before its timeouts, per the `O_NONBLOCK`
  inheritance trap) — the second caller is answered in well under two seconds and
  the silent client is told no command arrived.
- `camera-cli/tests/control.rs` — the words and exit codes a script sees:
  `status` prints `stopped` and exits 3, `start` prints `started`, `status` prints
  `running` and exits 0, a second `start` prints `already-running`, `stop` prints
  `stopped`, `status` prints `stopped` again, a second `stop` prints
  `already-stopped`, a second start/stop cycle works, and `shutdown` prints
  `shutting-down`, exits zero and leaves no socket file. Plus: an unknown control
  word is refused before any connection, `serve` without `--socket` names the
  missing option, and a command with no surface to talk to fails immediately
  instead of hanging.

Demonstrated by hand, without hardware:

```
camera-cli serve --socket /tmp/cw.sock --synthetic &
camera-cli control --socket /tmp/cw.sock status
camera-cli control --socket /tmp/cw.sock start
camera-cli control --socket /tmp/cw.sock status
camera-cli control --socket /tmp/cw.sock stop
camera-cli control --socket /tmp/cw.sock shutdown
```

### Unverified (no camera, no OBS, no second instance)

- **Another application taking the camera after a stop.** The session is released
  in a synthetic run, which is the release on our side; that gphoto2 then lets
  something else open the M50, and that a second instance of this program can,
  needs the camera. Closing evidence: `camera-cli control … stop`, then
  `gphoto2 --auto-detect` / `EOS Utility` (or a second `camera-cli serve`) working
  without `PTP Timeout`.
- **The virtual camera device being released so a consumer application notices.**
  `serve` publishes to the same `StatsSink` `stream` uses; wiring the macOS sink
  into the CLI is where the OBS-side close becomes observable. Closing evidence:
  a consumer application losing and regaining the `OBS Virtual Camera` device
  across a stop/start, plus `GetVirtualCamStatus`.
- **Stop latency against a real capture.** The bound that matters on hardware is
  how long `capture_live_view_frame` takes; two seconds is the expected capture
  timeout plus one tick, not a measurement of an M50.
- **A menu bar item.** Not built; it is a second front end over
  `FeedController`, and would want its own ticket.
- **Windows.** The front end is a Unix domain socket; a Windows build needs the
  named pipe ticket 08 would add, and `camera-cli`'s `serve`/`control` arms name
  the Unix module directly, so ticket 08 owns making them compile there.

### Notes for later tickets

- The contract a Linux tray or a Windows named pipe implements is
  `camera_core::control::FeedController`: `start`, `stop`, `state`, `status`. The
  reply words are `camera_core::control::{ControlRequest, ControlReply}`, shared
  by the server and the client so both sides of the protocol are in one place.
- `StreamEnd::Stopped` is a new ending and is **not** a failure
  (`StreamEnd::is_failure`), so nothing that treats a non-`DurationElapsed`
  ending as an error needs changing, and anything new must not.
- `runner::run_recovering_until` is `run_recovering` plus a `StopToken`;
  `run_recovering` passes a token nothing can set, so existing callers are
  unchanged.
- `camera-cli serve` publishes to `StatsSink`, exactly as `stream` does.
  Ticket 04's notes refer to `stream --sink virtual-camera`, which does not
  exist; wiring the macOS sink into the CLI is still outstanding and is not part
  of this ticket.
