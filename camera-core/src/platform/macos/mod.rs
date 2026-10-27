//! The macOS **platform backend**: OBS Virtual Camera, driven through
//! obs-websocket, with frames carried to an OBS browser source over a local
//! WebSocket.
//!
//! `docs/adr/0002-macos-virtual-camera-mechanism.md` is the authority here, and
//! it chose three things this module keeps:
//!
//! - the mechanism is `obs-virtual-camera`, and there is no fallback;
//! - the transport is ours — a page on a local HTTP port and a frame WebSocket —
//!   because no process can inject frames into somebody else's camera extension;
//! - `StartVirtualCam` returning success is not evidence that the virtual camera
//!   is running, so every open confirms it with `GetVirtualCamStatus` and with
//!   an AVFoundation listing of the registered devices.
//!
//! Nothing in this module touches capture or the pipeline: it is one
//! [`FrameSink`](crate::sink::FrameSink) implementation and the protocol it
//! needs.

pub mod device_probe;
pub mod frames_transport;
pub mod obs_websocket;
pub mod prerequisites;
pub mod sink;

pub use device_probe::{AvFoundationDeviceProbe, DeviceProbeFailure, VirtualCameraDeviceProbe};
pub use prerequisites::ObsPrerequisites;
pub use sink::{
    MacOsVirtualCameraConfig, MacOsVirtualCameraError, MacOsVirtualCameraSink, VirtualCameraStatus,
};
