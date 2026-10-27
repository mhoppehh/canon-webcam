The plan depends on several assumptions about the M50's behavior that cannot be confirmed from documentation alone. The search results show that reported live-view resolutions for the M50 vary widely—from 512×288 to 1024×576, with some users claiming 1080p via Windows tools—and that platform-specific driver issues on Windows and macOS can block capture entirely. A live test pass is necessary before the coding agent commits to the architecture.

---

# Live Validation Test Plan: Canon EOS M50 Virtual Camera

## 1. Purpose and Scope

This document defines the live tests that must pass **before** the implementation plan is committed. Each test validates one architectural assumption. If a test fails, the plan must be revised—not the test.

The tests are ordered by risk. Tests marked **BLOCKING** must pass before any code is written. Tests marked **PHASE GATE** must pass before the corresponding phase begins.

**Hardware prerequisite:** One Canon EOS M50, one USB cable, one computer per target OS (or virtual machines where USB passthrough is reliable). The camera must be in a mode that permits live view (not playback, not menu).

---

## 2. Test Groups

### Group A: Platform Capture Viability (BLOCKING)

These tests confirm that `gphoto2` can see and open the M50 on each target operating system.

#### Test A1 — Linux: Camera detection and live-view open

- **Assumption:** `gphoto2` discovers the M50 over USB on Linux without special driver configuration.
- **Procedure:**
  1. Connect the M50. Run `gphoto2 --auto-detect`. Confirm the M50 appears.
  2. Run `gphoto2 --set-config viewfinder=1` and then `gphoto2 --capture-movie=5s`. Confirm frames are produced.
- **Pass criterion:** Camera is listed by `--auto-detect`, and `--capture-movie` exits without a PTP error.
- **Relevant risk:** If detection fails, the entire capture layer assumption is invalid on Linux.

#### Test A2 — Windows: Driver substitution for libusb access

- **Assumption:** `libgphoto2` can access the M50 on Windows after installing a libusb-compatible driver (WinUSB via Zadig or equivalent).
- **Procedure:**
  1. Connect the M50. Confirm Windows does **not** automatically bind it to a driver that locks out libusb.
  2. Use Zadig to replace the camera's driver with WinUSB. Confirm the device remains visible in Device Manager.
  3. Build or install a minimal `gphoto2` binary for Windows. Run `gphoto2 --auto-detect` and `gphoto2 --capture-movie=5s`.
- **Pass criterion:** Camera is detected and frames flow after the driver substitution.
- **Relevant risk:** The gphoto2 maintainers confirm that Windows requires a manual libusb driver install, and this breaks Canon's own software. If this step cannot be automated or documented cleanly, the Windows capture path needs a fallback strategy (for example, shipping a signed libusb driver package or detecting when the wrong driver is bound).

#### Test A3 — macOS: Coexistence with `ptpcamerad`

- **Assumption:** `libgphoto2` can claim the M50 on macOS even though Apple's `ptpcamerad` daemon competes for PTP access.
- **Procedure:**
  1. Connect the M50. Run `gphoto2 --auto-detect` without killing any daemons.
  2. If detection fails or times out, kill `ptpcamerad` (`sudo pkill ptpcamerad`) and retry.
  3. Run `gphoto2 --capture-movie=5s` and confirm frames flow.
  4. Repeat after reconnecting the camera. Observe whether `ptpcamerad` restarts automatically and blocks access again.
- **Pass criterion:** Frames flow either on the first attempt or after a documented, repeatable daemon intervention. If `ptpcamerad` restarts and re-blocks access every time the camera is reconnected, the macOS capture path is **not reliable** as written and requires a different approach (for example, an ImageCaptureCore-based backend).
- **Relevant risk:** The gphoto2 issue tracker documents that on macOS 13 and later, `ptpcamerad` restarts whenever an application touches the camera, making `pkill` unreliable. This is the single largest threat to the macOS first-version plan.

---

### Group B: Live-View Resolution and Frame Rate (BLOCKING)

These tests determine what the M50 actually delivers through `gphoto2`, not what Canon's marketing materials claim.

#### Test B1 — Enumerate live-view size options

- **Assumption:** The M50 exposes multiple live-view sizes and `liveviewsize=0` selects the largest.
- **Procedure:**
  1. Run `gphoto2 --get-config liveviewsize` and record the options and current value.
  2. For each option (0, 1, 2), set it and capture a single frame to a file (`gphoto2 --set-config liveviewsize=N --capture-preview --filename probe_N.jpg`).
  3. Inspect each file's dimensions with an image tool.
- **Pass criterion:** At least one option produces a frame larger than 1024×576. If the largest option is 1024×576 or smaller, **the 1080p requirement in the plan is not achievable via `gphoto2` on this camera** , and the plan must be revised to either (a) relax the resolution target, or (b) add a different capture path.
- **Relevant risk:** Community reports for the M50 range from 512×288 to 1024×576 through `gphoto2`. A Windows tool (SparkoCam) reportedly achieves 1080p, but this does not prove `libgphoto2` can.

#### Test B2 — `output` setting interaction

- **Assumption:** Setting `output="TFT + PC"` is required (or beneficial) for stable movie capture, and it does not change the available resolution.
- **Procedure:**
  1. Run `gphoto2 --get-config output` and record the options.
  2. Capture a 10-second movie with the default output setting. Record the frame count and file size.
  3. Capture a 10-second movie with `--set-config output="TFT + PC"`. Record the same metrics.
  4. Extract one frame from each movie and compare dimensions.
- **Pass criterion:** Both settings produce frames. If `output="TFT + PC"` is required to avoid crashes or stalls, the plan must include it as a mandatory startup configuration. If it changes the resolution, the plan must account for that.
- **Relevant risk:** A gphoto-user thread shows that `output="TFT + PC"` made movie capture work on the M50, and that pressing the shutter could previously crash the capture.

#### Test B3 — Sustained frame rate over 60 seconds

- **Assumption:** The M50 can deliver a usable frame rate (target: ≥ 24 fps, acceptable: ≥ 15 fps) over a sustained period without the camera overheating or the USB connection dropping.
- **Procedure:**
  1. Set the largest live-view size. Run `gphoto2 --capture-movie=60s` and save the MJPEG stream.
  2. Count the total frames in the output. Calculate average fps.
  3. Monitor the camera body temperature (by touch or IR thermometer) at the start and end of the test.
  4. Repeat with the camera connected to a USB 3.0 port (if available) and a USB 2.0 port. Record the fps for each.
- **Pass criterion:** Average fps ≥ 15 for the full 60 seconds, with no USB disconnect. If fps drops below 10 or the camera disconnects, the plan's "bounded queue and drop frames" design is insufficient—the capture thread itself is the bottleneck.
- **Relevant risk:** Low frame rates through `gphoto2` are widely reported for Canon bodies (for example, 44 frames in 10 seconds on an EOS 650D). USB 2.0 vs 3.0 can materially change throughput. Thermal throttling on the M50 is documented after ~18 minutes of HDMI output, but USB live view may behave differently.

---

### Group C: Virtual Camera Backend Integration (PHASE GATE — Platform-Specific)

These tests confirm that frames from the M50 can reach a consumer application through each planned sink.

#### Test C1 — Linux: `v4l2loopback` end-to-end

- **Assumption:** A pipe from `gphoto2` through `ffmpeg` into `v4l2loopback` produces a device that Discord/Teams/Meet can read.
- **Procedure:**
  1. Load `v4l2loopback` with `exclusive_caps=1` and a known label (`card_label="M50_Test"`).
  2. Run `gphoto2 --stdout --set-config liveviewsize=0 --capture-movie | ffmpeg -fflags nobuffer -flags low_delay -i - -vcodec rawvideo -pix_fmt yuv420p -f v4l2 /dev/videoX`.
  3. Open a browser-based test page (for example, webcamtests.com) and select the `M50_Test` device. Confirm video appears.
  4. Measure end-to-end latency: point the camera at a stopwatch and capture the screen showing both the physical stopwatch and the browser preview. Subtract the displayed times.
- **Pass criterion:** Video appears in the browser and latency is **≤ 500 ms**. If latency exceeds 500 ms, the plan must add a latency budget and a low-latency configuration preset.
- **Relevant risk:** Users report 0.5–1 s latency with the naive `gphoto2 | ffmpeg` pipe. The `-fflags nobuffer` and `-tune zerolatency` flags reduce this, but the actual floor must be measured on the target hardware.

#### Test C2 — Windows: Virtual camera sink with OBS or Media Foundation

- **Assumption:** The `virtualcam` crate (or an equivalent mechanism) can expose M50 frames as a system camera on Windows.
- **Procedure:**
  1. Install OBS Studio. Confirm OBS Virtual Camera appears in the Windows camera list (Settings → Privacy → Camera, or in a browser test).
  2. Build or run a minimal producer that sends a solid color or a test pattern to the `virtualcam` OBS backend. Confirm the test pattern appears in a consumer application.
  3. If the Windows 11 Media Foundation backend is available, repeat with that backend.
  4. Connect the M50 via `gphoto2` and pipe its frames into the same sink. Confirm the live feed appears.
- **Pass criterion:** The virtual camera device is selectable in at least one consumer application (Teams, Discord, or a browser). If OBS Virtual Camera is the only working backend, the plan must document OBS as a **mandatory dependency** for Windows.
- **Relevant risk:** The `virtualcam` crate supports Windows 11 Media Foundation and OBS backends, but availability depends on OS version and installed components. This test confirms which backend is actually usable on a representative machine.

#### Test C3 — macOS: OBS Virtual Camera end-to-end

- **Assumption:** OBS Virtual Camera on macOS can receive frames from a `gphoto2`-based producer and expose them to consumer applications.
- **Procedure:**
  1. Install OBS Studio on macOS. Start the Virtual Camera (Tools → Start Virtual Camera). Confirm the device appears in a browser test or in Zoom/Teams.
  2. Build a minimal producer that sends a test pattern to OBS (for example, via a virtual output or a syphon-like mechanism). Confirm the test pattern appears.
  3. Connect the M50 and confirm the live feed reaches the OBS Virtual Camera output.
- **Pass criterion:** The live M50 feed appears in at least one consumer application on macOS. If OBS cannot accept frames from an external producer without manual scene configuration, the plan must include an OBS scene template or a headless OBS configuration step.
- **Relevant risk:** The macOS virtual camera landscape is fragmented: Apple's own apps (FaceTime, Photo Booth) refuse third-party virtual cameras, and OBS Virtual Camera requires OBS to be running. The test must confirm that the **target consumer applications** (Teams, Meet, Discord) accept the OBS device.

---

### Group D: Consumer Application Compatibility (PHASE GATE)

These tests confirm that the virtual camera is recognized by the programs the user actually cares about.

#### Test D1 — Discord, Teams, and Google Meet on each platform

- **Assumption:** Each consumer application enumerates the virtual camera device and displays its feed without special configuration.
- **Procedure:**
  1. With the virtual camera active and producing a live M50 feed, open Discord → Voice & Video settings. Select the virtual camera. Confirm the preview shows live video.
  2. Repeat in Microsoft Teams (Devices settings) and Google Meet (browser-based, using a test meeting).
  3. Record the exact device name each application displays.
- **Pass criterion:** At least one of the three applications works on each platform. If a specific application fails, record the failure mode (device not listed, device listed but black, device listed but frozen) so the plan can include a troubleshooting matrix.
- **Relevant risk:** Virtual camera device naming varies by backend, and some applications cache device lists aggressively. A device that works in a browser may not appear in a desktop app until restart.

---

### Group E: Stability, Recovery, and Resource Usage (PHASE GATE)

These tests validate the plan's error-handling and threading assumptions.

#### Test E1 — Camera disconnect during streaming

- **Assumption:** The capture thread detects a disconnect, the sink closes cleanly, and the process exits with a clear error (or reconnects, if reconnect is implemented).
- **Procedure:**
  1. Start a live stream through the full pipeline.
  2. Physically unplug the USB cable.
  3. Observe the software's behavior: does it hang, crash, or print a clear error?
  4. Reconnect the camera. If the software supports reconnect, confirm the stream resumes. If not, confirm the software does not leave the virtual camera device in a broken state.
- **Pass criterion:** No hang, no crash without an error message. The virtual camera device is either cleanly removed or remains selectable with a placeholder frame.
- **Relevant risk:** USB disconnects are common in webcam use. A blocking `gphoto2` call that never returns would freeze the capture thread and leave the sink waiting indefinitely.

#### Test E2 — CPU and memory usage over 30 minutes

- **Assumption:** The pipeline (decode + convert + sink) is lightweight enough to run alongside a video call.
- **Procedure:**
  1. Run the full pipeline for 30 minutes at the target resolution and frame rate.
  2. Sample CPU usage (per-core and total) and memory usage every 60 seconds.
  3. Record whether memory usage grows monotonically (a leak) or stabilizes.
- **Pass criterion:** Total CPU usage stays below 30% of one modern core (to leave headroom for the video call), and memory usage stabilizes within ±10% after the first 5 minutes.
- **Relevant risk:** MJPEG decoding and color-space conversion can be CPU-intensive if done naively. If CPU usage is too high, the plan must add a hardware-accelerated decode path or downscale earlier in the pipeline.

---

## 3. Decision Matrix

After all tests are executed, fill in the following matrix. Any row marked **FAIL** on a **BLOCKING** test triggers a plan revision.

| Test                     | Platform | Result | Blocking?    | Plan impact if FAIL                                                        |
| ------------------------ | -------- | ------ | ------------ | -------------------------------------------------------------------------- |
| A1 — Linux detection     | Linux    |        | **BLOCKING** | Capture layer unusable on Linux                                            |
| A2 — Windows driver      | Windows  |        | **BLOCKING** | Windows capture path requires alternate driver strategy                    |
| A3 — macOS `ptpcamerad`  | macOS    |        | **BLOCKING** | macOS capture path requires ImageCaptureCore backend                       |
| B1 — Max resolution      | All      |        | **BLOCKING** | 1080p target unachievable via gphoto2; relax target or change capture path |
| B2 — `output` setting    | All      |        | **BLOCKING** | Add mandatory startup config                                               |
| B3 — Sustained fps       | All      |        | **BLOCKING** | Capture thread bottleneck; redesign pacing                                 |
| C1 — Linux v4l2 loopback | Linux    |        | PHASE GATE   | Pipeline latency too high; add latency budget                              |
| C2 — Windows sink        | Windows  |        | PHASE GATE   | Windows sink backend selection must change                                 |
| C3 — macOS OBS sink      | macOS    |        | PHASE GATE   | macOS first-version dependency on OBS confirmed or rejected                |
| D1 — Consumer apps       | All      |        | PHASE GATE   | Troubleshooting matrix required                                            |
| E1 — Disconnect recovery | All      |        | PHASE GATE   | Error handling design must be revised                                      |
| E2 — Resource usage      | All      |        | PHASE GATE   | Pipeline must be optimized before release                                  |

---

## 4. Execution Order and Stop Conditions

1. Run **Group A** first. If any A test fails, stop and revise the cross-platform capture assumption before proceeding.
2. Run **Group B** next. If B1 fails (maximum resolution is below 1080p), the 1080p requirement must be renegotiated with the stakeholder before any sink work begins.
3. Run **Group C** per platform. A platform whose C test fails cannot proceed to Phase 2 of the implementation plan for that platform.
4. Run **Group D** and **Group E** after the first full pipeline exists on at least one platform.

A test is considered **passed** only when the pass criterion is met on the exact OS version and hardware configuration that the software will support. Virtual machine results are acceptable for initial screening but must be repeated on bare metal before the final plan commit.
