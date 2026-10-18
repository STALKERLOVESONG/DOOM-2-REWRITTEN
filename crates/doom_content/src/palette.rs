//! PLAYPAL and COLORMAP.

use crate::ContentError;

pub type Rgb = [u8; 3];

/// All palettes from PLAYPAL. Vanilla Doom has 14: 0 normal, 1-8 red
/// (damage/berserk), 9-12 gold (pickup), 13 green (radiation suit).
#[derive(Clone, Debug)]
pub struct Playpal {
    pub palettes: Vec<[Rgb; 256]>,
}

impl Playpal {
    pub fn parse(data: &[u8]) -> Result<Self, ContentError> {
        if data.len() < 768 {
            return Err(ContentError::malformed("PLAYPAL", "needs at least one 768-byte palette"));
        }
        let palettes = data
            .chunks_exact(768)
            .map(|p| {
                let mut out = [[0u8; 3]; 256];
                for (i, c) in p.chunks_exact(3).enumerate() {
                    out[i] = [c[0], c[1], c[2]];
                }
                out
            })
            .collect();
        Ok(Self { palettes })
    }

    pub fn base(&self) -> &[Rgb; 256] {
        &self.palettes[0]
    }

    /// Palette by index, clamped to the ones present (PWADs may ship fewer).
    pub fn get(&self, i: usize) -> &[Rgb; 256] {
        &self.palettes[i.min(self.palettes.len() - 1)]
    }
}

/// Light-level remap tables. Vanilla COLORMAP has 34 rows of 256 bytes:
/// 0..31 light levels (0 = brightest), 32 invulnerability, 33 all-black.
#[derive(Clone, Debug)]
pub struct Colormaps {
    pub maps: Vec<[u8; 256]>,
}

/// Number of light-level colormaps (`NUMCOLORMAPS`).
pub const NUMCOLORMAPS: usize = 32;
/// Index of the invulnerability colormap (`INVERSECOLORMAP`).
pub const INVERSECOLORMAP: usize = 32;

impl Colormaps {
    pub fn parse(lump: &str, data: &[u8]) -> Result<Self, ContentError> {
        if data.len() < 256 * NUMCOLORMAPS {
            return Err(ContentError::malformed(lump, format!("{} bytes; need at least {}", data.len(), 256 * NUMCOLORMAPS)));
        }
        let maps = data
            .chunks_exact(256)
            .map(|c| {
                let mut row = [0u8; 256];
                row.copy_from_slice(c);
                row
            })
            .collect();
        Ok(Self { maps })
    }
}
