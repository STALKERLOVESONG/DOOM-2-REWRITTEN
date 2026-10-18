//! Script runtimes for Doom mods.
//!
//! - [`acs`]: loads compiled ACS (`BEHAVIOR` lumps and libraries) and runs
//!   scripts in a virtual machine. Engine effects go through the
//!   [`acs::AcsHost`] trait, which the playsim implements, so scripts run
//!   inside the deterministic tic loop instead of beside it.
//!
//! This crate has no engine knowledge and no dependencies: it can be tested
//! with a fake host and reused by tools.

pub mod acs;
pub mod zscript;
