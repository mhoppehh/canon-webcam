# Can the live view feed reach a virtual camera without OBS?

Research note for the macOS **platform backend**. It examines four routes that publish the
**live view feed** as a camera device. It records what is proven, what is not proven, and what
this machine shows today.

Written in ASD-STE100 Simplified Technical English. Each claim has a source.

## Summary

Yes. The feed can reach a camera device without OBS. No route lets our own process publish a
camera device without help. A camera device is a system extension. A system extension needs a
signing identity, an app in `/Applications`, and one administrator approval on each Mac.

Three routes are practical:

- **Route A** keeps OBS and removes obs-websocket. OBS parses `--startvirtualcam`,
  `--collection`, `--scene` and `--minimize-to-tray` on its command line. Our process writes a
  scene collection file and starts OBS with one command. There is no control socket and no OBS
  password.
- **Route B** removes OBS. Our process sends an NDI stream. The free NDI Virtual Input tool
  owns a signed camera extension and publishes **NDI Virtual Camera** to all applications.
- **Route C** is a camera extension of our own. It is the only route that removes OBS and NDI.
  It needs a signing team, an app in `/Applications`, and one administrator approval.

**NOTE** No route delivers the camera JPEG bytes unchanged. A camera device always delivers
uncompressed frames. Our process must decode each JPEG one time. Route A also copies the frame
through OBS's compositor. Route B compresses the frame again with the NDI codec. Route C is the
shortest path: decode the JPEG, then write the pixel buffer to the stream.

The cheapest change is **Route A**. The install cost is the same as today: OBS in
`/Applications` plus one administrator approval. The cheapest change that removes OBS is
**Route B**. It needs no Developer ID and no notarisation.

## What each route requires

| Route | What we must build | What the user must install or approve | Proven on this machine |
| --- | --- | --- | --- |
| **A.** OBS, command line only | A scene collection JSON with one `browser_source`; a frame server on a fixed port | OBS in `/Applications`; one administrator approval of the OBS camera extension | **Yes.** OBS 33.0.0, extension `activated enabled`, device listed |
| **B.** NDI Virtual Input owns the device | An NDI sender for each JPEG frame | NDI Tools (free); one administrator approval of the NDI camera extension | **No.** NDI Tools is not installed here |
| **C.** Camera extension of our own | Host app, extension target, app group, pixel-buffer plumbing, activation request | App in `/Applications`; administrator approval; a signing team | **No.** See *Route C* |
| **D.** DAL plugin of our own | `CMIOHardwarePlugInInterface` vtable, sample-buffer timing, own shared memory | A root write into `/Library/CoreMediaIO/Plug-Ins/DAL` | **No.** That directory is `root:wheel` |
| **Today.** OBS plus obs-websocket | Already built | OBS, a WebSocket password, one administrator approval | **Yes** for the mechanism; the sink also needs OBS running |

## Route A: OBS without obs-websocket

OBS parses these arguments in `frontend/obs-main.cpp`
(<https://github.com/obsproject/obs-studio/blob/master/frontend/obs-main.cpp>):

```cpp
} else if (arg_is(argv[i], "--startvirtualcam", nullptr)) {
    opt_start_virtualcam = true;
} else if (arg_is(argv[i], "--collection", nullptr)) {
    if (++i < argc) {
        opt_starting_collection = argv[i];
    }
} else if (arg_is(argv[i], "--scene", nullptr)) {
    if (++i < argc) {
        opt_starting_scene = argv[i];
    }
} else if (arg_is(argv[i], "--minimize-to-tray", nullptr)) {
    opt_minimize_tray = true;
```

The help text of the same file says:

```
--startvirtualcam: Start virtual camera (if available).
--collection <string>: Use specific scene collection.
--profile <string>: Use specific profile.
--scene <string>: Start with specific scene.
--minimize-to-tray: Minimize to system tray.
```

The installed OBS 33.0.0 binary on this machine contains all six strings: `--startvirtualcam`,
`--collection`, `--profile`, `--scene`, `--minimize-to-tray`, `--multi`.

A scene collection is a JSON file. OBS reads it from `obs-studio/basic/scenes`
(`frontend/OBSApp.cpp`). Our process can write
`~/Library/Application Support/obs-studio/basic/scenes/canon-webcam.json` with one
`browser_source`, then start OBS:

```
open -a /Applications/OBS.app --args --collection canon-webcam --scene canon-webcam \
     --startvirtualcam --minimize-to-tray --multi
```

This removes three things from the macOS **platform backend**:

1. The obs-websocket client.
2. The password in the configuration.
3. Prerequisite P3, `ObsWebSocketUnavailable` and `ObsUnauthenticated`.

**CAUTION** OBS still creates the camera device with its own camera extension. One administrator
approval stays. OBS must also stay open during a run.

**CAUTION** The browser source needs a stable URL. The current sink binds the frame server to
`127.0.0.1:0`, which gives a new port on each run. Write the JSON again on each run with the
reported port, or bind a fixed port.

**NOTE** OBS converts every frame to its own output format before the camera extension reads it.
Set the OBS canvas size to the **expected frame size** to prevent a scale. The conversion itself
cannot be prevented.

## Route B: NDI Virtual Input

NDI Virtual Input is part of the free NDI Tools package. Vizrt documents it at
<https://docs.ndi.video/all/using-ndi/ndi-tools/ndi-tools-for-mac/virtual-input.md>. The page
states:

> While launching NDI Virtual Input for the first time, you will be prompted to install the NDI
> Virtual Camera extension before using the application. This is a needed step for macOS to
> recognize the application as a virtual camera you can access system-wide.

The approval is manual: **System Settings → General → Login Items & Extensions → Camera
Extensions**. The same page warns about known defects since NDI Tools 6.1.

Our process sends one NDI stream. NDI Virtual Input selects that stream. Every application then
sees the device **NDI Virtual Camera**.

Costs:

- **CAUTION** NDI uses a lossy codec. The feed is compressed again after the JPEG decode.
- **CAUTION** The sender needs the NDI SDK. The vendor licence covers personal and
  non-commercial use; read it before commercial use
  (<https://ndi.video/for-developers/ndi-sdk/>).
- **NOTE** The NDI stream is visible on the local network unless the sender restricts it to the
  loopback interface.

## Route C: a camera extension of our own

Apple documents the requirements in *Creating a camera extension with Core Media I/O*
(<https://developer.apple.com/documentation/coremediaio/creating-a-camera-extension-with-core-media-i-o>).
The host app needs two capabilities, **System Extension** and **App Groups**. The entitlements
file contains:

```xml
<key>com.apple.developer.system-extension.install</key>
<true/>
<key>com.apple.security.application-groups</key>
<array>
    <string>$(TeamIdentifierPrefix)com.example.CustomCamera</string>
</array>
```

The same article states two rules:

> Only apps that reside in the `/Applications` directory can activate an extension.

> Before the extension is available to the system, a person with Admin privileges for the Mac
> must explicitly allow access to it in the Systems & Privacy screen of System Settings.

The extension target needs a `CMIOExtension` dictionary with `CMIOExtensionMachServiceName`, and
an `NSSystemExtensionUsageDescription`. OBS ships the same shape in
`plugins/mac-virtualcam/src/camera-extension/cmake/macos/Info.plist.in`.

### Developer mode

Apple describes developer mode as a location bypass, not a signature bypass
(<https://developer.apple.com/documentation/driverkit/debugging-and-testing-system-extensions>):

> When in developer mode, the system doesn't check the location of your system extension prior to
> loading it.

This machine refuses the command (see *Evidence from this machine*). Apple's advice for local
testing is to disable SIP. This machine must not do that.

### Paid membership: required

Apple publishes a capability table at
<https://developer.apple.com/help/account/reference/supported-capabilities-macos/>. It states:

> The capabilities available to a macOS provisioning profile depend on your program membership
> and signing certificate.

The table has a **System Extension** row with three columns: **ADP** (paid), **Developer ID**,
and **Apple Developer** (no cost, no distribution). The check marks are images with no text, so
the table alone does not answer the question.

Xcode answers it. Xcode keeps one capability inventory for each signing team in
`~/Library/Developer/Xcode/UserData/Capabilities/capabilities-<version>-<teamID>-bundle.json`.
On this machine the team is a free team:

```
$ defaults read com.apple.dt.Xcode IDEProvisioningTeamByIdentifier
            isFreeProvisioningTeam = 1;
            teamID = <redacted>;
            teamName = "<redacted> (Personal Team)";
            teamType = "Personal Team";
```

The inventory for this team holds 11 capabilities:

```
HOMEKIT, INCREASED_MEMORY_LIMIT, MAPS, ENABLED_FOR_MAC, GAME_CENTER,
WIRELESS_ACCESSORY_CONFIGURATION, DATA_PROTECTION, AUTOFILL_CREDENTIAL_PROVIDER,
APP_GROUPS, INTER_APP_AUDIO, HEALTHKIT
```

`SYSTEM_EXTENSION` is not in the list. A camera extension needs the product type
`com.apple.product-type.system-extension`, which this team cannot request.

**Conclusion** A camera extension of our own needs a paid Apple Developer Program membership.
The free Apple ID can sign ordinary macOS apps, but it cannot sign a system extension. See
*What is still unknown*, item 1, for the one remaining test.

### Unproven: notarisation with SIP on

Apple's system-extension debugging article states:

> Xcode doesn't notarize apps or system extensions during the normal development cycle, so
> disabling SIP bypasses the notarization checks that the system normally performs.

This sentence implies that with SIP enabled, an unnotarised extension does not load. It does not
prove it. See *What is still unknown*, test 3.

## Route D: a DAL plugin of our own

Apple deprecated the interface in the SDK. On this machine, macOS 26.5 SDK,
`CMIOHardwarePlugIn.h` says:

```
} API_DEPRECATED("Use CMIOExtension instead", macos(10.7, 12.3));
```

Deprecation is not removal. OBS still builds and installs a DAL plugin, and the loader still
accepts it. OBS selects the route by macOS version, not by install path
(`plugins/mac-virtualcam/src/obs-plugin/plugin-main.mm`):

```objc
static bool cmio_extension_supported()
{
    if (@available(macOS 13.0, *)) {
        return true;
    } else {
        return false;
    }
}
```

**Correction to ADR-0002.** The DAL plugin is used below macOS 13, not below macOS 15. The string
`Error.SystemExtension.NotInstalled.MacOS15` is only an error message for macOS 15 and later.

Install cost is one root write into `/Library/CoreMediaIO/Plug-Ins/DAL`. OBS does this write with
`do shell script ... with administrator privileges`. The user must perform it, not our process.
This route is not recommended: it needs the most work and has the shortest life.

## Existing products to integrate instead

| Product | Device type | Free | Accepts an external feed |
| --- | --- | --- | --- |
| **OBS Virtual Camera** | Camera extension inside OBS | Yes | Only through OBS |
| **NDI Virtual Input** | Camera extension, signed by Vizrt | Yes | Yes, over NDI |
| **akvirtualcamera** (Webcamoid) | DAL plugin, GPL-3.0 | Yes | Yes, through its own API |
| **LemurCam** | Camera extension, MIT | Yes | No. It reads RTSP cameras itself |
| **ManyCam** | Commercial app | Freemium | Yes, through the app |

Sources: <https://github.com/obsproject/obs-studio>,
<https://docs.ndi.video/all/using-ndi/ndi-tools/ndi-tools-for-mac/virtual-input.md>,
<https://github.com/webcamoid/akvirtualcamera>, <https://github.com/steelbrain/LemurCam>.

**NOTE** akvirtualcamera is GPL-3.0. Linking it makes our binary GPL-3.0.

**NOTE** No maintained product was found that turns a Syphon share into a camera device. Syphon
is not a camera. Syphon can feed OBS, which ADR-0002 already permits.

## Evidence from this machine

Recorded on 2026-10-27, macOS 26.6.2 (build 25G83), Apple silicon.

```
$ sw_vers
ProductName:            macOS
ProductVersion:         26.6.2
BuildVersion:           25G83
```

```
$ csrutil status
System Integrity Protection status: enabled.
```

```
$ systemextensionsctl developer
At this time, this tool cannot be used if System Integrity Protection is enabled.
This limitation will be removed in the near future.
```

```
$ systemextensionsctl list
...
--- com.apple.system_extension.cmio (Go to 'System Settings > General > Login Items &
    Extensions > Camera Extensions' to modify these system extension(s))
enabled active  teamID  bundleID (version)      name    [state]
                2MMRE5MTB8  com.obsproject.obs-studio.mac-camera-extension (32.2.2/...)
                            OBS Virtual Camera  [terminated waiting to uninstall on reboot]
*       *       2MMRE5MTB8  com.obsproject.obs-studio.mac-camera-extension (33.0.0/...)
                            OBS Virtual Camera  [activated enabled]
```

```
$ system_profiler SPCameraDataType
    OBS Virtual Camera:
      Model ID: OBS Camera Extension
      Unique ID: <redacted>
    <built-in camera>:
      ...
```

```
$ defaults read /Applications/OBS.app/Contents/Info.plist CFBundleShortVersionString
33.0.0
```

```
$ strings /Applications/OBS.app/Contents/MacOS/OBS | grep -E '^--(startvirtualcam|...)$'
--collection
--minimize-to-tray
--multi
--profile
--scene
--startvirtualcam
```

```
$ ls -la /Library/CoreMediaIO/Plug-Ins/DAL
-rw-r--r--  1 root  wheel   37 Aug 12 20:51 plugins-info.txt
$ ls ~/Library/CoreMediaIO
ls: ~/Library/CoreMediaIO: No such file or directory
```

```
$ security find-identity -v -p codesigning
  1) 0AFF71A3...  "Apple Development: <redacted> (<redacted>)"
     1 valid identities found
```

**NOTE** This machine changed since ADR-0002. OBS is now in `/Applications` at version 33.0.0,
its camera extension is approved, and the system lists **OBS Virtual Camera** as a camera.
ADR-0002 records the opposite. The ADR needs an update.

**NOTE** The signing identity is `Apple Development`, not `Developer ID`. Notarisation is not
possible with this identity. Local development signing is possible.

The certificate belongs to a free Personal Team:

```
$ security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject
subject= /UID=<redacted>/CN=Apple Development: <redacted> (...)/
         OU=<redacted>/O=<redacted>/C=US
```

```
$ ls ~/Library/Developer/Xcode/UserData/Provisioning\ Profiles/
(empty)
```

**NOTE** There are no provisioning profiles on this machine. No capability has been requested
for this team.

## What this means for the code

The **virtual camera sink** boundary already fits Route A and Route C. The changes stay inside
the macOS **platform backend**:

- `camera-core/src/mechanism.rs` names the mechanisms. Route C adds a second value, for example
  `camera-extension`. Route A changes the behaviour behind `obs-virtual-camera` and keeps the
  name.
- `camera-core/src/platform/macos/prerequisites.rs` loses P3 (the WebSocket check) for Route A.
- `camera-core/src/platform/macos/obs_websocket.rs` becomes unused for Route A. The scene
  collection writer replaces it.
- `camera-core/src/platform/macos/frames_transport.rs` stays. Route A still needs the local
  frame server for the browser source.
- `GLOSSARY.md` gains one term for the new mechanism, because **virtual camera mechanism** is the
  user-facing configuration key.

Route A is the smaller change. Route C is the larger change, and it is the only one that removes
the OBS dependency, which is a stated **revisit trigger** in ADR-0002.

## What is still unknown

1. **Whether Xcode refuses a camera extension at build time for a Personal Team.** The cached
   capability inventory says yes (see *Route C*). No build was attempted. Test: open the Xcode
   *Camera Extension* template, select the Personal Team, build, and read the signing error.
2. **Whether `com.apple.CMIOExtensionProvider` is the extension point identifier.** It is absent
   from Apple's article and from the OBS plist. Test: `plutil -p
   <Extension>.appex/Contents/Info.plist` after a template build.
3. **Whether an unnotarised, development-signed camera extension loads with SIP on.** Test: build
   the template, put the app in `/Applications`, activate the extension, then read
   `systemextensionsctl list`.
4. **Whether `~/Library/CoreMediaIO/Plug-Ins/DAL` is scanned.** No primary source states it. Test:
   copy a plugin there, open a discovery session, then read
   `log stream --predicate 'process == "coremediaiod"'`.
5. **Whether the OBS scene collection JSON schema is stable.** Test: write a minimal collection,
   start OBS with `--collection`, and read `obs-studio/logs`.
6. **Whether the NDI route has acceptable latency and no visible defects.** Test: install NDI
   Tools, approve the extension, send a 1024x576 test pattern, and film a wall clock beside the
   camera in a **consumer application**.
7. **Whether the current OBS sink now works end to end.** The device is present, so the earlier
   blocker is gone. Test: run `./target/release/camera-cli stream --sink virtual-camera
   --config ./obs.toml` and open the device in FaceTime.
