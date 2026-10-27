//! The configuration surface: a TOML file, overridden by command line arguments.
//!
//! Five top-level keys, and only five:
//!
//! | Key | Meaning |
//! | --- | --- |
//! | `resolution` | a **requested size**: a preference the M50 cannot honour |
//! | `fps` | a frame rate preference for the sink to pace at |
//! | `pixel_format` | the normalised format the pipeline produces (`rgb24`) |
//! | `upscale` | whether delivered frames are **resized** to `resolution` |
//! | `mechanism` | the **virtual camera mechanism**, validated against the platform |
//!
//! The key is `mechanism` and not `backend`: there is exactly one **platform
//! backend** per build target, so it is not a user choice.
//!
//! On macOS the file may also carry a `[macos]` table: the prerequisites the
//! chosen mechanism reads (ADR-0002's P1, P3 and P4). It is a table rather than
//! five more top-level keys because those values belong to one mechanism of one
//! platform, and a Windows or Linux mechanism reads its own.
//!
//! Every rejection names the offending key and happens before a camera is
//! touched, because discovering a typo halfway through a capture is a worse
//! failure than refusing to start.
//!
//! `resolution` is a **requested size**. Nothing here reads a live view size from
//! the camera, and nothing reports the requested size as a result: the
//! **measured frame size** is whatever the frames turn out to be.

use std::error;
use std::fmt;

use crate::frame_size::FrameSize;
use crate::mechanism::{MechanismError, Platform, VirtualCameraMechanism};
use crate::pixel_format::{PixelFormat, UnknownPixelFormat};
use crate::stream::StreamPipelineConfig;

/// Every top-level key this program understands.
#[cfg(target_os = "macos")]
pub const KNOWN_KEYS: [&str; 6] = [
    "resolution",
    "fps",
    "pixel_format",
    "upscale",
    "mechanism",
    "macos",
];

/// Every top-level key this program understands.
///
/// There is no `macos` table in this build: the macOS mechanism is not compiled
/// in, and accepting its prerequisites would let a configuration name a
/// mechanism this build does not have.
#[cfg(not(target_os = "macos"))]
pub const KNOWN_KEYS: [&str; 5] = ["resolution", "fps", "pixel_format", "upscale", "mechanism"];

/// The keys of the `[macos]` table: the `ObsPrerequisites` ADR-0002's checks
/// read.
#[cfg(target_os = "macos")]
pub const MACOS_KNOWN_KEYS: [&str; 5] = [
    "obs_app_path",
    "websocket_endpoint",
    "websocket_password",
    "obs_log_directory",
    "launch_obs",
];

#[cfg(target_os = "macos")]
const MACOS_EXPECTED: &str = "a table of the macOS mechanism's prerequisites";
#[cfg(target_os = "macos")]
const OBS_APP_PATH_EXPECTED: &str = "a path such as /Applications/OBS.app";
#[cfg(target_os = "macos")]
const WEBSOCKET_ENDPOINT_EXPECTED: &str = "a socket address such as 127.0.0.1:4455";
#[cfg(target_os = "macos")]
const WEBSOCKET_PASSWORD_EXPECTED: &str = "the password obs-websocket was given";
#[cfg(target_os = "macos")]
const OBS_LOG_DIRECTORY_EXPECTED: &str = "the directory OBS writes its logs to";
#[cfg(target_os = "macos")]
const LAUNCH_OBS_EXPECTED: &str = "true or false";

/// The text a user sees whenever upscaling is mentioned, so it never reads as
/// added detail.
pub const UPSCALE_LABEL: &str = "resizing";
/// The caveat that must accompany [`UPSCALE_LABEL`].
pub const UPSCALE_DETAIL_NOTE: &str = "it adds no detail";

/// Why a configuration could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The file is not valid TOML.
    InvalidToml {
        /// What the parser said.
        detail: String,
    },
    /// The file carries a key this program does not have, so a typo would
    /// otherwise be ignored.
    UnknownKey {
        /// The key as written.
        key: String,
    },
    /// The `[macos]` table carries a key the macOS mechanism does not have.
    UnknownMacOsKey {
        /// The key as written, without the `macos.` prefix.
        key: String,
    },
    /// A key's value is not of the right shape.
    InvalidValue {
        /// The offending key.
        key: &'static str,
        /// The value as written.
        value: String,
        /// What the key accepts.
        expected: &'static str,
    },
    /// `pixel_format` names a format the pipeline does not produce.
    PixelFormat(UnknownPixelFormat),
    /// `mechanism` does not exist on the platform in use.
    Mechanism(MechanismError),
    /// `upscale` is on but `resolution` gives it nothing to resize to.
    UpscaleWithoutResolution,
    /// The file could not be read.
    Unreadable {
        /// The path as given.
        path: String,
        /// What the reader said.
        detail: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToml { detail } => {
                write!(
                    formatter,
                    "the configuration file is not valid TOML: {detail}"
                )
            }
            Self::UnknownKey { key } => write!(
                formatter,
                "`{key}` is not a configuration key. The keys are {}",
                KNOWN_KEYS.join(", ")
            ),
            #[cfg(target_os = "macos")]
            Self::UnknownMacOsKey { key } => write!(
                formatter,
                "`macos.{key}` is not a key of the macOS mechanism. The keys are {}",
                MACOS_KNOWN_KEYS
                    .iter()
                    .map(|key| format!("macos.{key}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::InvalidValue {
                key,
                value,
                expected,
            } => write!(formatter, "`{key}` is `{value}`, which is not {expected}"),
            Self::PixelFormat(error) => write!(formatter, "{error}"),
            Self::Mechanism(error) => write!(formatter, "{error}"),
            Self::UpscaleWithoutResolution => write!(
                formatter,
                "`upscale` is on but `resolution` is not set, so there is no size to {} to. \
                 `upscale` is {UPSCALE_LABEL} and {UPSCALE_DETAIL_NOTE}",
                UPSCALE_LABEL
            ),
            Self::Unreadable { path, detail } => {
                write!(
                    formatter,
                    "the configuration file {path} could not be read: {detail}"
                )
            }
        }
    }
}

impl error::Error for ConfigError {}

impl From<UnknownPixelFormat> for ConfigError {
    fn from(error: UnknownPixelFormat) -> Self {
        Self::PixelFormat(error)
    }
}

impl From<MechanismError> for ConfigError {
    fn from(error: MechanismError) -> Self {
        Self::Mechanism(error)
    }
}

/// Values supplied on the command line, which override the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigOverrides {
    /// Overrides `resolution`.
    pub resolution: Option<String>,
    /// Overrides `fps`.
    pub fps: Option<String>,
    /// Overrides `pixel_format`.
    pub pixel_format: Option<String>,
    /// Overrides `upscale`.
    pub upscale: Option<bool>,
    /// Overrides `mechanism`.
    pub mechanism: Option<String>,
}

impl ConfigOverrides {
    /// Whether any value was supplied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resolution.is_none()
            && self.fps.is_none()
            && self.pixel_format.is_none()
            && self.upscale.is_none()
            && self.mechanism.is_none()
    }
}

/// A resolved, validated configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamingConfig {
    requested_size: Option<FrameSize>,
    fps: Option<f64>,
    pixel_format: PixelFormat,
    upscale: bool,
    mechanism: VirtualCameraMechanism,
    #[cfg(target_os = "macos")]
    macos: crate::platform::macos::ObsPrerequisites,
}

impl StreamingConfig {
    /// Resolves a file's contents and any command line overrides.
    ///
    /// The file is applied first and the overrides second, so an override always
    /// wins. `file` is `None` when no configuration file was given, which is not
    /// an error: every key has a default.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] naming the offending key for anything that cannot
    /// be used.
    pub fn resolve(
        file: Option<&str>,
        overrides: ConfigOverrides,
        platform: Platform,
    ) -> Result<Self, ConfigError> {
        let mut draft = Draft::default();

        if let Some(text) = file {
            draft.apply_file(text)?;
        }
        draft.apply_overrides(overrides)?;

        let mechanism = match draft.mechanism {
            Some(value) => VirtualCameraMechanism::parse_for(platform, &value)?,
            None => VirtualCameraMechanism::default_for(platform)?,
        };

        let config = Self {
            requested_size: draft.requested_size,
            fps: draft.fps,
            pixel_format: draft.pixel_format.unwrap_or(PixelFormat::Rgb24),
            upscale: draft.upscale.unwrap_or(false),
            mechanism,
            #[cfg(target_os = "macos")]
            macos: draft.macos.unwrap_or_default(),
        };

        if config.upscale && config.requested_size.is_none() {
            return Err(ConfigError::UpscaleWithoutResolution);
        }

        Ok(config)
    }

    /// Reads a configuration file from disk and resolves it with `overrides`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Unreadable`] if the file cannot be read, and every
    /// error [`StreamingConfig::resolve`] can return.
    pub fn load(
        path: &std::path::Path,
        overrides: ConfigOverrides,
        platform: Platform,
    ) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|error| ConfigError::Unreadable {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
        Self::resolve(Some(&text), overrides, platform)
    }

    /// The **requested size**: a preference this camera cannot honour.
    #[must_use]
    pub fn requested_size(&self) -> Option<FrameSize> {
        self.requested_size
    }

    /// The frame rate preference for the sink to pace at, if any.
    ///
    /// A preference only: the **measured frame rate** is what the frames did.
    #[must_use]
    pub fn fps_preference(&self) -> Option<f64> {
        self.fps
    }

    /// The normalised pixel format the pipeline produces.
    #[must_use]
    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// Whether delivered frames are resized to the requested size.
    #[must_use]
    pub fn upscale(&self) -> bool {
        self.upscale
    }

    /// The **virtual camera mechanism** in use.
    #[must_use]
    pub fn mechanism(&self) -> VirtualCameraMechanism {
        self.mechanism
    }

    /// The `[macos]` table: what the **platform backend** needs before it can
    /// open the **virtual camera sink**.
    ///
    /// A section of its own rather than five more top-level keys, because these
    /// values are prerequisites of one mechanism on one platform; ADR-0002 names
    /// every one of them.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn macos_prerequisites(&self) -> &crate::platform::macos::ObsPrerequisites {
        &self.macos
    }

    /// The size delivered frames are resized to, which is only ever the
    /// **requested size** and only when `upscale` is on.
    #[must_use]
    pub fn resize_target(&self) -> Option<FrameSize> {
        if self.upscale {
            self.requested_size
        } else {
            None
        }
    }

    /// The sentence a user sees when resizing is on.
    ///
    /// It says **resizing** and says it adds no detail, so it can never read as
    /// extra resolution.
    #[must_use]
    pub fn upscale_note(&self) -> Option<String> {
        let target = self.resize_target()?;
        Some(format!(
            "{UPSCALE_LABEL} to {target} ({UPSCALE_DETAIL_NOTE}, so it is not extra resolution)"
        ))
    }

    /// Builds the pipeline configuration this settings set implies.
    #[must_use]
    pub fn pipeline_config(
        &self,
        calibration: crate::calibration::CalibrationPolicy,
    ) -> StreamPipelineConfig {
        let config = StreamPipelineConfig::new(calibration);
        match self.resize_target() {
            Some(target) => config.with_resize_to(target),
            None => config,
        }
    }
}

/// The keys as they were written, before validation.
#[derive(Debug, Default)]
struct Draft {
    requested_size: Option<FrameSize>,
    fps: Option<f64>,
    pixel_format: Option<PixelFormat>,
    upscale: Option<bool>,
    mechanism: Option<String>,
    #[cfg(target_os = "macos")]
    macos: Option<crate::platform::macos::ObsPrerequisites>,
}

impl Draft {
    fn apply_file(&mut self, text: &str) -> Result<(), ConfigError> {
        let table: toml::Table =
            text.parse()
                .map_err(|error: toml::de::Error| ConfigError::InvalidToml {
                    detail: error.to_string(),
                })?;

        for (key, value) in &table {
            match key.as_str() {
                "resolution" => {
                    let text = value.as_str().ok_or_else(|| ConfigError::InvalidValue {
                        key: "resolution",
                        value: value.to_string(),
                        expected: RESOLUTION_EXPECTED,
                    })?;
                    self.requested_size = Some(parse_requested_size(text).ok_or_else(|| {
                        ConfigError::InvalidValue {
                            key: "resolution",
                            value: text.to_owned(),
                            expected: RESOLUTION_EXPECTED,
                        }
                    })?);
                }
                "fps" => self.fps = Some(parse_fps(value)?),
                "pixel_format" => {
                    let text = value.as_str().ok_or_else(|| ConfigError::InvalidValue {
                        key: "pixel_format",
                        value: value.to_string(),
                        expected: PIXEL_FORMAT_EXPECTED,
                    })?;
                    self.pixel_format = Some(text.parse::<PixelFormat>()?);
                }
                "upscale" => {
                    self.upscale =
                        Some(value.as_bool().ok_or_else(|| ConfigError::InvalidValue {
                            key: "upscale",
                            value: value.to_string(),
                            expected: UPSCALE_EXPECTED,
                        })?);
                }
                "mechanism" => {
                    let text = value.as_str().ok_or_else(|| ConfigError::InvalidValue {
                        key: "mechanism",
                        value: value.to_string(),
                        expected: MECHANISM_EXPECTED,
                    })?;
                    self.mechanism = Some(text.to_owned());
                }
                #[cfg(target_os = "macos")]
                "macos" => {
                    self.macos = Some(parse_macos_section(value)?);
                }
                other => {
                    return Err(ConfigError::UnknownKey {
                        key: other.to_owned(),
                    })
                }
            }
        }
        Ok(())
    }

    fn apply_overrides(&mut self, overrides: ConfigOverrides) -> Result<(), ConfigError> {
        if let Some(value) = overrides.resolution {
            self.requested_size =
                Some(
                    parse_requested_size(&value).ok_or_else(|| ConfigError::InvalidValue {
                        key: "resolution",
                        value: value.clone(),
                        expected: RESOLUTION_EXPECTED,
                    })?,
                );
        }
        if let Some(value) = overrides.fps {
            self.fps = Some(
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|fps| *fps > 0.0)
                    .ok_or_else(|| ConfigError::InvalidValue {
                        key: "fps",
                        value: value.clone(),
                        expected: FPS_EXPECTED,
                    })?,
            );
        }
        if let Some(value) = overrides.pixel_format {
            self.pixel_format = Some(value.parse::<PixelFormat>()?);
        }
        if let Some(value) = overrides.upscale {
            self.upscale = Some(value);
        }
        if let Some(value) = overrides.mechanism {
            self.mechanism = Some(value);
        }
        Ok(())
    }
}

const RESOLUTION_EXPECTED: &str = "a frame size written `1080p`, `720p` or `1024x576`";
const FPS_EXPECTED: &str = "a positive number of frames per second";
const PIXEL_FORMAT_EXPECTED: &str = "a pixel format written as a string, such as \"rgb24\"";
const UPSCALE_EXPECTED: &str = "a TOML boolean, `true` or `false`";
const MECHANISM_EXPECTED: &str = "a virtual camera mechanism written as a string";

/// Parses the `[macos]` table into the prerequisites ADR-0002's checks read.
///
/// Every value is a prerequisite of the macOS mechanism, not a property of the
/// live view feed, which is why they are not top-level keys. `launch_obs`
/// defaults to on and a test can turn it off so no process is ever started.
#[cfg(target_os = "macos")]
fn parse_macos_section(
    value: &toml::Value,
) -> Result<crate::platform::macos::ObsPrerequisites, ConfigError> {
    let table = value.as_table().ok_or_else(|| ConfigError::InvalidValue {
        key: "macos",
        value: value.to_string(),
        expected: MACOS_EXPECTED,
    })?;

    let mut prerequisites = crate::platform::macos::ObsPrerequisites::default();
    for (key, value) in table {
        match key.as_str() {
            "obs_app_path" => {
                prerequisites.obs_app_path = std::path::PathBuf::from(text_value(
                    "macos.obs_app_path",
                    value,
                    OBS_APP_PATH_EXPECTED,
                )?);
            }
            "websocket_endpoint" => {
                let text = text_value(
                    "macos.websocket_endpoint",
                    value,
                    WEBSOCKET_ENDPOINT_EXPECTED,
                )?;
                prerequisites.websocket_endpoint =
                    text.parse().map_err(|_| ConfigError::InvalidValue {
                        key: "macos.websocket_endpoint",
                        value: text.to_owned(),
                        expected: WEBSOCKET_ENDPOINT_EXPECTED,
                    })?;
            }
            "websocket_password" => {
                prerequisites.websocket_password = Some(
                    text_value(
                        "macos.websocket_password",
                        value,
                        WEBSOCKET_PASSWORD_EXPECTED,
                    )?
                    .to_owned(),
                );
            }
            "obs_log_directory" => {
                prerequisites.obs_log_directory = std::path::PathBuf::from(text_value(
                    "macos.obs_log_directory",
                    value,
                    OBS_LOG_DIRECTORY_EXPECTED,
                )?);
            }
            "launch_obs" => {
                prerequisites.launch_obs =
                    value.as_bool().ok_or_else(|| ConfigError::InvalidValue {
                        key: "macos.launch_obs",
                        value: value.to_string(),
                        expected: LAUNCH_OBS_EXPECTED,
                    })?;
            }
            other => {
                return Err(ConfigError::UnknownMacOsKey {
                    key: other.to_owned(),
                })
            }
        }
    }
    Ok(prerequisites)
}

#[cfg(target_os = "macos")]
fn text_value<'a>(
    key: &'static str,
    value: &'a toml::Value,
    expected: &'static str,
) -> Result<&'a str, ConfigError> {
    value.as_str().ok_or_else(|| ConfigError::InvalidValue {
        key,
        value: value.to_string(),
        expected,
    })
}

fn parse_fps(value: &toml::Value) -> Result<f64, ConfigError> {
    let fps = value
        .as_float()
        .or_else(|| value.as_integer().map(|fps| fps as f64))
        .filter(|fps| *fps > 0.0)
        .ok_or_else(|| ConfigError::InvalidValue {
            key: "fps",
            value: value.to_string(),
            expected: FPS_EXPECTED,
        })?;
    Ok(fps)
}

/// Parses a **requested size**, which is a preference and never a promise.
///
/// `1080p` is shorthand for the 16:9 frame size at that height. Nothing promises
/// the camera will produce it: the M50 accepts no size request.
#[must_use]
pub fn parse_requested_size(value: &str) -> Option<FrameSize> {
    let text = value.trim().to_ascii_lowercase();

    if let Some(height) = text.strip_suffix('p') {
        let height = height.parse::<u32>().ok()?;
        if height == 0 {
            return None;
        }
        // 16:9 at that height, rounded to an even width so a resized frame keeps
        // whole chroma pairs.
        let width = ((u64::from(height) * 16 + 4) / 9) as u32;
        let width = if width.is_multiple_of(2) {
            width
        } else {
            width + 1
        };
        return Some(FrameSize::new(width, height));
    }

    let (width, height) = text.split_once('x')?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(FrameSize::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::parse_requested_size;
    use crate::frame_size::FrameSize;

    #[test]
    fn the_shorthand_p_forms_are_16_by_9() {
        assert_eq!(
            parse_requested_size("1080p"),
            Some(FrameSize::new(1920, 1080))
        );
        assert_eq!(
            parse_requested_size("720p"),
            Some(FrameSize::new(1280, 720))
        );
        assert_eq!(
            parse_requested_size("576p"),
            Some(FrameSize::new(1024, 576))
        );
    }

    #[test]
    fn the_explicit_form_is_taken_as_written() {
        assert_eq!(
            parse_requested_size("1024x576"),
            Some(FrameSize::new(1024, 576))
        );
        assert_eq!(
            parse_requested_size(" 800X600 "),
            Some(FrameSize::new(800, 600))
        );
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        for value in ["", "big", "1080", "0p", "1920x", "x1080", "0x0", "-5p"] {
            assert_eq!(parse_requested_size(value), None, "value was {value:?}");
        }
    }
}
