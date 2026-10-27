//! The macOS **platform backend**, as one [`FrameSink`].
//!
//! `docs/adr/0002-macos-virtual-camera-mechanism.md` fixes everything this module
//! does, including the order in which it does it:
//!
//! 1. validate the mechanism; check P1 and P2 before touching OBS;
//! 2. bind the frame transport and read back its port;
//! 3. connect to obs-websocket and authenticate (P3, and P4 if OBS is not up);
//! 4. create or update the `browser_source` input with the settings the ADR lists;
//! 5. create the scene, put one scene item in it, make it the program scene and
//!    remember the consumer's previous one;
//! 6. `StartVirtualCam`, then confirm with `GetVirtualCamStatus` **and** the
//!    AVFoundation listing. `StartVirtualCam` returning success is not evidence
//!    and is never treated as one;
//! 7. on any failure from 4 onwards, undo 4 to 6 in reverse.
//!
//! Close does the reverse of open and leaves OBS running. It is idempotent, it
//! runs on every exit path, and it succeeds when OBS has already exited.
//!
//! Nothing here resizes an open sink: a new **expected frame size** is a **sink
//! re-open**, which is what the pipeline asks for and what this module does.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::frame_size::FrameSize;
use crate::pixel_format::{LiveViewFrame, PixelFormat};
use crate::platform::macos::device_probe::VirtualCameraDeviceProbe;
use crate::platform::macos::frames_transport::FrameTransport;
use crate::platform::macos::obs_websocket::{ObsProtocolError, ObsWebSocketClient};
use crate::platform::macos::prerequisites::ObsPrerequisites;
use crate::sink::{FrameSink, SinkError};

/// The scene name ADR-0002 specifies.
pub const VIRTUAL_CAMERA_SCENE_NAME: &str = "canon-webcam";
/// The input that carries the live view feed into OBS.
///
/// It must not equal the scene name. OBS keeps scenes and inputs in one
/// namespace, so a scene and a source cannot share a name: `CreateInput` is
/// refused with "A source already exists by that input name" when the scene has
/// already been made under it.
pub const VIRTUAL_CAMERA_INPUT_NAME: &str = "canon-webcam source";
/// The OBS input kind a browser source is created as.
pub const BROWSER_SOURCE_KIND: &str = "browser_source";
/// The CSS ADR-0002 reads from OBS 32.2.1's own defaults, kept verbatim.
pub const BROWSER_SOURCE_CSS: &str =
    "body { background-color: rgba(0, 0, 0, 0); margin: 0px auto; overflow: hidden; }";
/// The JPEG quality a resized frame is encoded at. An un-resized frame keeps the
/// camera's own bytes and never reaches this encoder.
const RESIZED_FRAME_QUALITY: u8 = 90;
/// The frame rate written into the browser source when no measured frame rate is
/// known yet. A preference only: the frames themselves decide the rate.
const FALLBACK_FRAME_RATE: f64 = 30.0;

/// How long OBS may take to bring the virtual camera up before an open fails.
///
/// `StartVirtualCam` is not instant: OBS starts the output on its own thread, so
/// an immediate `GetVirtualCamStatus` can still answer `false`, and a run started
/// while the previous output is stopping is refused outright. Both are the normal
/// shape of a five-second run started twice in a row, not a broken mechanism.
const VIRTUAL_CAMERA_START_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the confirmation is re-read while OBS starts the output.
const VIRTUAL_CAMERA_START_POLL: Duration = Duration::from_millis(100);

/// Everything the macOS backend needs that is not its mechanism.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacOsVirtualCameraConfig {
    /// P1 to P4.
    pub prerequisites: ObsPrerequisites,
    /// The OBS scene the live view feed is composited in.
    pub scene_name: String,
    /// The browser source the feed arrives through.
    pub input_name: String,
    /// How long OBS may take to bring the virtual camera up. Tests set this
    /// short; a run leaves it at [`VIRTUAL_CAMERA_START_TIMEOUT`].
    pub virtual_camera_start_timeout: Duration,
}

impl Default for MacOsVirtualCameraConfig {
    fn default() -> Self {
        Self {
            prerequisites: ObsPrerequisites::default(),
            scene_name: VIRTUAL_CAMERA_SCENE_NAME.to_owned(),
            input_name: VIRTUAL_CAMERA_INPUT_NAME.to_owned(),
            virtual_camera_start_timeout: VIRTUAL_CAMERA_START_TIMEOUT,
        }
    }
}

/// Why the macOS backend could not open, keep open, or deliver.
///
/// The variants are the ADR's, and so are the messages: each one names the
/// command, the dialog or the setting the user has to apply, because a generic
/// "could not start" would leave a user with an OBS dependency and no way to
/// satisfy it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacOsVirtualCameraError {
    /// P1: there is no OBS application bundle where OBS insists on living.
    ObsNotInstalledInApplications {
        /// The path that was looked for.
        path: String,
    },
    /// P2: no `OBS Virtual Camera` device is registered, so no consumer
    /// application can see one.
    VirtualCameraDeviceAbsent {
        /// AVFoundation's own complaint, when it could not even be asked.
        detail: Option<String>,
    },
    /// P3 or P4: nothing answered on the obs-websocket port.
    ObsWebSocketUnavailable {
        /// What happened.
        detail: String,
    },
    /// P3: the server answered and refused the password.
    ObsUnauthenticated {
        /// What happened.
        detail: String,
    },
    /// `StartVirtualCam` was accepted and the virtual camera is not running.
    ObsVirtualCameraFailedToStart {
        /// What OBS's own log says.
        obs_explanation: String,
    },
    /// OBS went away, or stopped the virtual camera, while the live view feed was
    /// being delivered.
    ObsDisconnected {
        /// What happened.
        detail: String,
    },
    /// OBS understood a request and refused it.
    ObsRequestRefused {
        /// The request type.
        request_type: String,
        /// OBS's own comment.
        comment: String,
    },
    /// The local frame transport could not bind a port.
    FrameTransportUnavailable {
        /// What the operating system said.
        detail: String,
    },
    /// A frame that is not the camera's own JPEG could not be encoded.
    UnencodableFrame {
        /// What the encoder said.
        detail: String,
    },
}

impl fmt::Display for MacOsVirtualCameraError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ObsNotInstalledInApplications { path } => write!(
                formatter,
                "OBS Virtual Camera is macOS's only virtual camera mechanism, and OBS installs it \
                 only from `/Applications`, so {path} cannot be used. Install OBS Studio from \
                 obsproject.com and leave it at `/Applications/OBS.app`; a copy installed elsewhere \
                 (a Steam install lives in `~/Library/Application Support/Steam/...`) must be copied \
                 to `/Applications`"
            ),
            Self::VirtualCameraDeviceAbsent { detail } => {
                if let Some(detail) = detail {
                    write!(formatter, "{detail}. ")?;
                }
                write!(
                    formatter,
                    "No `OBS Virtual Camera` device is registered, so no consumer application can \
                     see the live view feed. Approve OBS's camera extension once:\n\
                     1. Open `/Applications/OBS.app`.\n\
                     2. In OBS, click **Start Virtual Camera** in the Controls panel.\n\
                     3. When macOS reports that the system extension was blocked, open **System \
                     Settings → General → Login Items & Extensions → Camera Extensions**, enable \
                     **OBS Virtual Camera**, and authenticate as an administrator.\n\
                     4. Click **Stop Virtual Camera** and quit OBS."
                )
            }
            Self::ObsWebSocketUnavailable { detail } => write!(
                formatter,
                "OBS's WebSocket server is not answering ({detail}). In OBS open **Tools → \
                 WebSocket Server Settings**, tick **Enable WebSocket server**, keep port `4455`, \
                 tick **Enable Authentication**, set a password, and put the same password in \
                 `websocket_password` in the macOS mechanism section of the configuration"
            ),
            Self::ObsUnauthenticated { detail } => write!(
                formatter,
                "OBS's WebSocket server refused the password ({detail}). In OBS open **Tools → \
                 WebSocket Server Settings** and set the password to the same value as \
                 `websocket_password` in the macOS mechanism section of the configuration"
            ),
            Self::ObsVirtualCameraFailedToStart { obs_explanation } => write!(
                formatter,
                "OBS accepted StartVirtualCam but the virtual camera is not running, so the live \
                 view feed has nowhere to go. StartVirtualCam reports success even when OBS never \
                 installed its camera extension, which is why this is confirmed with \
                 GetVirtualCamStatus and the AVFoundation listing. OBS's own explanation: \
                 {obs_explanation}"
            ),
            Self::ObsDisconnected { detail } => write!(
                formatter,
                "the connection to OBS was lost while the live view feed was being delivered \
                 ({detail}). The run has ended; start it again once OBS is available. This program \
                 does not restart OBS in a loop"
            ),
            Self::ObsRequestRefused {
                request_type,
                comment,
            } => write!(formatter, "OBS refused `{request_type}`: {comment}"),
            Self::FrameTransportUnavailable { detail } => write!(
                formatter,
                "the frame transport could not bind a loopback port: {detail}"
            ),
            Self::UnencodableFrame { detail } => write!(
                formatter,
                "a frame that is not the camera's own JPEG could not be encoded for the frame \
                 transport: {detail}"
            ),
        }
    }
}

impl std::error::Error for MacOsVirtualCameraError {}

impl From<ObsProtocolError> for MacOsVirtualCameraError {
    fn from(error: ObsProtocolError) -> Self {
        match error {
            ObsProtocolError::Rejected {
                request_type,
                comment,
            } => Self::ObsRequestRefused {
                request_type,
                comment,
            },
            ObsProtocolError::Disconnected { detail } => Self::ObsDisconnected { detail },
            other => Self::ObsWebSocketUnavailable {
                detail: other.to_string(),
            },
        }
    }
}

/// What the virtual camera sink is doing, for the start/stop control and the run
/// report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtualCameraStatus {
    /// Whether the sink is open.
    pub is_open: bool,
    /// The **expected frame size** the sink is open at.
    pub expected_frame_size: Option<FrameSize>,
    /// Whether OBS reported the virtual camera running when it was last asked.
    pub virtual_camera_active: bool,
    /// Whether the consumer page is attached to the frame transport.
    pub consumer_page_connected: bool,
    /// Whether a consumer page attached at any point while the sink was open.
    ///
    /// The counters below outlive the close, because a run report is printed
    /// after the run has closed the sink; this is the same fact for the page.
    pub consumer_page_seen: bool,
    /// How many times the transport served the page. Zero says nothing ever asked
    /// for it, which is a browser source problem and not a transport problem.
    pub consumer_pages_served: u64,
    /// Frames handed to the consumer page.
    pub frames_published: u64,
    /// Frames dropped because the page was still being written to.
    pub frames_dropped: u64,
    /// How many times this sink has been opened, **sink re-opens** included.
    pub sink_opens: u64,
    /// One-off remarks worth printing once, such as OBS being left running.
    pub notes: Vec<String>,
}

/// The state that exists only while the sink is open.
#[derive(Debug)]
struct OpenSink {
    expected_frame_size: FrameSize,
    pixel_format: PixelFormat,
    transport: FrameTransport,
    obs: Option<ObsWebSocketClient>,
    previous_program_scene: Option<String>,
    input_created: bool,
    scene_created: bool,
    program_scene_set: bool,
    virtual_camera_active: bool,
}

/// The macOS **virtual camera sink**: OBS Virtual Camera, driven over
/// obs-websocket, fed by the local frame transport.
pub struct MacOsVirtualCameraSink {
    config: MacOsVirtualCameraConfig,
    device_probe: Arc<dyn VirtualCameraDeviceProbe>,
    open: Option<OpenSink>,
    sink_opens: u64,
    /// Frames published across every open, so the totals a run report prints are
    /// still there after the last **sink re-open** has been closed.
    frames_published: u64,
    /// Frames dropped by a busy consumer page, across every open.
    frames_dropped: u64,
    /// Pages served across every open.
    consumer_pages_served: u64,
    /// Whether a consumer page ever attached to this sink.
    consumer_page_seen: bool,
    notes: Vec<String>,
}

impl MacOsVirtualCameraSink {
    /// Builds the sink. Nothing is started until
    /// [`open_virtual_camera`](Self::open_virtual_camera) is called.
    #[must_use]
    pub fn new(
        config: MacOsVirtualCameraConfig,
        device_probe: Arc<dyn VirtualCameraDeviceProbe>,
    ) -> Self {
        Self {
            config,
            device_probe,
            open: None,
            sink_opens: 0,
            frames_published: 0,
            frames_dropped: 0,
            consumer_pages_served: 0,
            consumer_page_seen: false,
            notes: Vec::new(),
        }
    }

    /// Opens the sink at the **expected frame size**, returning the ADR's own
    /// error for the prerequisite that failed.
    ///
    /// `measured_frame_rate` paces the browser source; `None` uses a preference
    /// that the frames themselves will show to be wrong or right.
    ///
    /// # Errors
    ///
    /// Returns [`MacOsVirtualCameraError`], whose message names the command or
    /// setting to apply.
    pub fn open_virtual_camera(
        &mut self,
        expected_frame_size: FrameSize,
        measured_frame_rate: Option<f64>,
        pixel_format: PixelFormat,
    ) -> Result<(), MacOsVirtualCameraError> {
        // A size change is a sink re-open and nothing else.
        self.close_virtual_camera()?;

        // P1 then P2, before OBS is touched at all.
        self.config.prerequisites.check_obs_in_applications()?;
        match self.device_probe.virtual_camera_device_present() {
            Ok(true) => {}
            Ok(false) => {
                return Err(MacOsVirtualCameraError::VirtualCameraDeviceAbsent { detail: None })
            }
            Err(failure) => {
                return Err(MacOsVirtualCameraError::VirtualCameraDeviceAbsent {
                    detail: Some(failure.to_string()),
                })
            }
        }

        let transport = FrameTransport::bind().map_err(|error| {
            MacOsVirtualCameraError::FrameTransportUnavailable {
                detail: error.to_string(),
            }
        })?;
        let mut obs = self.config.prerequisites.connect()?;

        let mut open = OpenSink {
            expected_frame_size,
            pixel_format,
            transport,
            obs: None,
            previous_program_scene: None,
            input_created: false,
            scene_created: false,
            program_scene_set: false,
            virtual_camera_active: false,
        };

        let prepared = Self::prepare(
            &self.config,
            &*self.device_probe,
            &mut obs,
            expected_frame_size,
            measured_frame_rate,
            &mut open,
        );
        if let Err(error) = prepared {
            Self::undo(&self.config, &mut obs, &open);
            obs.close();
            return Err(error);
        }

        open.obs = Some(obs);
        open.virtual_camera_active = true;
        self.open = Some(open);
        self.sink_opens += 1;
        Ok(())
    }

    /// Closes the sink, releasing the device so the next consumer application can
    /// open it.
    ///
    /// Idempotent: closing a closed sink succeeds. It leaves OBS running, because
    /// obs-websocket exposes no quit request and OBS is the user's application
    /// once it runs.
    ///
    /// # Errors
    ///
    /// Nothing at present — a sink whose OBS has already exited is already
    /// closed — but the signature is fallible so that a backend which cannot
    /// release the device can say so.
    pub fn close_virtual_camera(&mut self) -> Result<(), MacOsVirtualCameraError> {
        let Some(mut open) = self.open.take() else {
            return Ok(());
        };
        // The transport goes with the open sink, so its counters are moved to the
        // sink first: a run report is printed after this call and must still be
        // able to say what was published.
        self.frames_published += open.transport.frames_delivered();
        self.frames_dropped += open.transport.frames_dropped();
        self.consumer_pages_served += open.transport.pages_served();
        if let Some(mut obs) = open.obs.take() {
            Self::undo(&self.config, &mut obs, &open);
            obs.close();
            if !self
                .notes
                .iter()
                .any(|note| note.starts_with("OBS is left running"))
            {
                self.notes.push(
                    "OBS is left running: obs-websocket 5 exposes no quit request, and OBS is the \
                     user's application once this program has started it"
                        .to_owned(),
                );
            }
        }
        // Dropping the transport releases the port and ends the page's writer.
        drop(open.transport);
        Ok(())
    }

    /// What the sink is doing right now, without asking OBS anything.
    #[must_use]
    pub fn status(&self) -> VirtualCameraStatus {
        let mut status = VirtualCameraStatus {
            sink_opens: self.sink_opens,
            frames_published: self.frames_published,
            frames_dropped: self.frames_dropped,
            consumer_pages_served: self.consumer_pages_served,
            consumer_page_seen: self.consumer_page_seen,
            notes: self.notes.clone(),
            ..VirtualCameraStatus::default()
        };
        if let Some(open) = &self.open {
            status.is_open = true;
            status.expected_frame_size = Some(open.expected_frame_size);
            status.virtual_camera_active = open.virtual_camera_active;
            status.consumer_page_connected = open.transport.consumer_page_connected();
            status.frames_published += open.transport.frames_delivered();
            status.frames_dropped += open.transport.frames_dropped();
            status.consumer_pages_served += open.transport.pages_served();
        }
        status
    }

    /// Asks OBS whether the virtual camera is still running, which is the only
    /// trustworthy answer: `StartVirtualCam` was not one.
    ///
    /// # Errors
    ///
    /// Returns [`MacOsVirtualCameraError::ObsDisconnected`] when the control
    /// channel has gone.
    pub fn refresh_virtual_camera_active(&mut self) -> Result<bool, MacOsVirtualCameraError> {
        let Some(open) = self.open.as_mut() else {
            return Ok(false);
        };
        let Some(obs) = open.obs.as_mut() else {
            return Ok(false);
        };
        let response = obs.request("GetVirtualCamStatus", None)?;
        let active = response["outputActive"].as_bool().unwrap_or(false);
        open.virtual_camera_active = active;
        Ok(active)
    }

    /// The URL of the page OBS's browser source loads, while the sink is open.
    #[must_use]
    pub fn page_url(&self) -> Option<String> {
        self.open.as_ref().map(|open| open.transport.page_url())
    }

    /// The WebSocket URL the page opens for frames, while the sink is open.
    #[must_use]
    pub fn frames_websocket_url(&self) -> Option<String> {
        self.open.as_ref().map(|open| open.transport.frames_url())
    }

    /// Steps 4 to 7 of the open, in the ADR's order.
    fn prepare(
        config: &MacOsVirtualCameraConfig,
        device_probe: &dyn VirtualCameraDeviceProbe,
        obs: &mut ObsWebSocketClient,
        frame_size: FrameSize,
        measured_frame_rate: Option<f64>,
        open: &mut OpenSink,
    ) -> Result<(), MacOsVirtualCameraError> {
        let (previous_program_scene, scene_created) = Self::ensure_scene(obs, &config.scene_name)?;
        open.previous_program_scene = Some(previous_program_scene);
        open.scene_created = scene_created;
        // The input named `canon-webcam` is ours whether this run created it or a
        // crashed run left it behind, so close removes it either way.
        open.input_created = true;

        Self::ensure_input(
            obs,
            config,
            &open.transport,
            frame_size,
            measured_frame_rate,
        )?;
        Self::ensure_scene_item(obs, &config.scene_name, &config.input_name)?;
        obs.request(
            "SetCurrentProgramScene",
            Some(json!({ "sceneName": config.scene_name })),
        )?;
        open.program_scene_set = true;

        Self::start_virtual_camera(config, device_probe, obs)
    }

    /// Steps 6 and 7: start the virtual camera, then confirm it, giving OBS the
    /// time it actually takes.
    ///
    /// A single confirmation is a race. OBS starts the output asynchronously, so
    /// `GetVirtualCamStatus` immediately after `StartVirtualCam` can still answer
    /// `false`. The request is therefore sent **once** and only the status is
    /// re-read: sending it again while the output is starting makes OBS restart
    /// the output on every attempt, which against a camera extension macOS cannot
    /// reach turns one failure into a storm of "could not find the virtual camera
    /// system process" alerts.
    ///
    /// A refusal is not an answer by itself — the output may already be running
    /// from a killed run, or still stopping — so the status decides, and only a
    /// persistent answer past the deadline is a failure.
    fn start_virtual_camera(
        config: &MacOsVirtualCameraConfig,
        device_probe: &dyn VirtualCameraDeviceProbe,
        obs: &mut ObsWebSocketClient,
    ) -> Result<(), MacOsVirtualCameraError> {
        let deadline = Instant::now() + config.virtual_camera_start_timeout;

        match obs.request("StartVirtualCam", None) {
            Ok(_) => {}
            Err(ObsProtocolError::Rejected { .. }) => {}
            Err(error) => return Err(error.into()),
        }

        loop {
            let status = obs.request("GetVirtualCamStatus", None)?;
            let output_active = status["outputActive"].as_bool().unwrap_or(false);
            let device_present = device_probe
                .virtual_camera_device_present()
                .unwrap_or(false);
            if output_active && device_present {
                return Ok(());
            }

            if Instant::now() >= deadline {
                return Err(MacOsVirtualCameraError::ObsVirtualCameraFailedToStart {
                    obs_explanation: config.prerequisites.obs_explanation(),
                });
            }
            thread::sleep(VIRTUAL_CAMERA_START_POLL);
        }
    }

    /// Creates the scene if it is missing and returns the consumer application's
    /// current program scene, which close restores, and whether this run created
    /// the scene.
    fn ensure_scene(
        obs: &mut ObsWebSocketClient,
        scene_name: &str,
    ) -> Result<(String, bool), MacOsVirtualCameraError> {
        let list = obs.request("GetSceneList", None)?;
        let previous = list["currentProgramSceneName"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let exists = list["scenes"].as_array().is_some_and(|scenes| {
            scenes
                .iter()
                .any(|scene| scene["sceneName"].as_str() == Some(scene_name))
        });
        if exists {
            return Ok((previous, false));
        }
        obs.request("CreateScene", Some(json!({ "sceneName": scene_name })))?;
        Ok((previous, true))
    }

    fn ensure_input(
        obs: &mut ObsWebSocketClient,
        config: &MacOsVirtualCameraConfig,
        transport: &FrameTransport,
        frame_size: FrameSize,
        measured_frame_rate: Option<f64>,
    ) -> Result<(), MacOsVirtualCameraError> {
        let settings = browser_source_settings(transport, frame_size, measured_frame_rate);
        let inputs = obs.request("GetInputList", None)?;
        let exists = inputs["inputs"].as_array().is_some_and(|inputs| {
            inputs
                .iter()
                .any(|input| input["inputName"].as_str() == Some(config.input_name.as_str()))
        });
        if exists {
            // A crashed run can leave the input behind; the next run reuses it.
            obs.request(
                "SetInputSettings",
                Some(json!({
                    "inputName": config.input_name,
                    "inputSettings": settings,
                    "overlay": true,
                })),
            )?;
        } else {
            obs.request(
                "CreateInput",
                Some(json!({
                    // obs-websocket refuses a CreateInput that names no scene, and
                    // an input created into no scene is not visible to the virtual
                    // camera anyway: the scene is what holds it.
                    "sceneName": config.scene_name,
                    "inputName": config.input_name,
                    "inputKind": BROWSER_SOURCE_KIND,
                    "inputSettings": settings,
                    "sceneItemEnabled": true,
                })),
            )?;
        }
        Ok(())
    }

    fn ensure_scene_item(
        obs: &mut ObsWebSocketClient,
        scene_name: &str,
        input_name: &str,
    ) -> Result<(), MacOsVirtualCameraError> {
        let items = obs.request("GetSceneItemList", Some(json!({ "sceneName": scene_name })))?;
        let present = items["sceneItems"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["sourceName"].as_str() == Some(input_name))
        });
        if !present {
            obs.request(
                "CreateSceneItem",
                Some(json!({
                    "sceneName": scene_name,
                    "sourceName": input_name,
                    "sceneItemEnabled": true,
                })),
            )?;
        }
        Ok(())
    }

    /// The reverse of steps 4 to 7. Every request is best effort: undoing a
    /// failure must not hide it, and the sink is closing anyway.
    fn undo(config: &MacOsVirtualCameraConfig, obs: &mut ObsWebSocketClient, open: &OpenSink) {
        let _ = obs.request("StopVirtualCam", None);
        if open.program_scene_set {
            if let Some(previous) = &open.previous_program_scene {
                let _ = obs.request(
                    "SetCurrentProgramScene",
                    Some(json!({ "sceneName": previous })),
                );
            }
        }
        if open.input_created {
            let _ = obs.request(
                "RemoveInput",
                Some(json!({ "inputName": config.input_name })),
            );
        }
        if open.scene_created {
            let _ = obs.request(
                "RemoveScene",
                Some(json!({ "sceneName": config.scene_name })),
            );
        }
    }

    /// `Some(detail)` when the control channel has gone or OBS has stopped the
    /// virtual camera, which the ADR turns into `ObsDisconnected`.
    fn control_channel_failure(&mut self) -> Option<String> {
        let open = self.open.as_mut()?;
        let obs = open.obs.as_mut()?;
        match obs.poll_events() {
            Ok(()) => obs.virtual_camera_stopped().map(str::to_owned),
            Err(error) => Some(error.to_string()),
        }
    }
}

impl Drop for MacOsVirtualCameraSink {
    fn drop(&mut self) {
        let _ = self.close_virtual_camera();
    }
}

impl FrameSink for MacOsVirtualCameraSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        self.open_virtual_camera(expected_frame_size, None, pixel_format)
            .map_err(|error| SinkError::Backend {
                detail: error.to_string(),
            })
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        let Some(expected_frame_size) = self.open.as_ref().map(|open| open.expected_frame_size)
        else {
            return Err(SinkError::NotOpen);
        };
        if frame.frame_size() != expected_frame_size {
            return Err(SinkError::FrameSizeMismatch {
                expected: expected_frame_size,
                actual: frame.frame_size(),
            });
        }

        if let Some(detail) = self.control_channel_failure() {
            let message = MacOsVirtualCameraError::ObsDisconnected { detail }.to_string();
            let _ = self.close_virtual_camera();
            return Err(SinkError::Backend { detail: message });
        }

        let pixel_format = self
            .open
            .as_ref()
            .map_or(PixelFormat::Rgb24, |open| open.pixel_format);
        let jpeg = match frame.source_jpeg() {
            // The camera's own bytes, untouched: the live view feed is never
            // re-encoded unless it was resized.
            Some(jpeg) => Cow::Borrowed(jpeg),
            None => Cow::Owned(encode_frame(frame, pixel_format).map_err(|error| {
                SinkError::Backend {
                    detail: MacOsVirtualCameraError::UnencodableFrame {
                        detail: error.to_string(),
                    }
                    .to_string(),
                }
            })?),
        };

        if self
            .open
            .as_ref()
            .is_some_and(|open| open.transport.consumer_page_connected())
        {
            self.consumer_page_seen = true;
        }
        if let Some(open) = self.open.as_ref() {
            let _ = open.transport.publish(&jpeg);
        }
        Ok(())
    }

    fn close(&mut self) -> Result<(), SinkError> {
        self.close_virtual_camera()
            .map_err(|error| SinkError::Backend {
                detail: error.to_string(),
            })
    }
}

fn browser_source_settings(
    transport: &FrameTransport,
    frame_size: FrameSize,
    measured_frame_rate: Option<f64>,
) -> Value {
    json!({
        // Without this, OBS reads `url` as a path on disk: a browser source is a
        // local file by default, so the page is never requested and no consumer
        // ever attaches. The source then renders blank and nothing says why.
        "is_local_file": false,
        "url": transport.page_url(),
        "width": frame_size.width(),
        "height": frame_size.height(),
        "fps_custom": true,
        "fps": measured_frame_rate.unwrap_or(FALLBACK_FRAME_RATE),
        "shutdown": false,
        "restart_when_active": false,
        "reroute_audio": false,
        "css": BROWSER_SOURCE_CSS,
    })
}

/// Encodes a frame that has no camera JPEG left — a resized frame — exactly once.
fn encode_frame(
    frame: &LiveViewFrame,
    pixel_format: PixelFormat,
) -> Result<Vec<u8>, jpeg_encoder::EncodingError> {
    debug_assert_eq!(pixel_format, PixelFormat::Rgb24);
    let frame_size = frame.frame_size();
    let mut jpeg = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut jpeg, RESIZED_FRAME_QUALITY);
    encoder.encode(
        frame.pixels(),
        u16::try_from(frame_size.width()).unwrap_or(u16::MAX),
        u16::try_from(frame_size.height()).unwrap_or(u16::MAX),
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(jpeg)
}
