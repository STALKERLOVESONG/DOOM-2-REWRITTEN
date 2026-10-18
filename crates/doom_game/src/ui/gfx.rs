//! Screen-space drawing on the 320x200 palette-indexed framebuffer
//! (v_video.c): patches from the WAD, tiled flats, and the HUD font.

use std::collections::HashMap;
use std::sync::Arc;

use doom_content::picture::Picture;
use doom_content::{Namespace, ResourceStack};

pub const SCREENWIDTH: usize = 320;
pub const SCREENHEIGHT: usize = 200;

/// Decoded patches by lump name. Missing lumps are remembered as `None`.
#[derive(Default)]
pub struct Patches {
    cache: HashMap<String, Option<Arc<Picture>>>,
}

impl Patches {
    pub fn get(&mut self, stack: &ResourceStack, name: &str) -> Option<Arc<Picture>> {
        let key = name.to_ascii_uppercase();
        if let Some(p) = self.cache.get(&key) {
            return p.clone();
        }
        let pic = stack.check_num_for_name(&key).and_then(|id| Picture::parse(&key, stack.graphic(id)).ok()).map(Arc::new);
        self.cache.insert(key, pic.clone());
        pic
    }

    /// A patch that must exist for the UI to make sense; a 1x1 placeholder
    /// keeps a broken PWAD from crashing the menu.
    pub fn need(&mut self, stack: &ResourceStack, name: &str) -> Arc<Picture> {
        self.get(stack, name).unwrap_or_else(|| {
            Arc::new(Picture { width: 1, height: 1, left_offset: 0, top_offset: 0, columns: vec![(0, 0)], posts: vec![], pixels: vec![] })
        })
    }
}

/// `V_DrawPatch`: draw `pic` with its origin offsets at (x, y), clipped to
/// the screen (vanilla errors out instead of clipping).
pub fn draw_patch(screen: &mut [u8], x: i32, y: i32, pic: &Picture) {
    draw_patch_ex(screen, x, y, pic, false, None);
}

/// `V_DrawPatchFlipped`.
pub fn draw_patch_flipped(screen: &mut [u8], x: i32, y: i32, pic: &Picture) {
    draw_patch_ex(screen, x, y, pic, true, None);
}

/// Draw with an optional colour translation table (256 entries).
pub fn draw_patch_ex(screen: &mut [u8], x: i32, y: i32, pic: &Picture, flip: bool, translation: Option<&[u8]>) {
    let x0 = x - pic.left_offset;
    let y0 = y - pic.top_offset;
    for col in 0..pic.width {
        let sx = if flip { x0 + pic.width - 1 - col } else { x0 + col };
        if sx < 0 || sx >= SCREENWIDTH as i32 {
            continue;
        }
        for post in pic.column(col as usize) {
            let pixels = pic.post_pixels(post);
            for (i, &c) in pixels.iter().enumerate() {
                let sy = y0 + post.top + i as i32;
                if sy < 0 || sy >= SCREENHEIGHT as i32 {
                    continue;
                }
                let c = translation.map_or(c, |t| t[c as usize]);
                screen[sy as usize * SCREENWIDTH + sx as usize] = c;
            }
        }
    }
}

/// Fill a rectangle with a solid colour.
pub fn fill_rect(screen: &mut [u8], x: i32, y: i32, w: i32, h: i32, color: u8) {
    for sy in y.max(0)..(y + h).min(SCREENHEIGHT as i32) {
        for sx in x.max(0)..(x + w).min(SCREENWIDTH as i32) {
            screen[sy as usize * SCREENWIDTH + sx as usize] = color;
        }
    }
}

/// Tile a 64x64 flat over a rectangle (`R_FillBackScreen`, finale text
/// backgrounds).
pub fn tile_flat(screen: &mut [u8], flat: &[u8], x: i32, y: i32, w: i32, h: i32) {
    if flat.len() < 4096 {
        return fill_rect(screen, x, y, w, h, 0);
    }
    for sy in y.max(0)..(y + h).min(SCREENHEIGHT as i32) {
        for sx in x.max(0)..(x + w).min(SCREENWIDTH as i32) {
            screen[sy as usize * SCREENWIDTH + sx as usize] = flat[((sy & 63) * 64 + (sx & 63)) as usize];
        }
    }
}

/// Look up a flat's pixels by name in the merged flat namespace.
pub fn flat_pixels<'a>(stack: &'a ResourceStack, name: &str) -> Option<&'a [u8]> {
    stack.find_in(Namespace::Flats, doom_content::LumpName::new(name)).map(|id| stack.flat(id))
}

/// The HUD/menu font: `STCFN033`..`STCFN095` (`!` .. `_`).
pub struct Font {
    glyphs: Vec<Option<Arc<Picture>>>,
}

pub const HU_FONTSTART: u8 = b'!';
pub const HU_FONTEND: u8 = b'_';
const SPACE_WIDTH: i32 = 4;

impl Font {
    pub fn load(stack: &ResourceStack, patches: &mut Patches) -> Self {
        let glyphs = (HU_FONTSTART..=HU_FONTEND).map(|c| patches.get(stack, &format!("STCFN{c:03}"))).collect();
        Font { glyphs }
    }

    fn glyph(&self, c: u8) -> Option<&Picture> {
        let c = c.to_ascii_uppercase();
        if !(HU_FONTSTART..=HU_FONTEND).contains(&c) {
            return None;
        }
        self.glyphs[(c - HU_FONTSTART) as usize].as_deref()
    }

    /// Line height (`SHORT(hu_font[0]->height)`).
    pub fn height(&self) -> i32 {
        self.glyphs.first().and_then(|g| g.as_deref()).map_or(7, |g| g.height)
    }

    /// `M_StringWidth`.
    pub fn width(&self, s: &str) -> i32 {
        s.bytes().map(|c| self.glyph(c).map_or(SPACE_WIDTH, |g| g.width)).sum()
    }

    /// `M_WriteText`: multi-line text at (x, y); glyphs past the right edge
    /// are skipped as in vanilla.
    pub fn draw(&self, screen: &mut [u8], x: i32, y: i32, s: &str) {
        let (mut cx, mut cy) = (x, y);
        for c in s.bytes() {
            if c == b'\n' {
                cx = x;
                cy += 12;
                continue;
            }
            match self.glyph(c) {
                Some(g) => {
                    if cx + g.width > SCREENWIDTH as i32 {
                        break;
                    }
                    draw_patch(screen, cx, cy, g);
                    cx += g.width;
                }
                None => cx += SPACE_WIDTH,
            }
        }
    }

    /// Draw centred horizontally on the screen.
    pub fn draw_centered(&self, screen: &mut [u8], y: i32, s: &str) {
        let x = (SCREENWIDTH as i32 - self.width(s)) / 2;
        self.draw(screen, x, y, s);
    }
}
