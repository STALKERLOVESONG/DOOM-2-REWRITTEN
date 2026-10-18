//! Doom content layer.
//!
//! - [`wad`]: WAD container parsing.
//! - [`resources`]: the layered resource stack (IWAD + PWADs, later PK3s),
//!   lump numbering, override and namespace semantics.
//! - [`palette`], [`picture`], [`flat`], [`texture`]: graphics lump formats,
//!   kept palette-indexed (the renderer needs indices for COLORMAP lighting).
//! - [`map`]: raw map lump records and map-format detection.
//! - [`game`]: content-based game mode identification.
//!
//! This crate has no knowledge of gameplay. It only answers "what bytes does
//! the content stack provide for X, and what do they decode to".

pub mod error;
pub mod flat;
pub mod game;
pub mod inflate;
pub mod map;
pub mod mapinfo;
pub mod name;
pub mod nodebuild;
pub mod nodes;
pub mod palette;
pub mod picture;
pub mod png;
pub mod resources;
pub mod script;
pub mod texture;
pub mod udmf;
pub mod wad;
pub mod zip;

pub use error::ContentError;
pub use name::LumpName;
pub use resources::{LumpId, Namespace, ResourceStack};

/// Little-endian readers over byte slices. Callers check lengths first; these
/// panic on short input, which indicates a bug in the caller's validation.
pub(crate) mod le {
    #[inline]
    pub fn u16(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    #[inline]
    pub fn i16(b: &[u8], o: usize) -> i16 {
        i16::from_le_bytes([b[o], b[o + 1]])
    }
    #[inline]
    pub fn u32(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
    #[inline]
    pub fn i32(b: &[u8], o: usize) -> i32 {
        i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
}
