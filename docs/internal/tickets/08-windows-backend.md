# 08: Windows virtual camera backend

**What to build:** The M50 appears as a selectable camera on Windows, using the sink interface already proven on macOS. Prefer Media Foundation on Windows 11, fall back to OBS where Media Foundation is unavailable.

This is a port of an existing abstraction, not a second product. It ships against the same recovery and control behaviour as the macOS path, which is why it is gated on those tickets rather than only on the sink.

**Blocked by:** 06, 07

**Status:** ready-for-agent

- [ ] The Windows backend implements the existing sink interface with no changes to capture or pipeline code.
- [ ] Media Foundation is used on Windows 11, with OBS as the documented fallback.
- [ ] A consumer application on Windows lists the virtual camera and shows the feed at the measured frame size.
- [ ] PTP failure recovery (05) and start/stop control (07) are both exercised on Windows, not assumed to carry over.
- [ ] The build path for the capture dependency on Windows is documented and reproducible from a clean machine.
- [ ] Platform-specific setup requirements (driver replacement, a required runtime) are documented as prerequisites with the exact steps.
- [ ] The ticket is not closed until it has been verified on real Windows hardware.
