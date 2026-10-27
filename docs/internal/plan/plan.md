Based on the research, this response first identifies the best language and framework. Then it gives a high-level architectural plan in Simplified Technical English.

---

## 1. Research Summary: Language and Framework

### 1.1 The Hardware Constraint Comes First

Before choosing a language, the plan must account for the Canon EOS M50's hardware limits.

The M50 does **not** output a clean 1080p signal over HDMI. The HDMI output contains overlays and is not suitable for direct capture as a clean video feed. Over USB, Canon's own EOS Webcam Utility gives the M50 a resolution **lower than 1080p** (typically 1024×576 or similar). Some third-party tools report 1080p over USB, but this is not guaranteed by the camera firmware and may depend on the specific USB/PTP mode or driver.

**Architectural consequence:** The software cannot promise true 1080p from the M50 over USB or HDMI. The plan must include a **capability probe** step that asks the camera what live-view resolutions it supports, and then exposes the highest available resolution. The configuration must accept 1080p as a request, but the software must fall back to the highest supported resolution and clearly report the actual output to the user. This prevents the coding agent from building a system that fails silently when the camera does not provide 1080p.

### 1.2 Recommended Language: Rust

Rust is the best fit for this project.

- It compiles to native executables on Windows, macOS, and Linux.
- It has mature crates for the two hardest parts: camera capture and virtual-camera output.
- It gives memory safety for long-running video pipelines without a garbage collector, which reduces the risk of frame drops.
- It can call platform-specific C/Objective-C APIs through FFI when a pure-Rust solution is not available.

The main alternative is C++. C++ is what most virtual-camera drivers use, but it requires more manual memory management and makes the cross-platform build more complex. Rust gives the same low-level access with fewer classes of bugs.

### 1.3 Capture Framework: `gphoto2` (via Rust bindings)

The M50 is a PTP camera. The most reliable open-source way to get live-view frames is `libgphoto2`. The Rust crate `gphoto2` wraps `libgphoto2` and exposes camera discovery, configuration, and movie capture.

The capture path is already proven on Linux: `gphoto2 --stdout --set-config liveviewsize=0 --capture-movie` produces a live-view stream, and the `liveviewsize=0` setting requests the largest available size on the M50. The same library works on Windows and macOS, so the capture layer can be shared across all three platforms.

### 1.4 Virtual-Camera Framework: `virtualcam` Crate + Platform Fallbacks

The Rust crate `virtualcam` provides one API for sending frames to a virtual camera. It supports:

- **Windows 11:** Media Foundation (bundled VCam source) or OBS Virtual Camera.
- **Windows 10/11:** OBS Virtual Camera shared-memory queue.
- **Linux:** `v4l2loopback` via `mmap` streaming.

It does **not** currently provide a native macOS backend. On macOS, the two practical options are:

1. **Require OBS Virtual Camera** as an external dependency. OBS Studio 26.1+ ships a native virtual camera on macOS and Linux, and on Windows it is built in from 26.0.
2. **Ship a CoreMediaIO DAL plugin** written in Swift or Objective-C, and call it from Rust through FFI.

For the first implementation, the plan should use `virtualcam` for Windows and Linux, and use the **OBS Virtual Camera backend on macOS**. This keeps the first version smaller. A native macOS CoreMediaIO plugin can be added later as an optional enhancement.

### 1.5 Summary of the Recommended Stack

| Layer                        | Choice                                                                      |
| ---------------------------- | --------------------------------------------------------------------------- |
| **Language**                 | Rust (stable toolchain)                                                     |
| **Capture**                  | `gphoto2` crate → `libgphoto2` → PTP live view                              |
| **Virtual camera (Windows)** | `virtualcam` crate → Media Foundation / OBS                                 |
| **Virtual camera (Linux)**   | `virtualcam` crate → `v4l2loopback`                                         |
| **Virtual camera (macOS)**   | OBS Virtual Camera backend (first version); native CoreMediaIO plugin later |
| **Build system**             | Cargo workspace                                                             |
| **Configuration**            | TOML file + command-line overrides                                          |
| **GUI**                      | Optional; start with a CLI and a tray icon                                  |

---

## 2. High-Level Plan (Simplified Technical English)

### 2.1 Purpose

The software makes a Canon EOS M50 available as a virtual camera on Windows, macOS, and Linux. Other programs (Discord, Teams, Google Meet, etc.) select the virtual camera like a normal webcam. The software captures live-view frames from the camera over USB and sends them to the operating system's virtual-camera framework.

### 2.2 System Context and Data Flow

```
[M50 Camera]
     |
     | USB / PTP
     v
[Capture Layer]  (gphoto2)
     |
     | Raw frames (JPEG or uncompressed)
     v
[Frame Pipeline]  (decode → convert → scale)
     |
     | Frames in the backend's native format
     v
[Virtual Camera Sink]  (virtualcam crate or OBS)
     |
     | Normal camera API
     v
[Consumer Application]  (Discord, Teams, Meet, etc.)
```

The data flow is one-directional. No audio is captured from the camera in the first version. Audio can be added later as a separate module.

### 2.3 Major Components

#### 2.3.1 Camera Discovery and Capability Probe

> **DELETED. Do not implement this step.** See
> `docs/adr/0004-no-live-view-size-selection.md`. `liveviewsize` is a no-op on Canon, the
> M50 accepts no live-view size request, and the frame size is established by a
> calibration window from the captured frames themselves. A program that reported a size
> read from a property would report a confidently wrong answer.
>
> The text below is kept only as a record of what was planned.

- On startup, the software scans USB ports for a Canon EOS M50.
- It opens the camera with `gphoto2`.
- ~~It reads the camera's live-view size options. The M50 reports large, medium, and small options; the software selects the largest option that the camera accepts.~~ **Void.** No live-view size is read, requested or selected. See `docs/adr/0004-no-live-view-size-selection.md`.
- It reads the camera's supported frame rates and pixel formats.
- It writes the actual capabilities to the log and to the configuration state. The requested 1080p resolution is treated as a **preference**, not a guarantee. If the camera cannot deliver 1080p, the software uses the highest available resolution and continues.

#### 2.3.2 Capture Thread

- A dedicated capture thread owns the `gphoto2` camera handle.
- The thread calls the movie-capture function in a loop and receives raw frames.
- The thread timestamps each frame and pushes it into a bounded queue.
- If the queue is full, the oldest frame is dropped. This prevents latency from growing when the consumer is slow.
- The capture thread must not block on frame conversion or virtual-camera output. It only captures and enqueues.

#### 2.3.3 Frame Pipeline

- A worker thread or a small thread pool takes frames from the capture queue.
- If the camera outputs JPEG (common for PTP live view), the pipeline decodes the JPEG to a raw pixel buffer.
- The pipeline converts the raw buffer to the pixel format that the virtual-camera backend expects. The `virtualcam` crate accepts RGB, BGR, RGBA, grayscale, I420, NV12, YUYV, and UYVY.
- If the camera resolution is lower than the requested resolution, the pipeline can upscale. Upscaling does **not** create real detail; it only makes the frame size match the requested output. The configuration must let the user disable upscaling.
- The pipeline writes the converted frame into a shared frame buffer that the virtual-camera sink reads.

#### 2.3.4 Virtual Camera Sink

The virtual-camera sink is an abstraction with one trait and three implementations.

**Trait contract (conceptual):**

- `open(width, height, fps, pixel_format)`
- `send_frame(frame_buffer)`
- `close()`

**Windows implementation:**

- Use the `virtualcam` crate.
- Prefer the Media Foundation backend on Windows 11. Fall back to the OBS backend on Windows 10 or when Media Foundation is not available.

**Linux implementation:**

- Use the `virtualcam` crate with the `v4l2` backend.
- The software must check that `v4l2loopback` is loaded. If it is not loaded, the software prints a clear error message with the command to load it.

**macOS implementation (first version):**

> Superseded by `docs/adr/0002-macos-virtual-camera-mechanism.md`. OBS Virtual Camera remains the virtual camera mechanism, but frames do not reach it through a source we "send" to: they reach a browser source in OBS over a local WebSocket, and the virtual camera is started and stopped with obs-websocket. The ADR is authoritative; the paragraphs below describe the assumption it replaced.

- Use the OBS Virtual Camera backend.
- The software checks that OBS is installed and that the OBS Virtual Camera device is registered.
- If OBS is not installed, the software prints a clear error message and stops.
- A native CoreMediaIO DAL plugin is deferred to a later phase. The sink trait makes this replacement possible without changing the capture or pipeline code.

#### 2.3.5 Configuration and Control

- The software reads a configuration file in TOML format.
- Configuration keys include: `resolution` (requested), `fps`, `pixel_format`, `upscale` (true/false), `camera_port` (optional), and `backend` (auto / obs / mediafoundation / v4l2).
- Command-line arguments override the file.
- The software prints the **actual** negotiated resolution and frame rate at startup.
- The software exposes a minimal control interface (for example, a local socket or a tray menu) to start and stop the virtual camera without restarting the process.

### 2.4 Threading Model

Use three threads plus one optional UI thread.

1. **Capture thread:** Reads frames from `gphoto2` and enqueues them.
2. **Pipeline thread:** Dequeues raw frames, converts them, and writes them to the virtual-camera buffer.
3. **Sink thread (or callback-driven sink):** The `virtualcam` crate is designed to be called from the producer side. The pipeline thread can call `send` directly. If the backend requires a separate pacing thread, add a sink thread.
4. **UI thread (optional):** Runs only if a tray icon or control window is enabled. It communicates with the other threads through channels.

All shared state must be protected by channels or atomics. Do not use global mutable state.

### 2.5 Error Handling and Observability

- Every component returns a `Result` with a typed error. The top-level binary prints a human-readable error and exits with a non-zero code.
- The log includes: camera model, selected live-view size, actual frame size, actual frame rate, selected virtual-camera backend, and dropped-frame count.
- If the camera disconnects, the capture thread stops, the sink closes, and the process exits with a clear message. A later version can add automatic reconnect.

### 2.6 Build and Packaging

- Use a Cargo workspace with these crates:
  - `camera-core` (capture, pipeline, traits)
  - `virtualcam-backend` (platform sink implementations)
  - `camera-cli` (binary)
  - `camera-gui` (optional, later)
- On Linux, the build requires `libgphoto2-dev` and `libv4l2loopback-dev`.
- On macOS, the build requires Xcode command-line tools and `libgphoto2` via Homebrew.
- On Windows, the build requires the MSVC toolchain and the `gphoto2` dependency built through `vcpkg` or a prebuilt DLL.
- Provide a `Makefile` or `justfile` with targets: `build`, `test`, `package`, `install-v4l2loopback` (Linux), and `install-obs-virtualcam` (macOS, for the first version).

### 2.7 Testing Plan

- **Unit tests:** Frame conversion, queue behavior, configuration parsing.
- **Integration test (Linux):** Use `v4l2loopback` and a recorded frame source to verify that frames reach a consumer application.
- **Integration test (Windows):** Use the OBS Virtual Camera backend and a DirectShow test consumer.
- **Integration test (macOS):** Use OBS Virtual Camera and a simple AVFoundation consumer.
- **Hardware-in-the-loop test:** Connect an M50 and verify that the capability probe reports the correct live-view size. This test cannot run in CI without a physical camera; mark it as manual.

### 2.8 Phased Delivery

1. **Phase 1:** Capture thread + pipeline + Linux `v4l2` sink. CLI only. Prove that frames flow from the M50 to `/dev/videoX`.
2. **Phase 2:** Add Windows OBS and Media Foundation sinks. Add configuration file and capability logging.
3. **Phase 3:** Add macOS OBS sink. Add tray icon and start/stop control.
4. **Phase 4 (optional):** Replace the macOS OBS dependency with a native CoreMediaIO DAL plugin. Add automatic reconnect.

### 2.9 Key Architectural Decisions to Carry Forward

- **Rust workspace** with separated crates for core logic and platform backends.
- **`gphoto2` as the only capture path** on all platforms; do not attempt HDMI capture in software.
- **`virtualcam` crate for Windows and Linux**, **OBS Virtual Camera for macOS** in the first version.
- **Sink trait abstraction** so the macOS backend can be replaced without touching capture or pipeline code.
- **Capability probe before configuration** so the software reports the real resolution instead of assuming 1080p.
- **Bounded queues and explicit frame dropping** to keep latency predictable.
- **Clear error messages** for missing `v4l2loopback`, missing OBS, and camera disconnect.
