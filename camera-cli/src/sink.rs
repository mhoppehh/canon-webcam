//! Choosing where the **live view feed** is published, and building that sink.
//!
//! There are two sinks a run can end in, and they answer two different
//! questions:
//!
//! - the counting sink (`StatsSink`) counts what the pipeline delivered and
//!   publishes nothing, which is what a run with no OBS can still report
//!   honestly;
//! - the **platform backend** (`MacOsVirtualCameraSink`) is the **virtual camera
//!   sink** itself: the thing a **consumer application** opens.
//!
//! The mechanism is the one part of the output side a user chooses, and it is
//! already validated by `camera_core::config`. So this module adds no second
//! mechanism flag: it takes the resolved [`StreamingConfig`], and the platform
//! sink is built from the `[macos]` table in that same configuration. Naming a
//! mechanism this build does not have is an error and never a fallback to the
//! counting sink.
//!
//! Everything that builds the platform sink lives here, so `stream` and the
//! `serve`/`control` feed factory cannot drift apart: a `serve` start takes a
//! fresh sink from the same [`SinkPlan`], which is what makes a stop release the
//! virtual camera and a start take it again.

use std::fmt;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::{Mutex, PoisonError};

use camera_core::config::StreamingConfig;
#[cfg(target_os = "macos")]
use camera_core::mechanism::VirtualCameraMechanism;
use camera_core::mechanism::{MechanismError, Platform};
use camera_core::sink::FrameSink;
use camera_core::stats::{StatsSink, StreamStats};

#[cfg(target_os = "macos")]
use camera_core::frame_size::FrameSize;
#[cfg(target_os = "macos")]
use camera_core::pixel_format::{LiveViewFrame, PixelFormat};
#[cfg(target_os = "macos")]
use camera_core::platform::macos::{
    AvFoundationDeviceProbe, MacOsVirtualCameraConfig, MacOsVirtualCameraSink, VirtualCameraStatus,
};
#[cfg(target_os = "macos")]
use camera_core::sink::SinkError;

/// The sink a run publishes through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SinkSelection {
    /// The counting sink. It measures the live view feed and publishes nothing,
    /// so no consumer application can see anything.
    #[default]
    Stats,
    /// The **platform backend**: the virtual camera the operating system exposes
    /// and a consumer application opens.
    VirtualCamera,
}

/// The sink a run publishes through when nothing is asked for.
///
/// The counting sink, because a run must not fail merely because OBS is not
/// installed: the platform sink is asked for by name.
pub const DEFAULT_SINK: SinkSelection = SinkSelection::Stats;

impl SinkSelection {
    /// The word `--sink` takes, and the word a `serve` start prints.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stats => "stats",
            Self::VirtualCamera => "virtual-camera",
        }
    }

    /// Parses a `--sink` value, refusing one that is not a sink.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stats" => Some(Self::Stats),
            "virtual-camera" => Some(Self::VirtualCamera),
            _ => None,
        }
    }
}

impl fmt::Display for SinkSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why the platform sink could not be selected.
///
/// Both answers are configuration or build errors rather than a silent
/// substitution of the counting sink, because a run that appears to publish and
/// does not is worse than one that refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SinkSelectionError {
    /// The configured **virtual camera mechanism** does not exist on the
    /// platform in use. The message is the configuration's own wording.
    Mechanism(MechanismError),
    /// The mechanism exists on this platform but its **platform backend** is not
    /// compiled into this build.
    NotCompiledIn {
        /// The platform whose backend is missing.
        platform: Platform,
    },
}

impl fmt::Display for SinkSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mechanism(error) => write!(formatter, "{error}"),
            Self::NotCompiledIn { platform } => write!(
                formatter,
                "`--sink virtual-camera` needs the {platform} platform backend, which is not \
                 compiled into this build, so the live view feed has nowhere to go. This is a \
                 build limitation rather than a fallback: nothing is published, and the counting \
                 sink is used only when `--sink stats` asks for it"
            ),
        }
    }
}

impl std::error::Error for SinkSelectionError {}

/// What the sink that published did, in the terms the run report prints.
///
/// Kept separate from the pipeline's own counters: those describe what reached
/// the sink, and these describe what the sink itself did with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SinkReport {
    /// Which sink published.
    pub selection: SinkSelection,
    /// Whether a **consumer page** attached while the sink was open.
    pub consumer_page_seen: bool,
    /// How many times the sink's transport served the page. Zero says nothing
    /// ever asked for it, so the fault is in OBS's browser source.
    pub consumer_pages_served: u64,
    /// Frames the sink itself handed on.
    pub frames_published: u64,
    /// Frames the sink dropped because its consumer was behind.
    pub frames_dropped: u64,
    /// One-off remarks from the sink worth printing once.
    pub notes: Vec<String>,
}

/// A run's sink, built fresh for every segment and every start.
pub type SinkFactory = Box<dyn FnMut() -> Box<dyn FrameSink + Send> + Send>;

/// The **expected frame size**'s sink side of the run: which sink publishes, and
/// how to build one.
///
/// Cloneable and `Send + Sync`, because the `serve`/`control` feed factory holds
/// it across starts and builds a new sink for each one.
#[derive(Clone)]
pub struct SinkPlan {
    selection: SinkSelection,
    make: Arc<dyn Fn() -> SinkFactory + Send + Sync>,
    #[cfg(target_os = "macos")]
    status: StatusSlot,
}

impl SinkPlan {
    /// Resolves the sink to publish through on the platform this binary was
    /// built for.
    ///
    /// # Errors
    ///
    /// Returns [`SinkSelectionError`] when the platform sink is asked for and
    /// cannot be built, which is never a fallback to the counting sink.
    pub fn select(
        selection: SinkSelection,
        config: &StreamingConfig,
    ) -> Result<Self, SinkSelectionError> {
        Self::select_for(selection, config, Platform::current())
    }

    /// Resolves the sink against a named platform, which is what makes the
    /// "not compiled in" answer testable on the one machine the tests run on.
    ///
    /// # Errors
    ///
    /// Returns [`SinkSelectionError`] as [`Self::select`] does.
    pub fn select_for(
        selection: SinkSelection,
        config: &StreamingConfig,
        platform: Platform,
    ) -> Result<Self, SinkSelectionError> {
        match selection {
            SinkSelection::Stats => Ok(Self::counting()),
            SinkSelection::VirtualCamera => Self::platform(config, platform),
        }
    }

    /// Which sink this plan publishes through.
    #[must_use]
    pub fn selection(&self) -> SinkSelection {
        self.selection
    }

    /// Builds one fresh sink. A **sink re-open**, a resumed segment and a new
    /// start each get their own, so no sink is ever reused after it was closed.
    #[must_use]
    pub fn factory(&self) -> SinkFactory {
        (self.make)()
    }

    /// What the sink that published did.
    #[must_use]
    pub fn report(&self) -> SinkReport {
        #[cfg(target_os = "macos")]
        if let Some(status) = self
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return SinkReport {
                selection: self.selection,
                consumer_page_seen: status.consumer_page_seen,
                consumer_pages_served: status.consumer_pages_served,
                frames_published: status.frames_published,
                frames_dropped: status.frames_dropped,
                notes: status.notes,
            };
        }

        SinkReport {
            selection: self.selection,
            ..SinkReport::default()
        }
    }

    /// The counting sink, which every build can always build.
    fn counting() -> Self {
        Self {
            selection: SinkSelection::Stats,
            make: Arc::new(|| Box::new(stats_sink) as SinkFactory),
            #[cfg(target_os = "macos")]
            status: Arc::new(Mutex::new(None)),
        }
    }

    /// The **platform backend**, built from the configuration the mechanism
    /// validation already read.
    #[cfg(target_os = "macos")]
    fn platform(config: &StreamingConfig, platform: Platform) -> Result<Self, SinkSelectionError> {
        // The mechanism is validated in the configuration, so this can only fail
        // for a name the platform does not have — which must not fall back.
        VirtualCameraMechanism::parse_for(platform, config.mechanism().as_str())
            .map_err(SinkSelectionError::Mechanism)?;

        let platform_config = MacOsVirtualCameraConfig {
            prerequisites: config.macos_prerequisites().clone(),
            ..MacOsVirtualCameraConfig::default()
        };
        let status: StatusSlot = Arc::new(Mutex::new(None));

        let make = {
            let platform_config = platform_config.clone();
            let status = Arc::clone(&status);
            Arc::new(move || -> SinkFactory {
                let platform_config = platform_config.clone();
                let status = Arc::clone(&status);
                Box::new(move || {
                    Box::new(StatusRecordingSink::new(
                        MacOsVirtualCameraSink::new(
                            platform_config.clone(),
                            Arc::new(AvFoundationDeviceProbe::new()),
                        ),
                        Arc::clone(&status),
                    ))
                })
            })
        };

        Ok(Self {
            selection: SinkSelection::VirtualCamera,
            make,
            status,
        })
    }

    /// A build with no platform backend cannot publish at all, and says so.
    #[cfg(not(target_os = "macos"))]
    fn platform(_config: &StreamingConfig, platform: Platform) -> Result<Self, SinkSelectionError> {
        Err(SinkSelectionError::NotCompiledIn { platform })
    }
}

impl fmt::Debug for SinkPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SinkPlan")
            .field("selection", &self.selection)
            .finish_non_exhaustive()
    }
}

/// The sink that counts what it accepts.
///
/// It is a real [`FrameSink`], so it enforces the same rule a platform backend
/// does: a frame whose frame size is not the one the sink was opened at is
/// refused rather than reinterpreted.
fn stats_sink() -> Box<dyn FrameSink + Send> {
    Box::new(StatsSink::new(Arc::new(StreamStats::new())))
}

/// Where a platform sink's own status is left for the run report.
#[cfg(target_os = "macos")]
type StatusSlot = Arc<Mutex<Option<VirtualCameraStatus>>>;

/// A platform sink that leaves its status where the run report can read it.
///
/// The report is printed after the run, and the run owns and drops its sink, so
/// the status is copied out on every call and again on drop. Nothing is inferred:
/// every counter comes from [`MacOsVirtualCameraSink::status`].
#[cfg(target_os = "macos")]
struct StatusRecordingSink {
    inner: MacOsVirtualCameraSink,
    slot: StatusSlot,
}

#[cfg(target_os = "macos")]
impl StatusRecordingSink {
    fn new(inner: MacOsVirtualCameraSink, slot: StatusSlot) -> Self {
        Self { inner, slot }
    }

    fn record(&self) {
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(self.inner.status());
    }
}

#[cfg(target_os = "macos")]
impl FrameSink for StatusRecordingSink {
    fn open(
        &mut self,
        expected_frame_size: FrameSize,
        pixel_format: PixelFormat,
    ) -> Result<(), SinkError> {
        let result = self.inner.open(expected_frame_size, pixel_format);
        self.record();
        result
    }

    fn accept(&mut self, frame: &LiveViewFrame) -> Result<(), SinkError> {
        let result = self.inner.accept(frame);
        self.record();
        result
    }

    fn close(&mut self) -> Result<(), SinkError> {
        let result = self.inner.close();
        self.record();
        result
    }
}

#[cfg(target_os = "macos")]
impl Drop for StatusRecordingSink {
    fn drop(&mut self) {
        self.record();
    }
}
