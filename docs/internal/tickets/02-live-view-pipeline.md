# 02: Sustained live-view stream pipeline

**What to build:** A continuous live-view stream that survives a long run: capture thread feeding a bounded queue that drops the oldest frame when full, a pipeline that decodes JPEG and normalizes to one output pixel format, and honest statistics about what actually happened. Frames terminate in a stats sink in this ticket; no virtual camera yet.

This ticket also carries the configuration surface and the measurement reporting, because both are only meaningful once a pipeline exists and neither can be demonstrated on its own.

**Blocked by:** 01

**Status:** resolved (hardware verification outstanding)

- [ ] The capture loop runs for 60 seconds without interruption and reports frames delivered, frames dropped, measured rate, and data rate.
- [x] The queue is bounded and drops the **oldest** frame when full; latency does not grow when the consumer is slow.
- [x] The capture path never blocks on decode or on output.
- [x] The first leading frames of a run are handled by the calibration window, not by a discard count — leading frames appear only when the viewfinder action is toggled and are absent from captures that did not toggle it.
- [x] JPEG live-view frames are decoded and converted to one normalized pixel format that the sink layer can accept.
- [x] No frame handed downstream ever differs in size from the expected frame size. A frame of any other size is dropped, never forwarded.
- [x] About one second of consecutive size mismatches triggers a short recalibration; if it settles on a new size the sink is re-opened at that size, and if it does not settle the condition is escalated as wedged or disconnected.
- [x] A TOML config file supplies `resolution`, `fps`, `pixel_format`, `upscale` and `mechanism`; command-line arguments override the file. The key is `mechanism`, not `backend`: there is exactly one platform backend per build target, so it is not a user choice.
- [x] `mechanism` is validated against the platform in use; naming a mechanism that does not exist there, such as `v4l2` on macOS, is a configuration error and not a silent fallback.
- [x] Startup prints the measured frame size, the measured frame rate and the mechanism in use. Asking for 1080p prints `1024x576` and continues — it never echoes the request back as if it were the result.
- [x] `upscale` defaults off, and when enabled the upscaling is labelled as resizing, not as added detail.
- [ ] A 30-minute soak measures CPU and memory and shows no unbounded growth.
- [x] Rejected configuration fails with a message naming the offending key, before any camera connection is made.

## Comments

### What was verified, and how

No camera is attached to this machine, so the pipeline is demonstrated against
`camera_core::synthetic::SyntheticSession`: it replays the two committed JPEGs at
the only two **frame sizes** anyone has measured (`1024x576`, `512x288`) and
models the five **leading frames**, a mid-run size change, and both camera
failure states. Every pipeline test therefore runs real JPEG bytes at real
frame sizes, not hand-built markers.

Covered by tests:

- `camera-core/tests/stream_runner.rs` — a timed run reports what it did; the
  capture path only hands frames over; leading frames never reach the sink; a
  size change re-opens the sink; a camera that disappears is reported as
  **disconnected** and one that times out as **wedged**, with distinct remedies.
- `camera-core/tests/stream_pipeline.rs` — a frame of another size is dropped and
  never forwarded; under a second of mismatches does not recalibrate; about a
  second does; a recalibration that settles re-opens the sink; one that does not
  settle is reported rather than guessed at.
- `camera-core/tests/bounded_queue.rs` — drop-oldest with the newest frame
  surviving and depth bounded at capacity.
- `camera-core/tests/config.rs`, `camera-core/tests/pixel_format.rs`,
  `camera-core/tests/stats_sink.rs`.
- `camera-cli/tests/binary.rs` — `v4l2` on macOS is rejected before any camera is
  opened, and a `1080p` request prints `1024x576` as the measured size while the
  request stays labelled a preference.

Demonstrated without hardware:

```
cargo run -p camera-cli -- stream --synthetic --duration-secs 6 --resolution 1080p
```

### Unverified (no camera attached)

- **The 60-second capture loop.** The run is implemented (`--duration-secs`) and
  was exercised for 6 s against the synthetic source, delivering 65 frames with
  `measured frame size: 1024x576`. It has not run against an M50, so the claim
  "60 seconds without interruption" is untested. Command that would produce the
  evidence:
  `cargo run -p camera-cli -- stream --duration-secs 60`.
- **The 30-minute soak.** No CPU or memory growth was measured, because the soak
  needs the camera to be the frame source for 30 minutes. Command:
  `cargo run -p camera-cli -- stream --duration-secs 1800` while sampling
  `ps -o %cpu,rss -p <pid>` every 60 s. The synthetic source is not a substitute:
  it neither decodes camera output at the real rate nor proves anything about
  libgphoto2's memory behaviour.

### Notes for later tickets

- The sink contract ticket 04 implements is `camera_core::sink::FrameSink`
  (`open`, `accept`, `close`). `StatsSink` is a real implementation of it, so
  ticket 04 replaces the type, not the trait.
- `StreamPipelineConfig` owns the calibration policy, the recalibration policy,
  the mismatch grace and the resize target; `StreamingConfig::pipeline_config`
  builds it from the TOML/CLI settings.
- `camera_core::mechanism::VirtualCameraMechanism` is where ticket 04's macOS
  implementation is named; `Platform::mechanisms()` is the allowlist the
  validation already uses.
