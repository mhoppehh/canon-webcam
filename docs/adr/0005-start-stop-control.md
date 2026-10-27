# A local Unix socket is the start/stop control surface

Ticket 03 settled the virtual camera mechanism: OBS Virtual Camera on macOS, with our process holding one obs-websocket control connection and one loopback HTTP/WebSocket frame transport. Ticket 07 needs a surface a person or a script can use to turn the live view feed off and on while the process keeps running, releasing the **PTP session** and the **virtual camera sink** between runs. The decision here is the front end — the thing that accepts "start" and "stop" — not the state machine behind it.

The state machine is `camera_core::control::FeedController`, and it is the shared contract the ticket asks for. It owns the **PTP session** and the **sink** for the whole life of the process, and it exposes exactly three things: start the feed, stop the feed, and say whether the feed is running. A front end may only call those. Because the controller and not the front end owns the session and the sink, "a stop releases both" and "a start takes both again" hold for every front end, including one that does not exist yet.

## Decision

A local Unix domain socket is bound by the process that runs the feed, on a path the user names, and it speaks four one-line commands: `start`, `stop`, `status` and `shutdown`. `start` answers `started` or `already-running`; `stop` answers `stopped` or `already-stopped`; `status` answers `running` or `stopped`. Any failure answers a single `error: ...` line. `camera-cli serve` runs the socket beside the feed, and `camera-cli control <command>` is the client, so a script checks state with one command that prints one word.

The socket is `std::os::unix::net::UnixListener` — the standard library, no crate. One connection is handled at a time, which is a limit and not an accident: the controller is the one place the session is taken, so serialising callers keeps "there is exactly one **PTP session**" true without a lock around the controller.

## Why a socket and not a menu bar item

- **Nothing in the workspace links a GUI toolkit.** The only user interface this program has is the page OBS renders, and it is a browser asset, not a native window. A menu bar item would mean `objc2-app-kit` plus a run loop on the main thread, which is a dependency and an architectural constraint bought for one feature.
- **The process already owns sockets.** The macOS backend binds a loopback HTTP server and a frame WebSocket, and obs-websocket is a WebSocket client. A Unix socket sits beside them with no new dependency and no new failure mode, and the trap that has to be handled — an accepted socket on macOS inherits `O_NONBLOCK` from its listener, so a blocking read needs `set_nonblocking(false)` first — is already the kind of thing this codebase handles in the transport.
- **A script can drive it, which is the acceptance criterion.** "The command reports whether the feed is currently running, so a script can check before acting" is a shell check against one word, not something to read off a menu.
- **It is the same mechanism on Linux.** A tray does not exist in a headless session, and ticket 09's Linux backend faces the same control question. A Unix socket answers it once, and it also works over SSH on the machine that holds the camera.
- **The tray stays possible.** A menu bar item is a second front end over `FeedController`, and would be added when a user is sitting at the machine and wants a click instead of a command. Nothing in this decision prevents it, because the front end holds no state.

## What a stop does, in order

1. The controller sets a `StopToken` that the pipeline checks on every turn of its loop, so the check costs one atomic load per 20 ms tick rather than a wake-up.
2. The pipeline leaves with `StreamEnd::Stopped`, and its existing exit path closes the **sink** before the run returns — the same path every other ending takes (ticket 06), so a stop cannot skip it.
3. The pipeline closes the capture queue, the capture thread notices between captures and returns, and the scoped join completes. The capture call in flight is the only thing a stop waits for.
4. The run function returns, the **PTP session** (owned by the opener) and the sink go out of scope and are dropped, and only then does the worker report the feed stopped.
5. The controller joins the worker with a bounded wait and reports `stopped`. If the wait is exhausted, it reports an error and says the device is still held, rather than blocking.

A start is the other direction and nothing more: a fresh opener, a fresh sink from the same factory, a fresh `Recovery`, a fresh clock, and `run_recovering` from the beginning — so the **output setting** and the **viewfinder action** are applied again by the code that applies them on any run, and the **leading frames** are handled by a fresh **calibration window** rather than by a discard count.

## Consequences

- The control surface is Unix-only. Windows needs a named pipe, which is ticket 08's problem; the controller itself is platform-neutral and is not affected.
- A socket path is a filesystem object with a lifetime. `serve` removes a stale socket file at the path before binding it and removes its own on the way out, and refuses to remove anything at that path that is not a socket.
- The feed runs on its own thread so the socket stays answerable while frames arrive, which means the controller owns a thread handle and must bound every wait on it. Both waits — the client's and the stop's — have timeouts, and neither can hang.
- One caller at a time. A second client waits for the first to disconnect, and a client that connects and says nothing is dropped after the client timeout (one second by default) instead of stalling the surface.
- `StreamEnd::Stopped` is a new, non-failing ending. Anything that treats a non-`DurationElapsed` ending as a failure would be wrong, so `is_failure` is explicit about it.
