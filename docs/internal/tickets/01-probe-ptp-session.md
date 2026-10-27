# 01: Capture session foundation and live-view measurement

**What to build:** A `measure` command that holds one PTP session against the M50, applies the acquisition settings, captures through a calibration window, and reports the **measured** frame size and rate — never the requested or nominal one. This is the first slice that talks to hardware, so it also establishes the Cargo workspace and the `camera-core` / `camera-cli` split.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [x] The command runs as a single CLI subcommand and holds **one** PTP session for the whole operation; a second concurrent session is a bug, not a fallback.
- [x] `output` is set to `TFT + PC` (value 3) at session start.
- [x] `viewfinder=1` is set in the **same invocation** as the capture, on every run — it does not persist across invocations.
- [x] The expected frame size is established by a **calibration window**, not by a fixed count of discarded frames. A fixed count is rejected outright: it locks onto the reduced size, because that is exactly what the leading frames are.
- [x] The reported size and rate are measured from frame data, never read from a property.
- [x] A capture that produces no size recurring often enough inside the window fails loudly and reports why.
- [x] The `output` value is guarded by an **allowlist of `{0, 1, 2, 3}`**, not a blocklist. Only value 3 (full size) and value 8 (`MOBILE2`, reduced size) are verified on an M50; the rest are rejected because they are unverified here, and because a report from a different Canon body describes combinations of `PC` and `MOBILE` needing a battery removal to clear. The guard's comment states which part is verified and which is not.
- [x] The command determines and records whether `output` survives across process boundaries. The guard must be written to set it on every run unless persistence is actually proven.
- [x] The binding is decided and recorded: the `gphoto2` CLI, or the `gphoto2` crate with an owned handle. The implementation plan assumes the crate while all current evidence came from the CLI, so the choice is made explicitly rather than inherited. See ADR-0001.
- [x] `liveviewsize` is documented as a **no-op on Canon** (it accepts the write and changes nothing; it exists for Nikon/Fuji/Sony). The command does not attempt to use it to select a size, and does not report it, because doing so produces a confidently wrong answer.
- [x] The measured result is printed so a human can read and diff it between runs. `1024x576` at roughly 24 fps is used as a sanity check on the measurement, never as its source.
- [x] A pre-implementation note records that the `PLAN.md` §2.3.1 "select the largest live-view size option" step is deleted, so nobody re-implements it.

## Comments

### Binding decision (spike result)

**Chosen: the `gphoto2` crate (3.4.1) with one owned handle.** Recorded in
`docs/adr/0003-gphoto2-crate-binding.md`.

The spike inspected the crate source at
`~/.cargo/registry/src/index.crates.io-*/gphoto2-3.4.1` and compiled a program
against the installed libgphoto2 2.5.34. All three capabilities the design needs
exist in the public API, and the program compiled and ran:

| Capability | API | Result |
| --- | --- | --- |
| Write the `output` property | `Camera::config_key::<RadioWidget>`, `RadioWidget::set_choice`, `Camera::set_config` | Present |
| Press the viewfinder action | `Camera::config_key::<Widget>` → `ButtonWidget::press` / `ToggleWidget::set_toggled` / `TextWidget::set_value` | Present |
| Frames with their byte payload | `Camera::capture_preview` → `CameraFile::get_data` → `Box<[u8]>` | Present |

The spike ran with **no camera attached** and reported
`libgphoto2: Some("2.5.34")`, `cameras detected: 0`.

### Unverified (no camera attached)

No Canon camera is attached to this machine and `gphoto2 --auto-detect` returns
nothing, so **nothing in this ticket was verified against hardware**. Specifically:

- Which widget type the M50 reports for `/main/actions/viewfinder`. The binding
  handles button, toggle, text, radio and range shapes and refuses the rest with a
  message rather than guessing.
- Whether `capture_preview` on an M50 returns full size frames while `output` is
  `TFT + PC`. No frame was ever captured, so no frame size and no frame rate in
  this repository is a measurement. The `1024x576` / ~24 fps figures quoted in the
  report are the **sanity check** values from `TEST-RESULTS.md`.
- Whether the M50 accepts the `output` write from this binding. The CLI evidence
  in `TEST-RESULTS.md` §4.5 is the only evidence that value 3 works.
- Whether `output` persists across process boundaries. The evidence read the value
  back **inside the same session**, which proves nothing about a later run, so the
  guard writes the setting on every run.

### `output` persistence

Not proven, therefore the session writes the setting on every run. Recorded in
`docs/adr/0003-gphoto2-crate-binding.md` and in `camera-core/src/output_setting.rs`.

### A detected device is not necessarily a camera

Found while running the binary with no camera attached: `gp_camera_autodetect`
answers with the `Directory Browse` pseudo-camera (port `disk:`), whose handle then
fails every operation with `Bad parameters`. Opening it makes a **disconnected
camera** look like a live session. `CameraSession::open` therefore lists the
descriptors, refuses any that is not on a bus, and reports
`SessionError::NoCamera` — the disconnected-camera state, which later recovery
work must keep distinct from a **wedged camera**. The no-camera path now prints:

```
error: no camera could be opened; check that the M50 is connected and that no other program holds it (libgphoto2 detected Directory Browse on disk:, but none of them is a camera on a bus)
```

### `liveviewsize` is a no-op on Canon

Recorded in `docs/adr/0004-no-live-view-size-selection.md`, in the
`camera-core` module docs, in the `PtpSession` trait docs and in the `--help`
text. The property is not reachable through the trait, so no later ticket can use
it by accident.

### `PLAN.md` §2.3.1 is deleted

The pre-implementation note is `docs/adr/0004-no-live-view-size-selection.md`, and
`PLAN.md` §2.3.1 now opens with a **DELETED** banner pointing at it. Nothing
selects, negotiates or reads a live view size.

### What was built

Cargo workspace: `camera-core` (library) and `camera-cli` (binary). Module map for
later tickets:

| Path | Contents |
| --- | --- |
| `camera-core/src/session.rs` | `PtpSession` trait, `SessionError`, `SessionOperation`, `SessionExclusivity`, `SessionClaim`, `process_session_exclusivity()` |
| `camera-core/src/gphoto_session.rs` | `CameraSession` (the one libgphoto2 session) |
| `camera-core/src/output_setting.rs` | `OutputSetting` guard, `OutputSettingError` |
| `camera-core/src/calibration.rs` | `CalibrationPolicy`, `CalibrationWindow`, `CalibrationOutcome`, `CalibrationError` |
| `camera-core/src/jpeg.rs` | `parse_frame_size`, `JpegError` |
| `camera-core/src/frame_size.rs` | `FrameSize` |
| `camera-core/src/measurement.rs` | `measure()`, `MeasurementPlan`, `Measurement`, `MeasureError`, `Clock`, `MonotonicClock` |
| `camera-cli/src/args.rs` | `parse()`, `Command`, `MeasureArgs`, `ArgError`, `USAGE` |
| `camera-cli/src/report.rs` | `render()` |

Gates: `cargo test` (65 tests), `cargo fmt --check` and
`cargo clippy --all-targets -- -D warnings` all pass with no camera present.

### Interpretation recorded

- The frame rate is derived from `sampling_frames - 1` intervals across the
  sampling window, so frames of an unexpected size still occupy a slot: the
  reported rate is the rate the camera delivered frames, and
  `unexpected frames` in the report says how many of them were not at the
  expected frame size.
- A camera that keeps changing size inside the sampling window is **not** papered
  over: if the expected frame size never reappears, the run fails with
  `MeasureError::ExpectedFrameSizeNotRecurring`. **Sink re-open** belongs to the
  sink ticket, not here.
- The default calibration policy is 72 frames or 3 s, a size recurring 8 times
  wins. Eight is above the five **leading frames** reported in
  `TEST-RESULTS.md`, with margin, and is not a discard count.

