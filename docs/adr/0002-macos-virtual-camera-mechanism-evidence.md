# Evidence appendix for ADR 0002

Everything below was run on macOS 26.6.2 (build 25G83) on Apple silicon, on 2026-10-27. No third-party software was installed, no administrator password was used, and no Canon camera was attached. Probe code lived outside the repository; the OBS configuration file it edited was backed up and restored, and OBS was quit afterwards.

## Environment, as found

```
$ ls -d /Applications/OBS.app
No such file or directory
$ mdfind -name "OBS.app"
~/Library/Application Support/Steam/steamapps/common/OBS Studio/arm64/OBS.app
~/Library/Application Support/Steam/steamapps/common/OBS Studio/x86_64/OBS.app
$ ls -la /Library/CoreMediaIO/Plug-Ins/DAL
-rw-r--r--  1 root  wheel   37 Aug 12 20:51 plugins-info.txt      # nothing installed
$ ls -ld /Library/CoreMediaIO/Plug-Ins/DAL
drwxr-xr-x  3 root  wheel  96                                       # not writable by us
$ systemextensionsctl list | grep -i obs
(no OBS extension)
$ security find-identity -v -p codesigning
  1) 0AFF71A3...  "Apple Development: <redacted> (<redacted>)"
     1 valid identities found
$ csrutil status
System Integrity Protection status: enabled.
$ systemextensionsctl developer
At this time, this tool cannot be used if System Integrity Protection is enabled.
$ /usr/bin/defaults read ".../OBS.app/Contents/Info.plist" CFBundleShortVersionString
32.2.1
$ find ".../OBS.app" -name "*.systemextension"
.../OBS.app/Contents/Library/SystemExtensions/com.obsproject.obs-studio.mac-camera-extension.systemextension
```

OBS on macOS 15 and later is a camera extension; the DAL bundle in `Contents/Resources/obs-mac-virtualcam.plugin` is the pre-15 path. `mac-virtualcam.plugin` links `OSSystemExtensionRequestDelegate` and reports `Error.SystemExtension.NotInstalled.MacOS15`.

## Apple's primary sources

- `CMIOHardwarePlugIn.h` in the macOS 26 SDK annotates the DAL plugin API `API_DEPRECATED("Use CMIOExtension instead", macos(10.7, 12.3))`.
- *Creating a camera extension with Core Media I/O* requires `com.apple.developer.system-extension.install` and an app group on the host app, states that "only apps that reside in the `/Applications` directory can activate an extension", and that "a person with Admin privileges for the Mac must explicitly allow access to it in the Systems & Privacy screen".

## Trial A — what a consumer application sees

`AVCaptureDevice.DiscoverySession` over `[.externalUnknown, .builtInWideAngleCamera, .continuityCamera, .deskViewCamera]`, the API a consumer application uses to populate a camera picker:

```
authorizationStatus(video): 2
device count: 2
  name=<built-in camera> type=AVCaptureDeviceTypeBuiltInWideAngleCamera
  name=<built-in Desk View camera> type=AVCaptureDeviceTypeDeskViewCamera
```

`.externalUnknown` is deprecated on macOS 26 in favour of `.external`. `system_profiler SPCameraDataType` reports only the built-in camera and misses Desk View, so AVFoundation is the stronger instrument and the one the backend should use.

## Trial B — MJPEG over local HTTP, measured and rejected as the transport

Our own process served 1024×576 JPEG frames, paced at 30 fps, over `127.0.0.1`:

```
# ffmpeg (the decoder inside OBS's Media Source) reads the HTTP response correctly...
codec_name=mjpeg  width=1024  height=576
# ...but invents a frame rate, because MJPEG carries no timestamps:
r_frame_rate=25/1            # server paced at 30 fps
wall seconds for 60 frames: 2.11    # frames arrive at 30 fps
```

## Trial C — frames from our process reaching a Chromium page

Stdlib WebSocket server, page uses `createImageBitmap` and a canvas, exactly as an OBS browser source would. Measured in the embedded Chromium:

```
after 7 s: {"frames":211,"w":1024,"h":576,"fps":30.15,"steadyFps":30.03,"samples":90}
```

Sustained 30.03 fps at 1024×576 with the geometry taken from the frames.

## Trial D — obs-websocket control path against a live OBS

OBS launched from its Steam location with `--minimize-to-tray --multi`; obs-websocket enabled on a temporary password and the original configuration restored afterwards. OBS reported:

```
obsWebSocketVersion 5.7.4   obsVersion 32.2.1
platformDescription "macOS Tahoe (26.6.2)"
```

Requests exercised successfully: `GetVersion`, `GetSceneList`, `GetInputList`, `GetVirtualCamStatus`, `StartVirtualCam`, `GetInputDefaultSettings`. `availableRequests` contains no quit request, which is why the sink leaves OBS running.

The decisive observation — the request succeeds and nothing starts:

```
StartVirtualCam:            {"requestStatus":{"code":100,"result":true}}
GetVirtualCamStatus_after:  {"outputActive": false}
```

OBS's own log, at startup, names the reason:

```
[mac-virtualcam] mac-camera-extension: OBS cannot install the virtual camera if
it's not in "/Applications". Please move OBS to the "/Applications" directory.
```

And the device list was unchanged afterwards:

```
device count: 2   (built-in camera and Desk View only)
```

`GetInputDefaultSettings` for `browser_source` on OBS 32.2.1, the settings keys the ADR fixes:

```
css, fps (30), fps_custom (false), height (600), reroute_audio (false),
restart_when_active (false), shutdown (false), url, webpage_control_level (1), width (800)
```

Observed video settings: 1920×1080 at 30/1.

## Trial E — the stop point

The next step of the same trial would be to copy OBS into `/Applications`, call `StartVirtualCam` again, and approve the camera extension. Both were declined deliberately: copying an application into `/Applications` is an install, and the approval needs an administrator password and a GUI decision that a process must not make on the user's behalf. The chain is therefore proven up to the boundary macOS itself enforces, and the remaining evidence is exactly the one human action named in the ADR's prerequisite P2.
