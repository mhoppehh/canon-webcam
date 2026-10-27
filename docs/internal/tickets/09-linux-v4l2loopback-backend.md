# 09: Linux v4l2loopback backend

**What to build:** The M50 appears as `/dev/videoN` on Linux through `v4l2loopback`. Same sink interface, same recovery and control behaviour, a different device model.

`v4l2loopback` is a kernel module the user must load, so the failure to handle well is "module not loaded", not "device missing".

**Blocked by:** 06, 07

**Status:** ready-for-agent

- [ ] The Linux backend implements the existing sink interface with no changes to capture or pipeline code.
- [ ] The presence of a loaded `v4l2loopback` module is checked before streaming, and its absence produces a message naming the exact command to load it.
- [ ] The configured device is created as a virtual camera and appears in a consumer application.
- [ ] PTP failure recovery (05) and start/stop control (07) are both exercised on Linux, not assumed to carry over.
- [ ] Audio capture is explicitly out of scope, and the absence of audio is stated rather than silently implied.
- [ ] Build prerequisites for the capture dependency and the loopback module are documented and reproducible from a clean machine.
- [ ] The ticket is not closed until it has been verified on real Linux hardware.
