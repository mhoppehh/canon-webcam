//! Prerequisites P1 to P4 from `docs/adr/0002-macos-virtual-camera-mechanism.md`.
//!
//! The ADR is blunt about why these are checks rather than retries: an OBS copy
//! outside `/Applications` refuses even to submit its camera extension, a
//! camera extension is approved by a human in System Settings, and OBS's
//! WebSocket server reports nothing when it is disabled while a password still
//! sits in its configuration file. So each check is a live probe, and each
//! failure names the command or the setting the user must apply.
//!
//! Our process never performs P2. It is a one-time administrator decision taken
//! in the GUI, and a process that tried would be reaching for a privilege it does
//! not have.

use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::macos::obs_websocket::{
    ObsProtocolError, ObsWebSocketClient, DEFAULT_WEBSOCKET_PORT,
};
use crate::platform::macos::sink::MacOsVirtualCameraError;

/// How long to wait between polls of a just-launched OBS.
const LAUNCH_POLL: Duration = Duration::from_millis(250);

/// The OBS settings ADR-0002's prerequisites read, and how long they may take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObsPrerequisites {
    /// P1: the application bundle. OBS installs its virtual camera from
    /// `/Applications` and from nowhere else.
    pub obs_app_path: PathBuf,
    /// P3: where obs-websocket listens.
    pub websocket_endpoint: SocketAddr,
    /// P3: the password, if authentication is enabled.
    pub websocket_password: Option<String>,
    /// Where OBS writes its logs, which is where the explanation for a virtual
    /// camera that did not start is found.
    pub obs_log_directory: PathBuf,
    /// P4: whether launching OBS is allowed. Tests set this to `false` so no
    /// process is ever started.
    pub launch_obs: bool,
    /// How long one WebSocket connection attempt may take.
    pub websocket_timeout: Duration,
    /// P4: how long to wait for a launched OBS to answer.
    pub launch_deadline: Duration,
}

impl Default for ObsPrerequisites {
    fn default() -> Self {
        Self {
            obs_app_path: PathBuf::from("/Applications/OBS.app"),
            websocket_endpoint: SocketAddr::from(([127, 0, 0, 1], DEFAULT_WEBSOCKET_PORT)),
            websocket_password: None,
            obs_log_directory: default_log_directory(),
            launch_obs: true,
            websocket_timeout: Duration::from_secs(2),
            launch_deadline: Duration::from_secs(20),
        }
    }
}

fn default_log_directory() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
    home.join("Library/Application Support/obs-studio/logs")
}

impl ObsPrerequisites {
    /// P1: is OBS where it insists on being?
    ///
    /// # Errors
    ///
    /// Returns [`MacOsVirtualCameraError::ObsNotInstalledInApplications`], naming
    /// the directory and where OBS comes from.
    pub fn check_obs_in_applications(&self) -> Result<(), MacOsVirtualCameraError> {
        if self.obs_app_path.is_dir() {
            Ok(())
        } else {
            Err(MacOsVirtualCameraError::ObsNotInstalledInApplications {
                path: self.obs_app_path.display().to_string(),
            })
        }
    }

    /// P3 and P4: a connected and authenticated obs-websocket session.
    ///
    /// # Errors
    ///
    /// Returns [`MacOsVirtualCameraError::ObsUnauthenticated`] when the password
    /// is refused, and [`MacOsVirtualCameraError::ObsWebSocketUnavailable`] when
    /// nothing answers within the launch deadline.
    pub fn connect(&self) -> Result<ObsWebSocketClient, MacOsVirtualCameraError> {
        let first = match self.attempt() {
            Ok(client) => return Ok(client),
            Err(error) => error,
        };
        if matches!(first, ObsProtocolError::Unauthenticated { .. }) {
            return Err(first.into());
        }

        if !self.launch_obs {
            return Err(MacOsVirtualCameraError::ObsWebSocketUnavailable {
                detail: first.to_string(),
            });
        }

        // P4: OBS is the user's application. We start it if it is not running and
        // we never quit it, because obs-websocket 5 exposes no quit request and
        // OBS outlives this run.
        self.launch_application().map_err(|error| {
            MacOsVirtualCameraError::ObsWebSocketUnavailable {
                detail: format!(
                    "{first}; launching {} failed: {error}",
                    self.obs_app_path.display()
                ),
            }
        })?;

        let deadline = Instant::now() + self.launch_deadline;
        let mut last = first;
        loop {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(LAUNCH_POLL);
            match self.attempt() {
                Ok(client) => return Ok(client),
                Err(error @ ObsProtocolError::Unauthenticated { .. }) => return Err(error.into()),
                Err(error) => last = error,
            }
        }
        Err(MacOsVirtualCameraError::ObsWebSocketUnavailable {
            detail: format!(
                "{last} (waited {:.0}s for a launched {} to answer)",
                self.launch_deadline.as_secs_f64(),
                self.obs_app_path.display()
            ),
        })
    }

    fn attempt(&self) -> Result<ObsWebSocketClient, ObsProtocolError> {
        ObsWebSocketClient::connect(
            self.websocket_endpoint,
            self.websocket_password.as_deref(),
            self.websocket_timeout,
        )
    }

    /// Starts OBS minimised to the tray, so a run does not steal the screen.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if `open` cannot be started.
    pub fn launch_application(&self) -> io::Result<()> {
        let status = Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&self.obs_app_path)
            .args(["--args", "--minimize-to-tray"])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "`open -a {}` exited with {status}",
                self.obs_app_path.display()
            )))
        }
    }

    /// What OBS itself says about why its virtual camera is not running.
    ///
    /// Reads the newest log, because that is the run that just failed, and keeps
    /// the last lines that mention the virtual camera or its camera extension.
    #[must_use]
    pub fn obs_explanation(&self) -> String {
        let Some(path) = self.newest_log() else {
            return format!(
                "OBS's log directory {} holds no log, so OBS has never run from here",
                self.obs_log_directory.display()
            );
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return format!("OBS's log {} could not be read", path.display());
        };
        let relevant: Vec<&str> = text
            .lines()
            .filter(|line| {
                let lowered = line.to_ascii_lowercase();
                lowered.contains("virtualcam")
                    || lowered.contains("virtual camera")
                    || lowered.contains("camera-extension")
                    || lowered.contains("camera extension")
            })
            .collect();
        if relevant.is_empty() {
            return format!(
                "OBS's log {} says nothing about the virtual camera",
                path.display()
            );
        }
        let tail = &relevant[relevant.len().saturating_sub(4)..];
        tail.join(" | ")
    }

    fn newest_log(&self) -> Option<PathBuf> {
        let entries = fs::read_dir(&self.obs_log_directory).ok()?;
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
            .max_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok())
    }

    /// The application bundle as the messages name it.
    #[must_use]
    pub fn obs_app_path(&self) -> &Path {
        &self.obs_app_path
    }
}
