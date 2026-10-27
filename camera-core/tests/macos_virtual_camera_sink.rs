//! The macOS **platform backend** driven against a fake obs-websocket v5 server.
//!
//! Nothing here needs a camera or OBS. What a fake server can prove is the
//! protocol: the handshake and challenge authentication, request/response
//! correlation, the exact input settings, the order of the open and the close,
//! that `StartVirtualCam` returning success is not treated as the virtual camera
//! running, the prerequisite errors, that a frame reaches the consumer page as
//! the camera's own JPEG, and that close is idempotent.
//!
//! What it cannot prove is anything about a real OBS: that a consumer
//! application lists the device, that the feed survives ten minutes, or that the
//! consumer reports the same frame size. Those are recorded on the ticket.

#![cfg(target_os = "macos")]

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

use camera_core::frame_size::FrameSize;
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
use camera_core::platform::macos::device_probe::{DeviceProbeFailure, VirtualCameraDeviceProbe};
use camera_core::platform::macos::prerequisites::ObsPrerequisites;
use camera_core::platform::macos::sink::{
    MacOsVirtualCameraConfig, MacOsVirtualCameraSink, VIRTUAL_CAMERA_INPUT_NAME,
};
use camera_core::sink::FrameSink;

const FULL: FrameSize = FrameSize::new(1024, 576);
const REDUCED: FrameSize = FrameSize::new(512, 288);

/// How long a page waits for a frame before the test fails rather than hangs.
const PAGE_READ_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the fake server gives a client to finish its HTTP upgrade. It must
/// comfortably exceed this machine's scheduling delays: a shorter one turned
/// connection setup into an intermittent `Handshake not finished`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the fake server waits for a request before re-checking `stop`.
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// The page's end of the frame transport, in the clear: the transport is a
/// loopback HTTP server, so there is no TLS to negotiate.
type PageSocket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn asset(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/live-view")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path:?}: {error}"))
}

fn full_jpeg() -> Vec<u8> {
    asset("frame-1024x576.jpg")
}

fn frame(frame_size: FrameSize) -> LiveViewFrame {
    LiveViewFrame::from_jpeg(&asset(&format!("frame-{frame_size}.jpg")))
        .expect("the fixture is a real JPEG")
}

/// A directory holding a stand-in `OBS.app` and an OBS log naming the virtual
/// camera. Both are what the prerequisite checks read.
struct FakeInstall {
    root: PathBuf,
}

impl FakeInstall {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "canon-webcam-sink-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("OBS.app")).expect("a stand-in application bundle");
        std::fs::create_dir_all(root.join("logs")).expect("a log directory");
        std::fs::write(
            root.join("logs/2026-10-27 10-00-00.txt"),
            "10:00:00.000: [mac-virtualcam] mac-camera-extension: OBS cannot install the \
             virtual camera if it's not in \"/Applications\".\n",
        )
        .expect("an OBS log");
        Self { root }
    }

    fn obs_app(&self) -> PathBuf {
        self.root.join("OBS.app")
    }

    fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
}

impl Drop for FakeInstall {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------
// A fake obs-websocket v5 server
// ---------------------------------------------------------------------------

struct FakeObsConfig {
    password: Option<String>,
    /// Whether OBS's virtual camera really comes up when `StartVirtualCam` is
    /// accepted. `false` is the trap ADR-0002 measured.
    virtual_camera_starts: bool,
    /// How many status answers report the output as down before OBS admits it is
    /// up, which is what starting the output on another thread looks like.
    status_answers_down_first: usize,
    /// Whether `StartVirtualCam` is refused, as it is while the previous output is
    /// still stopping.
    refuse_start_virtual_cam: bool,
}

impl Default for FakeObsConfig {
    fn default() -> Self {
        Self {
            password: Some("canon-webcam".to_owned()),
            virtual_camera_starts: true,
            status_answers_down_first: 0,
            refuse_start_virtual_cam: false,
        }
    }
}

struct FakeObs {
    endpoint: SocketAddr,
    password: Option<String>,
    log: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<AtomicBool>,
    /// Connections accepted so far; instrumentation for diagnosing a client that
    /// reconnects when it should have reused its socket.
    #[allow(dead_code)]
    serviced: Arc<AtomicU64>,
    handles: Vec<JoinHandle<()>>,
}

impl FakeObs {
    fn start(config: FakeObsConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let endpoint = listener.local_addr().expect("a bound address");
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let serviced = Arc::new(AtomicU64::new(0));

        let mut fake = Self {
            endpoint,
            password: config.password.clone(),
            log: Arc::clone(&log),
            stop: Arc::clone(&stop),
            serviced: Arc::clone(&serviced),
            handles: Vec::new(),
        };
        fake.handles
            .push(spawn_accept_loop(listener, config, log, stop, serviced));
        fake
    }

    fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    fn password(&self) -> Option<String> {
        self.password.clone()
    }

    fn request_types(&self) -> Vec<String> {
        self.log
            .lock()
            .expect("the request log")
            .iter()
            .map(|(request_type, _)| request_type.clone())
            .collect()
    }

    fn request_data(&self, request_type: &str) -> Option<Value> {
        self.log
            .lock()
            .expect("the request log")
            .iter()
            .find(|(name, _)| name == request_type)
            .map(|(_, data)| data.clone())
    }

    /// The data of the most recent request of this type. `SetCurrentProgramScene`
    /// is sent twice by design — ours on open, the consumer's back on close — so
    /// the restore assertion must read the later one.
    fn last_request_data(&self, request_type: &str) -> Option<Value> {
        self.log
            .lock()
            .expect("the request log")
            .iter()
            .rev()
            .find(|(name, _)| name == request_type)
            .map(|(_, data)| data.clone())
    }

    fn has_request(&self, request_type: &str) -> bool {
        self.request_types().iter().any(|name| name == request_type)
    }

    /// Stops answering and drops every connection, which is what OBS quitting
    /// looks like to the sink.
    fn quit(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for FakeObs {
    fn drop(&mut self) {
        self.quit();
    }
}

fn spawn_accept_loop(
    listener: TcpListener,
    config: FakeObsConfig,
    log: Arc<Mutex<Vec<(String, Value)>>>,
    stop: Arc<AtomicBool>,
    serviced: Arc<AtomicU64>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut connections = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Served concurrently on purpose: a sink that re-opens must
                    // not queue behind the connection its previous open left
                    // behind, or the re-open sees a handshake that never ends.
                    connections.push(spawn_connection(stream, &config, &log, &stop));
                    serviced.fetch_add(1, Ordering::SeqCst);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break,
            }
        }
        // Every connection polls `stop`, so they all end once it is set.
        for handle in connections {
            let _ = handle.join();
        }
    })
}

// tungstenite's handshake callback fixes this signature, so the oversized
// `Err` variant it hands back is not ours to reshape.
#[allow(clippy::result_large_err)]
fn spawn_connection(
    stream: TcpStream,
    config: &FakeObsConfig,
    log: &Arc<Mutex<Vec<(String, Value)>>>,
    stop: &Arc<AtomicBool>,
) -> JoinHandle<()> {
    let config_password = config.password.clone();
    let virtual_camera_starts = config.virtual_camera_starts;
    let status_answers_down_first = config.status_answers_down_first;
    let start_is_refused = config.refuse_start_virtual_cam;
    let log = Arc::clone(log);
    let stop = Arc::clone(stop);

    thread::spawn(move || {
        // macOS gives an accepted socket its listener's `O_NONBLOCK`, and the
        // handshake below is a blocking read: without this, a request that has
        // not arrived yet is reported as a closed connection.
        if stream.set_nonblocking(false).is_err() {
            return;
        }
        let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let handshake = tungstenite::accept_hdr(
            stream,
            |request: &tungstenite::handshake::server::Request,
             mut response: tungstenite::handshake::server::Response| {
                // A real obs-websocket server echoes the subprotocol it accepted,
                // and a client that asked for one refuses a server that does not.
                let offered = request.headers().iter().any(|(name, value)| {
                    name.as_str().eq_ignore_ascii_case("sec-websocket-protocol")
                        && value
                            .to_str()
                            .is_ok_and(|value| value.contains("obswebsocket.json"))
                });
                if offered {
                    response.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        http_header_value("obswebsocket.json"),
                    );
                }
                Ok(response)
            },
        );
        let Ok(mut socket) = handshake else {
            return;
        };
        // Only now is the timeout short: after the upgrade it is the poll that
        // lets the accept loop shut down promptly when OBS quits.
        let _ = socket.get_ref().set_read_timeout(Some(POLL_TIMEOUT));

        let salt = "salt-value";
        let challenge = "challenge-value";
        let hello = json!({
            "op": 0,
            "d": {
                "obsWebSocketVersion": "5.7.4",
                "rpcVersion": 1,
                "authentication": { "challenge": challenge, "salt": salt },
            }
        });
        if socket.send(Message::text(hello.to_string())).is_err() {
            return;
        }

        let Some(identify) = next_message(&mut socket, &stop) else {
            return;
        };
        if identify["op"] != 1 {
            return;
        }
        if let Some(password) = config_password {
            let expected = expected_authentication(&password, salt, challenge);
            let offered = identify["d"]["authentication"].as_str().unwrap_or_default();
            if offered != expected {
                let _ = socket.close(Some(CloseFrame {
                    code: CloseCode::from(4009u16),
                    reason: "Authentication failed.".into(),
                }));
                return;
            }
        }

        let identified = json!({ "op": 2, "d": { "negotiatedRpcVersion": 1 } });
        if socket.send(Message::text(identified.to_string())).is_err() {
            return;
        }

        // A start that OBS refuses because the output is already running, which is
        // what a run over the same command finds while the last one winds down.
        let mut output_active = start_is_refused;
        let mut status_answers_down = status_answers_down_first;
        // OBS keeps scenes and inputs in one namespace: one name cannot be two
        // sources, and a scene cannot hold an input of its own name. The fake
        // models that, because a fake that let a scene and an input share a name
        // hid a refusal that real OBS makes on every open.
        let mut scenes: Vec<String> = vec!["previous scene".to_owned()];
        let mut program_scene = "previous scene".to_owned();
        let mut inputs: Vec<String> = Vec::new();
        let mut scene_items: Vec<(String, String)> = Vec::new();
        while let Some(value) = next_message(&mut socket, &stop) {
            if value["op"] != 6 {
                continue;
            }
            let request_type = value["d"]["requestType"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let request_id = value["d"]["requestId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let request_data = value["d"]
                .get("requestData")
                .cloned()
                .unwrap_or(Value::Null);
            // Obs-websocket refuses a CreateInput that names no scene, and the
            // fake refuses it too: a fake that accepts any well-formed request hid
            // exactly this defect once, because the payload was logged and never
            // checked against the protocol's required fields.
            let input_names_a_scene = names_a_scene(&request_data);
            let asked_for_scene = request_data["sceneName"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let asked_for_source = request_data["sourceName"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let named = request_data["inputName"]
                .as_str()
                .or_else(|| request_data["sceneName"].as_str())
                .unwrap_or_default()
                .to_owned();
            log.lock()
                .expect("the request log")
                .push((request_type.clone(), request_data));

            let (result, response) = match request_type.as_str() {
                "CreateInput" if !input_names_a_scene => (false, json!({})),
                "CreateInput" => {
                    if scenes.contains(&named) || inputs.contains(&named) {
                        (false, json!({}))
                    } else {
                        inputs.push(named.clone());
                        scene_items.push((asked_for_scene.clone(), named.clone()));
                        (true, json!({}))
                    }
                }
                "CreateScene" => {
                    if scenes.contains(&named) || inputs.contains(&named) {
                        (false, json!({}))
                    } else {
                        scenes.push(named.clone());
                        (true, json!({}))
                    }
                }
                "CreateSceneItem" => {
                    if scenes.contains(&asked_for_scene) && inputs.contains(&asked_for_source) {
                        scene_items.push((asked_for_scene.clone(), asked_for_source.clone()));
                        (true, json!({}))
                    } else {
                        (false, json!({}))
                    }
                }
                "SetCurrentProgramScene" => {
                    program_scene = asked_for_scene.clone();
                    (true, json!({}))
                }
                "RemoveInput" => {
                    inputs.retain(|name| *name != named);
                    scene_items.retain(|(_, source)| *source != named);
                    (true, json!({}))
                }
                "RemoveScene" => {
                    scenes.retain(|name| *name != named);
                    (true, json!({}))
                }
                "GetVersion" => (
                    true,
                    json!({
                        "obsVersion": "32.2.1",
                        "obsWebSocketVersion": "5.7.4",
                        "rpcVersion": 1,
                    }),
                ),
                "GetSceneList" => (
                    true,
                    json!({
                        "currentProgramSceneName": program_scene,
                        "scenes": scenes
                            .iter()
                            .enumerate()
                            .map(|(index, name)| json!({ "sceneName": name, "sceneIndex": index }))
                            .collect::<Vec<_>>(),
                    }),
                ),
                "GetInputList" => (
                    true,
                    json!({
                        "inputs": inputs
                            .iter()
                            .map(|name| json!({ "inputName": name }))
                            .collect::<Vec<_>>(),
                    }),
                ),
                "GetSceneItemList" => (
                    true,
                    json!({
                        "sceneItems": scene_items
                            .iter()
                            .filter(|(scene, _)| *scene == asked_for_scene)
                            .map(|(_, source)| json!({ "sourceName": source }))
                            .collect::<Vec<_>>(),
                    }),
                ),
                "StartVirtualCam" => {
                    if start_is_refused {
                        (false, json!({}))
                    } else {
                        output_active = virtual_camera_starts;
                        (true, json!({}))
                    }
                }
                "StopVirtualCam" => {
                    output_active = false;
                    (true, json!({}))
                }
                "GetVirtualCamStatus" => {
                    // The output comes up on OBS's own thread: the first answers
                    // can still report it down while it is starting.
                    if status_answers_down > 0 {
                        status_answers_down -= 1;
                        (true, json!({ "outputActive": false }))
                    } else {
                        (true, json!({ "outputActive": output_active }))
                    }
                }
                _ => (true, json!({})),
            };

            let reply = json!({
                "op": 7,
                "d": {
                    "requestType": request_type,
                    "requestId": request_id,
                    "requestStatus": {
                        "result": result,
                        "code": if result { 100 } else { 300 },
                        "comment": if result {
                            ""
                        } else {
                            "Your request must contain at least one of the following fields: \
                             `sceneName` with optional `canvasUuid` or `sceneUuid`."
                        },
                    },
                    "responseData": response,
                }
            });
            if socket.send(Message::text(reply.to_string())).is_err() {
                return;
            }
        }
        let _ = socket.close(None);
    })
}

/// Whether a request gives OBS a scene to place something in, which is what
/// obs-websocket requires of `CreateInput` and `CreateSceneItem`.
fn names_a_scene(request_data: &Value) -> bool {
    request_data.get("sceneName").is_some()
        || request_data.get("sceneUuid").is_some()
        || request_data.get("canvasUuid").is_some()
}

fn next_message(
    socket: &mut tungstenite::WebSocket<TcpStream>,
    stop: &AtomicBool,
) -> Option<Value> {
    loop {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        match socket.read() {
            Ok(Message::Text(text)) => return serde_json::from_str(text.as_str()).ok(),
            Ok(Message::Close(_)) => return None,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(_) => return None,
        }
    }
}

fn http_header_value(value: &str) -> tungstenite::http::HeaderValue {
    tungstenite::http::HeaderValue::from_str(value).expect("a header value")
}

/// Computed here rather than borrowed from the crate, so the test is an
/// independent witness of obs-websocket's challenge authentication.
fn expected_authentication(password: &str, salt: &str, challenge: &str) -> String {
    let secret = BASE64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    BASE64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

// ---------------------------------------------------------------------------
// A scripted device probe and sink construction
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ScriptedDeviceProbe {
    present: AtomicBool,
}

impl ScriptedDeviceProbe {
    fn new(present: bool) -> Arc<Self> {
        Arc::new(Self {
            present: AtomicBool::new(present),
        })
    }
}

impl VirtualCameraDeviceProbe for ScriptedDeviceProbe {
    fn virtual_camera_device_present(&self) -> Result<bool, DeviceProbeFailure> {
        Ok(self.present.load(Ordering::SeqCst))
    }
}

fn config(
    obs: &FakeObs,
    install: &FakeInstall,
    password: Option<String>,
) -> MacOsVirtualCameraConfig {
    MacOsVirtualCameraConfig {
        prerequisites: ObsPrerequisites {
            obs_app_path: install.obs_app(),
            websocket_endpoint: obs.endpoint(),
            websocket_password: password,
            obs_log_directory: install.logs(),
            launch_obs: false,
            websocket_timeout: Duration::from_secs(2),
            launch_deadline: Duration::from_millis(300),
        },
        scene_name: "canon-webcam".to_owned(),
        // Deliberately the library's own default rather than a literal: a default
        // that collides with the scene name is refused by OBS, and this test
        // configuration must not hide that behind a different name.
        input_name: VIRTUAL_CAMERA_INPUT_NAME.to_owned(),
        // Long enough for the fake to answer a few times, short enough that a
        // test which proves a persistent failure still runs quickly.
        virtual_camera_start_timeout: Duration::from_millis(400),
    }
}

fn open(sink: &mut MacOsVirtualCameraSink, frame_size: FrameSize) -> Result<(), String> {
    sink.open_virtual_camera(frame_size, Some(30.0), PixelFormat::Rgb24)
        .map_err(|error| error.to_string())
}

/// Waits until the transport has a consumer page attached, so a test does not
/// race the connection thread.
fn wait_for_consumer_page(sink: &MacOsVirtualCameraSink) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if sink.status().consumer_page_connected {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Attaches a page to the sink's frame transport.
///
/// `tungstenite::connect` performs the HTTP upgrade itself, so no handshake
/// message is left to read: a read here would wait for a frame that has not
/// been published yet. The socket also gets a read timeout, so a frame that
/// never arrives fails the test instead of blocking it forever.
fn attach_page(url: &str) -> PageSocket {
    let (page, _) = tungstenite::connect(url).expect("the page can attach");
    match page.get_ref() {
        MaybeTlsStream::Plain(stream) => stream
            .set_read_timeout(Some(PAGE_READ_TIMEOUT))
            .expect("a read timeout on the page socket"),
        _ => panic!("the page connects in the clear"),
    }
    page
}

/// Reads the page's next binary frame, failing rather than blocking forever.
fn read_binary_frame(page: &mut PageSocket) -> Vec<u8> {
    loop {
        match page.read() {
            Ok(Message::Binary(bytes)) => return bytes.into(),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                panic!("no frame reached the page within {PAGE_READ_TIMEOUT:?}")
            }
            Err(error) => panic!("the page read failed: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The open sequence
// ---------------------------------------------------------------------------

#[test]
fn opening_authenticates_and_drives_the_scene_input_and_virtual_camera_in_adr_order() {
    let install = FakeInstall::new("open");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    open(&mut sink, FULL).expect("the open must succeed against a fake OBS");

    let types = obs.request_types();
    let position = |name: &str| {
        types
            .iter()
            .position(|candidate| candidate == name)
            .unwrap_or_else(|| panic!("{name} was never requested; the order was {types:?}"))
    };
    assert!(
        position("CreateInput") < position("SetCurrentProgramScene"),
        "the input is created before the scene that holds it: {types:?}"
    );
    assert!(
        position("SetCurrentProgramScene") < position("StartVirtualCam"),
        "the scene is program before the virtual camera starts: {types:?}"
    );
    assert!(
        position("StartVirtualCam") < position("GetVirtualCamStatus"),
        "StartVirtualCam is never evidence on its own: {types:?}"
    );

    let settings = obs.request_data("CreateInput").expect("the input settings");
    assert_eq!(settings["inputName"], VIRTUAL_CAMERA_INPUT_NAME);
    assert_eq!(
        settings["sceneName"], "canon-webcam",
        "obs-websocket refuses a CreateInput that names no scene"
    );
    assert_ne!(
        settings["inputName"], settings["sceneName"],
        "OBS keeps scenes and inputs in one namespace, so the input cannot carry \
         the scene's name"
    );
    assert_eq!(settings["inputKind"], "browser_source");
    let input = &settings["inputSettings"];
    assert_eq!(
        input["is_local_file"], false,
        "a browser source is a local file unless it is told otherwise, and OBS then \
         reads the url as a path on disk and loads nothing"
    );
    let url = input["url"].as_str().expect("a url");
    assert!(
        url.starts_with("http://127.0.0.1:") && url.ends_with('/'),
        "the browser source reads the local transport page, not {url}"
    );
    assert_eq!(input["width"], 1024);
    assert_eq!(input["height"], 576);
    assert_eq!(input["fps_custom"], true);
    assert_eq!(input["fps"], 30.0);
    assert_eq!(input["shutdown"], false);
    assert_eq!(input["restart_when_active"], false);
    assert_eq!(input["reroute_audio"], false);
    assert_eq!(
        input["css"],
        "body { background-color: rgba(0, 0, 0, 0); margin: 0px auto; overflow: hidden; }"
    );

    assert_eq!(sink.status().expected_frame_size, Some(FULL));
    assert!(sink.status().virtual_camera_active);
}

#[test]
fn an_accepted_start_virtual_cam_that_leaves_the_camera_down_is_a_failure() {
    let install = FakeInstall::new("not-started");
    let obs = FakeObs::start(FakeObsConfig {
        virtual_camera_starts: false,
        ..FakeObsConfig::default()
    });
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let error = open(&mut sink, FULL).expect_err("a down virtual camera is not a success");

    assert!(
        error.contains("OBS accepted StartVirtualCam"),
        "the failure must name the trap; message was {error}"
    );
    assert!(
        error.contains("cannot install the virtual camera"),
        "the failure carries OBS's own explanation from its log; message was {error}"
    );
    assert!(
        obs.has_request("StopVirtualCam") && obs.has_request("RemoveInput"),
        "opening must undo its own mutations; the requests were {:?}",
        obs.request_types()
    );
    assert!(!sink.status().is_open);
}

#[test]
fn a_status_that_reads_down_while_obs_starts_the_output_is_not_a_failure() {
    let install = FakeInstall::new("starting");
    let obs = FakeObs::start(FakeObsConfig {
        // OBS starts the output on a thread of its own, so the first status reads
        // still say down. Reading the status once failed the open here, and the
        // stop that followed made the next run over the same command fail
        // differently — the alternating pair a user sees as a broken mechanism.
        status_answers_down_first: 3,
        ..FakeObsConfig::default()
    });
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    open(&mut sink, FULL).expect("a status read while OBS is starting is not a failure");

    assert!(sink.status().is_open, "the sink must be open");
    assert!(sink.status().virtual_camera_active);
    let starts = obs
        .request_types()
        .iter()
        .filter(|request| request.as_str() == "StartVirtualCam")
        .count();
    assert_eq!(
        starts, 1,
        "the start is asked for once and the status is re-read; asking again restarts \
         the output on every attempt, which a broken camera extension turns into macOS alerts"
    );
}

#[test]
fn a_start_refused_because_the_output_already_runs_is_adopted() {
    let install = FakeInstall::new("already-running");
    let obs = FakeObs::start(FakeObsConfig {
        // A previous run that was killed leaves the output running, and OBS then
        // refuses StartVirtualCam. Failing here would mean a run can never be
        // restarted without quitting OBS.
        refuse_start_virtual_cam: true,
        ..FakeObsConfig::default()
    });
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    open(&mut sink, FULL).expect("a virtual camera that already runs is adopted, not fought");

    assert!(sink.status().virtual_camera_active);

    sink.close_virtual_camera().expect("close must succeed");

    assert!(
        obs.has_request("StopVirtualCam"),
        "an adopted output is ours to stop, or the next run is blocked by it"
    );
}

#[test]
fn a_wrong_password_fails_with_the_websocket_setting_that_holds_it() {
    let install = FakeInstall::new("password");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, Some("not-the-password".to_owned())),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let error = open(&mut sink, FULL).expect_err("the password is wrong");

    assert!(
        error.contains("Tools → WebSocket Server Settings"),
        "the message must name the OBS dialog; message was {error}"
    );
    assert!(
        error.contains("websocket_password"),
        "the message must name our own configuration key; message was {error}"
    );
}

// ---------------------------------------------------------------------------
// Prerequisites
// ---------------------------------------------------------------------------

#[test]
fn an_obs_outside_applications_names_the_directory_obs_insists_on() {
    let install = FakeInstall::new("no-app");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut config = config(&obs, &install, obs.password());
    config.prerequisites.obs_app_path = install.root.join("Somewhere/OBS.app");
    let mut sink = MacOsVirtualCameraSink::new(
        config,
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let error = open(&mut sink, FULL).expect_err("OBS is not in /Applications");

    assert!(
        error.contains("/Applications"),
        "the message must name the directory; message was {error}"
    );
    assert!(
        error.contains("obsproject.com"),
        "the message must name where OBS comes from; message was {error}"
    );
    assert!(
        !obs.has_request("StartVirtualCam"),
        "nothing may be started before a prerequisite passes"
    );
}

#[test]
fn an_absent_virtual_camera_device_lists_the_administrator_approval_steps() {
    let install = FakeInstall::new("no-device");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(false);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let error = open(&mut sink, FULL).expect_err("no device is registered");

    for expected in [
        "OBS Virtual Camera",
        "Start Virtual Camera",
        "Login Items & Extensions",
        "Camera Extensions",
        "administrator",
    ] {
        assert!(
            error.contains(expected),
            "the message must contain {expected:?}; message was {error}"
        );
    }
    assert!(
        !obs.has_request("StartVirtualCam"),
        "the approval is a human's one-time action, never ours"
    );
}

#[test]
fn a_closed_websocket_port_names_the_setting_that_opens_it() {
    let install = FakeInstall::new("no-websocket");
    // A bound-then-dropped port: nothing listens there.
    let dead = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let dead_endpoint = dead.local_addr().expect("an address");
    drop(dead);
    let probe = ScriptedDeviceProbe::new(true);
    let mut config = config(&FakeObs::start(FakeObsConfig::default()), &install, None);
    config.prerequisites.websocket_endpoint = dead_endpoint;
    let mut sink = MacOsVirtualCameraSink::new(
        config,
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let error = open(&mut sink, FULL).expect_err("nothing is listening");

    assert!(
        error.contains("Enable WebSocket server") && error.contains("4455"),
        "the message must name the setting and the port; message was {error}"
    );
}

#[test]
fn launching_obs_is_never_attempted_when_the_configuration_forbids_it() {
    // `launch_obs: false` is what every test here sets; the guard is that the
    // closed port above produced the websocket error immediately rather than
    // waiting out the launch deadline.
    let install = FakeInstall::new("no-launch");
    let dead = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let dead_endpoint = dead.local_addr().expect("an address");
    drop(dead);
    let probe = ScriptedDeviceProbe::new(true);
    let mut config = config(&FakeObs::start(FakeObsConfig::default()), &install, None);
    config.prerequisites.websocket_endpoint = dead_endpoint;
    let mut sink = MacOsVirtualCameraSink::new(
        config,
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let started = Instant::now();
    let _ = open(&mut sink, FULL);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "with launching forbidden the failure must be immediate, took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Frame delivery
// ---------------------------------------------------------------------------

#[test]
fn a_frame_reaches_the_consumer_page_as_the_jpeg_the_camera_delivered() {
    let install = FakeInstall::new("frames");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the open must succeed");

    let url = sink
        .frames_websocket_url()
        .expect("an open sink has a transport");
    let mut page = attach_page(url.as_str());

    assert!(
        wait_for_consumer_page(&sink),
        "the transport must notice the page"
    );

    let jpeg = full_jpeg();
    sink.accept(&frame(FULL))
        .expect("a frame at the expected size");

    let received = read_binary_frame(&mut page);
    assert_eq!(
        received.as_slice(),
        jpeg.as_slice(),
        "the page must receive the camera's JPEG unchanged"
    );
    assert_eq!(sink.status().frames_published, 1);
}

#[test]
fn the_sink_counters_outlive_the_close_so_a_run_report_can_print_them() {
    // A run report is printed after the run, and the run has already closed and
    // dropped its sink. What was published must therefore still be reportable.
    let install = FakeInstall::new("counters");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the open must succeed");

    let url = sink
        .frames_websocket_url()
        .expect("an open sink has a transport");
    let mut page = attach_page(url.as_str());
    assert!(
        wait_for_consumer_page(&sink),
        "the transport must notice the page"
    );

    for _ in 0..2 {
        sink.accept(&frame(FULL))
            .expect("a frame at the expected size");
        let received = read_binary_frame(&mut page);
        assert_eq!(received.as_slice(), full_jpeg().as_slice());
    }

    sink.close_virtual_camera().expect("the close must succeed");

    let status = sink.status();
    assert!(!status.is_open, "a closed sink says it is closed");
    assert_eq!(
        status.frames_published, 2,
        "the report is printed after the close and must still know what was published"
    );
    assert!(
        status.consumer_page_seen,
        "and that a consumer page attached while the sink was open"
    );
}

#[test]
fn a_frame_of_another_size_is_refused_rather_than_reinterpreted() {
    let install = FakeInstall::new("size-mismatch");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the open must succeed");

    let error = sink
        .accept(&frame(REDUCED))
        .expect_err("a sink is never resized while it is open");

    assert!(
        error.to_string().contains("closed and re-opened"),
        "message was {error}"
    );
    assert_eq!(
        sink.status().expected_frame_size,
        Some(FULL),
        "the open sink keeps the size it was opened at"
    );
}

#[test]
fn a_frame_without_a_camera_jpeg_is_encoded_once_at_its_own_frame_size() {
    let install = FakeInstall::new("resized");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, REDUCED).expect("the open must succeed");

    let url = sink
        .frames_websocket_url()
        .expect("an open sink has a transport");
    let mut page = attach_page(url.as_str());
    assert!(wait_for_consumer_page(&sink));

    let resized = frame(FULL).resize_to(REDUCED);
    sink.accept(&resized).expect("a resized frame is delivered");

    let received = read_binary_frame(&mut page);
    let decoded = LiveViewFrame::from_jpeg(&received).expect("the transport sends decodable JPEG");
    assert_eq!(decoded.frame_size(), REDUCED);
}

// ---------------------------------------------------------------------------
// Close, teardown and re-open
// ---------------------------------------------------------------------------

#[test]
fn closing_stops_the_virtual_camera_removes_what_we_made_and_is_idempotent() {
    let install = FakeInstall::new("close");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the open must succeed");

    sink.close_virtual_camera().expect("close must succeed");
    assert!(
        sink.close_virtual_camera().is_ok(),
        "close must succeed when already closed"
    );

    let types = obs.request_types();
    for expected in [
        "StopVirtualCam",
        "SetCurrentProgramScene",
        "RemoveInput",
        "RemoveScene",
    ] {
        assert!(
            types.iter().any(|name| name == expected),
            "{expected} was not requested; the requests were {types:?}"
        );
    }
    let restored = obs
        .last_request_data("SetCurrentProgramScene")
        .expect("a program scene restore");
    assert_eq!(
        restored["sceneName"], "previous scene",
        "the consumer's own program scene is restored"
    );
    assert!(!sink.status().is_open);
}

#[test]
fn closing_succeeds_when_obs_has_already_exited() {
    let install = FakeInstall::new("obs-gone");
    let mut obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the open must succeed");

    obs.quit();

    assert!(
        sink.close_virtual_camera().is_ok(),
        "a sink whose OBS has exited is already closed"
    );
    assert!(!sink.status().is_open);
}

#[test]
fn a_lost_connection_while_delivering_ends_the_sink_with_obs_disconnected() {
    let install = FakeInstall::new("disconnected");
    let mut obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the open must succeed");
    obs.quit();

    let error = sink
        .accept(&frame(FULL))
        .expect_err("a dead control channel is not a delivered frame");

    let message = error.to_string();
    assert!(
        message.contains("OBS") && message.contains("ended"),
        "the message must say the run ended; message was {message}"
    );
    assert!(
        !sink.status().is_open,
        "the sink closes itself on this path"
    );
}

#[test]
fn a_sink_re_open_delivers_the_new_frame_size_and_nothing_else() {
    let install = FakeInstall::new("reopen");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );
    open(&mut sink, FULL).expect("the first open must succeed");
    sink.close_virtual_camera().expect("close must succeed");
    open(&mut sink, REDUCED).expect("re-opening at the new size must succeed");

    sink.accept(&frame(REDUCED))
        .expect("the new size is delivered");
    assert!(sink.accept(&frame(FULL)).is_err());
    assert_eq!(sink.status().expected_frame_size, Some(REDUCED));
    assert_eq!(
        sink.status().sink_opens,
        2,
        "a size change is a sink re-open and nothing else"
    );
}

#[test]
fn a_sink_that_was_never_opened_refuses_a_frame() {
    let install = FakeInstall::new("not-open");
    let obs = FakeObs::start(FakeObsConfig::default());
    let probe = ScriptedDeviceProbe::new(true);
    let mut sink = MacOsVirtualCameraSink::new(
        config(&obs, &install, obs.password()),
        Arc::clone(&probe) as Arc<dyn VirtualCameraDeviceProbe>,
    );

    let error = sink.accept(&frame(FULL)).expect_err("nothing is open");

    assert_eq!(error, camera_core::sink::SinkError::NotOpen);
    assert!(sink.close_virtual_camera().is_ok());
}

#[test]
fn the_real_avfoundation_probe_answers_without_a_camera_attached() {
    use camera_core::platform::macos::device_probe::AvFoundationDeviceProbe;

    let present = AvFoundationDeviceProbe::new()
        .virtual_camera_device_present()
        .expect("the AVFoundation discovery session must answer");

    // This machine has no approved OBS camera extension, so the answer is
    // expected to be `false`; what is asserted is that the check runs at all.
    let _ = present;
}
