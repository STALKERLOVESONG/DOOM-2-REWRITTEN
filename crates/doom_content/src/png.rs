//! PNG decoding (all standard color types and bit depths, Adam7) to RGBA8,
//! plus ZDoom's `grAb` chunk (sprite/patch offsets).
//!
//! Modern mods store most graphics as PNG. The renderer works in palette
//! indices, so callers convert the RGBA result with [`Palette::nearest`].

use crate::inflate::zlib_decompress;

pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

pub fn is_png(data: &[u8]) -> bool {
    data.starts_with(&SIGNATURE)
}

#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8.
    pub pixels: Vec<u8>,
    /// `grAb` offsets (x, y): the same meaning as a Doom patch's
    /// left/top offsets.
    pub offsets: Option<(i32, i32)>,
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Size and `grAb` offsets of a PNG, read from chunk headers only (no
/// decompression): enough to lay out sprites without decoding them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PngHeader {
    pub width: u32,
    pub height: u32,
    pub offsets: Option<(i32, i32)>,
}

pub fn header(data: &[u8]) -> Option<PngHeader> {
    if !is_png(data) {
        return None;
    }
    let mut pos = 8;
    let mut h = PngHeader { width: 0, height: 0, offsets: None };
    while pos + 8 <= data.len() {
        let len = be32(data, pos) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data.get(pos + 8..pos + 8 + len)?;
        match kind {
            b"IHDR" if len >= 13 => (h.width, h.height) = (be32(body, 0), be32(body, 4)),
            b"grAb" if len >= 8 => h.offsets = Some((be32(body, 0) as i32, be32(body, 4) as i32)),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    (h.width > 0 && h.height > 0).then_some(h)
}

pub fn decode(data: &[u8]) -> Result<RgbaImage, String> {
    if !is_png(data) {
        return Err("not a PNG".into());
    }
    let mut pos = 8;
    let (mut width, mut height, mut depth, mut color, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut trns: Option<Vec<u8>> = None;
    let mut idat = Vec::new();
    let mut offsets = None;
    while pos + 8 <= data.len() {
        let len = be32(data, pos) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data.get(pos + 8..pos + 8 + len).ok_or("chunk past end of file")?;
        match kind {
            b"IHDR" => {
                if len < 13 {
                    return Err("short IHDR".into());
                }
                width = be32(body, 0);
                height = be32(body, 4);
                depth = body[8];
                color = body[9];
                interlace = body[12];
                if body[10] != 0 || body[11] != 0 {
                    return Err("unknown compression or filter method".into());
                }
            }
            b"PLTE" => palette = body.chunks_exact(3).map(|c| [c[0], c[1], c[2], 255]).collect(),
            b"tRNS" => trns = Some(body.to_vec()),
            b"IDAT" => idat.extend_from_slice(body),
            b"grAb" if len >= 8 => offsets = Some((be32(body, 0) as i32, be32(body, 4) as i32)),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(format!("bad dimensions {width}x{height}"));
    }
    let channels = match color {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err(format!("bad color type {color}")),
    };
    if !matches!(depth, 1 | 2 | 4 | 8 | 16) || (depth > 8 && color == 3) || (depth < 8 && !matches!(color, 0 | 3)) {
        return Err(format!("bad bit depth {depth} for color type {color}"));
    }
    if let (3, Some(t)) = (color, &trns) {
        for (p, &a) in palette.iter_mut().zip(t) {
            p[3] = a;
        }
    }
    let bits_pp = depth as usize * channels;
    let raw = zlib_decompress(&idat, 0).map_err(|e| e.to_string())?;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];

    // (x0, y0, dx, dy) per pass.
    let passes: &[(usize, usize, usize, usize)] = if interlace == 1 {
        &[(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    } else {
        &[(0, 0, 1, 1)]
    };
    let (w, h) = (width as usize, height as usize);
    let mut off = 0;
    for &(x0, y0, dx, dy) in passes {
        if x0 >= w || y0 >= h {
            continue;
        }
        let pw = (w - x0).div_ceil(dx);
        let ph = (h - y0).div_ceil(dy);
        let stride = (pw * bits_pp).div_ceil(8);
        let bpp = bits_pp.div_ceil(8).max(1);
        let mut prev = vec![0u8; stride];
        let mut cur = vec![0u8; stride];
        for row in 0..ph {
            let filter = *raw.get(off).ok_or("image data truncated")?;
            let line = raw.get(off + 1..off + 1 + stride).ok_or("image data truncated")?;
            off += 1 + stride;
            cur.copy_from_slice(line);
            unfilter(filter, &mut cur, &prev, bpp)?;
            for col in 0..pw {
                let rgba = sample(&cur, col, depth, color, &palette, trns.as_deref());
                let (x, y) = (x0 + col * dx, y0 + row * dy);
                pixels[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&rgba);
            }
            std::mem::swap(&mut prev, &mut cur);
        }
    }
    Ok(RgbaImage { width, height, pixels, offsets })
}

fn unfilter(filter: u8, cur: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), String> {
    match filter {
        0 => {}
        1 => {
            for i in bpp..cur.len() {
                cur[i] = cur[i].wrapping_add(cur[i - bpp]);
            }
        }
        2 => {
            for i in 0..cur.len() {
                cur[i] = cur[i].wrapping_add(prev[i]);
            }
        }
        3 => {
            for i in 0..cur.len() {
                let left = if i >= bpp { cur[i - bpp] as u16 } else { 0 };
                cur[i] = cur[i].wrapping_add(((left + prev[i] as u16) / 2) as u8);
            }
        }
        4 => {
            for i in 0..cur.len() {
                let a = if i >= bpp { cur[i - bpp] as i16 } else { 0 };
                let b = prev[i] as i16;
                let c = if i >= bpp { prev[i - bpp] as i16 } else { 0 };
                let p = a + b - c;
                let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                let pred = if pa <= pb && pa <= pc {
                    a
                } else if pb <= pc {
                    b
                } else {
                    c
                };
                cur[i] = cur[i].wrapping_add(pred as u8);
            }
        }
        _ => return Err(format!("bad filter type {filter}")),
    }
    Ok(())
}

fn sample(line: &[u8], col: usize, depth: u8, color: u8, palette: &[[u8; 4]], trns: Option<&[u8]>) -> [u8; 4] {
    let get = |i: usize| -> u16 {
        // i-th sample (channel value) of the line.
        match depth {
            8 => line[i] as u16,
            16 => u16::from_be_bytes([line[2 * i], line[2 * i + 1]]),
            d => {
                let bit = i * d as usize;
                let byte = line[bit / 8];
                let shift = 8 - d as usize - bit % 8;
                ((byte >> shift) & ((1u8 << d) - 1)) as u16
            }
        }
    };
    let to8 = |v: u16| -> u8 {
        match depth {
            16 => (v >> 8) as u8,
            8 => v as u8,
            d => (v * 255 / ((1u16 << d) - 1)) as u8,
        }
    };
    match color {
        0 => {
            let v = get(col);
            let g = to8(v);
            let transparent = trns.is_some_and(|t| t.len() >= 2 && u16::from_be_bytes([t[0], t[1]]) == v);
            [g, g, g, if transparent { 0 } else { 255 }]
        }
        2 => {
            let (r, g, b) = (get(col * 3), get(col * 3 + 1), get(col * 3 + 2));
            let transparent = trns.is_some_and(|t| {
                t.len() >= 6
                    && u16::from_be_bytes([t[0], t[1]]) == r
                    && u16::from_be_bytes([t[2], t[3]]) == g
                    && u16::from_be_bytes([t[4], t[5]]) == b
            });
            [to8(r), to8(g), to8(b), if transparent { 0 } else { 255 }]
        }
        3 => palette.get(get(col) as usize).copied().unwrap_or([0, 0, 0, 255]),
        4 => {
            let g = to8(get(col * 2));
            [g, g, g, to8(get(col * 2 + 1))]
        }
        _ => [to8(get(col * 4)), to8(get(col * 4 + 1)), to8(get(col * 4 + 2)), to8(get(col * 4 + 3))],
    }
}

/// A 256-colour palette with fast nearest-colour lookup.
pub struct Palette {
    pub colors: [[u8; 3]; 256],
    exact: std::collections::HashMap<[u8; 3], u8>,
    /// Nearest index for every 15-bit colour.
    table: Vec<u8>,
}

impl Palette {
    pub fn new(colors: [[u8; 3]; 256]) -> Palette {
        let mut exact = std::collections::HashMap::new();
        for (i, c) in colors.iter().enumerate() {
            exact.entry(*c).or_insert(i as u8);
        }
        let mut table = vec![0u8; 32768];
        for (k, t) in table.iter_mut().enumerate() {
            let (r, g, b) = (((k >> 10) & 31) << 3 | 4, ((k >> 5) & 31) << 3 | 4, (k & 31) << 3 | 4);
            *t = Self::search(&colors, [r as u8, g as u8, b as u8]);
        }
        Palette { colors, exact, table }
    }

    fn search(colors: &[[u8; 3]; 256], c: [u8; 3]) -> u8 {
        let mut best = (u32::MAX, 0u8);
        for (i, p) in colors.iter().enumerate() {
            let d: u32 = (0..3).map(|k| (p[k] as i32 - c[k] as i32).pow(2) as u32).sum();
            if d < best.0 {
                best = (d, i as u8);
            }
        }
        best.1
    }

    /// The palette index closest to an RGB colour.
    pub fn nearest(&self, c: [u8; 3]) -> u8 {
        if let Some(&i) = self.exact.get(&c) {
            return i;
        }
        self.table[((c[0] as usize >> 3) << 10) | ((c[1] as usize >> 3) << 5) | (c[2] as usize >> 3)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_python_pngs() {
        // Fixture: PNGs written by tools in tests/fixtures/png_cases.bin
        // (len, png, expected RGBA) for each color type / depth / interlace.
        let d = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/png_cases.bin")).unwrap();
        let mut pos = 0;
        let mut n = 0;
        while pos < d.len() {
            let len = u32::from_le_bytes(d[pos..pos + 4].try_into().unwrap()) as usize;
            let png = &d[pos + 4..pos + 4 + len];
            pos += 4 + len;
            let img = decode(png).unwrap_or_else(|e| panic!("case {n}: {e}"));
            let rlen = (img.width * img.height * 4) as usize;
            assert_eq!(img.pixels, &d[pos..pos + rlen], "case {n}");
            pos += rlen;
            n += 1;
        }
        assert!(n >= 10);
    }
}
