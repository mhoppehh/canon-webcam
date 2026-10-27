# OBS Virtual Camera is the macOS virtual camera mechanism

On macOS the live view feed reaches a consumer application through OBS Virtual Camera, and through nothing else. Our process publishes frames over a local WebSocket to a browser source inside OBS, then starts and stops OBS's virtual camera through obs-websocket, which OBS has built in since 28. No process can inject frames into a virtual camera that another process publishes: OBS Virtual Camera is a camera extension driven by OBS's own render pipeline, so the plan's phrase "sends frames to OBS" described an interface that does not exist. What replaces it is two explicit parts — a transport our process owns, and a device OBS owns — joined by a control channel that OBS documents and we can test.

This is also the only one of the three options whose mechanism could be exercised on the available test machine at all. A camera extension of our own needs a `com.apple.developer.system-extension.install` entitlement, an app in `/Applications`, an administrator's explicit approval in System Settings, and — for anything a user downloads — a notarised Developer ID signature. The test machine held one Apple Development signing identity and no Developer ID identity, so notarisation was impossible there, and the developer-mode bypass was closed too: `systemextensionsctl developer` answers "this tool cannot be used if System Integrity Protection is enabled", and SIP is enabled. A DAL plugin of our own is deprecated in Apple's own headers — `CMIOHardwarePlugIn.h` carries `API_DEPRECATED("Use CMIOExtension instead", macos(10.7, 12.3))` — and the most widely deployed virtual camera on macOS has already abandoned it: OBS 32.2.1 ships `com.obsproject.obs-studio.mac-camera-extension.systemextension` and uses `OSSystemExtensionRequest`, keeping the DAL bundle only for macOS before 15.

## Comparison

|                                                   | Cost to build                                                                                                                                | Cost to install                                                                                                               | Consumer application sees the device without the user touching OBS                                                                                   | Verifiable today                                                                                                                                                      |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **OBS Virtual Camera, transport + obs-websocket** | Low: a WebSocket server, a ~40-line page, an obs-websocket v5 client, source and scene lifecycle                                             | OBS installed and kept in `/Applications`, obs-websocket enabled with a password, one administrator-approved camera extension | No. OBS must run. The program launches it, minimises it to the tray, and starts and stops the virtual camera itself, so the user need not touch it during a run | **Partially.** Transport, control, and every failure message were exercised. The device itself is one administrator approval away, which this trial could not perform |
| **CMIOExtension of our own**                      | High: signed app plus extension target, app-group IPC, pixel-buffer plumbing, notarisation pipeline                                          | App in `/Applications`, administrator approval, and a paid Developer ID for distribution                                      | Yes                                                                                                                                                  | **No.** No Developer ID identity, so it cannot be notarised; SIP blocks developer mode, so the local bypass is closed as well                                         |
| **DAL plugin of our own**                         | High: hand-written `CMIOHardwarePlugInInterface` vtable, `CMSampleBuffer` timing, our own shared-memory IPC, on an API deprecated since 12.3 | Copy a bundle into `/Library/CoreMediaIO/Plug-Ins/DAL`, which is `root:wheel` and not writable by the user                    | Yes                                                                                                                                                  | **No.** The install directory is root-owned, and the API is deprecated in the SDK                                                                                     |

## The contract this backend implements

### Mechanism name

The macOS platform backend exposes one mechanism, `obs-virtual-camera`. A configuration that names another mechanism — `media-foundation`, `v4l2loopback` — is a validation error, `MechanismNotAvailableOnPlatform { requested, platform }`, and never a silent fallback.

### Prerequisites, checked before anything is started

**P1 — OBS lives in `/Applications`.** OBS refuses to install its virtual camera from anywhere else. Verified: launched from its Steam location, OBS logged `[mac-virtualcam] mac-camera-extension: OBS cannot install the virtual camera if it's not in "/Applications". Please move OBS to the "/Applications" directory.` If `/Applications/OBS.app` is absent, raise `ObsNotInstalledInApplications` and print: install OBS Studio from obsproject.com and leave it in `/Applications`; a Steam copy in `~/Library/Application Support/Steam/...` is rejected, so it must be copied to `/Applications`.

**P2 — the OBS camera extension is approved and the device is registered.** Check with an AVFoundation discovery session over `[.external, .builtInWideAngleCamera, .continuityCamera, .deskViewCamera]` for a device whose `localizedName` is `OBS Virtual Camera`; `.externalUnknown` is deprecated on macOS 26 in favour of `.external`. If it is absent, raise `VirtualCameraDeviceAbsent` and print these steps verbatim:

1. Open `/Applications/OBS.app`.
2. In OBS, click **Start Virtual Camera** in the Controls panel.
3. When macOS reports that the system extension was blocked, open **System Settings → General → Login Items & Extensions → Camera Extensions**, enable **OBS Virtual Camera**, and authenticate as an administrator.
4. Click **Stop Virtual Camera** and quit OBS.

This is a one-time action. Our process must not attempt it: system-extension approval is an administrator-authorised decision taken in the GUI, and the trial proved that a process cannot reach it — OBS declined even to submit the request from outside `/Applications`, and the approval UI cannot be driven from a process.

**P3 — obs-websocket is enabled and authenticated.** Connect to `127.0.0.1:4455` and complete the v5 handshake, checking `hello.d.authentication` for the salt-and-challenge step. If the connection is refused or the handshake fails, raise `ObsWebSocketUnavailable`, or `ObsUnauthenticated` when the password is wrong, and print: in OBS open **Tools → WebSocket Server Settings**, tick **Enable WebSocket server**, keep port `4455`, tick **Enable Authentication**, set a password, and put the same password in our configuration under the macOS mechanism. The check must be a live connection, not the presence of OBS's configuration file: on the test machine the server was disabled while a password was already stored in `plugin_config/obs-websocket/config.json`.

**P4 — OBS runs.** If no process is listening, launch `/Applications/OBS.app` with `open -a /Applications/OBS.app --args --minimize-to-tray` and poll the WebSocket for up to 20 s. Our process does not quit OBS when it closes the sink: obs-websocket 5.7.4 exposes no quit request — verified against the `availableRequests` list of the running OBS — and OBS is the user's application once it is running. Say so once in the log.

### Open, in this order

1. Validate the mechanism name; check P1 and P2, in that order, before touching OBS.
2. Bind a local HTTP server on `127.0.0.1:0` and read back the port; serve the page at `/` and the frame WebSocket at `/frames`.
3. Ensure OBS is running and authenticated (P3, P4).
4. Create or update the input named `canon-webcam source` of kind `browser_source` with exactly these settings, every key of which was read from OBS 32.2.1 with `GetInputDefaultSettings`:
   - `url` = `http://127.0.0.1:<port>/`
   - `width`, `height` = the expected frame size
   - `fps_custom` = `true`, `fps` = the measured frame rate
   - `shutdown` = `false`, `restart_when_active` = `false`, `reroute_audio` = `false`
   - `css` = `body { background-color: rgba(0, 0, 0, 0); margin: 0px auto; overflow: hidden; }`
5. Create the scene `canon-webcam` if it does not exist, put one scene item for that input in it, and set it as the program scene, remembering the previous program scene name.

   The input must not be called `canon-webcam`. OBS keeps scenes and inputs in one namespace, so `CreateInput` for a scene's own name is refused with "A source already exists by that input name", and step 4 would fail on every open. This ADR originally named both `canon-webcam`; the input was renamed after the collision was observed against a real OBS.

6. Call `StartVirtualCam`.
7. Confirm with `GetVirtualCamStatus` that `outputActive` is `true` **and** re-run the P2 device check. `StartVirtualCam` returning `requestStatus.result == true` is not evidence: it returned success on the test machine while the virtual camera stayed inactive because OBS had refused to install the extension. If either confirmation fails, undo steps 4 to 6 and raise `ObsVirtualCameraFailedToStart` carrying OBS's own explanation from its log.

### Frame delivery

Each frame is one binary WebSocket message holding the JPEG exactly as the camera delivered it, when no resizing is requested, so the live view feed is never re-encoded on the way to the screen; a resized frame is encoded once. The page decodes with `createImageBitmap` and draws to a canvas whose size it takes from the first frame, so the geometry is whatever the sink sends. Delivery is paced to the measured frame rate and drops rather than queues when the page falls behind, matching the bounded capture queue.

A new expected frame size from a calibration window is handled by sink re-open and by nothing else: close the sink, then open it again with the new `width`, `height` and `fps` in the input settings. Do not resize an open sink.

### Close

Call `StopVirtualCam`, remove the scene item and the input, restore the previous program scene and remove the scene if we created it, close the WebSocket, and stop the HTTP server. Leave OBS running. Close is idempotent and must succeed when OBS has already exited, treating that as already closed.

### Failure

A closed WebSocket or a `VirtualCamStateChanged` event with `outputActive: false` while we believe the live view feed is being delivered is `ObsDisconnected`: the sink closes, the run ends with a clear error, and the program does not restart OBS in a loop. A consumer application is the only proof that the feed works, and the automated stand-in is the pair `GetVirtualCamStatus` plus the AVFoundation device listing, never OBS's window.

## What the trial proved and what it did not

Proved on the test machine, macOS 26.6.2 on Apple silicon with OBS 32.2.1 and obs-websocket 5.7.4, without installing anything and without administrator rights:

- Our process can publish 1024×576 JPEG frames to a Chromium page — the engine OBS's browser source embeds — at 30.03 fps sustained over 7 s, 211 frames, canvas geometry taken from the frames themselves.
- Our process can drive a live OBS through obs-websocket v5: challenge authentication, `GetVersion`, `GetSceneList`, `GetInputList`, `GetVirtualCamStatus`, `StartVirtualCam`, and `GetInputDefaultSettings` for `browser_source`.
- `StartVirtualCam` reports success while the virtual camera stays down, and OBS's log names the reason. The ticket's original trap is therefore real and the confirmation step above is not optional.
- OBS will not install its virtual camera from outside `/Applications`; no virtual camera device was registered on the test machine at that time (`systemextensionsctl list` shows none, `/Library/CoreMediaIO/Plug-Ins/DAL` holds only `plugins-info.txt`); and a consumer application sees only the two built-in cameras.

Not proved, and the evidence that would close it:

- **Frames from our process appearing in a consumer application.** Blocked by one administrator action: OBS must be in `/Applications`, its camera extension must be approved, and the device then inspected. Closing evidence is the AVFoundation listing containing `OBS Virtual Camera` and a frame from it in FaceTime or Photo Booth.
- **That an OBS browser source accepts our WebSocket and paints at 30 fps.** Blocked by the same approval, because a browser source is only visible once the virtual camera renders. Closing evidence is the OBS Statistics window showing 30 fps plus a screenshot of the device in a consumer application. The transport itself is proved in Chromium, which is the same engine and the same `createImageBitmap` path.
- **Latency and jitter through OBS compositing**, and therefore the 10-minute-call criterion. Closing evidence is a wall clock filmed beside the camera in a consumer application.

A second transport was measured and rejected rather than assumed: MJPEG over local HTTP with an OBS media source needs no extra software and ffmpeg decodes it correctly at 1024×576, but the format carries no timestamps. A sequence our server paced at a true 30 fps was read by ffmpeg as `r_frame_rate=25/1` while 60 frames arrived in 2.11 s, so the consumer's timeline falls behind the frames it is receiving — tolerable for a clip, not for a 10-minute call. A browser source composites the page on OBS's own clock and involves no invented timestamps, which is why it carries the live view feed.

## Considered Options

- **OBS Virtual Camera, with a local WebSocket transport and obs-websocket control.** Chosen. The only option whose mechanism can be exercised here, the cheapest to build, and the only one that displaces the signing burden onto somebody who already holds a Developer ID. Its cost is a hard OBS dependency and a transport we must write ourselves.
- **CMIOExtension of our own.** The right destination and the wrong next step. It is the supported modern route and the only one that gives a device without OBS, but it needs a Developer ID, notarisation, an app in `/Applications`, and an administrator's approval per machine, and none of that can be built or verified where this work happens. Rejected because an option that cannot be trialled has no evidence behind it, and because a hobby project has no reason to buy a Developer ID before the feed itself is known to work.
- **DAL plugin of our own.** Rejected. It is deprecated in the SDK since 12.3, its install directory is root-owned, and OBS — the largest deployment of this technique — has already replaced it with a camera extension for macOS 15 and later. It costs the most to build of the three while saving nothing on install: it needs the same single administrator authorisation as OBS's extension, only paid in a directory we would have to write a plugin for.

## Revisit triggers

- **Requiring OBS to be installed and running is judged unacceptable.** This is the likeliest trigger and the reason the decision is written down. The answer then is a CMIOExtension, not a DAL plugin.
- OBS stops publishing a virtual camera, or macOS stops accepting OBS's camera extension.
- Latency or jitter through browser-source compositing fails the 10-minute-call criterion. The answer is a Syphon transport inside the same mechanism, which changes one part rather than the decision.
- A Developer ID and notarisation become acceptable while the OBS dependency becomes intolerable, which flips the same two options in the other direction.

## Consequences

- OBS becomes a runtime dependency of the macOS platform backend, and its presence is a prerequisite check rather than a fallback. The user installs OBS, enables obs-websocket with a password, and approves a camera extension once.
- The transport is ours: a WebSocket server and a page, not a shared-memory framework. That keeps the dependency count at zero but puts frame pacing and backpressure on our side of the boundary.
- The macOS backend mutates OBS's scene collection on every run and restores it on close, so a crashed run can leave a `canon-webcam` scene behind. The next run reuses it rather than failing.
- `StartVirtualCam` cannot be trusted, so every open costs one extra `GetVirtualCamStatus` and one AVFoundation enumeration. The backend must not treat the request result as a success indicator.
- Nothing here transfers to Windows or Linux: the mechanism belongs to its platform, so those backends name Media Foundation and `v4l2loopback` and share only the sink trait.
