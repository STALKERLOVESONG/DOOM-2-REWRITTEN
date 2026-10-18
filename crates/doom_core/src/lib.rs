//! Bit-exact Doom numerics.
//!
//! Everything here reproduces the integer behaviour of vanilla Doom (DOS v1.9,
//! as preserved by Chocolate Doom). Gameplay code must use these types rather
//! than floats: demo sync and many movement/collision quirks depend on exact
//! truncation, wrap-around and table values.
//!
//! Overflow policy: the original C relies on two's-complement wrap-around of
//! `int` on x86 (technically UB in C, but it is what the shipped executable
//! did). All arithmetic here therefore wraps instead of panicking.

pub mod angle;
pub mod bbox;
pub mod fixed;
pub mod geom;
pub mod random;
pub mod tables;
mod tables_data;

pub use angle::Angle;
pub use bbox::BBox;
pub use fixed::{Fixed, FRACBITS, FRACUNIT};
pub use random::DoomRandom;
