//! Command line interface for the Canon EOS M50 virtual camera.
//!
//! The binary is a thin shell: parsing ([`args`]) and reporting ([`report`]) live
//! here so they can be tested without a camera.

pub mod args;
pub mod report;
pub mod sink;
