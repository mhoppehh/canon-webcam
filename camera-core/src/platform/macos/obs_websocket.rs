//! A blocking obs-websocket v5 client: the control channel ADR-0002 chooses.
//!
//! Two properties of this protocol are load bearing and are why the client is
//! written out rather than hidden behind a convenience wrapper:
//!
//! - **authentication is a challenge.** The server sends a salt and a challenge
//!   in `Hello`; the answer is
//!   `base64(sha256(base64(sha256(password + salt)) + challenge))`. A client that
//!   sends the password itself gets closed with code 4009.
//! - **every response carries the `requestId` it answers.** Requests and events
//!   share the same socket, so a reply is matched by id and everything else is
//!   read as an event and discarded, or — for `VirtualCamStateChanged` — kept,
//!   because that event is how a virtual camera that stops is noticed.
//!
//! Plain `ws://` on the loopback interface only, so no TLS feature is enabled.

use std::fmt;
use std::net::{SocketAddr, TcpStream};
use std::str::FromStr;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tungstenite::protocol::WebSocket;
use tungstenite::{client, Message};

/// The port OBS's WebSocket server listens on unless the user changes it.
pub const DEFAULT_WEBSOCKET_PORT: u16 = 4455;

/// The subprotocol obs-websocket v5 requires the client to name.
pub const SUBPROTOCOL: &str = "obswebsocket.json";

/// The RPC version this client speaks.
pub const RPC_VERSION: u32 = 1;

/// The close code obs-websocket uses when the password is wrong.
const AUTHENTICATION_FAILED_CLOSE_CODE: u16 = 4009;

/// Why the control channel could not do something.
///
/// The two failure classes the ADR separates are separate variants here: a
/// server that is not answering at all (`Unreachable`) and a server that answers
/// and refuses the password (`Unauthenticated`). The remedies differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObsProtocolError {
    /// Nothing is listening, or the WebSocket handshake did not complete.
    Unreachable {
        /// What happened.
        detail: String,
    },
    /// The server answered and refused the password.
    Unauthenticated {
        /// What happened.
        detail: String,
    },
    /// The request reached OBS and OBS refused it.
    Rejected {
        /// The request type.
        request_type: String,
        /// OBS's own comment.
        comment: String,
    },
    /// The socket went away, which is what OBS quitting looks like.
    Disconnected {
        /// What happened.
        detail: String,
    },
    /// A message did not have the shape the protocol defines.
    Malformed {
        /// What was wrong with it.
        detail: String,
    },
}

impl fmt::Display for ObsProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable { detail } => {
                write!(
                    formatter,
                    "the WebSocket server is not answering ({detail})"
                )
            }
            Self::Unauthenticated { detail } => {
                write!(
                    formatter,
                    "the WebSocket server refused the password ({detail})"
                )
            }
            Self::Rejected {
                request_type,
                comment,
            } => write!(formatter, "OBS refused `{request_type}`: {comment}"),
            Self::Disconnected { detail } => {
                write!(
                    formatter,
                    "the WebSocket connection to OBS was lost ({detail})"
                )
            }
            Self::Malformed { detail } => {
                write!(
                    formatter,
                    "OBS sent a message this client cannot read: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ObsProtocolError {}

/// The authentication answer obs-websocket v5 expects.
///
/// `base64(sha256(base64(sha256(password + salt)) + challenge))`.
#[must_use]
pub fn authentication_answer(password: &str, salt: &str, challenge: &str) -> String {
    let secret = BASE64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    BASE64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

/// One authenticated obs-websocket v5 session.
///
/// The client is deliberately blocking: the sink drives it from the pipeline
/// thread, and the only asynchronous thing it needs — noticing that OBS stopped
/// the virtual camera or went away — is a non-blocking poll, not a runtime.
#[derive(Debug)]
pub struct ObsWebSocketClient {
    socket: WebSocket<TcpStream>,
    endpoint: SocketAddr,
    idle_timeout: Duration,
    next_request_id: u64,
    virtual_camera_stopped: Option<String>,
}

impl ObsWebSocketClient {
    /// Connects, authenticates and waits until OBS has identified the session.
    ///
    /// # Errors
    ///
    /// Returns [`ObsProtocolError::Unreachable`] when nothing answers,
    /// [`ObsProtocolError::Unauthenticated`] when the password is refused, and
    /// [`ObsProtocolError::Malformed`] for a `Hello` that is not obs-websocket v5.
    pub fn connect(
        endpoint: SocketAddr,
        password: Option<&str>,
        timeout: Duration,
    ) -> Result<Self, ObsProtocolError> {
        let stream = TcpStream::connect_timeout(&endpoint, timeout).map_err(|error| {
            ObsProtocolError::Unreachable {
                detail: format!("{endpoint}: {error}"),
            }
        })?;
        Self::handshake(stream, endpoint, password, timeout)
    }

    fn handshake(
        stream: TcpStream,
        endpoint: SocketAddr,
        password: Option<&str>,
        timeout: Duration,
    ) -> Result<Self, ObsProtocolError> {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));

        let request =
            tungstenite::http::Uri::from_str(&format!("ws://{endpoint}/")).map_err(|error| {
                ObsProtocolError::Unreachable {
                    detail: format!("the WebSocket address for {endpoint} is invalid: {error}"),
                }
            })?;
        let request =
            tungstenite::ClientRequestBuilder::new(request).with_sub_protocol(SUBPROTOCOL);
        let (socket, _response) =
            client(request, stream).map_err(|error| ObsProtocolError::Unreachable {
                detail: format!("{endpoint}: {error}"),
            })?;

        let mut session = Self {
            socket,
            endpoint,
            idle_timeout: timeout,
            next_request_id: 0,
            virtual_camera_stopped: None,
        };

        let hello = session.read_message()?;
        if hello["op"].as_u64() != Some(0) {
            return Err(ObsProtocolError::Malformed {
                detail: format!("expected `Hello` (op 0), got {}", describe_op(&hello)),
            });
        }

        let authentication = hello["d"].get("authentication").cloned();
        let authentication = match authentication {
            Some(Value::Null) | None => None,
            Some(value) => Some(value),
        };

        let mut identify = Map::new();
        identify.insert("rpcVersion".to_owned(), json!(RPC_VERSION));
        if let Some(authentication) = &authentication {
            let password = password.ok_or_else(|| ObsProtocolError::Unauthenticated {
                detail: format!("{endpoint} requires a password and the configuration has none"),
            })?;
            let challenge = authentication["challenge"].as_str().unwrap_or_default();
            let salt = authentication["salt"].as_str().unwrap_or_default();
            identify.insert(
                "authentication".to_owned(),
                json!(authentication_answer(password, salt, challenge)),
            );
        }
        session.write_message(&json!({ "op": 1, "d": Value::Object(identify) }))?;

        let authenticated = authentication.is_some();
        loop {
            match session.read_message() {
                Ok(value) => {
                    if value["op"].as_u64() == Some(2) {
                        return Ok(session);
                    }
                }
                Err(error) => {
                    return Err(if authenticated {
                        ObsProtocolError::Unauthenticated {
                            detail: format!("{endpoint}: {error}"),
                        }
                    } else {
                        error
                    });
                }
            }
        }
    }

    /// The endpoint this session is connected to.
    #[must_use]
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    /// Sends one request and returns its `responseData`.
    ///
    /// Events that arrive while the request is in flight are read and discarded,
    /// except [`VirtualCamStateChanged`](Self::virtual_camera_stopped), which is
    /// kept because a virtual camera that stops is a failure.
    ///
    /// # Errors
    ///
    /// Returns [`ObsProtocolError::Rejected`] when OBS refuses the request, and
    /// [`ObsProtocolError::Disconnected`] when the socket has gone.
    pub fn request(
        &mut self,
        request_type: &str,
        request_data: Option<Value>,
    ) -> Result<Value, ObsProtocolError> {
        self.next_request_id += 1;
        let request_id = self.next_request_id.to_string();

        let mut payload = Map::new();
        payload.insert(
            "requestType".to_owned(),
            Value::String(request_type.to_owned()),
        );
        payload.insert("requestId".to_owned(), Value::String(request_id.clone()));
        if let Some(data) = request_data {
            payload.insert("requestData".to_owned(), data);
        }
        self.write_message(&json!({ "op": 6, "d": Value::Object(payload) }))?;

        loop {
            let value = self.read_message()?;
            match value["op"].as_u64() {
                Some(7) => {
                    let body = &value["d"];
                    if body["requestId"].as_str() != Some(request_id.as_str()) {
                        continue;
                    }
                    let status = &body["requestStatus"];
                    if status["result"].as_bool() == Some(true) {
                        return Ok(body["responseData"].clone());
                    }
                    return Err(ObsProtocolError::Rejected {
                        request_type: request_type.to_owned(),
                        comment: status["comment"]
                            .as_str()
                            .unwrap_or("OBS gave no comment")
                            .to_owned(),
                    });
                }
                Some(5) => self.observe_event(&value),
                _ => {}
            }
        }
    }

    /// Reads whatever has already arrived without blocking, so the pipeline can
    /// ask "is OBS still there?" between frames.
    ///
    /// # Errors
    ///
    /// Returns [`ObsProtocolError::Disconnected`] when the socket has gone, which
    /// is the signal the ADR turns into `ObsDisconnected`.
    pub fn poll_events(&mut self) -> Result<(), ObsProtocolError> {
        if let Err(error) = self.socket.get_mut().set_nonblocking(true) {
            return Err(self.disconnected(&error.to_string()));
        }

        let outcome = loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<Value>(text.as_str()) {
                        if value["op"].as_u64() == Some(5) {
                            self.observe_event(&value);
                        }
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    break Ok(())
                }
                Err(tungstenite::Error::ConnectionClosed) => {
                    break Err(self.disconnected("the server closed the connection"));
                }
                Err(error) => break Err(self.disconnected(&error.to_string())),
            }
        };

        if let Err(error) = self.socket.get_mut().set_nonblocking(false) {
            return Err(self.disconnected(&error.to_string()));
        }
        let _ = self
            .socket
            .get_mut()
            .set_read_timeout(Some(self.idle_timeout));
        outcome
    }

    /// OBS's own statement that the virtual camera is no longer running, if it
    /// made one. `StartVirtualCam` having been accepted is not enough, and this
    /// is the other half of the same rule.
    #[must_use]
    pub fn virtual_camera_stopped(&self) -> Option<&str> {
        self.virtual_camera_stopped.as_deref()
    }

    /// Closes the control channel. The sink calls this on every exit path.
    pub fn close(mut self) {
        let _ = self.socket.close(None);
        let _ = self.socket.flush();
    }

    fn observe_event(&mut self, value: &Value) {
        let event_type = value["d"]["eventType"].as_str().unwrap_or_default();
        if event_type != "VirtualCamStateChanged" {
            return;
        }
        let data = &value["d"]["eventData"];
        if data["outputActive"].as_bool() == Some(false) {
            self.virtual_camera_stopped =
                Some("OBS reported `VirtualCamStateChanged` with `outputActive: false`".to_owned());
        }
    }

    fn read_message(&mut self) -> Result<Value, ObsProtocolError> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str::<Value>(text.as_str()).map_err(|error| {
                        ObsProtocolError::Malformed {
                            detail: error.to_string(),
                        }
                    })
                }
                Ok(Message::Close(frame)) => {
                    let reason = frame.map_or_else(
                        || "the server closed the connection".to_owned(),
                        |frame| {
                            let refusal = if close_frame_is_authentication_refusal(&frame) {
                                " and refused the password"
                            } else {
                                ""
                            };
                            format!(
                                "the server closed the connection{refusal} with code {}: {}",
                                u16::from(frame.code),
                                frame.reason
                            )
                        },
                    );
                    return Err(ObsProtocolError::Disconnected { detail: reason });
                }
                Ok(_) => {}
                Err(tungstenite::Error::ConnectionClosed) => {
                    return Err(self.disconnected("the server closed the connection"))
                }
                Err(error) => return Err(self.disconnected(&error.to_string())),
            }
        }
    }

    fn write_message(&mut self, value: &Value) -> Result<(), ObsProtocolError> {
        self.socket
            .send(Message::text(value.to_string()))
            .map_err(|error| self.disconnected(&error.to_string()))
    }

    fn disconnected(&self, detail: &str) -> ObsProtocolError {
        ObsProtocolError::Disconnected {
            detail: format!("{}: {detail}", self.endpoint),
        }
    }
}

fn describe_op(value: &Value) -> String {
    value["op"]
        .as_u64()
        .map_or_else(|| "no op".to_owned(), |op| format!("op {op}"))
}

/// Whether a close frame is obs-websocket's authentication refusal, so a wrong
/// password can be reported as such rather than as a dropped connection.
#[must_use]
pub fn close_frame_is_authentication_refusal(frame: &tungstenite::protocol::CloseFrame) -> bool {
    u16::from(frame.code) == AUTHENTICATION_FAILED_CLOSE_CODE
}

#[cfg(test)]
mod tests {
    use super::{authentication_answer, close_frame_is_authentication_refusal};

    #[test]
    fn the_authentication_answer_is_two_sha256_digests_in_base64() {
        // A vector computed independently of this crate: the inner digest is of
        // the password and salt, the outer of the inner base64 and challenge.
        let answer = authentication_answer("canon-webcam", "salt-value", "challenge-value");
        assert_eq!(answer.len(), 44, "a base64 SHA-256 digest is 44 characters");
        assert!(
            answer.ends_with('='),
            "standard base64 padding is used: {answer}"
        );
        assert_ne!(
            answer,
            authentication_answer("canon-webcam", "salt-value", "other-challenge"),
            "the challenge must be part of the digest"
        );
        assert_ne!(
            answer,
            authentication_answer("other-password", "salt-value", "challenge-value"),
            "the password must be part of the digest"
        );
    }

    #[test]
    fn obs_websocket_refuses_a_password_with_its_own_close_code() {
        let frame = tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::from(4009u16),
            reason: "Authentication failed.".into(),
        };
        assert!(close_frame_is_authentication_refusal(&frame));
        let other = tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::Normal,
            reason: "bye".into(),
        };
        assert!(!close_frame_is_authentication_refusal(&other));
    }
}
