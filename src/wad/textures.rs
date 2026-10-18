//! PLAYPAL / COLORMAP handling, picture decoding, composite wall textures and flats.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::log::{info, warn};
use bevy::prelude::{Image, Resource};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use byteorder::{ByteOrder, LittleEndian as LE, ReadBytesExt};

use super::loader::{lump_name, WadDirectory, WadError};

fn malformed(lump: &str, reason: &str) -> WadError {
    WadError::MalformedLump {
        lump: lump.to_string(),
        reason: reason.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Pixel buffers
// ---------------------------------------------------------------------------

/// Tightly packed RGBA8, row 0 at the top.
#[derive(Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height * 4) as usize],
        }
    }

    /// Upload-ready Bevy image. Nearest filtering for the pixelated DOOM look.
    pub fn to_bevy_image(&self, repeat: bool) -> Image {
        let mut image = Image::new(
            Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            self.data.clone(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        let mode = if repeat {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        };
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: mode,
            address_mode_v: mode,
            address_mode_w: mode,
            mag_filter: ImageFilterMode::Nearest,
            min_filter: ImageFilterMode::Nearest,
            mipmap_filter: ImageFilterMode::Nearest,
            ..Default::default()
        });
        image
    }
}

// ---------------------------------------------------------------------------
// Palette / colormap
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Palette(pub Vec<[u8; 3]>);

impl Palette {
    /// Uses the first of the 14 palettes in PLAYPAL.
    pub fn from_playpal(data: &[u8]) -> Result<Self, WadError> {
        if data.len() < 768 {
            return Err(malformed("PLAYPAL", "needs at least 768 bytes"));
        }
        Ok(Self(data[..768].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()))
    }

    fn luma(&self, i: u8) -> f32 {
        let c = self.0[i as usize];
        0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32
    }
}

/// COLORMAP: 32 light-level rows (+ invulnerability, + unused) of 256 palette remaps.
pub struct Colormap {
    pub rows: Vec<[u8; 256]>,
}

impl Colormap {
    pub fn from_lump(data: &[u8]) -> Result<Self, WadError> {
        if data.len() < 256 {
            return Err(malformed("COLORMAP", "shorter than one 256-byte row"));
        }
        let rows = data
            .chunks_exact(256)
            .take(34)
            .map(|c| {
                let mut row = [0u8; 256];
                row.copy_from_slice(c);
                row
            })
            .collect();
        Ok(Self { rows })
    }

    /// Average brightness retained by a given colormap row, 0..=1.
    fn row_brightness(&self, row: usize, pal: &Palette) -> f32 {
        let r = &self.rows[row.min(self.rows.len() - 1)];
        let (mut shaded, mut base) = (0.0, 0.0);
        for i in 0..=255u8 {
            shaded += pal.luma(r[i as usize]);
            base += pal.luma(i);
        }
        if base > 0.0 {
            (shaded / base).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

// ---------------------------------------------------------------------------
// Picture (patch / sprite) decoding
// ---------------------------------------------------------------------------

pub struct Picture {
    pub image: RgbaImage,
    /// Pixels from the origin to the left edge (sprites).
    pub left: i32,
    /// Pixels from the origin up to the top edge (sprites).
    pub top: i32,
}

/// Decode a DOOM "picture format" column-post lump.
pub fn decode_picture(name: &str, data: &[u8], pal: &Palette) -> Result<Picture, WadError> {
    if data.len() < 8 {
        return Err(malformed(name, "shorter than the 8-byte picture header"));
    }
    let w = LE::read_i16(&data[0..2]) as i32;
    let h = LE::read_i16(&data[2..4]) as i32;
    let left = LE::read_i16(&data[4..6]) as i32;
    let top = LE::read_i16(&data[6..8]) as i32;
    if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) {
        return Err(malformed(name, "implausible picture dimensions"));
    }
    let (w, h) = (w as usize, h as usize);
    if data.len() < 8 + w * 4 {
        return Err(malformed(name, "column offset table truncated"));
    }

    let mut img = RgbaImage::new(w as u32, h as u32);
    for x in 0..w {
        let mut pos = LE::read_u32(&data[8 + x * 4..]) as usize;
        loop {
            if pos + 1 >= data.len() {
                break;
            }
            let top_delta = data[pos] as usize;
            if top_delta == 0xFF {
                break;
            }
            let len = data[pos + 1] as usize;
            let start = pos + 3; // skip top_delta, length, pad byte
            let end = start + len;
            if end > data.len() {
                break;
            }
            for i in 0..len {
                let y = top_delta + i;
                if y >= h {
                    break;
                }
                let c = pal.0[data[start + i] as usize];
                let o = (y * w + x) * 4;
                img.data[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
            pos = end + 1; // skip trailing pad byte
        }
    }
    Ok(Picture {
        image: img,
        left,
        top,
    })
}

/// Raw 64x64 flat -> RGBA.
fn decode_flat(data: &[u8], pal: &Palette) -> RgbaImage {
    let mut img = RgbaImage::new(64, 64);
    for (i, &idx) in data.iter().take(4096).enumerate() {
        let c = pal.0[idx as usize];
        img.data[i * 4..i * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    }
    img
}

// ---------------------------------------------------------------------------
// TEXTURE1/2 + PNAMES
// ---------------------------------------------------------------------------

struct PatchRef {
    patch: usize,
    ox: i32,
    oy: i32,
}

struct TextureDef {
    name: String,
    width: i32,
    height: i32,
    patches: Vec<PatchRef>,
}

fn parse_pnames(data: &[u8]) -> Result<Vec<String>, WadError> {
    if data.len() < 4 {
        return Err(malformed("PNAMES", "truncated"));
    }
    let n = LE::read_i32(&data[0..4]);
    if n < 0 || 4 + n as usize * 8 > data.len() {
        return Err(malformed("PNAMES", "count exceeds lump size"));
    }
    Ok((0..n as usize)
        .map(|i| lump_name(&data[4 + i * 8..4 + i * 8 + 8]))
        .collect())
}

fn parse_one_texture(c: &mut Cursor<&[u8]>) -> std::io::Result<TextureDef> {
    let mut raw = [0u8; 8];
    c.read_exact(&mut raw)?;
    let _masked = c.read_i32::<LE>()?;
    let width = c.read_i16::<LE>()? as i32;
    let height = c.read_i16::<LE>()? as i32;
    let _column_directory = c.read_i32::<LE>()?;
    let count = c.read_i16::<LE>()?.max(0);
    let mut patches = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let ox = c.read_i16::<LE>()? as i32;
        let oy = c.read_i16::<LE>()? as i32;
        let patch = c.read_i16::<LE>()?;
        let _stepdir = c.read_i16::<LE>()?;
        let _colormap = c.read_i16::<LE>()?;
        if patch >= 0 {
            patches.push(PatchRef {
                patch: patch as usize,
                ox,
                oy,
            });
        }
    }
    Ok(TextureDef {
        name: lump_name(&raw),
        width,
        height,
        patches,
    })
}

fn parse_texture_lump(lump: &str, data: &[u8]) -> Result<Vec<TextureDef>, WadError> {
    if data.len() < 4 {
        return Err(malformed(lump, "truncated"));
    }
    let n = LE::read_i32(&data[0..4]);
    if n < 0 || 4 + n as usize * 4 > data.len() {
        return Err(malformed(lump, "texture count exceeds lump size"));
    }
    let mut defs = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let ofs = LE::read_i32(&data[4 + i * 4..]);
        let mut c = Cursor::new(data);
        c.set_position(ofs.max(0) as u64);
        match parse_one_texture(&mut c) {
            Ok(t) => defs.push(t),
            Err(_) => warn!("{lump}: texture #{i} at offset {ofs} is truncated; skipped"),
        }
    }
    Ok(defs)
}

fn compose_texture(
    def: &TextureDef,
    patch_names: &[String],
    wad: &WadDirectory,
    pal: &Palette,
    cache: &mut HashMap<usize, Option<Picture>>,
) -> RgbaImage {
    let mut img = RgbaImage::new(def.width as u32, def.height as u32);
    for p in &def.patches {
        let Some(pname) = patch_names.get(p.patch) else {
            warn!("texture {}: patch index {} out of PNAMES range", def.name, p.patch);
            continue;
        };
        let pic = cache.entry(p.patch).or_insert_with(|| {
            let Some(bytes) = wad.try_lump(pname) else {
                warn!("patch lump {pname} (used by {}) is missing", def.name);
                return None;
            };
            match decode_picture(pname, bytes, pal) {
                Ok(pic) => Some(pic),
                Err(e) => {
                    warn!("{e}");
                    None
                }
            }
        });
        let Some(pic) = pic else { continue };

        let (pw, ph) = (pic.image.width as i32, pic.image.height as i32);
        for sy in 0..ph {
            let dy = p.oy + sy;
            if dy < 0 || dy >= def.height {
                continue;
            }
            for sx in 0..pw {
                let dx = p.ox + sx;
                if dx < 0 || dx >= def.width {
                    continue;
                }
                let s = ((sy * pw + sx) * 4) as usize;
                if pic.image.data[s + 3] == 0 {
                    continue; // transparent post gap: keep what's underneath
                }
                let d = ((dy * def.width + dx) * 4) as usize;
                img.data[d..d + 4].copy_from_slice(&pic.image.data[s..s + 4]);
            }
        }
    }
    img
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

/// CPU-side decoded textures for the merged WAD stack.
#[derive(Resource)]
pub struct TextureLibrary {
    pub palette: Palette,
    pub colormap: Colormap,
    pub walls: HashMap<String, RgbaImage>,
    pub flats: HashMap<String, RgbaImage>,
    /// Retained brightness (0..=1) for sector light levels 0..=255, derived from COLORMAP.
    light_table: Vec<f32>,
}

impl TextureLibrary {
    pub fn load(wad: &WadDirectory) -> Result<Self, WadError> {
        let palette = Palette::from_playpal(wad.lump("PLAYPAL")?)?;
        let colormap = Colormap::from_lump(wad.lump("COLORMAP")?)?;
        let pnames = parse_pnames(wad.lump("PNAMES")?)?;

        let mut defs = parse_texture_lump("TEXTURE1", wad.lump("TEXTURE1")?)?;
        if let Some(t2) = wad.try_lump("TEXTURE2") {
            defs.extend(parse_texture_lump("TEXTURE2", t2)?);
        }

        let mut patch_cache = HashMap::new();
        let mut walls = HashMap::with_capacity(defs.len());
        for def in &defs {
            if !(1..=2048).contains(&def.width) || !(1..=2048).contains(&def.height) {
                warn!("texture {} has invalid size {}x{}; skipped", def.name, def.width, def.height);
                continue;
            }
            let img = compose_texture(def, &pnames, wad, &palette, &mut patch_cache);
            walls.insert(def.name.clone(), img); // later definitions override earlier
        }

        let mut flats = HashMap::new();
        for (name, data) in wad.flats() {
            flats.insert(name.to_string(), decode_flat(data, &palette));
        }

        let row_b: Vec<f32> = (0..32).map(|r| colormap.row_brightness(r, &palette)).collect();
        let light_table = (0..256usize)
            .map(|l| row_b[((255 - l) >> 3).min(31)])
            .collect();

        info!("decoded {} wall textures, {} flats", walls.len(), flats.len());
        Ok(Self {
            palette,
            colormap,
            walls,
            flats,
            light_table,
        })
    }

    pub fn wall_size(&self, name: &str) -> Option<(u32, u32)> {
        self.walls.get(name).map(|i| (i.width, i.height))
    }

    /// Sector light level -> linear RGBA vertex colour (used with an unlit material).
    pub fn light_color(&self, light: i16) -> [f32; 4] {
        let b = self.light_table[light.clamp(0, 255) as usize];
        // COLORMAP rows scale sRGB-ish palette values; convert to linear for vertex colour.
        let lin = b.powf(2.2).max(0.04);
        [lin, lin, lin, 1.0]
    }
}
