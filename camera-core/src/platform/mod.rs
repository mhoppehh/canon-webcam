//! The **platform backend** that exists in this build.
//!
//! A backend is fixed at build time, one per target, so this module has exactly
//! one child and it is selected by `cfg` rather than by configuration. Windows
//! and Linux children do not exist yet; until they do, naming their mechanisms
//! is a configuration error, which `crate::mechanism` already reports.

#[cfg(target_os = "macos")]
pub mod macos;
