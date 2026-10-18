//! The Doom presentation layer: everything drawn in screen space on the
//! 320x200 framebuffer around (or instead of) the 3D view. All graphics
//! come from the loaded WADs.

pub mod automap;
pub mod finale;
pub mod gfx;
pub mod hud;
pub mod intermission;
pub mod menu;
pub mod status_bar;
pub mod strings;

use std::sync::Arc;

use doom_content::game::GameMode;
use doom_content::picture::Picture;
use doom_content::ResourceStack;

use gfx::{draw_patch, flat_pixels, tile_flat, Patches};

/// A key as the game layer sees it (host-independent). Movement keys are
/// still sampled by the host as held state for ticcmds; these events drive
/// menus, the automap, cheats and other one-shot actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    Tab,
    Pause,
    Minus,
    Equals,
    /// F1..F12.
    F(u8),
    /// The console key (` / ~).
    Grave,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    /// Printable characters, lower case.
    Char(char),
}

/// The bevelled view border (`R_FillBackScreen` + `R_DrawViewBorder`) for
/// view sizes smaller than the full width.
pub struct ViewBorder {
    flat: Vec<u8>,
    t: Option<Arc<Picture>>,
    b: Option<Arc<Picture>>,
    l: Option<Arc<Picture>>,
    r: Option<Arc<Picture>>,
    tl: Option<Arc<Picture>>,
    tr: Option<Arc<Picture>>,
    bl: Option<Arc<Picture>>,
    br: Option<Arc<Picture>>,
}

impl ViewBorder {
    pub fn new(stack: &ResourceStack, p: &mut Patches, mode: GameMode) -> Self {
        let flat = if mode == GameMode::Commercial { "GRNROCK" } else { "FLOOR7_2" };
        ViewBorder {
            flat: flat_pixels(stack, flat).map(<[u8]>::to_vec).unwrap_or_default(),
            t: p.get(stack, "BRDR_T"),
            b: p.get(stack, "BRDR_B"),
            l: p.get(stack, "BRDR_L"),
            r: p.get(stack, "BRDR_R"),
            tl: p.get(stack, "BRDR_TL"),
            tr: p.get(stack, "BRDR_TR"),
            bl: p.get(stack, "BRDR_BL"),
            br: p.get(stack, "BRDR_BR"),
        }
    }

    /// Fill the area above the status bar outside the view window
    /// (x, y, w, h) and draw the bevel.
    pub fn draw(&self, screen: &mut [u8], x: i32, y: i32, w: i32, h: i32, bar_top: i32) {
        if w >= gfx::SCREENWIDTH as i32 {
            return;
        }
        // Background around the window only, so the view stays intact.
        tile_flat(screen, &self.flat, 0, 0, gfx::SCREENWIDTH as i32, y);
        tile_flat(screen, &self.flat, 0, y + h, gfx::SCREENWIDTH as i32, bar_top - (y + h));
        tile_flat(screen, &self.flat, 0, y, x, h);
        tile_flat(screen, &self.flat, x + w, y, gfx::SCREENWIDTH as i32 - (x + w), h);
        let mut i = 0;
        while i < w {
            if let Some(t) = &self.t {
                draw_patch(screen, x + i, y - 8, t);
            }
            if let Some(b) = &self.b {
                draw_patch(screen, x + i, y + h, b);
            }
            i += 8;
        }
        let mut j = 0;
        while j < h {
            if let Some(l) = &self.l {
                draw_patch(screen, x - 8, y + j, l);
            }
            if let Some(r) = &self.r {
                draw_patch(screen, x + w, y + j, r);
            }
            j += 8;
        }
        if let Some(p) = &self.tl {
            draw_patch(screen, x - 8, y - 8, p);
        }
        if let Some(p) = &self.tr {
            draw_patch(screen, x + w, y - 8, p);
        }
        if let Some(p) = &self.bl {
            draw_patch(screen, x - 8, y + h, p);
        }
        if let Some(p) = &self.br {
            draw_patch(screen, x + w, y + h, p);
        }
    }
}
