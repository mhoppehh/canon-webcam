# Canon M50 Virtual Camera

This context makes a Canon EOS M50 usable as an operating-system camera. The M50 has no USB video class mode, so its live view must be captured over PTP, decoded, and re-published to the operating system as a virtual camera.

## Language

### Camera and session

**Live view**:
The continuous preview stream the M50 sends over USB while its screen is active. It is not a recording, and it is not the HDMI signal.
_Avoid_: preview, movie, stream

**PTP session**:
An exclusive handle to the camera through libgphoto2. Only one may exist at a time, and every property read, property write and capture in a run shares it.
_Avoid_: connection, handle, gphoto2 process

**Recovery reset**:
The `gphoto2 --reset` operation that destroys a wedged PTP session and returns the camera to the USB bus. It is a recovery action, not an additional session.
_Avoid_: reset, reconnect, restart

**Wedged camera**:
The state in which the camera is still on the USB bus and still auto-detected, but every operation fails with `PTP Timeout` — usually after a rejected property write. A recovery reset or a cable reconnect clears it.
_Avoid_: hung, stuck, frozen, crashed, dead

**Disconnected camera**:
The camera is absent from the USB bus. The remedy differs from that of a wedged camera, so the two states are never reported as one.
_Avoid_: unplugged, lost, unavailable

**Output setting**:
The Canon camera property named `output`, which selects which displays receive live view. Only two of its values are verified on an M50 — `TFT + PC` gives the full frame size and `MOBILE2` the reduced size; the others are unverified here, and a report from a different Canon body describes combinations of `PC` and `MOBILE` causing an error that needs the battery removed.
_Avoid_: output, video output, display mode

**Viewfinder action**:
The camera action that turns live view on at the full frame size. It does not persist between invocations.
_Avoid_: viewfinder mode, live view enable

### Frames and measurement

**Frame size**:
The pixel dimensions of one captured live-view frame.
_Avoid_: resolution, dimensions, size

**Requested size**:
The frame size a user asks for. The M50 accepts no size request, so a requested size is a preference and never a promise.
_Avoid_: target resolution, desired resolution

**Measurement**:
The act of learning a frame size and a frame rate by capturing frames and reading them. It is the only way to learn them, because this camera honours no live-view format it is asked for.
_Avoid_: capability probe, negotiation, negotiated value, actual resolution

**Measured frame size** / **Measured frame rate**:
The values a measurement produces, as opposed to the requested size.

**Calibration window**:
The opening period in which the expected frame size is established, by taking the largest size that recurs at least a threshold number of times inside the window. It rejects a transient of the reduced size without assuming how long that transient lasts.
_Avoid_: probe period, warm-up, settle time, discard count

**Expected frame size**:
The steady frame size a calibration window establishes. The sink is opened at this size, and no frame of any other size is ever handed to it. `1024x576` is a sanity check on this value, never its source.
_Avoid_: target size, negotiated size, canonical size

**Sink re-open**:
Closing the virtual camera and opening it again at a newly measured frame size, after the camera stops producing the expected size. Re-opening is the only honest response to a size change; handing a different geometry to an open sink is not.
_Avoid_: reconfigure, resize, restart

**Leading frames**:
The reduced-size frames that can precede the steady state at the start of a run. They were observed as five frames in the captures that toggled the viewfinder action, and were absent from captures that did not toggle it, so a leading run is not guaranteed.
_Avoid_: warm-up frames, first frames, junk frames

### Output

**Live view feed**:
The normalized frames this software publishes to the operating system as a camera.
_Avoid_: output, video output, stream, signal

**Virtual camera sink**:
The abstraction by which live view frames become an operating-system camera: open, accept a frame, close. There is one sink and one implementation per platform.
_Avoid_: output, driver, publisher

**Platform backend**:
The per-platform implementation of the virtual camera sink, fixed at build time rather than chosen by a user.
_Avoid_: adapter, provider, implementation, backend setting

**Virtual camera mechanism**:
The operating-system facility a platform backend drives so the camera appears — for example OBS Virtual Camera, Media Foundation or `v4l2loopback`. A mechanism belongs to its platform, so naming one on a platform where it does not exist is an error rather than a fallback, and it is the one part of the output side a user chooses.
_Avoid_: backend, driver, backend setting

**Consumer application**:
An application that opens the virtual camera, such as Discord, Teams or Meet. It is the only observer that proves the feed works.
_Avoid_: client, target application, user application
