# Bind the camera with the `gphoto2` crate, not the `gphoto2` CLI

The capture layer binds libgphoto2 directly through the `gphoto2` crate (version 3.4.1) and holds one owned camera handle for the whole run. ADR-0001 left this open: the implementation plan assumed the crate, while every piece of evidence in `docs/internal/plan/test-results.md` came from the CLI. The choice is now made explicitly, from a spike rather than from the plan.

## Spike result

The spike was run on 2026-10-27 against libgphoto2 2.5.34 with **no camera attached**, so it answers "can this binding express the operations" and not "does the M50 accept them". The crate source in `~/.cargo/registry/src/*/gphoto2-3.4.1` was read, and a program using the three operations was compiled and run against the installed library.

| Capability the design needs | Public API in `gphoto2` 3.4.1 | Result |
| --- | --- | --- |
| Write the `output` property | `Camera::config_key::<RadioWidget>("output")`, `RadioWidget::set_choice`, `Camera::set_config` | Present |
| Press the viewfinder action `/main/actions/viewfinder` | `Camera::config_key::<Widget>(...)` then `ButtonWidget::press`, or `ToggleWidget::set_toggled`, or `TextWidget::set_value` | Present |
| Return live view frames with their byte payload | `Camera::capture_preview` → `CameraFile::get_data` → `Box<[u8]>` | Present |

The compiled program ran, reported `libgphoto2: Some("2.5.34")` and `cameras detected: 0`.

**Unverified (no camera attached):** which widget type the M50 reports for `/main/actions/viewfinder`, and whether `capture_preview` on an M50 returns full size frames while `output` is `TFT + PC`. Nothing in this repository has measured a frame with the crate. Both facts are recorded in the module documentation and in the ticket, and the binding handles the button, toggle, text and radio widget shapes and refuses the rest with a message rather than guessing.

## Why the crate rather than the CLI

- **The rule becomes structural.** `CameraSession` owns one `Camera` handle, is not `Clone`, and its only constructor takes the process-wide session claim. ADR-0001's rule cannot be broken by adding a call, only by editing the type.
- **Settings can be interleaved with a running capture.** Recalibration needs the `output` setting and the viewfinder action changed while a capture is already running, and stop/start needs the session released and retaken. No CLI form can do that without a second process, which is exactly what ADR-0001 forbids.
- **The CLI alternative was only ever correct in one shape**, one `gphoto2` invocation per run carrying every setting and the capture. It was verified working, but it leaves no way to react to frames as they arrive, so the pipeline in later tickets could not use it.

## Considered options

- **`gphoto2` CLI, one invocation per run.** Verified working and the cheapest form, but it cannot interleave a setting change with an open capture, so the sink could never be re-opened at a newly measured frame size without a second process.
- **`gphoto2 --shell` held open as a subprocess.** Satisfies the single-session rule, but makes the session's lifetime depend on a child process's pipes, and leaves the frame payload as a file or a pipe rather than a buffer in this process.
- **The `gphoto2` crate with an owned handle.** Chosen. It needs libgphoto2 present at build and run time, which is a package install on macOS and Linux and hard on Windows. The Windows backend therefore keeps the CLI or targets the GNU toolchain; that fork belongs to the Windows ticket.

## Consequences

- `camera-core` has one non-optional native dependency, `libgphoto2`, behind the default-enabled `gphoto2-binding` feature. `pkg-config`, the library and its headers are required to build.
- The `CamerasError`-free framing matters: a camera that is absent from the USB bus is reported as a **disconnected camera**, and a `PTP Timeout` from a camera that is still detected is reported as a possible **wedged camera**, because the remedies differ.
- The viewfinder action is pressed on every run inside the session that captures, because it does not persist.
- `liveviewsize` is not exposed by `PtpSession` at all, so no later ticket can reach for it by accident.
