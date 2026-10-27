//! Shared test support: a fake PTP session and a scripted clock.
//!
//! No test in this repository may need a camera, so every test that needs frames
//! uses this fake. It records the order of the operations it receives, which is
//! how the "same invocation" requirement is checked.

#![allow(dead_code)]

pub mod control;

use std::collections::VecDeque;
use std::time::Duration;

use camera_core::frame_size::FrameSize;
use camera_core::measurement::Clock;
use camera_core::output_setting::OutputSetting;
use camera_core::session::{PtpSession, SessionError};

/// Builds the smallest byte sequence that carries a JPEG start-of-frame marker.
///
/// The measurement layer only reads the marker, so a full JPEG is unnecessary
/// for tests that are not about the parser itself.
pub fn jpeg_claiming(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

pub fn frame_of_size(frame_size: FrameSize) -> Vec<u8> {
    jpeg_claiming(
        u16::try_from(frame_size.width()).expect("test frame width fits"),
        u16::try_from(frame_size.height()).expect("test frame height fits"),
    )
}

/// The operation order the fake session observed, as short strings.
pub type OperationLog = Vec<String>;

pub struct FakeSession {
    model: String,
    frames: VecDeque<Result<Vec<u8>, SessionError>>,
    log: OperationLog,
    fail_on_output_setting: Option<SessionError>,
    fail_on_viewfinder: Option<SessionError>,
}

impl FakeSession {
    pub fn new(model: &str, frames: Vec<Result<Vec<u8>, SessionError>>) -> Self {
        Self {
            model: model.to_owned(),
            frames: frames.into(),
            log: Vec::new(),
            fail_on_output_setting: None,
            fail_on_viewfinder: None,
        }
    }

    pub fn with_frames(frames: Vec<Vec<u8>>) -> Self {
        Self::new(
            "Canon EOS M50",
            frames.into_iter().map(Ok).collect::<Vec<_>>(),
        )
    }

    #[must_use]
    pub fn failing_output_setting(mut self, error: SessionError) -> Self {
        self.fail_on_output_setting = Some(error);
        self
    }

    #[must_use]
    pub fn failing_viewfinder(mut self, error: SessionError) -> Self {
        self.fail_on_viewfinder = Some(error);
        self
    }

    pub fn log(&self) -> &OperationLog {
        &self.log
    }
}

impl PtpSession for FakeSession {
    fn model(&self) -> &str {
        &self.model
    }

    fn set_output_setting(&mut self, setting: OutputSetting) -> Result<(), SessionError> {
        self.log.push(format!("set-output:{}", setting.raw()));
        match self.fail_on_output_setting.clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn activate_viewfinder_action(&mut self) -> Result<(), SessionError> {
        self.log.push("viewfinder".to_owned());
        match self.fail_on_viewfinder.clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn capture_live_view_frame(&mut self) -> Result<Vec<u8>, SessionError> {
        self.log.push("capture".to_owned());
        self.frames.pop_front().unwrap_or_else(|| {
            panic!(
                "the fake session ran out of frames after {} captures",
                self.log.iter().filter(|entry| *entry == "capture").count()
            )
        })
    }
}

/// A clock that returns scripted elapsed times, so a measurement can be tested
/// without waiting.
pub struct ScriptedClock {
    times: std::cell::RefCell<VecDeque<Duration>>,
}

impl ScriptedClock {
    pub fn from_milliseconds(times: impl IntoIterator<Item = u64>) -> Self {
        Self {
            times: std::cell::RefCell::new(times.into_iter().map(Duration::from_millis).collect()),
        }
    }

    /// A clock that advances by one fixed step for every frame.
    pub fn stepping(frame_count: usize, step: Duration) -> Self {
        let times = (1..=frame_count).map(|frame| step * u32::try_from(frame).expect("fits"));
        Self {
            times: std::cell::RefCell::new(times.collect()),
        }
    }
}

impl Clock for ScriptedClock {
    fn elapsed(&self) -> Duration {
        self.times
            .borrow_mut()
            .pop_front()
            .expect("the scripted clock ran out of times")
    }
}
