//! Doom "picture" format (patches, sprites, menu graphics): a header plus one
//! list of opaque runs ("posts") per column.

use crate::le;
use crate::ContentError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Post {
    /// First row covered by this post.
    pub top: i32,
    /// Index of the first pixel in [`Picture::pixels`].
    pub start: u32,
    pub len: u32,
}

/// A decoded picture, kept in column/post form because the renderer draws it
/// that way (and transparency is defined by the absence of posts).
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: i32,
    pub height: i32,
    /// `leftoffset`: pixels from the left edge to the origin (sprites: the
    /// thing's position; HUD graphics: the anchor).
    pub left_offset: i32,
    /// `topoffset`: pixels from the top edge up to the origin.
    pub top_offset: i32,
    /// Per column: range into `posts`.
    pub columns: Vec<(u32, u32)>,
    pub posts: Vec<Post>,
    /// Palette indices for all posts, concatenated.
    pub pixels: Vec<u8>,
}

impl Picture {
    /// Decode a picture lump.
    ///
    /// "Tall patches" (DeePsea extension) are supported: a post whose
    /// `topdelta` is not greater than the previous one is taken as relative.
    /// For any picture vanilla can represent (`topdelta` strictly increasing)
    /// this is identical to the vanilla interpretation.
    pub fn parse(name: &str, data: &[u8]) -> Result<Self, ContentError> {
        let bad = |r: &str| ContentError::malformed(name, r);
        if data.len() < 8 {
            return Err(bad("shorter than the 8-byte picture header"));
        }
        let width = le::i16(data, 0) as i32;
        let height = le::i16(data, 2) as i32;
        let left_offset = le::i16(data, 4) as i32;
        let top_offset = le::i16(data, 6) as i32;
        if width <= 0 || height < 0 {
            return Err(bad("non-positive dimensions"));
        }
        if data.len() < 8 + width as usize * 4 {
            return Err(bad("column offset table truncated"));
        }

        let mut columns = Vec::with_capacity(width as usize);
        let mut posts = Vec::new();
        let mut pixels = Vec::new();
        for x in 0..width as usize {
            let first = posts.len() as u32;
            let mut pos = le::u32(data, 8 + x * 4) as usize;
            let mut top = -1i32;
            loop {
                let Some(&delta) = data.get(pos) else {
                    return Err(bad(&format!("column {x} runs past the end of the lump")));
                };
                if delta == 0xFF {
                    break;
                }
                let Some(&len) = data.get(pos + 1) else {
                    return Err(bad(&format!("column {x}: truncated post header")));
                };
                let delta = delta as i32;
                top = if delta <= top { top + delta } else { delta };
                let start = pos + 3; // topdelta, length, unused pad byte
                let end = start + len as usize;
                if end > data.len() {
                    return Err(bad(&format!("column {x}: post data truncated")));
                }
                posts.push(Post { top, start: pixels.len() as u32, len: len as u32 });
                pixels.extend_from_slice(&data[start..end]);
                pos = end + 1; // trailing pad byte
            }
            columns.push((first, posts.len() as u32 - first));
        }
        Ok(Self { width, height, left_offset, top_offset, columns, posts, pixels })
    }

    /// Build a picture from an RGBA image: pixels with alpha >= 128 are
    /// opaque and mapped to the nearest palette colour.
    pub fn from_rgba(img: &crate::png::RgbaImage, palette: &crate::png::Palette) -> Picture {
        let (w, h) = (img.width as i32, img.height as i32);
        let (left_offset, top_offset) = img.offsets.unwrap_or((0, 0));
        let mut columns = Vec::with_capacity(w as usize);
        let mut posts = Vec::new();
        let mut pixels = Vec::new();
        for x in 0..w as usize {
            let first = posts.len() as u32;
            let mut y = 0usize;
            while y < h as usize {
                let px = |y: usize| &img.pixels[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4];
                if px(y)[3] < 128 {
                    y += 1;
                    continue;
                }
                let top = y;
                let start = pixels.len() as u32;
                while y < h as usize && px(y)[3] >= 128 {
                    let c = px(y);
                    pixels.push(palette.nearest([c[0], c[1], c[2]]));
                    y += 1;
                }
                posts.push(Post { top: top as i32, start, len: (y - top) as u32 });
            }
            columns.push((first, posts.len() as u32 - first));
        }
        Picture { width: w, height: h, left_offset, top_offset, columns, posts, pixels }
    }

    /// Encode in the Doom picture format, so code that reads raw patch
    /// columns (the vanilla renderer) works on converted images too. Posts
    /// longer than 255 pixels or starting below row 254 use the DeePsea
    /// "tall patch" convention (a delta not above the previous post's top
    /// is relative to it), padded with empty posts where needed.
    pub fn to_lump(&self) -> Vec<u8> {
        let w = self.width.max(1) as usize;
        let mut out = Vec::with_capacity(8 + w * 4 + self.pixels.len() + self.posts.len() * 4);
        for v in [self.width.clamp(0, 32767), self.height.clamp(0, 32767), self.left_offset, self.top_offset] {
            out.extend_from_slice(&(v.clamp(-32768, 32767) as i16).to_le_bytes());
        }
        let table = out.len();
        out.resize(table + w * 4, 0);
        for x in 0..w {
            let ofs = out.len() as u32;
            out[table + x * 4..table + x * 4 + 4].copy_from_slice(&ofs.to_le_bytes());
            let mut last_top: i32 = -1;
            let emit_header = |out: &mut Vec<u8>, last_top: &mut i32, target: i32| {
                // Step until `target` can be expressed, then return the byte.
                loop {
                    if target <= 254 && target > *last_top {
                        *last_top = target;
                        return target as u8;
                    }
                    let d = target - *last_top;
                    if d >= 1 && d <= (*last_top).min(254) {
                        *last_top = target;
                        return d as u8;
                    }
                    // Empty post to move closer.
                    if *last_top < 254 {
                        out.extend_from_slice(&[254, 0, 0, 0]);
                        *last_top = 254;
                    } else {
                        let s = (*last_top).min(254);
                        out.extend_from_slice(&[s as u8, 0, 0, 0]);
                        *last_top += s;
                    }
                }
            };
            if x < self.columns.len() {
                for p in self.column(x) {
                    let px = self.post_pixels(p);
                    for (k, chunk) in px.chunks(255).enumerate() {
                        let top = p.top + (k * 255) as i32;
                        let delta = emit_header(&mut out, &mut last_top, top);
                        out.push(delta);
                        out.push(chunk.len() as u8);
                        out.push(chunk[0]);
                        out.extend_from_slice(chunk);
                        out.push(*chunk.last().unwrap());
                    }
                }
            }
            out.push(0xff);
        }
        out
    }

    pub fn column(&self, x: usize) -> &[Post] {
        let (s, n) = self.columns[x];
        &self.posts[s as usize..(s + n) as usize]
    }

    pub fn post_pixels(&self, p: &Post) -> &[u8] {
        &self.pixels[p.start as usize..(p.start + p.len) as usize]
    }

    /// Rasterise into a row-major indexed image with an opacity mask.
    /// Posts that extend past `height` are clipped.
    pub fn to_indexed(&self) -> IndexedImage {
        let mut img = IndexedImage::new(self.width as u32, self.height as u32);
        for x in 0..self.width as usize {
            for p in self.column(x) {
                for (i, &c) in self.post_pixels(p).iter().enumerate() {
                    let y = p.top + i as i32;
                    if (0..self.height).contains(&y) {
                        img.set(x as u32, y as u32, c);
                    }
                }
            }
        }
        img
    }
}

/// Row-major palette-indexed image with a per-pixel opacity mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub opaque: Vec<bool>,
}

impl IndexedImage {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        Self { width, height, pixels: vec![0; n], opaque: vec![false; n] }
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, c: u8) {
        let i = (y * self.width + x) as usize;
        self.pixels[i] = c;
        self.opaque[i] = true;
    }

    /// Expand to RGBA8 (transparent pixels become `[0,0,0,0]`).
    pub fn to_rgba(&self, palette: &[[u8; 3]; 256]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for (&c, &o) in self.pixels.iter().zip(&self.opaque) {
            if o {
                let p = palette[c as usize];
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            } else {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_column(posts: &[(u8, &[u8])], height: i16) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&1i16.to_le_bytes());
        d.extend_from_slice(&height.to_le_bytes());
        d.extend_from_slice(&0i16.to_le_bytes());
        d.extend_from_slice(&0i16.to_le_bytes());
        d.extend_from_slice(&12u32.to_le_bytes());
        for (top, px) in posts {
            d.push(*top);
            d.push(px.len() as u8);
            d.push(0);
            d.extend_from_slice(px);
            d.push(0);
        }
        d.push(0xFF);
        d
    }

    #[test]
    fn decodes_posts() {
        let p = Picture::parse("T", &one_column(&[(0, &[1, 2]), (5, &[3])], 8)).unwrap();
        assert_eq!(p.column(0).len(), 2);
        let img = p.to_indexed();
        assert_eq!(&img.pixels[..2], &[1, 2]);
        assert!(!img.opaque[2]);
        assert!(img.opaque[5]);
    }

    #[test]
    fn rgba_round_trip_with_tall_columns() {
        let mut colors = [[0u8; 3]; 256];
        for (i, c) in colors.iter_mut().enumerate() {
            *c = [i as u8, (255 - i) as u8, (i * 7) as u8];
        }
        let pal = crate::png::Palette::new(colors);
        let (w, h) = (3u32, 700u32);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                // Column 0 fully opaque (long runs), 1 sparse, 2 empty.
                let opaque = x == 0 || (x == 1 && (y % 97 < 3 || y > 600));
                if opaque {
                    let c = colors[((x * 31 + y) % 256) as usize];
                    let i = ((y * w + x) * 4) as usize;
                    pixels[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        }
        let img = crate::png::RgbaImage { width: w, height: h, pixels: pixels.clone(), offsets: Some((5, -7)) };
        let pic = Picture::from_rgba(&img, &pal);
        let back = Picture::parse("T", &pic.to_lump()).unwrap();
        assert_eq!((back.left_offset, back.top_offset), (5, -7));
        let a = pic.to_indexed();
        let b = back.to_indexed();
        assert_eq!(a, b);
        assert!(a.opaque[(699 * w) as usize] && !a.opaque[2]);
    }

    #[test]
    fn tall_patch_relative_delta() {
        // Second post's delta (100) is <= previous top (200) -> relative: 300.
        let p = Picture::parse("T", &one_column(&[(200, &[1]), (100, &[2])], 400)).unwrap();
        assert_eq!(p.column(0)[1].top, 300);
    }
}
