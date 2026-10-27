# 01: Capture session foundation and live-view measurement

**What to build:** A `measure` command that holds one PTP session against the M50, applies the acquisition settings, captures through a calibration window, and reports the **measured** frame size and rate — never the requested or nominal one. This is the first slice that talks to hardware, so it also establishes the Cargo workspace and the `camera-core` / `camera-cli` split.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] The command runs as a single CLI subcommand and holds **one** PTP session for the whole operation; a second concurrent session is a bug, not a fallback.
- [ ] `output` is set to `TFT + PC` (value 3) at session start.
- [ ] `viewfinder=1` is set in the **same invocation** as the capture, on every run — it does not persist across invocations.
- [ ] The expected frame size is established by a **calibration window**, not by a fixed count of discarded frames. A fixed count is rejected outright: it locks onto the reduced size, because that is exactly what the leading frames are.
- [ ] The reported size and rate are measured from frame data, never read from a property.
- [ ] A capture that produces no size recurring often enough inside the window fails loudly and reports why.
- [ ] The `output` value is guarded by an **allowlist of `{0, 1, 2, 3}`**, not a blocklist. Only value 3 (full size) and value 8 (`MOBILE2`, reduced size) are verified on an M50; the rest are rejected because they are unverified here, and because a report from a different Canon body describes combinations of `PC` and `MOBILE` needing a battery removal to clear. The guard's comment states which part is verified and which is not.
- [ ] The command determines and records whether `output` survives across process boundaries. The guard must be written to set it on every run unless persistence is actually proven.
- [ ] The binding is decided by a spike, not by argument: the `gphoto2` crate must prove it can press the viewfinder action, write the output setting, and return full-size frames from preview capture. The CLI is the fallback only if the crate fails one of those.
- [ ] The spike's outcome is written down, because it decides whether recalibration and start/stop are achievable at all under the one-session rule. See ADR-0001.
- [ ] `liveviewsize` is documented as a **no-op on Canon** (it accepts the write and changes nothing; it exists for Nikon/Fuji/Sony). The command does not attempt to use it to select a size, and does not report it, because doing so produces a confidently wrong answer.
- [ ] The measured result is printed so a human can read and diff it between runs. `1024x576` at roughly 24 fps is used as a sanity check on the measurement, never as its source.
- [ ] A pre-implementation note records that the `PLAN.md` §2.3.1 "select the largest live-view size option" step is deleted, so nobody re-implements it.
