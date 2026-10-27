//! The **output setting**: the Canon camera property named `output`.
//!
//! The property selects which displays receive live view, and it changes the
//! **frame size**. This module is a guard, and it is an allowlist rather than a
//! blocklist: a value nobody has verified is refused instead of being passed
//! through.
//!
//! # What is verified on an M50
//!
//! Evidence: `TEST-RESULTS.md` §4.5, measured on an M50 with firmware `3-1.1.0`.
//!
//! - `3` (`TFT + PC`) gives the full frame size, `1024x576` with the viewfinder
//!   action pressed. **Verified.**
//! - `8` (`MOBILE2`) gives the reduced frame size, `512x288`. **Verified**, and
//!   still refused here, because it is the reduced size.
//!
//! # What is not verified
//!
//! - `0`, `1` and `2` (the Canon menu labels `Off`, `TFT` and `PC`) are in the
//!   allowlist because the report above groups them with `TFT + PC` as large
//!   frame sizes, but no capture in this repository separates them. Their label
//!   order for `0..=2` is the camera menu order and is **unverified here (no
//!   camera attached)**.
//! - `6`, `7`, `10` and `11` combine `PC` and `MOBILE`. A report from a
//!   **different Canon body** says these cause an error that needs the battery
//!   removed to clear. They are refused.
//! - Everything else is unverified and refused.
//!
//! # Persistence across process boundaries
//!
//! Not proven: the captures that verified `3` also read the value back inside the
//! same session, and nothing here observes the value at the start of a later
//! run. The guard therefore re-applies the setting on every run.
//!
//! The separate property `liveviewsize` is **not** used and must not be: on Canon
//! it accepts the write and changes nothing, so reading it would produce a
//! confidently wrong answer. See `docs/adr/0004-no-live-view-size-selection.md`.

use std::error;
use std::fmt;

/// A verified-or-allowlisted value of the Canon `output` property.
///
/// The inner value is private, so the only way to obtain one is
/// [`OutputSetting::from_raw`] or [`OutputSetting::tft_plus_pc`]. That is what
/// makes the allowlist unbypassable: a [`crate::session::PtpSession`] cannot be
/// asked to set a value outside the allowlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputSetting(u8);

impl OutputSetting {
    /// The allowlist. Values outside it are refused.
    pub const ALLOWED_RAW_VALUES: [u8; 4] = [0, 1, 2, 3];

    /// The full size setting: `TFT + PC`, value `3`.
    #[must_use]
    pub const fn tft_plus_pc() -> Self {
        Self(3)
    }

    /// Returns the setting for an allowlisted raw value.
    ///
    /// # Errors
    ///
    /// Returns [`OutputSettingError`] for every value outside
    /// [`OutputSetting::ALLOWED_RAW_VALUES`].
    pub fn from_raw(raw: u8) -> Result<Self, OutputSettingError> {
        if Self::ALLOWED_RAW_VALUES.contains(&raw) {
            Ok(Self(raw))
        } else {
            Err(OutputSettingError::new(raw))
        }
    }

    /// The raw value the camera expects.
    #[must_use]
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// Whether this value's effect on the **frame size** is verified on an M50.
    ///
    /// Only `3` is. Value `8` is also verified but is not representable here.
    #[must_use]
    pub const fn is_verified_on_m50(self) -> bool {
        self.0 == 3
    }

    /// The choice label the camera's `output` property uses for this value.
    ///
    /// The label for `3` is verified. The labels for `0..=2` follow the camera
    /// menu order and are unverified here (no camera attached).
    #[must_use]
    pub const fn choice_name(self) -> &'static str {
        match self.0 {
            0 => "Off",
            1 => "TFT",
            2 => "PC",
            _ => "TFT + PC",
        }
    }
}

impl fmt::Display for OutputSetting {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.choice_name(), self.0)
    }
}

/// The refusal of a raw `output` value that is not in the allowlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputSettingError {
    raw: u8,
}

impl OutputSettingError {
    #[must_use]
    pub const fn new(raw: u8) -> Self {
        Self { raw }
    }

    /// The value that was refused.
    #[must_use]
    pub const fn raw(self) -> u8 {
        self.raw
    }
}

impl fmt::Display for OutputSettingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "`output` value {} is not allowlisted. The allowlist is 0 (Off), 1 (TFT), 2 (PC) and \
             3 (TFT + PC). Verified on an M50: 3 (TFT + PC) gives the full frame size, and 8 \
             (MOBILE2) gives the reduced frame size and is therefore refused even though it is \
             verified. Values 6, 7, 10 and 11 combine PC and MOBILE; a report from a different \
             Canon body says they need the battery removed to clear the error. Every other value \
             is unverified here (no camera attached).",
            self.raw
        )
    }
}

impl error::Error for OutputSettingError {}
