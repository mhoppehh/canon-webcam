//! The **frame transport**: an HTTP page on a loopback port, and a WebSocket at
//! `/frames` carrying one camera JPEG per binary message.
//!
//! ADR-0002 chose this transport over MJPEG because MJPEG carries no timestamps:
//! a page in OBS's browser source composites on OBS's own clock, so nothing has
//! to invent a timeline for a ten-minute call. What the transport must therefore
//! get right is pacing and backpressure, and both are single-slot:
//!
//! - each page has a one-frame mailbox, so a page that falls behind loses the
//!   newest frame instead of growing a queue — the same choice the capture queue
//!   makes;
//! - the frame is written from a thread of its own, so a write can never block
//!   the pipeline, and the socket is never left half a frame behind because a
//!   timed-out write drops the page instead of resuming it.
//!
//! The bytes are the camera's own JPEG unless the frame was resized, which is
//! ADR-0002's requirement: the live view feed is never re-encoded on the way to
//! the screen.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use sha1::{Digest, Sha1};

/// The page OBS's browser source loads. It takes its geometry from the frames it
/// receives, so a **sink re-open** at a new **frame size** needs no page change.
const FRAMES_PAGE: &str = include_str!("../../../assets/virtual-camera/frames.html");

/// The GUID RFC 6455 appends to the client's key.
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// How long a page's socket may refuse a write before the page is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the accept loop sleeps between polls of its non-blocking listener.
const ACCEPT_POLL: Duration = Duration::from_millis(2);

/// How long a connection has to send its request head before it is dropped.
const REQUEST_HEAD_TIMEOUT: Duration = Duration::from_secs(5);

/// What happened to one frame offered to the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDelivery {
    /// The frame went into a page's mailbox and is being written to it.
    Delivered,
    /// No page is attached: OBS has not created the browser source yet, or the
    /// page reloaded. The frame is lost, not queued.
    NoConsumerPage,
    /// The page's mailbox still held the previous frame, so this one was
    /// dropped. That is the backpressure rule, not an error.
    Dropped,
}

impl fmt::Display for FrameDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Delivered => "delivered",
            Self::NoConsumerPage => "no consumer page",
            Self::Dropped => "dropped",
        })
    }
}

#[derive(Debug)]
struct Consumer {
    id: u64,
    mailbox: SyncSender<Arc<Vec<u8>>>,
}

#[derive(Debug, Default)]
struct Shared {
    consumer: Mutex<Option<Consumer>>,
    delivered: AtomicU64,
    dropped: AtomicU64,
    /// The times the page itself was asked for. A consumer that never attaches
    /// and a consumer that never even loads the page look the same in
    /// `delivered`, and they are different faults.
    pages: AtomicU64,
}

impl Shared {
    fn consumer(&self) -> MutexGuard<'_, Option<Consumer>> {
        self.consumer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The local HTTP and WebSocket server a consumer page attaches to.
#[derive(Debug)]
pub struct FrameTransport {
    address: SocketAddr,
    shared: Arc<Shared>,
    running: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl FrameTransport {
    /// Binds a loopback port and starts serving.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if no loopback port can be bound.
    pub fn bind() -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;

        let shared = Arc::new(Shared::default());
        let running = Arc::new(AtomicBool::new(true));

        let thread_shared = Arc::clone(&shared);
        let thread_running = Arc::clone(&running);
        let accept_thread = thread::Builder::new()
            .name("frames-transport".to_owned())
            .spawn(move || accept_loop(listener, thread_shared, thread_running))?;

        Ok(Self {
            address,
            shared,
            running,
            accept_thread: Some(accept_thread),
        })
    }

    /// The address the transport bound.
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The URL the browser source is pointed at: the page, not the frames.
    #[must_use]
    pub fn page_url(&self) -> String {
        format!("http://{}/", self.address)
    }

    /// The URL the page itself opens for frames.
    #[must_use]
    pub fn frames_url(&self) -> String {
        format!("ws://{}/frames", self.address)
    }

    /// Offers one JPEG to whatever page is attached.
    ///
    /// Never blocks: either the frame enters an empty mailbox or it is dropped.
    pub fn publish(&self, jpeg: &[u8]) -> FrameDelivery {
        let consumer = self.shared.consumer();
        let Some(consumer) = consumer.as_ref() else {
            return FrameDelivery::NoConsumerPage;
        };
        match consumer.mailbox.try_send(Arc::new(jpeg.to_vec())) {
            Ok(()) => {
                self.shared.delivered.fetch_add(1, Ordering::Relaxed);
                FrameDelivery::Delivered
            }
            Err(TrySendError::Full(_)) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                FrameDelivery::Dropped
            }
            Err(TrySendError::Disconnected(_)) => FrameDelivery::NoConsumerPage,
        }
    }

    /// Whether a consumer page is attached right now.
    #[must_use]
    pub fn consumer_page_connected(&self) -> bool {
        self.shared.consumer().is_some()
    }

    /// Frames handed to a page.
    #[must_use]
    pub fn frames_delivered(&self) -> u64 {
        self.shared.delivered.load(Ordering::Relaxed)
    }

    /// Frames dropped because the page was still being written to.
    #[must_use]
    pub fn frames_dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    /// How many times the page was served. Zero means nothing ever asked for it,
    /// so the fault is above the transport — a browser source that is disabled,
    /// points elsewhere, or was never loaded.
    #[must_use]
    pub fn pages_served(&self) -> u64 {
        self.shared.pages.load(Ordering::Relaxed)
    }
}

impl Drop for FrameTransport {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
        // Dropping the mailbox ends the page's writer thread, which is what
        // releases the port for the next open.
        self.shared.consumer().take();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, running: Arc<AtomicBool>) {
    let mut next_consumer_id = 0_u64;
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // macOS gives an accepted socket its listener's `O_NONBLOCK`, so
                // the page's request head would be read as `WouldBlock` and the
                // connection dropped. Every read below expects a blocking socket.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                next_consumer_id += 1;
                let shared = Arc::clone(&shared);
                let running = Arc::clone(&running);
                let consumer_id = next_consumer_id;
                let _ = thread::Builder::new()
                    .name(format!("frames-page-{consumer_id}"))
                    .spawn(move || serve(stream, shared, running, consumer_id));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            Err(_) => break,
        }
    }
}

fn serve(mut stream: TcpStream, shared: Arc<Shared>, running: Arc<AtomicBool>, consumer_id: u64) {
    // A connection that opens and then says nothing must not keep a thread.
    let _ = stream.set_read_timeout(Some(REQUEST_HEAD_TIMEOUT));
    let Ok(head) = read_request_head(&mut stream) else {
        return;
    };
    let request = RequestHead::parse(&head);
    if request.path == "/frames" {
        upgrade_to_frames(stream, &request, shared, running, consumer_id);
        return;
    }
    let body = FRAMES_PAGE.as_bytes();
    shared.pages.fetch_add(1, Ordering::Relaxed);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    if stream.write_all(response.as_bytes()).is_ok() {
        let _ = stream.write_all(body);
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn upgrade_to_frames(
    mut stream: TcpStream,
    request: &RequestHead,
    shared: Arc<Shared>,
    running: Arc<AtomicBool>,
    consumer_id: u64,
) {
    let Some(key) = request.header("sec-websocket-key") else {
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        websocket_accept_key(key)
    );
    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let (mailbox, frames) = mpsc::sync_channel::<Arc<Vec<u8>>>(1);
    {
        let mut slot = shared.consumer();
        *slot = Some(Consumer {
            id: consumer_id,
            mailbox,
        });
    }

    while running.load(Ordering::SeqCst) {
        let Ok(frame) = frames.recv() else {
            break;
        };
        if write_binary_frame(&mut stream, &frame).is_err() {
            break;
        }
    }

    let mut slot = shared.consumer();
    if slot
        .as_ref()
        .is_some_and(|consumer| consumer.id == consumer_id)
    {
        *slot = None;
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// The accept key RFC 6455 defines: base64 of the SHA-1 of the client key and
/// the GUID.
#[must_use]
pub fn websocket_accept_key(key: &str) -> String {
    let mut digest = Sha1::new();
    digest.update(key.as_bytes());
    digest.update(WEBSOCKET_GUID.as_bytes());
    BASE64.encode(digest.finalize())
}

/// Writes one unmasked binary frame. A server never masks, and a page never
/// sends anything this transport reads.
fn write_binary_frame(stream: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x82);
    let length = payload.len();
    if length < 126 {
        frame.push(u8::try_from(length).unwrap_or(125));
    } else if let Ok(length) = u16::try_from(length) {
        frame.push(126);
        frame.extend_from_slice(&length.to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(length as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame)
}

fn read_request_head(stream: &mut TcpStream) -> io::Result<String> {
    let _ = stream.set_read_timeout(Some(WRITE_TIMEOUT));
    let mut head = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the request ended before its headers did",
            ));
        }
        head.extend_from_slice(&chunk[..read]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if head.len() > 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the request head is larger than any page or page-frame request",
            ));
        }
    }
    String::from_utf8(head)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the request head is not UTF-8"))
}

struct RequestHead {
    path: String,
    headers: Vec<(String, String)>,
}

impl RequestHead {
    fn parse(head: &str) -> Self {
        let mut lines = head.split("\r\n");
        let path = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_owned();
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();
        Self { path, headers }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::websocket_accept_key;

    #[test]
    fn the_accept_key_matches_the_example_rfc_6455_publishes() {
        assert_eq!(
            websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn the_page_reads_its_geometry_from_the_frames_it_receives() {
        let page = include_str!("../../../assets/virtual-camera/frames.html");
        assert!(
            page.contains("createImageBitmap"),
            "the page decodes the camera JPEG without re-encoding it"
        );
        assert!(
            page.contains("canvas.width = bitmap.width"),
            "the canvas takes its geometry from the first frame"
        );
        assert!(
            page.contains("/frames"),
            "the page opens the frame WebSocket the transport serves"
        );
    }
}
