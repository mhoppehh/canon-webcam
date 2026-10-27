# One long-lived PTP session per run

Every camera operation in a run — the output setting, the viewfinder action and the capture — shares a single PTP session held for the life of the run. The evidence is direct: running each operation as its own `gphoto2` process produced rejected property writes, after which the camera stayed on the USB bus and remained auto-detected while every operation returned `PTP Timeout` — a wedged camera that only a recovery reset or a cable reconnect cleared. gphoto bug 913 reports that on Canon bodies only the first `gphoto2` process after the cable connects behaves correctly. The same property write and capture succeed when performed inside one session, which is why this is structural rather than a preference. A recovery reset is not an exception to the rule: `gphoto2 --reset` destroys the wedged session and returns the camera to the bus, it does not open a second session beside it.

## Considered Options

- **A process per operation.** The obvious design, and the one the evidence above rules out. This is what a reader would expect the code to do, which is why the rule is written down.
- **`gphoto2 --shell` held open as a subprocess.** Satisfies the rule and was used to verify it by hand, but makes the session's lifetime depend on a child process's pipes, and the capture loop needs to own the handle.
- **One `gphoto2` invocation per run carrying every setting and the capture.** Correct and the cheapest form, verified working, but leaves no way to react to frames as they arrive.
- **Binding libgphoto2 directly through the `gphoto2` crate.** The strongest alternative, because a single owned camera handle makes the rule structural — enforced by the type system instead of by discipline. On inspection this is the leading candidate for a reason stronger than ergonomics: recalibration needs the output setting and the viewfinder action changed while a capture is already running, and start/stop needs the session released and retaken, and no CLI form can interleave settings with an open capture without starting a second process. Two things remain unproven — whether the crate can press the viewfinder action, and whether preview capture returns full-size frames — so it is settled by a spike in the first slice rather than by argument.

## Consequences

- The program is a long-running process rather than a series of short commands. Any setting that does not persist — notably the viewfinder action — must be re-applied inside the same session as the capture, on every run.
- A wedged camera becomes a state the capture layer must detect and recover from, not an error it can retry away by starting again.
- Because the session is exclusive, the CLI cannot offer two camera operations at once, including a status check during a running stream.
- Binding the library needs `libgphoto2` present at build and run time. That is a package install on macOS and Linux, and hard on Windows, where no official install exists — only an MSYS2 build, which does not link against the MSVC toolchain. The Windows backend therefore either targets the GNU toolchain or keeps the CLI, and that fork is decided there.
