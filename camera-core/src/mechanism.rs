//! The **virtual camera mechanism**, and the platform that owns it.
//!
//! A mechanism belongs to its platform: naming one on a platform where it does
//! not exist is an error, never a fallback. `docs/adr/0002-macos-virtual-camera-mechanism.md`
//! chooses `obs-virtual-camera` for macOS and rejects a camera extension and a
//! DAL plugin of our own, so macOS has exactly one.
//!
//! The **platform backend** is fixed at build time and is not a user choice; the
//! mechanism is the one part of the output side a user configures, which is why
//! the configuration key is `mechanism` and not `backend`.

use std::fmt;
use std::str::FromStr;

/// The operating system a mechanism has to exist on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
    /// Linux.
    Linux,
}

impl Platform {
    /// The platform this binary was built for.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }

    /// The name used in messages and in this repository's documentation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The operating-system facility a **platform backend** drives so the camera
/// appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VirtualCameraMechanism {
    /// OBS Virtual Camera, driven through obs-websocket, with frames carried over
    /// a local WebSocket to a browser source. See ADR-0002.
    ObsVirtualCamera,
}

impl VirtualCameraMechanism {
    /// The mechanisms that exist on a platform.
    ///
    /// Windows and Linux are empty because those **platform backends** do not
    /// exist in this build: inventing a mechanism for them here would make the
    /// configuration lie.
    #[must_use]
    pub const fn supported_on(platform: Platform) -> &'static [Self] {
        match platform {
            Platform::MacOs => &[Self::ObsVirtualCamera],
            Platform::Windows | Platform::Linux => &[],
        }
    }

    /// The configuration key value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObsVirtualCamera => "obs-virtual-camera",
        }
    }
}

impl fmt::Display for VirtualCameraMechanism {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A mechanism was named that does not exist on the platform in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MechanismError {
    requested: String,
    platform: Platform,
}

impl MechanismError {
    /// The value as configured.
    #[must_use]
    pub fn requested(&self) -> &str {
        &self.requested
    }

    /// The platform it was refused on.
    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }
}

impl fmt::Display for MechanismError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let available = VirtualCameraMechanism::supported_on(self.platform);
        write!(
            formatter,
            "`mechanism` is `{}`, which does not exist on {}",
            self.requested, self.platform
        )?;
        if available.is_empty() {
            write!(
                formatter,
                ". This build has no virtual camera mechanism on {}, so the live view feed has \
                 nowhere to go; the {} platform backend is not implemented",
                self.platform, self.platform
            )
        } else {
            let names = available
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            write!(
                formatter,
                ". On {} the only virtual camera mechanism is {names}; a mechanism belongs to its \
                 platform, so this is a configuration error rather than a fallback",
                self.platform
            )
        }
    }
}

impl std::error::Error for MechanismError {}

impl VirtualCameraMechanism {
    /// Parses a configured mechanism name, refusing one the platform does not
    /// have.
    ///
    /// # Errors
    ///
    /// Returns [`MechanismError`] for a name that is not a mechanism of
    /// `platform`, including a name that exists on a different platform.
    pub fn parse_for(platform: Platform, value: &str) -> Result<Self, MechanismError> {
        let refused = || MechanismError {
            requested: value.to_owned(),
            platform,
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "obs-virtual-camera" if platform == Platform::MacOs => Ok(Self::ObsVirtualCamera),
            _ => Err(refused()),
        }
    }

    /// The mechanism a platform uses when the configuration does not name one.
    ///
    /// # Errors
    ///
    /// Returns [`MechanismError`] when the platform has no mechanism at all, so
    /// there is nothing to default to.
    pub fn default_for(platform: Platform) -> Result<Self, MechanismError> {
        Self::supported_on(platform)
            .first()
            .copied()
            .ok_or_else(|| MechanismError {
                requested: "the default".to_owned(),
                platform,
            })
    }
}

impl FromStr for VirtualCameraMechanism {
    type Err = MechanismError;

    /// Parses against the platform this binary was built for.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_for(Platform::current(), value)
    }
}

#[cfg(test)]
mod tests {
    use super::{Platform, VirtualCameraMechanism};

    #[test]
    fn macos_has_exactly_the_mechanism_its_adr_chose() {
        assert_eq!(
            VirtualCameraMechanism::supported_on(Platform::MacOs),
            &[VirtualCameraMechanism::ObsVirtualCamera]
        );
        assert_eq!(
            VirtualCameraMechanism::ObsVirtualCamera.as_str(),
            "obs-virtual-camera"
        );
    }

    #[test]
    fn obsws_is_refused_on_every_other_platform() {
        for platform in [Platform::Windows, Platform::Linux] {
            assert!(
                VirtualCameraMechanism::parse_for(platform, "obs-virtual-camera").is_err(),
                "{platform} must not accept a mechanism it does not have"
            );
            assert!(
                VirtualCameraMechanism::supported_on(platform).is_empty(),
                "{platform} has no platform backend in this build"
            );
        }
    }

    #[test]
    fn the_name_is_case_and_whitespace_insensitive() {
        assert_eq!(
            VirtualCameraMechanism::parse_for(Platform::MacOs, "  OBS-Virtual-Camera "),
            Ok(VirtualCameraMechanism::ObsVirtualCamera)
        );
    }

    #[test]
    fn a_refusal_names_the_mechanism_and_the_platform() {
        let error = VirtualCameraMechanism::parse_for(Platform::MacOs, "v4l2loopback")
            .expect_err("must be refused");

        assert_eq!(error.requested(), "v4l2loopback");
        assert_eq!(error.platform(), Platform::MacOs);
        let message = error.to_string();
        assert!(message.contains("`mechanism`"), "message was {message}");
        assert!(message.contains("v4l2loopback"), "message was {message}");
        assert!(message.contains("macOS"), "message was {message}");
        assert!(
            message.contains("obs-virtual-camera"),
            "message was {message}"
        );
    }
}
