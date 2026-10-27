//! A throwaway probe: what does OBS think of the browser source we create?
//!
//! `sink pages served: 0` says the page is never requested, which is above the
//! frame transport. This asks OBS itself: is the source active, is its scene item
//! enabled, and what settings did it actually keep?
//!
//! Usage: `cargo run --example obs_probe -- <password> <input-name>`

use std::net::TcpStream;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tungstenite::{connect, Message};

fn authentication(password: &str, salt: &str, challenge: &str) -> String {
    let secret = BASE64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    BASE64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

struct Client {
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Client {
    fn connect(password: &str) -> Self {
        let (mut socket, _) = connect("ws://127.0.0.1:4455").expect("the OBS WebSocket");
        let hello: Value =
            serde_json::from_str(socket.read().expect("hello").to_text().unwrap()).expect("json");
        let data = &hello["d"]["authentication"];
        let identify = json!({
            "op": 1,
            "d": {
                "rpcVersion": 1,
                "authentication": authentication(
                    password,
                    data["salt"].as_str().unwrap_or_default(),
                    data["challenge"].as_str().unwrap_or_default(),
                ),
            }
        });
        socket
            .send(Message::text(identify.to_string()))
            .expect("identify");
        loop {
            let value: Value =
                serde_json::from_str(socket.read().expect("identified").to_text().unwrap())
                    .expect("json");
            if value["op"] == 2 {
                break;
            }
            if value["op"] == 9 {
                panic!("OBS refused: {value}");
            }
        }
        Self { socket, next_id: 0 }
    }

    fn request(&mut self, request_type: &str, data: Option<Value>) -> Value {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let mut payload = json!({ "requestType": request_type, "requestId": id });
        if let Some(data) = data {
            payload["requestData"] = data;
        }
        self.socket
            .send(Message::text(json!({ "op": 6, "d": payload }).to_string()))
            .expect("send");
        loop {
            let value: Value =
                serde_json::from_str(self.socket.read().expect("reply").to_text().unwrap())
                    .expect("json");
            if value["op"] != 7 || value["d"]["requestId"].as_str() != Some(id.as_str()) {
                continue;
            }
            if value["d"]["requestStatus"]["result"].as_bool() != Some(true) {
                println!(
                    "  {request_type}: refused ({})",
                    value["d"]["requestStatus"]
                );
                return Value::Null;
            }
            return value["d"]["responseData"].clone();
        }
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let password = arguments.next().expect("a password argument");
    let input = arguments.next().expect("an input name argument");

    let mut client = Client::connect(&password);

    let scenes = client.request("GetSceneList", None);
    println!(
        "current program scene: {}",
        scenes["currentProgramSceneName"]
    );
    println!("scenes: {}", scenes["scenes"]);

    let settings = client.request("GetInputSettings", Some(json!({ "inputName": input })));
    println!("input settings: {}", settings["inputSettings"]);

    let active = client.request("GetSourceActive", Some(json!({ "sourceName": input })));
    println!("source active: {active}");

    let scene = scenes["currentProgramSceneName"]
        .as_str()
        .unwrap_or_default();
    let items = client.request("GetSceneItemList", Some(json!({ "sceneName": scene })));
    println!("scene items: {}", items["sceneItems"]);
}
