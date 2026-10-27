# 04: Virtual camera sink and macOS backend

**What to build:** The sink abstraction — open, send frame, close — with a macOS implementation built from the decision in 03. This is the first slice where the M50 appears as a selectable camera in another application.

**Blocked by:** 02, 03

**Status:** resolved (hardware verification outstanding)

- [x] The sink is an interface with one implementation per platform, so a platform backend can be added or replaced without touching capture or pipeline code.
- [x] The macOS backend implements exactly the mechanism chosen in 03, and carries none of the two rejected mechanisms as dead scaffolding.
- [ ] A consumer video application (Discord, Teams, or Meet) lists the virtual camera and shows the live feed.
- [ ] The feed is stable for a 10-minute call without visible stalls or accumulating delay.
- [x] Opening the sink fails with an actionable message when its prerequisites are missing (for example OBS not installed, or the device not yet permitted), and names the command or setting the user must apply.
- [ ] Closing the sink releases the device so the next consumer app can open it.
- [x] The sink is opened at the measured frame size, and no frame of any other size is ever handed to it. A size change closes and re-opens the sink; it never reinterprets the sink already open.
- [ ] The measured frame size and rate printed at startup match what the consumer application reports.

## Comments

### What was verified, and how

`camera-core/tests/macos_virtual_camera_sink.rs` drives the backend against a
fake obs-websocket v5 server, so it proves the protocol and not the hardware:

- **The mechanism, and only the mechanism.** `opening_authenticates_and_drives_the_scene_input_and_virtual_camera_in_adr_order`
  asserts the handshake, the challenge authentication, the scene and input
  creation and `StartVirtualCam` in the order ADR-0002 measured. No MJPEG or DAL
  path exists in the backend to fall back to.
- **`StartVirtualCam` returning success is not the camera running.**
  `an_accepted_start_virtual_cam_that_leaves_the_camera_down_is_a_failure`
  reproduces the trap ADR-0002 recorded: the sink only believes the camera is up
  when `GetVirtualCamStatus` says `outputActive`.
- **Actionable prerequisites.** Four tests each fail on one missing piece — OBS
  outside `/Applications`, an absent virtual camera device, a closed port and a
  wrong password — and assert the message names the setting or the step to apply.
- **Frame delivery.** `a_frame_reaches_the_consumer_page_as_the_jpeg_the_camera_delivered`
  asserts the page receives the camera's own JPEG byte-for-byte; the
  resized-frame test asserts one encode at the frame's own size; the mismatch
  test asserts a frame of another size is refused rather than reinterpreted.
- **Close and re-open.** `closing_stops_the_virtual_camera_removes_what_we_made_and_is_idempotent`
  asserts the teardown order and the restore of the consumer's own program
  scene; `a_sink_re_open_delivers_the_new_frame_size_and_nothing_else` asserts a
  size change is exactly one re-open; `a_lost_connection_while_delivering_ends_the_sink_with_obs_disconnected`
  asserts a dead control channel ends the sink instead of pretending to deliver.

### Defect found while implementing

`TcpListener::set_nonblocking(true)` is inherited by sockets returned from
`accept()` on macOS (verified: `fcntl(F_GETFL)` shows `O_NONBLOCK` on the accepted
socket). The frame transport read the page's request head as `WouldBlock` and
dropped the connection, so OBS's browser source would have shown nothing. Fixed
with `set_nonblocking(false)` plus a bounded request-head read. Any loopback
server added later needs the same line; `set_read_timeout` does not compensate
for a non-blocking socket.

### Unverified (no OBS in `/Applications`, no camera attached)

- **The consumer app lists the camera.** Needs OBS installed under
  `/Applications` and the camera extension approved, then a Discord/Teams/Meet
  device list. Nothing in this environment can produce that evidence.
- **Ten minutes of stable feed.** Same prerequisite, plus a real M50 as the frame
  source: `cargo run -p camera-cli -- stream --sink virtual-camera --duration-secs 600`.
- **The device is released for the next consumer app.** The teardown order is
  asserted, but that another app can then open the device is only observable
  across two real applications.
- **Startup size and rate match the consumer's report.** The fake server never
  decodes a frame, so it cannot report a geometry to compare against.
