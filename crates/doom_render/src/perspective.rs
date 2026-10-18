//! Perspective software renderer with a free camera (true pitch).
//!
//! The classic renderer (render.rs) is vanilla's column/span renderer: it
//! can only draw world-vertical edges as screen columns, so it cannot look
//! up or down without shearing. This renderer instead rasterises the level
//! as polygons through a [`Camera`] with yaw and pitch:
//!
//! - walls (upper/lower/middle with vanilla pegging, masked middles),
//!   floor/ceiling polygons per subsector (derived from the BSP), sky,
//!   and billboard sprites with vanilla rotation frames;
//! - texture coordinates and depth come from exact per-pixel ray/plane
//!   intersection, so every surface is perspective correct at any pitch;
//! - occlusion uses a depth buffer; masked surfaces are drawn back to front;
//! - lighting uses vanilla's light tables driven by horizontal forward
//!   distance, so the light does not change when looking up or down.
//!
//! At pitch 0 the projection equals vanilla's (focal length = half the view
//! width, same centre, Doom's 1.2 pixel aspect), so the view lines up with
//! the classic renderer; output is not bit-identical (different sampling).
//! The player's weapon is screen-space and drawn by the classic renderer.

use std::collections::HashMap;

use doom_core::angle::ANG45;
use doom_core::FRACUNIT;
use doom_sim::info::FF_FRAMEMASK;
use doom_sim::level::{Level, NF_SUBSECTOR};
use doom_sim::mobj::flags::{MF_SHADOW, MF_TRANSLATION, MF_TRANSSHIFT};
use doom_sim::World;

use crate::data::{RenderData, TexImage};
use crate::debug_draw::{DebugLine, DebugScene, DebugTri, SceneSprite, TexTri};
use crate::render::{SCREENHEIGHT, SCREENWIDTH};

/// Doom pixels are 1.2 times taller than wide on a 4:3 display; world
/// heights are scaled by this to make pitch a true rotation.
pub const PIXEL_ASPECT: f32 = 1.2;
const NEAR: f32 = 1.0;
const LIGHTLEVELS: i32 = 16;
const NUMCOLORMAPS: i32 = 32;
const MAXLIGHTSCALE: i32 = 48;
const MAXLIGHTZ: i32 = 128;
const DISTMAP: i32 = 2;
/// Fuzz table (r_draw.c `fuzzoffset`), in rows.
const FUZZOFFSET: [i32; 50] = [
    1, -1, 1, -1, 1, 1, -1, 1, 1, -1, 1, 1, 1, -1, 1, 1, 1, -1, -1, -1, -1, 1, -1, -1, 1, 1, 1, 1, -1, 1, -1, 1, 1, -1,
    -1, 1, 1, -1, -1, -1, -1, 1, 1, 1, 1, -1, 1, 1, -1, 1,
];

/// A viewpoint. Angles in radians: yaw 0 = east, counter-clockwise;
/// pitch > 0 looks up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    /// The console player's eye, with the given pitch.
    pub fn from_player(w: &World, player: usize, pitch: f32) -> Option<Camera> {
        let p = &w.players[player];
        let mo = w.thinkers.get_mobj(p.mo?)?;
        Some(Camera {
            x: fx(mo.x.0),
            y: fx(mo.y.0),
            z: fx(p.viewz.0),
            yaw: mo.angle.0 as f32 / 4294967296.0 * std::f32::consts::TAU,
            pitch,
        })
    }

    /// The largest pitch (radians) for which the view never sees behind the
    /// camera, given the view's vertical half-extent in pixels and focal
    /// length. Callers should clamp to something smaller.
    pub fn pitch_limit(view_h: f32, focal: f32) -> f32 {
        let half_v = ((view_h / 2.0) * PIXEL_ASPECT / focal).atan();
        std::f32::consts::FRAC_PI_2 - half_v - 0.02
    }
}

#[inline]
fn fx(v: i32) -> f32 {
    v as f32 / FRACUNIT as f32
}

/// Per-frame projection state.
#[derive(Clone, Copy)]
struct View {
    eye: [f32; 3],
    fwd: [f32; 2],
    right: [f32; 2],
    cp: f32,
    sp: f32,
    cx: f32,
    cy: f32,
    /// Horizontal focal length in (possibly low-detail) columns.
    focal: f32,
    /// Vertical focal length in rows: half the full view width, also in
    /// low detail (vanilla's `<< detailshift` on scales). Light levels use
    /// this scale.
    focal_y: f32,
    w: i32,
    h: i32,
}

/// A point in pitched view space: right, up (aspect-scaled), depth.
#[derive(Clone, Copy, Debug)]
struct VPoint {
    a: f32,
    u: f32,
    d: f32,
}

impl View {
    fn new(cam: &Camera, rect: ViewRect) -> View {
        let detail = if rect.low_detail { 2 } else { 1 };
        let (vw, vh) = (rect.width / detail, rect.height);
        View {
            eye: [cam.x, cam.y, cam.z],
            fwd: [cam.yaw.cos(), cam.yaw.sin()],
            right: [cam.yaw.sin(), -cam.yaw.cos()],
            cp: cam.pitch.cos(),
            sp: cam.pitch.sin(),
            cx: vw as f32 / 2.0,
            cy: vh as f32 / 2.0,
            focal: vw as f32 / 2.0,
            focal_y: rect.width as f32 / 2.0,
            w: vw,
            h: vh,
        }
    }

    fn to_view(&self, p: [f32; 3]) -> VPoint {
        let (rx, ry, rz) = (p[0] - self.eye[0], p[1] - self.eye[1], p[2] - self.eye[2]);
        let f = rx * self.fwd[0] + ry * self.fwd[1];
        let a = rx * self.right[0] + ry * self.right[1];
        let up = rz * PIXEL_ASPECT;
        VPoint { a, d: f * self.cp + up * self.sp, u: up * self.cp - f * self.sp }
    }

    fn project(&self, v: VPoint) -> (f32, f32) {
        (self.cx + v.a * self.focal / v.d, self.cy - v.u * self.focal_y / (v.d * PIXEL_ASPECT))
    }

    /// World-space ray direction through sample point (x, y) (see
    /// [`raster`]), scaled so that the ray parameter equals pitched view
    /// depth.
    fn ray(&self, x: f32, y: f32) -> [f32; 3] {
        let a = (x - self.cx) / self.focal;
        let b = (self.cy - y) * PIXEL_ASPECT / self.focal_y;
        let f = self.cp - self.sp * b;
        let up = self.sp + self.cp * b;
        [f * self.fwd[0] + a * self.right[0], f * self.fwd[1] + a * self.right[1], up / PIXEL_ASPECT]
    }
}

/// A plane n·p = d.
#[derive(Clone, Copy)]
struct Plane {
    n: [f32; 3],
    d: f32,
}

impl Plane {
    #[inline]
    fn hit(&self, eye: [f32; 3], dir: [f32; 3]) -> Option<f32> {
        let den = self.n[0] * dir[0] + self.n[1] * dir[1] + self.n[2] * dir[2];
        if den.abs() < 1e-9 {
            return None;
        }
        let t = (self.d - (self.n[0] * eye[0] + self.n[1] * eye[1] + self.n[2] * eye[2])) / den;
        (t > 0.0).then_some(t)
    }
}

/// Clip a polygon in view space to depth >= NEAR.
fn clip_near(poly: &[VPoint]) -> Vec<VPoint> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        let (pin, qin) = (p.d >= NEAR, q.d >= NEAR);
        if pin {
            out.push(p);
        }
        if pin != qin {
            let t = (NEAR - p.d) / (q.d - p.d);
            out.push(VPoint { a: p.a + (q.a - p.a) * t, u: p.u + (q.u - p.u) * t, d: NEAR });
        }
    }
    out
}

/// Visit the pixels covered by a convex screen polygon. Pixel (x, y) is
/// sampled at the point (x, y), i.e. its top-left corner, as vanilla does
/// (`centerx`/`centery` offsets, `yl = ceil(top)`); spans are half-open so
/// shared edges are not drawn twice.
fn raster(poly: &[(f32, f32)], w: i32, h: i32, mut f: impl FnMut(i32, i32)) {
    if poly.len() < 3 {
        return;
    }
    let (mut miny, mut maxy) = (f32::MAX, f32::MIN);
    for &(_, y) in poly {
        miny = miny.min(y);
        maxy = maxy.max(y);
    }
    let y0 = (miny.ceil() as i32).max(0);
    let y1 = (maxy.ceil() as i32).min(h);
    for y in y0..y1 {
        let yc = y as f32;
        let (mut xl, mut xr) = (f32::MAX, f32::MIN);
        for i in 0..poly.len() {
            let (ax, ay) = poly[i];
            let (bx, by) = poly[(i + 1) % poly.len()];
            if (ay <= yc && by > yc) || (by <= yc && ay > yc) {
                let x = ax + (yc - ay) * (bx - ax) / (by - ay);
                xl = xl.min(x);
                xr = xr.max(x);
            }
        }
        if xl > xr {
            continue;
        }
        let x0 = (xl.ceil() as i32).max(0);
        let x1 = (xr.ceil() as i32).min(w);
        for x in x0..x1 {
            f(x, y);
        }
    }
}

/// Floor/ceiling polygons for every subsector, derived from the BSP.
struct LevelGeometry {
    subsector_polys: Vec<Vec<[f32; 2]>>,
    /// Bounds of the polygons under each node child, `[minx, miny, maxx,
    /// maxy]`. The WAD's node boxes only bound segs, but a subsector's
    /// floor can reach past its segs (out to partition lines), so they
    /// can't be used to cull polygons.
    node_bounds: Vec<[[f32; 4]; 2]>,
    num_subsectors: usize,
    num_vertexes: usize,
}

fn clip_halfplane(poly: &[[f64; 2]], o: [f64; 2], d: [f64; 2], keep_front: bool) -> Vec<[f64; 2]> {
    // Doom's point-on-side: front when dy*(px-ox) - dx*(py-oy) > 0.
    let side = |p: [f64; 2]| {
        let s = d[1] * (p[0] - o[0]) - d[0] * (p[1] - o[1]);
        if keep_front {
            s
        } else {
            -s
        }
    };
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        let (sp, sq) = (side(p), side(q));
        if sp >= -1e-6 {
            out.push(p);
        }
        if (sp >= -1e-6) != (sq >= -1e-6) {
            let t = sp / (sp - sq);
            out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
        }
    }
    out
}

impl LevelGeometry {
    fn build(level: &Level) -> LevelGeometry {
        let n = level.subsectors.len();
        let mut polys = vec![Vec::new(); n];
        let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for v in &level.vertexes {
            let (x, y) = (v.x.0 as f64 / 65536.0, v.y.0 as f64 / 65536.0);
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        let m = 64.0;
        let root = vec![[minx - m, miny - m], [maxx + m, miny - m], [maxx + m, maxy + m], [minx - m, maxy + m]];
        if level.nodes.is_empty() {
            if n > 0 {
                polys[0] = Self::finish(level, 0, root);
            }
        } else {
            let mut stack = vec![((level.nodes.len() - 1) as u32, root)];
            while let Some((node, poly)) = stack.pop() {
                let nd = &level.nodes[node as usize];
                let o = [nd.x.0 as f64 / 65536.0, nd.y.0 as f64 / 65536.0];
                let d = [nd.dx.0 as f64 / 65536.0, nd.dy.0 as f64 / 65536.0];
                for (side, &child) in nd.children.iter().enumerate() {
                    let p = clip_halfplane(&poly, o, d, side == 0);
                    if p.len() < 3 {
                        continue;
                    }
                    if child & NF_SUBSECTOR != 0 {
                        let ss = (child & !NF_SUBSECTOR) as usize;
                        if ss < n {
                            polys[ss] = Self::finish(level, ss, p);
                        }
                    } else {
                        stack.push((child, p));
                    }
                }
            }
        }
        let mut node_bounds = vec![[[f32::MAX, f32::MAX, f32::MIN, f32::MIN]; 2]; level.nodes.len()];
        if !level.nodes.is_empty() {
            Self::bounds(level, &polys, (level.nodes.len() - 1) as u32, &mut node_bounds);
        }
        LevelGeometry { subsector_polys: polys, node_bounds, num_subsectors: n, num_vertexes: level.vertexes.len() }
    }

    fn bounds(level: &Level, polys: &[Vec<[f32; 2]>], child: u32, out: &mut [[[f32; 4]; 2]]) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut add = |c: [f32; 4]| {
            b = [b[0].min(c[0]), b[1].min(c[1]), b[2].max(c[2]), b[3].max(c[3])];
        };
        if child & NF_SUBSECTOR != 0 {
            for p in polys.get((child & !NF_SUBSECTOR) as usize).into_iter().flatten() {
                add([p[0], p[1], p[0], p[1]]);
            }
        } else {
            let nd = &level.nodes[child as usize];
            for side in 0..2 {
                let c = Self::bounds(level, polys, nd.children[side], out);
                out[child as usize][side] = c;
                add(c);
            }
        }
        b
    }

    /// Clip by the subsector's own segs, then grow slightly to hide cracks
    /// between neighbouring polygons.
    fn finish(level: &Level, ss: usize, mut poly: Vec<[f64; 2]>) -> Vec<[f32; 2]> {
        let sub = &level.subsectors[ss];
        for i in sub.first_line as usize..(sub.first_line + sub.num_lines) as usize {
            let seg = &level.segs[i];
            let a = level.vertex(seg.v1);
            let b = level.vertex(seg.v2);
            let o = [a.x.0 as f64 / 65536.0, a.y.0 as f64 / 65536.0];
            let d = [(b.x.0 - a.x.0) as f64 / 65536.0, (b.y.0 - a.y.0) as f64 / 65536.0];
            let p = clip_halfplane(&poly, o, d, true);
            if p.len() >= 3 {
                poly = p;
            }
        }
        if poly.len() < 3 {
            return Vec::new();
        }
        let (mut cx, mut cy) = (0.0, 0.0);
        for p in &poly {
            cx += p[0];
            cy += p[1];
        }
        cx /= poly.len() as f64;
        cy /= poly.len() as f64;
        poly.iter()
            .map(|p| {
                let (dx, dy) = (p[0] - cx, p[1] - cy);
                let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                let g = 0.35 / len;
                [(p[0] + dx * g) as f32, (p[1] + dy * g) as f32]
            })
            .collect()
    }
}

/// What an opaque wall piece shows.
#[derive(Clone, Copy)]
enum Surface {
    Texture(usize),
    /// Missing upper/lower texture: vanilla leaves those pixels to the
    /// front sector's ceiling/floor visplane, so the flat appears to
    /// continue behind the step ("flat bleeding", used for deep-water
    /// tricks). Emulated by sampling that plane along the pixel's ray.
    Bleed { height: f32, pic: doom_sim::ids::FlatNum, light: i32 },
    Black,
}

/// The floor/ceiling polygon of every subsector (slightly expanded), as the
/// perspective renderer uses them. For tools and tests.
pub fn subsector_polygons(level: &Level) -> Vec<Vec<[f32; 2]>> {
    LevelGeometry::build(level).subsector_polys
}

/// Something drawn after the opaque pass, back to front.
enum Masked {
    Sprite { depth: f32, mobj: doom_sim::MobjId },
    Wall { depth: f32, seg: usize, bottom: f32, top: f32, tex: usize, tex_top: f32, u0: f32, light: i32 },
}

impl Masked {
    fn depth(&self) -> f32 {
        match self {
            Masked::Sprite { depth, .. } | Masked::Wall { depth, .. } => *depth,
        }
    }
}

/// See the module docs.
pub struct PerspectiveRenderer {
    geometry: Option<LevelGeometry>,
    zbuf: Vec<f32>,
    rays: Vec<[f32; 3]>,
    sprite_cache: HashMap<usize, TexImage>,
    /// Linedefs drawn so far (automap), by line number.
    pub mapped_lines: Vec<bool>,
    fuzzpos: usize,
    /// Frame number at which each sector's things were last collected.
    sector_stamp: Vec<u32>,
    frame: u32,
    /// Sky cap colours by texture.
    sky_caps: HashMap<usize, (u8, u8)>,
    /// Scratch for [`PerspectiveRenderer::draw_scene_overlay`].
    overlay_buf: Vec<u8>,
    overlay_zbuf: Vec<f32>,
    /// Lowest opaque row of each sprite lump (scene billboards' feet).
    sprite_feet: HashMap<usize, i32>,
}

impl Default for PerspectiveRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// The view window inside the 320x200 framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Render at half horizontal resolution and double pixels.
    pub low_detail: bool,
}

struct Frame<'a> {
    data: &'a RenderData,
    w: &'a World,
    view: View,
    extralight: i32,
    fixedcolormap: Option<usize>,
    /// `scalelight` divisor term: SCREENWIDTH / viewwidth.
    scale_div: f32,
    /// The viewing player's body, never drawn.
    viewer: Option<doom_sim::MobjId>,
    sky: Sky<'a>,
}

impl PerspectiveRenderer {
    pub fn new() -> Self {
        PerspectiveRenderer {
            geometry: None,
            zbuf: Vec::new(),
            rays: Vec::new(),
            sprite_cache: HashMap::new(),
            mapped_lines: Vec::new(),
            fuzzpos: 0,
            sector_stamp: Vec::new(),
            frame: 0,
            sky_caps: HashMap::new(),
            overlay_buf: Vec::new(),
            overlay_zbuf: Vec::new(),
            sprite_feet: HashMap::new(),
        }
    }

    /// Forget per-level data (call on level change).
    pub fn level_changed(&mut self) {
        self.geometry = None;
        self.mapped_lines.clear();
    }

    /// Render `w` from `cam` into the view window `rect` of `screen`.
    pub fn render(&mut self, data: &RenderData, w: &World, player: usize, cam: &Camera, rect: ViewRect, screen: &mut [u8]) {
        self.render_scene(data, w, player, cam, rect, screen, &DebugScene::default());
    }

    /// [`PerspectiveRenderer::render`] plus extra geometry, depth tested
    /// against the level.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene(
        &mut self,
        data: &RenderData,
        w: &World,
        player: usize,
        cam: &Camera,
        rect: ViewRect,
        screen: &mut [u8],
        scene: &DebugScene,
    ) {
        assert_eq!(screen.len(), SCREENWIDTH * SCREENHEIGHT);
        let level = &w.level;
        let stale = self
            .geometry
            .as_ref()
            .map_or(true, |g| g.num_subsectors != level.subsectors.len() || g.num_vertexes != level.vertexes.len());
        if stale {
            self.geometry = Some(LevelGeometry::build(level));
        }
        let detail = if rect.low_detail { 2 } else { 1 };
        let view = View::new(cam, rect);
        let (vw, vh) = (view.w, view.h);
        let n = (vw * vh) as usize;
        self.frame = self.frame.wrapping_add(1);
        if self.sector_stamp.len() != level.sectors.len() {
            self.sector_stamp = vec![u32::MAX; level.sectors.len()];
        }
        self.zbuf.clear();
        self.zbuf.resize(n, f32::INFINITY);
        self.rays.clear();
        for y in 0..vh {
            for x in 0..vw {
                self.rays.push(view.ray(x as f32, y as f32));
            }
        }
        let p = &w.players[player];
        let frame = Frame {
            data,
            w,
            view,
            extralight: p.extralight,
            fixedcolormap: (p.fixedcolormap != 0).then_some(p.fixedcolormap as usize),
            scale_div: SCREENWIDTH as f32 / (vw * detail) as f32,
            viewer: p.mo,
            sky: self.sky(data, w),
        };
        // Render into a local buffer, then blit (doubling for low detail).
        let mut buf = vec![0u8; n];
        let mut masked = Vec::new();
        if let Some(c) = scene.clear_view {
            buf.fill(c);
        } else {
            let order = self.bsp_order(level, &view);
            for &ss in &order {
                self.draw_subsector(&frame, ss, &mut buf, &mut masked);
            }
            // Anything still empty looks out at the sky (vanilla draws the
            // sky wherever a sky ceiling's visplane reaches; see draw_seg).
            for k in 0..n {
                if self.zbuf[k] == f32::INFINITY {
                    buf[k] = frame.sky.texel(self.rays[k], &view, k as i32 % vw, k as i32 / vw);
                }
            }
        }
        masked.sort_by(|a, b| b.depth().partial_cmp(&a.depth()).unwrap_or(std::cmp::Ordering::Equal));
        for m in &masked {
            match *m {
                Masked::Sprite { mobj, .. } => self.draw_sprite(&frame, mobj, &mut buf),
                Masked::Wall { seg, bottom, top, tex, tex_top, u0, light, .. } => {
                    self.draw_masked_wall(&frame, seg, bottom, top, tex, tex_top, u0, light, &mut buf)
                }
            }
        }
        // Extra geometry goes in after the masked layer: it is depth tested
        // against walls and planes (sprites are not in the depth buffer).
        // Models first, then billboards against them, then debug lines.
        draw_scene_tris(data, &view, frame.fixedcolormap, scene, &mut self.zbuf, &mut buf);
        let extralight = frame.extralight;
        self.draw_scene_sprites(data, &view, frame.fixedcolormap, extralight, scene, &mut buf);
        draw_scene_lines(&view, scene, &self.zbuf, &mut buf);
        for y in 0..vh {
            let srow = &buf[(y * vw) as usize..((y + 1) * vw) as usize];
            let dst = ((rect.y + y) as usize) * SCREENWIDTH + rect.x as usize;
            if detail == 1 {
                screen[dst..dst + vw as usize].copy_from_slice(srow);
            } else {
                for (i, &c) in srow.iter().enumerate() {
                    screen[dst + 2 * i] = c;
                    screen[dst + 2 * i + 1] = c;
                }
            }
        }
    }

    /// Draw `scene` over a view another renderer drew (the classic column
    /// renderer): no level depth, so everything shows through walls and
    /// triangles are only sorted among themselves.
    pub fn draw_scene_overlay(&mut self, data: &RenderData, w: &World, player: usize, cam: &Camera, rect: ViewRect, screen: &mut [u8], scene: &DebugScene) {
        if scene.is_empty() {
            return;
        }
        let detail = if rect.low_detail { 2 } else { 1 };
        let view = View::new(cam, rect);
        let n = (view.w * view.h) as usize;
        let mut buf = std::mem::take(&mut self.overlay_buf);
        buf.clear();
        for y in 0..view.h {
            let row = ((rect.y + y) as usize) * SCREENWIDTH + rect.x as usize;
            buf.extend((0..view.w).map(|x| screen[row + (x * detail) as usize]));
        }
        if let Some(c) = scene.clear_view {
            buf.fill(c);
        }
        self.overlay_zbuf.clear();
        self.overlay_zbuf.resize(n, f32::INFINITY);
        let p = &w.players[player];
        let fixed = (p.fixedcolormap != 0).then_some(p.fixedcolormap as usize);
        let mut zbuf = std::mem::take(&mut self.overlay_zbuf);
        draw_scene_tris(data, &view, fixed, scene, &mut zbuf, &mut buf);
        // Billboards test against `self.zbuf`: use the overlay's.
        let level_zbuf = std::mem::replace(&mut self.zbuf, zbuf);
        self.draw_scene_sprites(data, &view, fixed, p.extralight, scene, &mut buf);
        let zbuf = std::mem::replace(&mut self.zbuf, level_zbuf);
        draw_scene_lines(&view, scene, &zbuf, &mut buf);
        self.overlay_zbuf = zbuf;
        for y in 0..view.h {
            let row = ((rect.y + y) as usize) * SCREENWIDTH + rect.x as usize;
            for x in 0..view.w {
                let c = buf[(y * view.w + x) as usize];
                for k in 0..detail {
                    screen[row + (x * detail + k) as usize] = c;
                }
            }
        }
        self.overlay_buf = buf;
    }

    fn sky<'a>(&mut self, data: &'a RenderData, w: &World) -> Sky<'a> {
        let idx = w.texture_translation.get(w.sky_texture.0 as usize).map(|&t| t as usize);
        let Some((i, img)) = idx.and_then(|i| Some((i, data.image(i)?))).filter(|(_, im)| im.width > 0 && im.height > 0) else {
            return Sky { img: None, cm0: &data.colormaps[..256], top: 0, bottom: 0 };
        };
        let &mut (top, bottom) =
            self.sky_caps.entry(i).or_insert_with(|| (row_average(data, img, 0), row_average(data, img, img.height - 1)));
        Sky { img: Some(img), cm0: &data.colormaps[..256], top, bottom }
    }

    /// Subsectors front to back, skipping nodes entirely behind the camera
    /// or outside the horizontal view cone.
    fn bsp_order(&self, level: &Level, view: &View) -> Vec<usize> {
        let mut out = Vec::new();
        if level.nodes.is_empty() {
            return (0..level.subsectors.len()).collect();
        }
        // Horizontal extent of the frustum: the widest corner ray.
        let mut max_tan = 0.0f32;
        let mut side_cull = true;
        for (x, y) in [(0.0, 0.0), (view.w as f32 - 1.0, 0.0), (0.0, view.h as f32 - 1.0), (view.w as f32 - 1.0, view.h as f32 - 1.0)] {
            let r = view.ray(x, y);
            let f = r[0] * view.fwd[0] + r[1] * view.fwd[1];
            let a = (r[0] * view.right[0] + r[1] * view.right[1]).abs();
            if f <= 1e-3 {
                side_cull = false;
            } else {
                max_tan = max_tan.max(a / f);
            }
        }
        let max_tan = max_tan * 1.02 + 0.01;
        let Some(geo) = self.geometry.as_ref() else { return out };
        let visible = |bb: &[f32; 4]| {
            if bb[0] > bb[2] {
                return false; // nothing under this child
            }
            let corners = [(bb[0], bb[1]), (bb[2], bb[1]), (bb[0], bb[3]), (bb[2], bb[3])];
            let mut any_front = false;
            let (mut all_left, mut all_right) = (true, true);
            for (x, y) in corners {
                let (rx, ry) = (x - view.eye[0], y - view.eye[1]);
                let f = rx * view.fwd[0] + ry * view.fwd[1];
                let a = rx * view.right[0] + ry * view.right[1];
                if f > -2.0 {
                    any_front = true;
                }
                if a > -max_tan * f.max(0.0) - 2.0 {
                    all_left = false;
                }
                if a < max_tan * f.max(0.0) + 2.0 {
                    all_right = false;
                }
            }
            // The camera inside the box is always visible.
            let inside = view.eye[0] >= bb[0] && view.eye[0] <= bb[2] && view.eye[1] >= bb[1] && view.eye[1] <= bb[3];
            inside || (any_front && !(side_cull && (all_left || all_right)))
        };
        let mut stack = vec![(level.nodes.len() - 1) as u32];
        while let Some(n) = stack.pop() {
            if n & NF_SUBSECTOR != 0 {
                out.push((n & !NF_SUBSECTOR) as usize);
                continue;
            }
            let nd = &level.nodes[n as usize];
            let (dx, dy) = (view.eye[0] - fx(nd.x.0), view.eye[1] - fx(nd.y.0));
            let front_side = if fx(nd.dy.0) * dx - fx(nd.dx.0) * dy > 0.0 { 0 } else { 1 };
            let back_side = front_side ^ 1;
            // Push back first so the front is processed first.
            let bounds = &geo.node_bounds[n as usize];
            if visible(&bounds[back_side]) {
                stack.push(nd.children[back_side]);
            }
            if visible(&bounds[front_side]) {
                stack.push(nd.children[front_side]);
            }
        }
        out
    }

    fn colormap_scale(f: &Frame, light: i32, scale: f32) -> usize {
        // scalelight[light][min(47, scale >> LIGHTSCALESHIFT)]
        let light = light.clamp(0, LIGHTLEVELS - 1);
        let startmap = ((LIGHTLEVELS - 1 - light) * 2) * NUMCOLORMAPS / LIGHTLEVELS;
        let j = ((scale * 16.0) as i32).clamp(0, MAXLIGHTSCALE - 1);
        let level = startmap - (j as f32 * f.scale_div) as i32 / DISTMAP;
        level.clamp(0, NUMCOLORMAPS - 1) as usize
    }

    fn colormap_dist(light: i32, dist: f32) -> usize {
        // zlight[light][min(127, dist >> LIGHTZSHIFT)]
        let light = light.clamp(0, LIGHTLEVELS - 1);
        let startmap = ((LIGHTLEVELS - 1 - light) * 2) * NUMCOLORMAPS / LIGHTLEVELS;
        let j = ((dist / 16.0) as i32).clamp(0, MAXLIGHTZ - 1);
        let scale = (((SCREENWIDTH as i64 / 2 * 65536 * 65536) / (((j + 1) as i64) << 20)) >> 12) as i32;
        (startmap - scale / DISTMAP).clamp(0, NUMCOLORMAPS - 1) as usize
    }

    fn draw_subsector(&mut self, f: &Frame, ss: usize, buf: &mut [u8], masked: &mut Vec<Masked>) {
        let level = &f.w.level;
        let sub = &level.subsectors[ss];
        let sector_id = sub.sector;
        let sector = level.sector(sector_id);
        let view = f.view;
        let light = (sector.light_level as i32 >> 4) + f.extralight;
        let sky = f.w.sky_flat;
        let floor_h = fx(sector.floor_height.0);
        let ceil_h = fx(sector.ceiling_height.0);

        // Things are collected once per sector per frame, from the first of
        // its subsectors reached (R_AddSprites with validcount).
        let si = sector_id.idx();
        if self.sector_stamp[si] != self.frame {
            self.sector_stamp[si] = self.frame;
            let mut m = sector.thing_list;
            while let Some(id) = m {
                let Some(mo) = f.w.thinkers.get_mobj(id) else { break };
                let horiz = (fx(mo.x.0) - view.eye[0]) * view.fwd[0] + (fx(mo.y.0) - view.eye[1]) * view.fwd[1];
                // Things well behind the eye can't reach the view (sprites
                // are at most a few hundred units wide).
                if horiz > -256.0 {
                    masked.push(Masked::Sprite { depth: horiz, mobj: id });
                }
                m = mo.snext;
            }
        }

        // Walls.
        for i in sub.first_line as usize..(sub.first_line + sub.num_lines) as usize {
            self.draw_seg(f, i, light, buf, masked);
        }

        // Floor and ceiling.
        let Some(poly) = self.geometry.as_ref().map(|g| g.subsector_polys[ss].clone()) else { return };
        if poly.len() < 3 {
            return;
        }
        // UDMF plane light: absolute, or relative to the sector's.
        let plane_light = |(v, absolute): (i16, bool)| {
            let level = if absolute { v as i32 } else { sector.light_level as i32 + v as i32 };
            (level.clamp(0, 255) >> 4) + f.extralight
        };
        if view.eye[2] > floor_h {
            let pic = sector.floor_pic;
            let light = plane_light(sector.floor_light);
            self.draw_flat(f, &poly, floor_h, pic, pic == sky, light, &sector.floor_xform, buf);
        }
        // Sky ceilings are not planes: see the sky walls in draw_seg.
        if view.eye[2] < ceil_h && sector.ceiling_pic != sky {
            let light = plane_light(sector.ceiling_light);
            self.draw_flat(f, &poly, ceil_h, sector.ceiling_pic, false, light, &sector.ceiling_xform, buf);
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn draw_flat(
        &mut self,
        f: &Frame,
        poly: &[[f32; 2]],
        h: f32,
        pic: doom_sim::ids::FlatNum,
        is_sky: bool,
        light: i32,
        xform: &doom_content::map::PlaneXform,
        buf: &mut [u8],
    ) {
        let view = f.view;
        let vpoly: Vec<VPoint> = poly.iter().map(|p| view.to_view([p[0], p[1], h])).collect();
        let clipped = clip_near(&vpoly);
        if clipped.len() < 3 {
            return;
        }
        let spoly: Vec<(f32, f32)> = clipped.iter().map(|&v| view.project(v)).collect();
        let plane = Plane { n: [0.0, 0.0, 1.0], d: h };
        let data = f.data;
        let src = if is_sky { FlatSrc::None } else { flat_src(f, pic) };
        let cm = &data.colormaps;
        let eye = view.eye;
        let (zbuf, rays) = (&mut self.zbuf, &self.rays);
        raster(&spoly, view.w, view.h, |x, y| {
            let k = (y * view.w + x) as usize;
            let dir = rays[k];
            let Some(t) = plane.hit(eye, dir) else { return };
            if t >= zbuf[k] {
                return;
            }
            zbuf[k] = t;
            if is_sky {
                buf[k] = f.sky.texel(dir, &view, x, y);
                return;
            }
            let hx = eye[0] + dir[0] * t;
            let hy = eye[1] + dir[1] * t;
            let Some(texel) = src.texel(hx, hy, xform) else {
                buf[k] = 0;
                return;
            };
            let map = match f.fixedcolormap {
                Some(m) => m,
                None => {
                    let dist = t * (dir[0] * view.fwd[0] + dir[1] * view.fwd[1]);
                    Self::colormap_dist(light, dist)
                }
            };
            buf[k] = cm[map * 256 + texel as usize];
        });
    }

    fn draw_seg(&mut self, f: &Frame, segi: usize, sector_light: i32, buf: &mut [u8], masked: &mut Vec<Masked>) {
        let level = &f.w.level;
        let seg = &level.segs[segi];
        let view = f.view;
        let v1 = level.vertex(seg.v1);
        let v2 = level.vertex(seg.v2);
        let (x1, y1, x2, y2) = (fx(v1.x.0), fx(v1.y.0), fx(v2.x.0), fx(v2.y.0));
        // Back-face cull: the eye must be on the seg's front side.
        let (dx, dy) = (x2 - x1, y2 - y1);
        if dy * (view.eye[0] - x1) - dx * (view.eye[1] - y1) <= 0.0 {
            return;
        }
        let line = level.line(seg.linedef);
        let side = level.side(seg.sidedef);
        let front = level.sector(seg.front_sector);
        let tr = &f.w.texture_translation;
        let tex = |t: doom_sim::ids::TexNum| -> usize { tr.get(t.0 as usize).map_or(0, |&x| x as usize) };
        let fl = fx(front.floor_height.0);
        let fc = fx(front.ceiling_height.0);
        // UDMF side light, then fake contrast.
        let mut light = match side.light {
            (0, false) => sector_light,
            (v, absolute) => {
                let level = if absolute { v as i32 } else { front.light_level as i32 + v as i32 };
                (level.clamp(0, 255) >> 4) + f.extralight
            }
        };
        if v1.y == v2.y {
            light -= 1;
        } else if v1.x == v2.x {
            light += 1;
        }
        let base_u = fx(seg.offset.0) + fx(side.texture_offset.0);
        let base_row = fx(side.row_offset.0);
        // UDMF per-part offsets: [top, mid, bottom].
        let part = |p: usize| (base_u + side.part_offsets[p][0], base_row + side.part_offsets[p][1]);
        let pegtop = line.flags & doom_content::map::ml::DONTPEGTOP != 0;
        let pegbot = line.flags & doom_content::map::ml::DONTPEGBOTTOM != 0;
        // Texture height in map units (ZDoom texture scale).
        let theight = |t: usize| {
            let sy = f.data.texture_scale.get(t).map_or(1.0, |s| s.1);
            f.data.image(t).map_or(0.0, |i| i.height as f32 / sy)
        };
        let mut drew = false;
        match seg.back_sector.map(|b| level.sector(b)) {
            None => {
                let t = tex(side.mid_texture);
                let (u0, row) = part(1);
                if side.mid_texture.0 != 0 {
                    let top = if pegbot { fl + theight(t) } else { fc };
                    drew |= self.draw_wall(f, (x1, y1, x2, y2), fl, fc, Surface::Texture(t), top + row, u0, light, buf);
                } else {
                    drew |= self.draw_wall(f, (x1, y1, x2, y2), fl, fc, Surface::Black, 0.0, u0, light, buf);
                }
            }
            Some(back) => {
                let bl = fx(back.floor_height.0);
                let bc = fx(back.ceiling_height.0);
                let sky = f.w.sky_flat;
                // Upper wall (skipped between two sky ceilings).
                let both_sky = front.ceiling_pic == sky && back.ceiling_pic == sky;
                if bc < fc && !both_sky {
                    let t = (side.top_texture.0 != 0).then(|| tex(side.top_texture));
                    let (u0, row) = part(0);
                    let top = if pegtop { fc } else { bc + t.map_or(0.0, theight) };
                    let surf = match t {
                        Some(t) => Surface::Texture(t),
                        None => Surface::Bleed { height: fc, pic: front.ceiling_pic, light: sector_light },
                    };
                    drew |= self.draw_wall(f, (x1, y1, x2, y2), bc.max(fl), fc, surf, top + row, u0, light, buf);
                }
                if bl > fl {
                    let (u0, row) = part(2);
                    let top = if pegbot { fc } else { bl };
                    let surf = if side.bottom_texture.0 != 0 {
                        Surface::Texture(tex(side.bottom_texture))
                    } else {
                        Surface::Bleed { height: fl, pic: front.floor_pic, light: sector_light }
                    };
                    drew |= self.draw_wall(f, (x1, y1, x2, y2), fl, bl.min(fc), surf, top + row, u0, light, buf);
                }
                if side.mid_texture.0 != 0 {
                    let t = tex(side.mid_texture);
                    let (u0, row) = part(1);
                    let open_bot = fl.max(bl);
                    let open_top = fc.min(bc);
                    let h = theight(t);
                    let tex_top = if pegbot { open_bot + h } else { open_top } + row;
                    let (bottom, top) = (open_bot.max(tex_top - h), open_top.min(tex_top));
                    if top > bottom {
                        let (mx, my) = ((x1 + x2) / 2.0 - view.eye[0], (y1 + y2) / 2.0 - view.eye[1]);
                        let depth = mx * view.fwd[0] + my * view.fwd[1];
                        masked.push(Masked::Wall { depth, seg: segi, bottom, top, tex: t, tex_top, u0, light });
                    }
                }
                // An open line counts as seen (automap) when any of its
                // opening is visible.
                if !drew {
                    drew = self.probe_wall(f, (x1, y1, x2, y2), fl.max(bl), fc.min(bc));
                }
            }
        }
        // A sky ceiling is infinitely high: above it, the sector's lines show
        // sky. Between two sky sectors the sky starts at the higher ceiling
        // (vanilla's `worldtop = worldhigh`), so the higher sector's walls
        // show above the lower sky and the lower sector's sky hides what is
        // above its ceiling.
        let sky = f.w.sky_flat;
        if front.ceiling_pic == sky {
            let start = match seg.back_sector.map(|b| level.sector(b)) {
                Some(back) if back.ceiling_pic == sky => fc.max(fx(back.ceiling_height.0)),
                _ => fc,
            };
            self.draw_sky_wall(f, (x1, y1, x2, y2), start, start.max(view.eye[2]) + 16384.0, buf);
        }
        if drew {
            let li = seg.linedef.idx();
            if li >= self.mapped_lines.len() {
                self.mapped_lines.resize(li + 1, false);
            }
            self.mapped_lines[li] = true;
        }
    }

    /// Draw one opaque wall piece between heights `bottom` and `top`.
    #[allow(clippy::too_many_arguments)]
    fn draw_wall(
        &mut self,
        f: &Frame,
        (x1, y1, x2, y2): (f32, f32, f32, f32),
        bottom: f32,
        top: f32,
        surface: Surface,
        tex_top: f32,
        u0: f32,
        light: i32,
        buf: &mut [u8],
    ) -> bool {
        if top <= bottom {
            return false;
        }
        let view = f.view;
        let quad = [
            view.to_view([x1, y1, bottom]),
            view.to_view([x2, y2, bottom]),
            view.to_view([x2, y2, top]),
            view.to_view([x1, y1, top]),
        ];
        let clipped = clip_near(&quad);
        if clipped.len() < 3 {
            return false;
        }
        let spoly: Vec<(f32, f32)> = clipped.iter().map(|&v| view.project(v)).collect();
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt().max(1e-6);
        let (ux, uy) = ((x2 - x1) / len, (y2 - y1) / len);
        // Wall plane: normal perpendicular to the seg.
        let n = [uy, -ux, 0.0];
        let plane = Plane { n, d: n[0] * x1 + n[1] * y1 };
        let data = f.data;
        let img = match surface {
            Surface::Texture(t) => data.image(t).filter(|i| i.width > 0 && i.height > 0),
            _ => None,
        };
        let tscale = match surface {
            Surface::Texture(t) => data.texture_scale.get(t).copied().unwrap_or((1.0, 1.0)),
            _ => (1.0, 1.0),
        };
        let bleed_flat = match surface {
            Surface::Bleed { pic, .. } if pic != f.w.sky_flat => flat_src(f, pic),
            _ => FlatSrc::None,
        };
        let identity = doom_content::map::PlaneXform::default();
        let cm = &data.colormaps;
        let eye = view.eye;
        let (zbuf, rays) = (&mut self.zbuf, &self.rays);
        let mut any = false;
        raster(&spoly, view.w, view.h, |x, y| {
            let k = (y * view.w + x) as usize;
            let dir = rays[k];
            let Some(t) = plane.hit(eye, dir) else { return };
            if t >= zbuf[k] {
                return;
            }
            zbuf[k] = t;
            any = true;
            let Some(img) = img else {
                buf[k] = match surface {
                    Surface::Bleed { height, pic, light } => {
                        let pt = Plane { n: [0.0, 0.0, 1.0], d: height }.hit(eye, dir);
                        match pt {
                            Some(_) if pic == f.w.sky_flat => f.sky.texel(dir, &view, x, y),
                            Some(pt) => {
                                let (hx, hy) = (eye[0] + dir[0] * pt, eye[1] + dir[1] * pt);
                                match bleed_flat.texel(hx, hy, &identity) {
                                    Some(texel) => {
                                        let map = f.fixedcolormap.unwrap_or_else(|| {
                                            Self::colormap_dist(light, pt * (dir[0] * view.fwd[0] + dir[1] * view.fwd[1]))
                                        });
                                        cm[map * 256 + texel as usize]
                                    }
                                    None => 0,
                                }
                            }
                            None => 0,
                        }
                    }
                    _ => 0,
                };
                return;
            };
            let hx = eye[0] + dir[0] * t - x1;
            let hy = eye[1] + dir[1] * t - y1;
            let hz = eye[2] + dir[2] * t;
            let u = ((u0 + hx * ux + hy * uy) * tscale.0).floor() as i32;
            let v = ((tex_top - hz) * tscale.1).floor() as i32;
            let texel = img.pixels[(v.rem_euclid(img.height) * img.width + u.rem_euclid(img.width)) as usize];
            let map = match f.fixedcolormap {
                Some(m) => m,
                None => {
                    let dist = t * (dir[0] * view.fwd[0] + dir[1] * view.fwd[1]);
                    Self::colormap_scale(f, light, view.focal_y / dist.max(1.0))
                }
            };
            buf[k] = cm[map * 256 + texel as usize];
        });
        any
    }

    /// Whether any pixel of a wall-shaped quad passes the depth test.
    fn probe_wall(&self, f: &Frame, (x1, y1, x2, y2): (f32, f32, f32, f32), bottom: f32, top: f32) -> bool {
        let view = f.view;
        let top = top.max(bottom + 1.0);
        let quad = [
            view.to_view([x1, y1, bottom]),
            view.to_view([x2, y2, bottom]),
            view.to_view([x2, y2, top]),
            view.to_view([x1, y1, top]),
        ];
        let clipped = clip_near(&quad);
        if clipped.len() < 3 {
            return false;
        }
        let spoly: Vec<(f32, f32)> = clipped.iter().map(|&v| view.project(v)).collect();
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt().max(1e-6);
        let n = [(y2 - y1) / len, -(x2 - x1) / len, 0.0];
        let plane = Plane { n, d: n[0] * x1 + n[1] * y1 };
        let mut seen = false;
        raster(&spoly, view.w, view.h, |x, y| {
            if !seen {
                let k = (y * view.w + x) as usize;
                seen = plane.hit(view.eye, self.rays[k]).is_some_and(|t| t < self.zbuf[k]);
            }
        });
        seen
    }

    /// An occluding wall piece showing the sky.
    fn draw_sky_wall(&mut self, f: &Frame, (x1, y1, x2, y2): (f32, f32, f32, f32), bottom: f32, top: f32, buf: &mut [u8]) {
        let view = f.view;
        let quad = [
            view.to_view([x1, y1, bottom]),
            view.to_view([x2, y2, bottom]),
            view.to_view([x2, y2, top]),
            view.to_view([x1, y1, top]),
        ];
        let clipped = clip_near(&quad);
        if clipped.len() < 3 {
            return;
        }
        let spoly: Vec<(f32, f32)> = clipped.iter().map(|&v| view.project(v)).collect();
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt().max(1e-6);
        let n = [(y2 - y1) / len, -(x2 - x1) / len, 0.0];
        let plane = Plane { n, d: n[0] * x1 + n[1] * y1 };
        let eye = view.eye;
        let (zbuf, rays) = (&mut self.zbuf, &self.rays);
        raster(&spoly, view.w, view.h, |x, y| {
            let k = (y * view.w + x) as usize;
            let dir = rays[k];
            let Some(t) = plane.hit(eye, dir) else { return };
            if t >= zbuf[k] {
                return;
            }
            zbuf[k] = t;
            buf[k] = f.sky.texel(dir, &view, x, y);
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_masked_wall(
        &mut self,
        f: &Frame,
        segi: usize,
        bottom: f32,
        top: f32,
        tex: usize,
        tex_top: f32,
        u0: f32,
        light: i32,
        buf: &mut [u8],
    ) {
        let level = &f.w.level;
        let seg = &level.segs[segi];
        let v1 = level.vertex(seg.v1);
        let v2 = level.vertex(seg.v2);
        let (x1, y1, x2, y2) = (fx(v1.x.0), fx(v1.y.0), fx(v2.x.0), fx(v2.y.0));
        let view = f.view;
        let quad = [
            view.to_view([x1, y1, bottom]),
            view.to_view([x2, y2, bottom]),
            view.to_view([x2, y2, top]),
            view.to_view([x1, y1, top]),
        ];
        let clipped = clip_near(&quad);
        if clipped.len() < 3 {
            return;
        }
        let spoly: Vec<(f32, f32)> = clipped.iter().map(|&v| view.project(v)).collect();
        let len = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt().max(1e-6);
        let (ux, uy) = ((x2 - x1) / len, (y2 - y1) / len);
        let n = [uy, -ux, 0.0];
        let plane = Plane { n, d: n[0] * x1 + n[1] * y1 };
        let data = f.data;
        let Some(img) = data.image(tex).filter(|i| i.width > 0 && i.height > 0) else { return };
        let tscale = data.texture_scale.get(tex).copied().unwrap_or((1.0, 1.0));
        let cm = &data.colormaps;
        let eye = view.eye;
        let (zbuf, rays) = (&self.zbuf, &self.rays);
        raster(&spoly, view.w, view.h, |x, y| {
            let k = (y * view.w + x) as usize;
            let dir = rays[k];
            let Some(t) = plane.hit(eye, dir) else { return };
            if t >= zbuf[k] {
                return;
            }
            let hx = eye[0] + dir[0] * t - x1;
            let hy = eye[1] + dir[1] * t - y1;
            let hz = eye[2] + dir[2] * t;
            let u = ((u0 + hx * ux + hy * uy) * tscale.0).floor() as i32;
            let v = ((tex_top - hz) * tscale.1).floor() as i32;
            if v < 0 || v >= img.height {
                return;
            }
            let idx = (v * img.width + u.rem_euclid(img.width)) as usize;
            if !img.opaque[idx] {
                return;
            }
            let map = match f.fixedcolormap {
                Some(m) => m,
                None => {
                    let dist = t * (dir[0] * view.fwd[0] + dir[1] * view.fwd[1]);
                    Self::colormap_scale(f, light, view.focal_y / dist.max(1.0))
                }
            };
            buf[k] = cm[map * 256 + img.pixels[idx] as usize];
        });
    }

    fn draw_sprite(&mut self, f: &Frame, id: doom_sim::MobjId, buf: &mut [u8]) {
        let w = f.w;
        let Some(mo) = w.thinkers.get_mobj(id) else { return };
        // Never draw the viewer's own body.
        if f.viewer == Some(id) {
            return;
        }
        let data = f.data;
        let Some(def) = data.sprites.get(mo.sprite.idx()) else { return };
        let Some(frame) = def.frames.get((mo.frame & FF_FRAMEMASK) as usize) else { return };
        let view = f.view;
        let (tx, ty, tz) = (fx(mo.x.0), fx(mo.y.0), fx(mo.z.0));
        // Rotation frame (R_ProjectSprite).
        let (lump, flip) = if frame.rotate {
            let ang = doom_core::geom::point_to_angle(
                doom_core::Fixed(mo.x.0 - (view.eye[0] * 65536.0) as i32),
                doom_core::Fixed(mo.y.0 - (view.eye[1] * 65536.0) as i32),
            );
            let rot = (ang.0.wrapping_sub(mo.angle.0).wrapping_add((ANG45 / 2).wrapping_mul(9)) >> 29) as usize;
            (frame.lump[rot] as usize, frame.flip[rot])
        } else {
            (frame.lump[0] as usize, frame.flip[0])
        };
        let Some(sl) = data.sprite_lumps.get(lump) else { return };
        let img = self.sprite_cache.entry(lump).or_insert_with(|| data.sprite_image(lump).unwrap_or_default());
        if img.width <= 0 || img.height <= 0 {
            return;
        }
        let (width, offset, topoff) = (fx(sl.width), fx(sl.offset), fx(sl.topoffset));
        let top = tz + topoff;
        let bottom = top - img.height as f32;
        let r = view.right;
        let left = [tx - r[0] * offset, ty - r[1] * offset];
        let right = [left[0] + r[0] * width, left[1] + r[1] * width];
        let quad = [
            view.to_view([left[0], left[1], bottom]),
            view.to_view([right[0], right[1], bottom]),
            view.to_view([right[0], right[1], top]),
            view.to_view([left[0], left[1], top]),
        ];
        let clipped = clip_near(&quad);
        if clipped.len() < 3 {
            return;
        }
        let spoly: Vec<(f32, f32)> = clipped.iter().map(|&v| view.project(v)).collect();
        let plane = Plane { n: [view.fwd[0], view.fwd[1], 0.0], d: view.fwd[0] * tx + view.fwd[1] * ty };
        let horiz = (tx - view.eye[0]) * view.fwd[0] + (ty - view.eye[1]) * view.fwd[1];
        let sector = w.level.sector(w.level.subsectors[mo.subsector.idx()].sector);
        let light = (sector.light_level as i32 >> 4) + f.extralight;
        let fullbright = mo.frame & doom_sim::info::FF_FULLBRIGHT != 0;
        let map = match f.fixedcolormap {
            Some(m) => m,
            None if fullbright => 0,
            // R_ProjectSprite: xscale >> (LIGHTSCALESHIFT - detailshift).
            None => Self::colormap_scale(f, light, view.focal_y / horiz.max(1.0)),
        };
        let shadow = mo.flags & MF_SHADOW != 0;
        let translation = ((mo.flags & MF_TRANSLATION) >> MF_TRANSSHIFT) as usize;
        let cm = &data.colormaps;
        let eye = view.eye;
        let img = &*img;
        let (zbuf, rays) = (&self.zbuf, &self.rays);
        let mut fuzzpos = self.fuzzpos;
        // Snapshot for fuzz reads (vanilla reads the framebuffer itself).
        let snapshot = if shadow { buf.to_vec() } else { Vec::new() };
        let (vw, vh) = (view.w, view.h);
        raster(&spoly, vw, vh, |x, y| {
            let k = (y * vw + x) as usize;
            let dir = rays[k];
            let Some(t) = plane.hit(eye, dir) else { return };
            if t >= zbuf[k] {
                return;
            }
            let hx = eye[0] + dir[0] * t - left[0];
            let hy = eye[1] + dir[1] * t - left[1];
            let hz = eye[2] + dir[2] * t;
            let mut u = (hx * r[0] + hy * r[1]).floor() as i32;
            let v = (top - hz).floor() as i32;
            if u < 0 || u >= img.width || v < 0 || v >= img.height {
                return;
            }
            if flip {
                u = img.width - 1 - u;
            }
            let idx = (v * img.width + u) as usize;
            if !img.opaque[idx] {
                return;
            }
            if shadow {
                let oy = (y + FUZZOFFSET[fuzzpos]).clamp(0, vh - 1);
                fuzzpos = (fuzzpos + 1) % FUZZOFFSET.len();
                buf[k] = cm[6 * 256 + snapshot[(oy * vw + x) as usize] as usize];
                return;
            }
            let mut texel = img.pixels[idx] as usize;
            if translation > 0 {
                texel = data.translations[(translation - 1) * 256 + texel] as usize;
            }
            buf[k] = cm[map * 256 + texel];
        });
        self.fuzzpos = fuzzpos;
    }
}

/// Where a floor/ceiling's texels come from.
#[derive(Clone, Copy)]
enum FlatSrc<'a> {
    /// A 64x64 flat.
    Flat(&'a [u8]),
    /// A wall texture used as a flat (ZDoom), at its own size and scale.
    Image(&'a TexImage, (f32, f32)),
    None,
}

/// Resolve a flat number: real flats go through the animation translation;
/// numbers past the flat namespace are wall textures (`num_flats + tex`).
/// Draw a scene's triangles (flat and textured) into a view buffer
/// (`buf`, `zbuf`: view sized), depth tested and written.
fn draw_scene_tris(data: &RenderData, view: &View, fixed: Option<usize>, scene: &DebugScene, zbuf: &mut [f32], buf: &mut [u8]) {
    for t in &scene.tris {
        draw_tri(data, view, fixed, t, zbuf, buf);
    }
    for t in &scene.tex_tris {
        if let Some(tex) = scene.textures.get(t.tex as usize) {
            draw_tex_tri(data, view, fixed, t, tex, zbuf, buf);
        }
    }
}

/// A scene's debug lines (depth tested, not written).
fn draw_scene_lines(view: &View, scene: &DebugScene, zbuf: &[f32], buf: &mut [u8]) {
    for l in &scene.lines {
        draw_line(view, l, zbuf, buf);
    }
}

/// Near-clip a triangle in view space, carrying extra attributes (texture
/// coordinates) along the clipped edges. Returns the vertex count (0..=4).
fn clip_tri_attr(v: [VPoint; 3], a: [[f32; 2]; 3], out: &mut [(VPoint, [f32; 2]); 4]) -> usize {
    let mut n = 0;
    for i in 0..3 {
        let (p, q) = (v[i], v[(i + 1) % 3]);
        let (pa, qa) = (a[i], a[(i + 1) % 3]);
        let (pin, qin) = (p.d >= NEAR, q.d >= NEAR);
        if pin {
            out[n] = (p, pa);
            n += 1;
        }
        if pin != qin {
            let k = (NEAR - p.d) / (q.d - p.d);
            let vp = VPoint { a: p.a + (q.a - p.a) * k, u: p.u + (q.u - p.u) * k, d: NEAR };
            out[n] = (vp, [pa[0] + (qa[0] - pa[0]) * k, pa[1] + (qa[1] - pa[1]) * k]);
            n += 1;
        }
    }
    n
}

/// A textured, depth-tested triangle: perspective-correct texture
/// coordinates (u/d, v/d and 1/d are affine in screen space), the mip level
/// from the triangle's texel density on screen.
fn draw_tex_tri(data: &RenderData, view: &View, fixed: Option<usize>, t: &TexTri, tex: &crate::debug_draw::SceneTexture, zbuf: &mut [f32], buf: &mut [u8]) {
    if tex.mips.is_empty() {
        return;
    }
    let v = [view.to_view(t.p[0]), view.to_view(t.p[1]), view.to_view(t.p[2])];
    let mut clipped = [(VPoint { a: 0.0, u: 0.0, d: 0.0 }, [0.0f32; 2]); 4];
    let n = clip_tri_attr(v, t.uv, &mut clipped);
    if n < 3 {
        return;
    }
    let mut screen = [(0.0f32, 0.0f32); 4];
    let mut attr = [[0.0f32; 3]; 4]; // 1/d, u/d, v/d
    let mut depth = 0.0;
    for i in 0..n {
        let (p, uv) = clipped[i];
        screen[i] = view.project(p);
        let iw = 1.0 / p.d;
        attr[i] = [iw, uv[0] * iw, uv[1] * iw];
        depth += p.d / n as f32;
    }
    let ((x0, y0), (x1, y1), (x2, y2)) = (screen[0], screen[1], screen[2]);
    let det = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    // (Back faces are culled by the caller, which knows the normals.)
    if det.abs() < 1e-4 {
        return;
    }
    // Plane coefficients for each attribute: k = a x + b y + c.
    let plane = |k: usize| {
        let a = ((attr[1][k] - attr[0][k]) * (y2 - y0) - (attr[2][k] - attr[0][k]) * (y1 - y0)) / det;
        let b = ((x1 - x0) * (attr[2][k] - attr[0][k]) - (x2 - x0) * (attr[1][k] - attr[0][k])) / det;
        (a, b, attr[0][k] - a * x0 - b * y0)
    };
    let (pw, pu, pv) = (plane(0), plane(1), plane(2));
    // Mip level: texels per pixel from the uv area vs. screen area.
    let (u0, v0) = (clipped[0].1, clipped[1].1);
    let u2 = clipped[2].1;
    let uv_area = ((u0[0] - u2[0]) * (v0[1] - u2[1]) - (v0[0] - u2[0]) * (u0[1] - u2[1])).abs() * 0.5;
    let (w0, h0, _) = tex.mips[0];
    let texels = uv_area * (w0 * h0) as f32;
    let pixels = (det.abs() * 0.5).max(1e-3);
    let level = ((texels / pixels).max(1.0).log2() * 0.5).floor().max(0.0) as usize;
    let cm = match (fixed, t.light) {
        (Some(f), _) => f,
        (None, None) => 0,
        (None, Some(light)) => {
            let base = PerspectiveRenderer::colormap_dist(light as i32 >> 4, depth) as i32;
            (base + t.shade as i32).clamp(0, NUMCOLORMAPS - 1) as usize
        }
    };
    let cmap = &data.colormaps[cm * 256..cm * 256 + 256];
    let w = view.w;
    raster(&screen[..n], view.w, view.h, |x, y| {
        let (xf, yf) = (x as f32, y as f32);
        let iw = pw.0 * xf + pw.1 * yf + pw.2;
        if iw <= 0.0 {
            return;
        }
        let d = 1.0 / iw;
        let k = (y * w + x) as usize;
        if d >= zbuf[k] {
            return;
        }
        let u = (pu.0 * xf + pu.1 * yf + pu.2) * d;
        let v = (pv.0 * xf + pv.1 * yf + pv.2) * d;
        zbuf[k] = d;
        buf[k] = cmap[tex.sample(level, u, v) as usize];
    });
}

impl PerspectiveRenderer {
    /// World billboards ([`SceneSprite`]): the frame for the view
    /// direction, scaled, depth tested against `self.zbuf` (level and
    /// scene triangles) and written to it.
    fn draw_scene_sprites(&mut self, data: &RenderData, view: &View, fixed: Option<usize>, extralight: i32, scene: &DebugScene, buf: &mut [u8]) {
        for s in &scene.sprites {
            self.draw_scene_sprite(data, view, fixed, extralight, s, buf);
        }
    }

    fn draw_scene_sprite(&mut self, data: &RenderData, view: &View, fixed: Option<usize>, extralight: i32, s: &SceneSprite, buf: &mut [u8]) {
        let Some(def) = data.sprites.get(s.sprite as usize) else { return };
        let Some(frame) = def.frames.get(s.frame as usize) else { return };
        let [tx, ty, tz] = s.pos;
        // Rotation frame as R_ProjectSprite picks it.
        let (lump, flip) = if frame.rotate {
            let ang = (ty - view.eye[1]).atan2(tx - view.eye[0]);
            let rel = (ang - s.facing).rem_euclid(std::f32::consts::TAU);
            let rot = (((rel + std::f32::consts::PI + std::f32::consts::FRAC_PI_8) / std::f32::consts::FRAC_PI_4) as usize) % 8;
            (frame.lump[rot] as usize, frame.flip[rot])
        } else {
            (frame.lump[0] as usize, frame.flip[0])
        };
        let Some(sl) = data.sprite_lumps.get(lump) else { return };
        let img = self.sprite_cache.entry(lump).or_insert_with(|| data.sprite_image(lump).unwrap_or_default());
        if img.width <= 0 || img.height <= 0 {
            return;
        }
        let (sx, sy) = (s.scale[0].max(0.01), s.scale[1].max(0.01));
        let (width, offset) = (fx(sl.width) * sx, fx(sl.offset) * sx);
        // Feet at `pos`: the lowest opaque row stands there (frames often
        // have empty rows below the feet).
        let feet_row = *self.sprite_feet.entry(lump).or_insert_with(|| {
            (0..img.height).rev().find(|&y| (0..img.width).any(|x| img.opaque[(y * img.width + x) as usize])).unwrap_or(img.height - 1)
        });
        let height = img.height as f32 * sy;
        let top = tz + (feet_row + 1) as f32 * sy;
        let bottom = top - height;
        let r = view.right;
        let left = [tx - r[0] * offset, ty - r[1] * offset];
        let right = [left[0] + r[0] * width, left[1] + r[1] * width];
        let quad = [
            view.to_view([left[0], left[1], bottom]),
            view.to_view([right[0], right[1], bottom]),
            view.to_view([right[0], right[1], top]),
            view.to_view([left[0], left[1], top]),
        ];
        let mut clipped = [VPoint { a: 0.0, u: 0.0, d: 0.0 }; 8];
        let mut n = 0;
        for i in 0..4 {
            let (p, q) = (quad[i], quad[(i + 1) % 4]);
            let (pin, qin) = (p.d >= NEAR, q.d >= NEAR);
            if pin {
                clipped[n] = p;
                n += 1;
            }
            if pin != qin {
                let k = (NEAR - p.d) / (q.d - p.d);
                clipped[n] = VPoint { a: p.a + (q.a - p.a) * k, u: p.u + (q.u - p.u) * k, d: NEAR };
                n += 1;
            }
        }
        if n < 3 {
            return;
        }
        let mut spoly = [(0.0f32, 0.0f32); 8];
        for i in 0..n {
            spoly[i] = view.project(clipped[i]);
        }
        let plane = Plane { n: [view.fwd[0], view.fwd[1], 0.0], d: view.fwd[0] * tx + view.fwd[1] * ty };
        let horiz = ((tx - view.eye[0]) * view.fwd[0] + (ty - view.eye[1]) * view.fwd[1]).max(1.0);
        let map = match (fixed, s.light) {
            (Some(m), _) => m,
            (None, None) => 0,
            (None, Some(l)) => {
                let light = (l as i32 >> 4) + extralight;
                let startmap = ((LIGHTLEVELS - 1 - light.clamp(0, LIGHTLEVELS - 1)) * 2) * NUMCOLORMAPS / LIGHTLEVELS;
                let j = ((view.focal_y / horiz * 16.0) as i32).clamp(0, MAXLIGHTSCALE - 1);
                let scale_div = SCREENWIDTH as f32 / view.focal_y / 2.0;
                (startmap - (j as f32 * scale_div) as i32 / DISTMAP).clamp(0, NUMCOLORMAPS - 1) as usize
            }
        };
        let cm = &data.colormaps;
        let eye = view.eye;
        let img = &*img;
        let zbuf = &mut self.zbuf;
        let (vw, vh) = (view.w, view.h);
        let bias = s.bias;
        let translation = s.translation as usize;
        raster(&spoly[..n], vw, vh, |x, y| {
            let k = (y * vw + x) as usize;
            let dir = view.ray(x as f32, y as f32);
            let Some(t) = plane.hit(eye, dir) else { return };
            if t - bias >= zbuf[k] {
                return;
            }
            let hx = eye[0] + dir[0] * t - left[0];
            let hy = eye[1] + dir[1] * t - left[1];
            let hz = eye[2] + dir[2] * t;
            let mut u = ((hx * r[0] + hy * r[1]) / sx).floor() as i32;
            let v = ((top - hz) / sy).floor() as i32;
            if u < 0 || u >= img.width || v < 0 || v >= img.height {
                return;
            }
            if flip {
                u = img.width - 1 - u;
            }
            let idx = (v * img.width + u) as usize;
            if !img.opaque[idx] {
                return;
            }
            let mut texel = img.pixels[idx] as usize;
            if translation > 0 {
                texel = data.translations[(translation - 1) * 256 + texel] as usize;
            }
            zbuf[k] = t;
            buf[k] = cm[map * 256 + texel];
        });
    }
}

/// A flat-shaded, depth-tested triangle.
fn draw_tri(data: &RenderData, view: &View, fixed: Option<usize>, t: &DebugTri, zbuf: &mut [f32], buf: &mut [u8]) {
    let v = [view.to_view(t.p[0]), view.to_view(t.p[1]), view.to_view(t.p[2])];
    // Clip to the near plane on the stack: one plane adds at most one vertex.
    let mut poly = [VPoint { a: 0.0, u: 0.0, d: 0.0 }; 4];
    let mut n = 0;
    for i in 0..3 {
        let (p, q) = (v[i], v[(i + 1) % 3]);
        let (pin, qin) = (p.d >= NEAR, q.d >= NEAR);
        if pin {
            poly[n] = p;
            n += 1;
        }
        if pin != qin {
            let k = (NEAR - p.d) / (q.d - p.d);
            poly[n] = VPoint { a: p.a + (q.a - p.a) * k, u: p.u + (q.u - p.u) * k, d: NEAR };
            n += 1;
        }
    }
    if n < 3 {
        return;
    }
    let mut screen = [(0.0f32, 0.0f32); 4];
    let mut inv = [0.0f32; 4];
    let mut depth = 0.0;
    for i in 0..n {
        screen[i] = view.project(poly[i]);
        inv[i] = 1.0 / poly[i].d;
        depth += poly[i].d / n as f32;
    }
    // 1/depth is affine in screen space: solve w = a x + b y + c.
    let ((x0, y0), (x1, y1), (x2, y2)) = (screen[0], screen[1], screen[2]);
    let det = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if det.abs() < 1e-4 {
        return;
    }
    let a = ((inv[1] - inv[0]) * (y2 - y0) - (inv[2] - inv[0]) * (y1 - y0)) / det;
    let b = ((x1 - x0) * (inv[2] - inv[0]) - (x2 - x0) * (inv[1] - inv[0])) / det;
    let c = inv[0] - a * x0 - b * y0;
    let cm = match (fixed, t.light) {
        (Some(f), _) => f,
        (None, None) => 0,
        (None, Some(light)) => {
            let base = PerspectiveRenderer::colormap_dist(light as i32 >> 4, depth) as i32;
            (base + t.shade as i32).clamp(0, NUMCOLORMAPS - 1) as usize
        }
    };
    let color = data.colormaps.get(cm * 256 + t.color as usize).copied().unwrap_or(t.color);
    let w = view.w;
    raster(&screen[..n], view.w, view.h, |x, y| {
        let iw = a * x as f32 + b * y as f32 + c;
        if iw <= 0.0 {
            return;
        }
        let d = 1.0 / iw;
        let k = (y * w + x) as usize;
        if d < zbuf[k] {
            zbuf[k] = d;
            buf[k] = color;
        }
    });
}

/// A one-pixel line, depth tested (with a little bias so lines lying on
/// walls and floors show) unless it is x-ray.
fn draw_line(view: &View, l: &DebugLine, zbuf: &[f32], buf: &mut [u8]) {
    let (mut p, mut q) = (view.to_view(l.a), view.to_view(l.b));
    if p.d < NEAR && q.d < NEAR {
        return;
    }
    let lerp = |p: VPoint, q: VPoint| {
        let k = (NEAR - p.d) / (q.d - p.d);
        VPoint { a: p.a + (q.a - p.a) * k, u: p.u + (q.u - p.u) * k, d: NEAR }
    };
    if p.d < NEAR {
        p = lerp(p, q);
    } else if q.d < NEAR {
        q = lerp(q, p);
    }
    let ((x0, y0), (x1, y1)) = (view.project(p), view.project(q));
    let (w0, w1) = (1.0 / p.d, 1.0 / q.d);
    let (dx, dy) = (x1 - x0, y1 - y0);
    // Liang-Barsky against the buffer.
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let (wmax, hmax) = (view.w as f32 - 0.001, view.h as f32 - 0.001);
    for (pk, qk) in [(-dx, x0), (dx, wmax - x0), (-dy, y0), (dy, hmax - y0)] {
        if pk == 0.0 {
            if qk < 0.0 {
                return;
            }
        } else {
            let r = qk / pk;
            if pk < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if t0 > t1 {
        return;
    }
    let steps = ((dx.abs().max(dy.abs()) * (t1 - t0)).ceil() as i32).clamp(1, 4096);
    for i in 0..=steps {
        let t = t0 + (t1 - t0) * i as f32 / steps as f32;
        let (x, y) = ((x0 + dx * t) as i32, (y0 + dy * t) as i32);
        if x < 0 || y < 0 || x >= view.w || y >= view.h {
            continue;
        }
        let k = (y * view.w + x) as usize;
        let d = 1.0 / (w0 + (w1 - w0) * t);
        if l.xray || d - (2.0 + d * 0.02) <= zbuf[k] {
            buf[k] = l.color;
        }
    }
}

fn flat_src<'a>(f: &Frame<'a>, pic: doom_sim::ids::FlatNum) -> FlatSrc<'a> {
    let data = f.data;
    let n = pic.0 as usize;
    if n < data.num_flats {
        return f.w.flat_translation.get(n).and_then(|&t| data.flat(t as usize)).map_or(FlatSrc::None, |fl| FlatSrc::Flat(fl));
    }
    let tex = n - data.num_flats;
    let tex = f.w.texture_translation.get(tex).map_or(tex, |&t| t as usize);
    match data.image(tex).filter(|i| i.width > 0 && i.height > 0) {
        Some(img) => FlatSrc::Image(img, data.texture_scale.get(tex).copied().unwrap_or((1.0, 1.0))),
        None => FlatSrc::None,
    }
}

impl FlatSrc<'_> {
    /// The texel at world (x, y), with UDMF panning/scale/rotation.
    #[inline]
    fn texel(&self, x: f32, y: f32, xf: &doom_content::map::PlaneXform) -> Option<u8> {
        // Flats are world-aligned with v pointing south (-y).
        let (mut u, mut v) = (x, -y);
        if xf.rotation != 0.0 {
            let (s, c) = xf.rotation.to_radians().sin_cos();
            (u, v) = (u * c - v * s, u * s + v * c);
        }
        u = u * xf.x_scale + xf.x_offset;
        v = v * xf.y_scale + xf.y_offset;
        match *self {
            FlatSrc::Flat(fl) => Some(fl[((v.floor() as i32 & 63) * 64 + (u.floor() as i32 & 63)) as usize]),
            FlatSrc::Image(img, (sx, sy)) => {
                let (u, v) = ((u * sx).floor() as i32, (v * sy).floor() as i32);
                Some(img.pixels[(v.rem_euclid(img.height) * img.width + u.rem_euclid(img.width)) as usize])
            }
            FlatSrc::None => None,
        }
    }
}

/// The current sky texture and the solid colours that cap it above and
/// below (vanilla never shows more than its rows; looking further up or
/// down fades into the average of the top/bottom row, like GZDoom).
#[derive(Clone, Copy)]
struct Sky<'a> {
    img: Option<&'a TexImage>,
    /// Colormap 0: the sky is always full bright, even under the
    /// invulnerability colormap (vanilla quirk kept: `R_DrawPlanes`).
    cm0: &'a [u8],
    top: u8,
    bottom: u8,
}

/// 4x4 ordered-dither thresholds (0..16).
const BAYER4: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
/// Rows over which the sky fades into its cap colour.
const SKY_FADE: f32 = 24.0;

impl Sky<'_> {
    /// The sky texel seen along `dir` at view pixel (x, y). Vanilla mapping:
    /// 1024 columns per turn, row 100 at the horizon, one texel per pixel
    /// at full view width. The sky is always full bright.
    fn texel(&self, dir: [f32; 3], view: &View, x: i32, y: i32) -> u8 {
        let Some(img) = self.img else { return self.cm0[0] };
        let ang = dir[1].atan2(dir[0]);
        let bam = (ang / std::f32::consts::TAU * 4294967296.0) as i64 as u32;
        let u = (bam >> 22) as i32;
        let fwd = dir[0] * view.fwd[0] + dir[1] * view.fwd[1];
        let v = if fwd > 1e-4 { 100.0 - 160.0 * dir[2] / fwd } else if dir[2] > 0.0 { -1e6 } else { 1e6 };
        let h = img.height as f32;
        let dither = (BAYER4[((y & 3) * 4 + (x & 3)) as usize] as f32 + 0.5) / 16.0;
        let texel = if v < 0.0 && -v / SKY_FADE > dither {
            self.top
        } else if v >= h && (v - h) / SKY_FADE > dither {
            self.bottom
        } else {
            let v = (v.floor() as i32).clamp(0, img.height - 1);
            img.pixels[(v * img.width + u.rem_euclid(img.width)) as usize]
        };
        self.cm0[texel as usize]
    }
}

/// The palette index closest to the average colour of one texture row.
fn row_average(data: &RenderData, img: &TexImage, row: i32) -> u8 {
    let pal = &data.palettes.palettes[0];
    let mut sum = [0u32; 3];
    for x in 0..img.width {
        let c = pal[img.pixels[(row * img.width + x) as usize] as usize];
        for i in 0..3 {
            sum[i] += c[i] as u32;
        }
    }
    let avg = sum.map(|s| (s / img.width.max(1) as u32) as i32);
    (0..256)
        .min_by_key(|&i| {
            let c = pal[i];
            (0..3).map(|k| (c[k] as i32 - avg[k]).pow(2)).sum::<i32>()
        })
        .unwrap_or(0) as u8
}

