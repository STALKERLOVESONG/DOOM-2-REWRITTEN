//! Automap (am_map.c): seen walls coloured by type, the player arrow,
//! grid, marks, follow mode, zoom and pan, and the iddt reveal cheat.
//!
//! Geometry is drawn in floating point rather than vanilla's 16.16 maths;
//! the automap does not affect the simulation.

use std::sync::Arc;

use doom_content::map::ml;
use doom_content::picture::Picture;
use doom_content::ResourceStack;
use doom_core::FRACUNIT;
use doom_sim::player::power;
use doom_sim::World;

use super::gfx::{draw_patch, fill_rect, Patches, SCREENWIDTH};
use super::Key;

const REDS: u8 = 176; // 256 - 5*16
const REDRANGE: u8 = 16;
const GRAYS: u8 = 6 * 16;
const GRAYSRANGE: u8 = 16;
const BROWNS: u8 = 4 * 16;
const YELLOWS: u8 = (256 - 32 + 7) as u8;
const GREENS: u8 = 7 * 16;
const WHITE: u8 = (256 - 47) as u8;
const BACKGROUND: u8 = 0;
const WALLCOLORS: u8 = REDS;
const TSWALLCOLORS: u8 = GRAYS;
const FDWALLCOLORS: u8 = BROWNS;
const CDWALLCOLORS: u8 = YELLOWS;
const THINGCOLORS: u8 = GREENS;
const GRIDCOLORS: u8 = GRAYS + GRAYSRANGE / 2;
const XHAIRCOLORS: u8 = GRAYS;
const PLAYERRADIUS: f64 = 16.0;
const MAPBLOCKUNITS: f64 = 128.0;
const AM_NUMMARKPOINTS: usize = 10;
/// `F_PANINC` pixels per tic.
const F_PANINC: f64 = 4.0;
const M_ZOOMIN: f64 = 1.02;

type MLine = ((f64, f64), (f64, f64));

fn player_arrow(cheat: bool) -> Vec<MLine> {
    let r = (8.0 * PLAYERRADIUS) / 7.0;
    let mut v = vec![
        ((-r + r / 8.0, 0.0), (r, 0.0)),
        ((r, 0.0), (r - r / 2.0, r / 4.0)),
        ((r, 0.0), (r - r / 2.0, -r / 4.0)),
        ((-r + r / 8.0, 0.0), (-r - r / 8.0, r / 4.0)),
        ((-r + r / 8.0, 0.0), (-r - r / 8.0, -r / 4.0)),
        ((-r + 3.0 * r / 8.0, 0.0), (-r + r / 8.0, r / 4.0)),
        ((-r + 3.0 * r / 8.0, 0.0), (-r + r / 8.0, -r / 4.0)),
    ];
    if cheat {
        // "ddt" letters drawn into the arrow.
        let s = r / 6.0;
        v[1].1 .1 = s;
        v[2].1 .1 = -s;
        v[3].1 .1 = s;
        v[4].1 .1 = -s;
        v[5].1 .1 = s;
        v[6].1 .1 = -s;
        v.extend([
            ((-r / 2.0, 0.0), (-r / 2.0, -s)),
            ((-r / 2.0, -s), (-r / 2.0 + s, -s)),
            ((-r / 2.0 + s, -s), (-r / 2.0 + s, r / 4.0)),
            ((-s, 0.0), (-s, -s)),
            ((-s, -s), (0.0, -s)),
            ((0.0, -s), (0.0, r / 4.0)),
            ((s, r / 4.0), (s, -r / 7.0)),
            ((s, -r / 7.0), (s + r / 32.0, -r / 7.0 - r / 32.0)),
            ((s + r / 32.0, -r / 7.0 - r / 32.0), (s + r / 10.0, -r / 7.0)),
        ]);
    }
    v
}

const THINTRIANGLE: [MLine; 3] = [((-0.5, -0.7), (1.0, 0.0)), ((1.0, 0.0), (-0.5, 0.7)), ((-0.5, 0.7), (-0.5, -0.7))];

pub struct Automap {
    pub active: bool,
    follow: bool,
    grid: bool,
    /// 0 = normal, 1 = all lines (iddt), 2 = lines and things.
    pub cheating: u8,
    /// Frame size in pixels.
    f_w: f64,
    f_h: f64,
    /// Lower-left corner of the window in map units, and pixels per unit.
    m_x: f64,
    m_y: f64,
    scale: f64,
    min_scale: f64,
    max_scale: f64,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    zoom: i32,
    pan: (i32, i32),
    bigstate: Option<(f64, f64, f64)>,
    marks: Vec<(f64, f64)>,
    mark_digits: Vec<Arc<Picture>>,
}

impl Automap {
    pub fn new(stack: &ResourceStack, p: &mut Patches, f_w: i32, f_h: i32) -> Self {
        Automap {
            active: false,
            follow: true,
            grid: false,
            cheating: 0,
            f_w: f_w as f64,
            f_h: f_h as f64,
            m_x: 0.0,
            m_y: 0.0,
            scale: 0.2,
            min_scale: 0.01,
            max_scale: 1.0,
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: 0.0,
            zoom: 0,
            pan: (0, 0),
            bigstate: None,
            marks: Vec::new(),
            mark_digits: (0..10).map(|i| p.need(stack, &format!("AMMNUM{i}"))).collect(),
        }
    }

    /// `AM_LevelInit`.
    pub fn level_init(&mut self, w: &World) {
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for v in &w.level.vertexes {
            let (x, y) = (v.x.0 as f64 / FRACUNIT as f64, v.y.0 as f64 / FRACUNIT as f64);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        if min_x > max_x {
            (min_x, min_y, max_x, max_y) = (0.0, 0.0, 1.0, 1.0);
        }
        (self.min_x, self.min_y, self.max_x, self.max_y) = (min_x, min_y, max_x, max_y);
        let a = self.f_w / (max_x - min_x).max(1.0);
        let b = self.f_h / (max_y - min_y).max(1.0);
        self.min_scale = a.min(b);
        self.max_scale = self.f_h / (2.0 * PLAYERRADIUS);
        self.scale = self.min_scale / 0.7;
        if self.scale > self.max_scale {
            self.scale = self.min_scale;
        }
        self.marks.clear();
        self.bigstate = None;
        self.cheating = 0;
    }

    fn player_pos(w: &World) -> Option<(f64, f64, f64)> {
        let mo = w.thinkers.get_mobj(w.console_player().mo?)?;
        let ang = mo.angle.0 as f64 / 4294967296.0 * std::f64::consts::TAU;
        Some((mo.x.0 as f64 / FRACUNIT as f64, mo.y.0 as f64 / FRACUNIT as f64, ang))
    }

    fn window(&self) -> (f64, f64) {
        (self.f_w / self.scale, self.f_h / self.scale)
    }

    /// `AM_Start`: open, centred on the player.
    pub fn start(&mut self, w: &World) {
        self.active = true;
        self.zoom = 0;
        self.pan = (0, 0);
        if let Some((x, y, _)) = Self::player_pos(w) {
            let (mw, mh) = self.window();
            self.m_x = x - mw / 2.0;
            self.m_y = y - mh / 2.0;
        }
    }

    pub fn stop(&mut self) {
        self.active = false;
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    /// `AM_Responder` for key presses (`down`) and releases. Returns true if
    /// the key was used.
    pub fn responder(&mut self, key: Key, down: bool, w: &World) -> Option<String> {
        if !self.active {
            return None;
        }
        if !down {
            match key {
                Key::Minus | Key::Equals => self.zoom = 0,
                Key::Left | Key::Right if !self.follow => self.pan.0 = 0,
                Key::Up | Key::Down if !self.follow => self.pan.1 = 0,
                _ => {}
            }
            return None;
        }
        match key {
            Key::Left if !self.follow => self.pan.0 = -1,
            Key::Right if !self.follow => self.pan.0 = 1,
            Key::Up if !self.follow => self.pan.1 = 1,
            Key::Down if !self.follow => self.pan.1 = -1,
            Key::Minus => self.zoom = -1,
            Key::Equals => self.zoom = 1,
            Key::Char('0') => {
                // Toggle between the whole map and the previous view.
                if let Some((x, y, s)) = self.bigstate.take() {
                    (self.m_x, self.m_y, self.scale) = (x, y, s);
                } else {
                    self.bigstate = Some((self.m_x, self.m_y, self.scale));
                    self.scale = self.min_scale;
                    self.m_x = self.min_x;
                    self.m_y = self.min_y;
                }
            }
            Key::Char('f') => {
                self.follow = !self.follow;
                self.pan = (0, 0);
                return Some(if self.follow { "Follow Mode ON" } else { "Follow Mode OFF" }.into());
            }
            Key::Char('g') => {
                self.grid = !self.grid;
                return Some(if self.grid { "Grid ON" } else { "Grid OFF" }.into());
            }
            Key::Char('m') => {
                let (mw, mh) = self.window();
                if self.marks.len() >= AM_NUMMARKPOINTS {
                    self.marks.remove(0);
                }
                self.marks.push((self.m_x + mw / 2.0, self.m_y + mh / 2.0));
                return Some(format!("Marked Spot {}", self.marks.len() - 1));
            }
            Key::Char('c') => {
                self.marks.clear();
                return Some("All Marks Cleared".into());
            }
            _ => {}
        }
        let _ = w;
        None
    }

    /// `AM_Ticker`.
    pub fn ticker(&mut self, w: &World) {
        if !self.active {
            return;
        }
        let (mw, mh) = self.window();
        let (cx, cy) = (self.m_x + mw / 2.0, self.m_y + mh / 2.0);
        if self.zoom != 0 {
            let z = if self.zoom > 0 { M_ZOOMIN } else { 1.0 / M_ZOOMIN };
            self.scale = (self.scale * z).clamp(self.min_scale, self.max_scale);
            let (mw, mh) = self.window();
            self.m_x = cx - mw / 2.0;
            self.m_y = cy - mh / 2.0;
        }
        if self.follow {
            if let Some((x, y, _)) = Self::player_pos(w) {
                let (mw, mh) = self.window();
                self.m_x = x - mw / 2.0;
                self.m_y = y - mh / 2.0;
            }
        } else if self.pan != (0, 0) {
            self.m_x += self.pan.0 as f64 * F_PANINC / self.scale;
            self.m_y += self.pan.1 as f64 * F_PANINC / self.scale;
        }
    }

    fn to_screen(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.m_x) * self.scale, self.f_h - (y - self.m_y) * self.scale)
    }

    /// Clip (Liang-Barsky) and draw a map-space line.
    fn draw_mline(&self, screen: &mut [u8], a: (f64, f64), b: (f64, f64), color: u8) {
        let (x0, y0) = self.to_screen(a.0, a.1);
        let (x1, y1) = self.to_screen(b.0, b.1);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let xmax = self.f_w - 1.0;
        let ymax = self.f_h - 1.0;
        for (p, q) in [(-dx, x0), (dx, xmax - x0), (-dy, y0), (dy, ymax - y0)] {
            if p == 0.0 {
                if q < 0.0 {
                    return;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    if r > t1 {
                        return;
                    }
                    t0 = t0.max(r);
                } else {
                    if r < t0 {
                        return;
                    }
                    t1 = t1.min(r);
                }
            }
        }
        let (mut x, mut y) = ((x0 + t0 * dx).round() as i32, (y0 + t0 * dy).round() as i32);
        let (ex, ey) = ((x0 + t1 * dx).round() as i32, (y0 + t1 * dy).round() as i32);
        // Bresenham.
        let (adx, ady) = ((ex - x).abs(), -(ey - y).abs());
        let (sx, sy) = (if x < ex { 1 } else { -1 }, if y < ey { 1 } else { -1 });
        let mut err = adx + ady;
        let (w, h) = (self.f_w as i32, self.f_h as i32);
        loop {
            if x >= 0 && x < w && y >= 0 && y < h {
                screen[y as usize * SCREENWIDTH + x as usize] = color;
            }
            if x == ex && y == ey {
                break;
            }
            let e2 = 2 * err;
            if e2 >= ady {
                err += ady;
                x += sx;
            }
            if e2 <= adx {
                err += adx;
                y += sy;
            }
        }
    }

    fn draw_shape(&self, screen: &mut [u8], lines: &[MLine], scale: f64, angle: f64, x: f64, y: f64, color: u8) {
        let (s, c) = angle.sin_cos();
        let tr = |(px, py): (f64, f64)| (x + (px * c - py * s) * scale, y + (px * s + py * c) * scale);
        for &(a, b) in lines {
            self.draw_mline(screen, tr(a), tr(b), color);
        }
    }

    /// `AM_Drawer`: the map replaces the view area (rows 0..f_h).
    pub fn draw(&self, screen: &mut [u8], w: &World, mapped: &[bool]) {
        fill_rect(screen, 0, 0, self.f_w as i32, self.f_h as i32, BACKGROUND);
        let unit = FRACUNIT as f64;
        if self.grid {
            let bm = &w.level.blockmap;
            let (ox, oy) = (bm.org_x.0 as f64 / unit, bm.org_y.0 as f64 / unit);
            let (mw, mh) = self.window();
            let mut x = ox + ((self.m_x - ox) / MAPBLOCKUNITS).floor() * MAPBLOCKUNITS;
            while x < self.m_x + mw {
                self.draw_mline(screen, (x, self.m_y), (x, self.m_y + mh), GRIDCOLORS);
                x += MAPBLOCKUNITS;
            }
            let mut y = oy + ((self.m_y - oy) / MAPBLOCKUNITS).floor() * MAPBLOCKUNITS;
            while y < self.m_y + mh {
                self.draw_mline(screen, (self.m_x, y), (self.m_x + mw, y), GRIDCOLORS);
                y += MAPBLOCKUNITS;
            }
        }
        let allmap = w.console_player().powers[power::ALLMAP] != 0;
        let cheating = self.cheating > 0;
        for (i, line) in w.level.lines.iter().enumerate() {
            let v1 = w.level.vertex(line.v1);
            let v2 = w.level.vertex(line.v2);
            let a = (v1.x.0 as f64 / unit, v1.y.0 as f64 / unit);
            let b = (v2.x.0 as f64 / unit, v2.y.0 as f64 / unit);
            let seen = mapped.get(i).copied().unwrap_or(false) || line.flags & ml::MAPPED != 0;
            if cheating || seen {
                if line.flags & ml::DONTDRAW != 0 && !cheating {
                    continue;
                }
                let color = match (line.front_sector, line.back_sector) {
                    (_, None) => Some(WALLCOLORS),
                    (Some(f), Some(bk)) => {
                        let (f, bk) = (w.level.sector(f), w.level.sector(bk));
                        if line.special == 39 {
                            Some(WALLCOLORS + REDRANGE / 2) // teleporters
                        } else if line.flags & ml::SECRET != 0 {
                            Some(WALLCOLORS) // secret door looks like a wall
                        } else if bk.floor_height != f.floor_height {
                            Some(FDWALLCOLORS)
                        } else if bk.ceiling_height != f.ceiling_height {
                            Some(CDWALLCOLORS)
                        } else if cheating {
                            Some(TSWALLCOLORS)
                        } else {
                            None
                        }
                    }
                    _ => Some(WALLCOLORS),
                };
                if let Some(c) = color {
                    self.draw_mline(screen, a, b, c);
                }
            } else if allmap && line.flags & ml::DONTDRAW == 0 {
                self.draw_mline(screen, a, b, GRAYS + 3);
            }
        }
        if let Some((x, y, ang)) = Self::player_pos(w) {
            self.draw_shape(screen, &player_arrow(cheating), 1.0, ang, x, y, WHITE);
        }
        if self.cheating == 2 {
            for (_, mo) in w.thinkers.mobjs() {
                let ang = mo.angle.0 as f64 / 4294967296.0 * std::f64::consts::TAU;
                self.draw_shape(screen, &THINTRIANGLE, 16.0, ang, mo.x.0 as f64 / unit, mo.y.0 as f64 / unit, THINGCOLORS);
            }
        }
        for (i, &(mx, my)) in self.marks.iter().enumerate() {
            let (sx, sy) = self.to_screen(mx, my);
            let d = &self.mark_digits[i % 10];
            if sx >= 0.0 && sy >= 0.0 && sx < self.f_w - 5.0 && sy < self.f_h - 6.0 {
                draw_patch(screen, sx as i32, sy as i32, d);
            }
        }
        // Crosshair.
        let (cx, cy) = ((self.f_w / 2.0) as usize, ((self.f_h + 1.0) / 2.0) as usize);
        screen[cy * SCREENWIDTH + cx] = XHAIRCOLORS;
    }
}
