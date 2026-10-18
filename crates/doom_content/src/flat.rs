//! Flats: raw 64x64 palette-indexed floor/ceiling images.

use crate::picture::IndexedImage;
use crate::ContentError;

pub const FLAT_SIZE: usize = 64;

/// Decode a flat. Vanilla reads exactly 4096 bytes and ignores the rest.
/// Larger square flats (128², 256² — a ZDoom extension) are recognised by size.
pub fn parse_flat(name: &str, data: &[u8]) -> Result<IndexedImage, ContentError> {
    let side = match data.len() {
        16384 => 128,
        65536 => 256,
        n if n >= 4096 => 64,
        n => return Err(ContentError::malformed(name, format!("flat is {n} bytes; expected at least 4096"))),
    };
    let n = side * side;
    Ok(IndexedImage {
        width: side as u32,
        height: side as u32,
        pixels: data[..n].to_vec(),
        opaque: vec![true; n],
    })
}
