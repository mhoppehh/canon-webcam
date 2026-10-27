//! Core of the Canon EOS M50 virtual camera.
//!
//! The vocabulary of this crate is defined in `GLOSSARY.md` at the repository
//! root. In short: a **PTP session** is the single exclusive handle to the
//! camera, a **frame size** is the pixel size of one live view frame, and a
//! **measurement** learns the frame size and frame rate from frame data because
//! the M50 honours no live view format it is asked for.
//!
//! No module in this crate reads a frame size from a camera property: the M50
//! accepts `liveviewsize` writes and changes nothing (see
//! `docs/adr/0004-no-live-view-size-selection.md`).

pub mod bounded_queue;
pub mod calibration;
pub mod config;
pub mod control;
pub mod frame_size;
pub mod jpeg;
pub mod measurement;
pub mod mechanism;
pub mod output_setting;
pub mod pixel_format;
pub mod platform;
pub mod recovery;
pub mod runner;
pub mod session;
pub mod sink;
pub mod stats;
pub mod stream;
pub mod synthetic;

#[cfg(feature = "gphoto2-binding")]
pub mod gphoto_session;
