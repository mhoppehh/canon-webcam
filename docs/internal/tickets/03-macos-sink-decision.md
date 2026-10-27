# 03: macOS virtual-camera sink decision

**What to build:** A recorded decision on how frames reach a consumer application on macOS, backed by enough of a trial to make the decision credible. This ticket deliberately produces a decision document, not a product.

The current plan says the sink "sends frames to OBS". No such interface exists: OBS Virtual Camera is a CoreMediaIO DAL plugin driven by OBS's own render pipeline, and no external process can inject frames into it. That assumption must be replaced before the sink is written.

The three real options to weigh, explicitly:

1. **OBS + Syphon/NDI source + obs-websocket** — publish frames over a shared-frame transport, ship a saved scene collection, drive `StartVirtualCam` through obs-websocket (built in since OBS 28). Low cost, testable today, but makes OBS a hard runtime dependency and leaves the user's OBS UI visible in the loop.
2. **CMIOExtension (macOS 12.3+)** — the supported modern route. High cost: signing, notarisation, and a user permission grant on install.
3. **DAL plugin** — what the plan implicitly assumed. High cost, and Apple has been steering away from it.

**Blocked by:** None (can start immediately, in parallel with 01 and 02)

**Status:** done

- [x] The three options are compared on: cost to build, cost to install, whether a consumer app sees the device without the user touching OBS, and whether each can be verified on the available machine today.
- [ ] The cheapest option is trialled far enough to prove or disprove the mechanism — frames published from our process appear in a consumer application.
- [x] A single option is chosen and the reason for rejecting the other two is written down.
- [x] The decision names what would force a revisit (for example, requiring the user to install OBS is unacceptable).
- [x] The decision is recorded in a form the next ticket can implement from without re-reading the plan.
- [ ] If the trial disproves the cheapest option, the decision falls through to the next cheapest with the same evidence standard.

## Comments

**Decision:** `docs/adr/0002-macos-virtual-camera-mechanism.md`, with the raw observations in `docs/adr/0002-macos-virtual-camera-mechanism-evidence.md`. OBS Virtual Camera is the macOS virtual camera mechanism; our process carries the live view feed to an OBS browser source over a local WebSocket and starts the camera through obs-websocket.

**Corrections to this ticket's framing, from primary sources.** On macOS 26 with OBS 32.2.1, OBS Virtual Camera is not a DAL plugin: OBS ships `com.obsproject.obs-studio.mac-camera-extension.systemextension` and activates it with `OSSystemExtensionRequest`, keeping the DAL bundle only for macOS before 15. The description of option 1 as necessarily "Syphon/NDI" is also too narrow — a local WebSocket to a browser source costs less and installs nothing extra. Both were checked, not assumed.

**The trial bullet is not satisfied, and cannot be satisfied here.** The chain was proven up to the boundary macOS enforces and no further:

- Proved: our process publishes 1024x576 JPEG frames to a Chromium page — the engine OBS's browser source embeds — at a sustained 30.03 fps (211 frames over 7 s).
- Proved: our process drives a live OBS through obs-websocket v5 (challenge authentication, `GetVersion`, `GetSceneList`, `GetInputList`, `GetVirtualCamStatus`, `StartVirtualCam`, `GetInputDefaultSettings`).
- Proved, and the reason ticket 04 must not trust it: `StartVirtualCam` returned `{"code":100,"result":true}` while `GetVirtualCamStatus` stayed `outputActive: false`. OBS's log gave the cause: `OBS cannot install the virtual camera if it's not in "/Applications".`
- Proved: no consumer application sees any virtual camera on this machine. `systemextensionsctl list` shows no OBS extension, `/Library/CoreMediaIO/Plug-Ins/DAL` holds only `plugins-info.txt`, and AVFoundation lists the built-in camera and Desk View only.

Missing evidence, and precisely why: the OBS present here is a Steam install outside `/Applications`, and installing its camera extension needs (a) the app in `/Applications` and (b) an administrator approving a system extension in System Settings. Copying an application into `/Applications` is an install, and the approval is a GUI decision for an administrator, so both were declined deliberately. The report that would close the gap is: AVFoundation listing a device named `OBS Virtual Camera`, a frame from it visible in FaceTime or Photo Booth, and OBS's frame-rate statistics. Latency and the 10-minute-call criterion remain unmeasured for the same reason; the obvious next measurement is a wall clock filmed beside the camera.

**The fall-through bullet did not apply.** The trial neither proved nor disproved the mechanism end to end; it proved the transport and the control channel and stopped at the operating-system boundary. Because the failure observed is a missing administrator approval rather than a broken mechanism, there is no evidence that would justify falling through to a more expensive option — and the two more expensive options were themselves ruled out on verifiable grounds: a CMIOExtension cannot be signed or notarised here (the only identity is `Apple Development`, and SIP blocks developer mode), and a DAL plugin is deprecated in Apple's own SDK since macOS 12.3.

**Environment facts that differ from the plan's assumptions.** OBS is installed, via Steam, not absent — the plan's "OBS not installed" was true only of `/Applications/OBS.app`. obs-websocket was disabled on this machine (`server_enabled: false`) with a password already stored, so a prerequisite check must be a live connection rather than the presence of OBS's configuration file. Both are recorded in the ADR.

**Process.** The trial launched OBS with `--minimize-to-tray`, backed up and restored `plugin_config/obs-websocket/config.json`, and quit OBS afterwards. Probe code stayed in `/tmp/cw-notes/03-trial/`; nothing but documentation was added to the repository.
