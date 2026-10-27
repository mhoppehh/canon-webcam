//! The local Unix socket front end of [`FeedController`].
//!
//! `docs/adr/0005-start-stop-control.md` chose a Unix domain socket over a menu
//! bar item: it needs no GUI toolkit, it is the same mechanism on Linux, a
//! headless machine can still be controlled over SSH, and one word in and one
//! word out is something a shell script can drive. The tray remains possible
//! later because it would be a second front end over the same controller, and
//! would own no state of its own.
//!
//! # The trap this module exists to get right
//!
//! On macOS an accepted socket inherits `O_NONBLOCK` from its listener, and a
//! read timeout has no effect on a non-blocking socket: a read returns
//! `WouldBlock` immediately instead of waiting. A listener is non-blocking here
//! (so the accept loop can also notice a shutdown), so every accepted connection
//! is put back into blocking mode *before* the timeouts are set, and every wait —
//! the client's and the server's — has a bound. A client that connects and says
//! nothing is dropped after [`DEFAULT_CLIENT_TIMEOUT`] rather than stalling the
//! surface.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use super::{ControlReply, ControlRequest, FeedController, FeedState};

/// How long the surface waits for a connected client to send its one line, and
/// how long the client waits for its one line of answer.
pub const DEFAULT_CLIENT_TIMEOUT: Duration = Duration::from_secs(1);

/// How often the accept loop looks up from the (non-blocking) listener.
const ACCEPT_TICK: Duration = Duration::from_millis(10);

/// The control surface: a socket path, the listener on it, and the controller it
/// drives.
///
/// One connection is served at a time. That is what keeps the controller free of
/// a lock and keeps "exactly one **PTP session**" true no matter how many callers
/// there are: they queue.
pub struct ControlServer {
    listener: UnixListener,
    path: PathBuf,
    controller: FeedController,
    client_timeout: Duration,
}

impl ControlServer {
    /// Binds the control socket at `path` and takes the controller it drives.
    ///
    /// A socket file left behind by a run that did not shut down cleanly is
    /// replaced, because a stale path is the normal way this starts. Anything at
    /// that path which is not a socket is refused: this function removes a file,
    /// and it will not remove a file it does not understand.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error if the path cannot be inspected,
    /// removed, or bound.
    pub fn bind(path: impl AsRef<Path>, controller: FeedController) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "{} exists and is not a socket, so it is not a control surface this \
                             program left behind and it will not be removed",
                            path.display()
                        ),
                    ));
                }
                fs::remove_file(&path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;

        Ok(Self {
            listener,
            path,
            controller,
            client_timeout: DEFAULT_CLIENT_TIMEOUT,
        })
    }

    /// Replaces the bound on how long one client may take to say its one word.
    #[must_use]
    pub fn with_client_timeout(mut self, timeout: Duration) -> Self {
        self.client_timeout = timeout;
        self
    }

    /// The path the socket is bound at, which is what a caller connects to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the feed behind this surface is running.
    #[must_use]
    pub fn state(&self) -> FeedState {
        self.controller.state()
    }

    /// Serves commands until a client sends `shutdown`.
    ///
    /// The socket file is removed on the way out, and the controller is dropped
    /// afterwards, which stops a feed that is still running.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error if accepting or answering a client
    /// fails for a reason other than the connection being closed.
    pub fn serve_until_shutdown(mut self) -> io::Result<()> {
        loop {
            match self.listener.accept() {
                Ok((stream, _address)) => {
                    if self.handle(stream)? {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_TICK);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.remove_socket_file();
                    return Err(error);
                }
            }
        }

        self.remove_socket_file();
        Ok(())
    }

    /// Answers one connection. Returns whether the surface should close.
    fn handle(&mut self, stream: UnixStream) -> io::Result<bool> {
        // See the module documentation: the accepted socket inherits
        // `O_NONBLOCK`, and a read timeout on a non-blocking socket does nothing.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(self.client_timeout))?;
        stream.set_write_timeout(Some(self.client_timeout))?;

        let mut reader = BufReader::new(stream);
        let command = read_line(&mut reader);
        let (reply, shutdown) = self.dispatch(&command);

        let mut stream = reader.into_inner();
        // A client that left before its answer is not this surface's problem: a
        // rude client must not be able to close the control surface, and there is
        // nobody left to tell about the failure anyway.
        let _ = stream
            .write_all(format!("{reply}\n").as_bytes())
            .and_then(|()| stream.flush());

        Ok(shutdown)
    }

    /// Carries out one command and answers it.
    fn dispatch(&mut self, command: &str) -> (ControlReply, bool) {
        match ControlRequest::parse(command) {
            None if command.is_empty() => (
                ControlReply::Failed(
                    "no command was received; send start, stop, status or shutdown".to_owned(),
                ),
                false,
            ),
            None => (
                ControlReply::Failed(format!(
                    "unknown command `{command}`; send start, stop, status or shutdown"
                )),
                false,
            ),
            Some(ControlRequest::Status) => {
                // A run that ended on its own is recorded here, so a script that
                // only ever asks `status` learns why the feed is not running
                // instead of just that it stopped.
                self.controller.record_finished_run();
                let status = self.controller.status();
                match (status.state, status.error) {
                    (FeedState::Stopped, Some(detail)) => (ControlReply::Failed(detail), false),
                    (state, _) => (ControlReply::state(state), false),
                }
            }
            Some(ControlRequest::Start) => match self.controller.start() {
                Ok(change) => (ControlReply::from(change), false),
                Err(error) => (ControlReply::Failed(error.to_string()), false),
            },
            Some(ControlRequest::Stop) => match self.controller.stop() {
                Ok(change) => (ControlReply::from(change), false),
                Err(error) => (ControlReply::Failed(error.to_string()), false),
            },
            Some(ControlRequest::Shutdown) => match self.controller.stop() {
                Ok(_) => (ControlReply::ShuttingDown, true),
                // The feed may still hold the camera, which the message says; the
                // surface closes anyway rather than becoming unreachable.
                Err(error) => (ControlReply::Failed(error.to_string()), true),
            },
        }
    }

    /// Removes the socket file, ignoring the case where it is already gone.
    fn remove_socket_file(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                // Nothing actionable: the path is gone enough that binding it
                // again works, and refusing to close the surface would be worse.
            }
        }
    }
}

/// Sends one command to the surface at `path` and returns its answer.
///
/// # Errors
///
/// Returns the operating system's error if the surface is not there, does not
/// answer within `timeout`, or answers something that is not part of the
/// protocol.
pub fn send_command(
    path: impl AsRef<Path>,
    request: ControlRequest,
    timeout: Duration,
) -> io::Result<ControlReply> {
    let mut stream = UnixStream::connect(path)?;
    // Blocking again before the timeouts, for the same reason the server does it:
    // a timeout is only honoured by a blocking socket.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    stream.write_all(format!("{}\n", request.as_str()).as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let line = read_line(&mut reader);
    ControlReply::parse(&line).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the control surface answered {line:?}, which is not part of the protocol; \
                 expected started, already-running, stopped, already-stopped, running, \
                 shutting-down or `error: ...`"
            ),
        )
    })
}

/// Reads one line, treating a closed or silent connection as "no command".
fn read_line(reader: &mut BufReader<UnixStream>) -> String {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(_) => line.trim().to_owned(),
        // A client that says nothing, and a client that leaves without saying
        // anything, are both answered as "no command" rather than waited on.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            String::new()
        }
        Err(_) => String::new(),
    }
}
