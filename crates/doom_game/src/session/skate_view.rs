//! The skateboard mode's presentation: the third-person chase camera, the
//! skateboard model and the Doomguy riding it, and the debug views (grind
//! edges, collision boxes, velocity, wireframe) with their text readouts.
//!
//! The board is the real model from `RESOURCES/skateboard.model`
//! ([`crate::skate_model`]), drawn as textured triangles on the skate
//! physics' board pose. The rider is the game's own Doomguy sprite (`PLAY`
//! frames) as a world billboard standing on the deck, with rotations,
//! push / crouch / air / bail poses. Both go through the perspective
//! renderer's extra-geometry pass ([`doom_render::DebugScene`]), depth
//! tested against the level.
//!
//! The simulation runs at 35 Hz; the camera, board and rider are drawn
//! from a pose interpolated between the last two tics at the frame's
//! position between them ([`Session::set_frame_time`]), and the camera
//! smooths toward its target with the real frame time, so nothing steps
//! at 35 Hz on a faster display.

use doom_core::{Fixed, FRACUNIT};
use doom_render::{Camera, DebugScene, SceneSprite, TexTri};
use doom_sim::grind::{ConnKind, GrindKind};
use doom_sim::level::Level;
use doom_sim::mobj::flags::{MF_SHOOTABLE, MF_SOLID, MF_SPECIAL};
use doom_sim::skate::{SkateState, Skater, DECK_HEIGHT};

use super::{GameState, Session};
use crate::console::{draw_text_shadow, LINE_H};
use crate::cvar;
use crate::skate_model::BoardModel;

/// The chase camera's smoothed state.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ChaseCam {
    pub pos: [f32; 3],
    /// Smoothed direction the camera trails.
    pub yaw: f32,
    /// Current distance behind the focus (pulled in by walls at once,
    /// eased back out).
    pub dist: f32,
    /// Point looked at.
    pub look: [f32; 3],
    /// False: jump straight to the target next update (after a level load
    /// or teleport).
    pub valid: bool,
}

/// Camera tuning (the `skate_camera_*` cvars).
#[derive(Clone, Copy, Debug)]
pub(super) struct CameraTuning {
    pub distance: f32,
    pub height: f32,
    pub look_height: f32,
    /// Position and rotation smoothing rates (per second).
    pub follow: f32,
    pub rotation: f32,
    pub radius: f32,
    pub min_distance: f32,
    pub max_distance: f32,
}

/// Palette indices for the fallback board (no model asset).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ModelColors {
    deck: u8,
    grip: u8,
    truck: u8,
    wheel: u8,
}

impl ModelColors {
    pub fn from_playpal(pal: &[[u8; 3]; 256]) -> ModelColors {
        let n = |rgb| crate::console::nearest(pal, rgb);
        ModelColors {
            deck: n([170, 110, 55]),
            grip: n([45, 45, 45]),
            truck: n([175, 175, 180]),
            wheel: n([235, 225, 190]),
        }
    }
}

#[inline]
fn fx(v: Fixed) -> f32 {
    v.0 as f32 / FRACUNIT as f32
}

fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let mut a = a % t;
    if a > std::f32::consts::PI {
        a -= t;
    } else if a < -std::f32::consts::PI {
        a += t;
    }
    a
}

type V3 = [f32; 3];

#[inline]
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
#[inline]
fn dot3(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
fn norm(a: V3) -> V3 {
    let l = dot3(a, a).sqrt();
    if l < 1e-6 {
        [0.0, 0.0, 1.0]
    } else {
        mul(a, 1.0 / l)
    }
}

/// How far (0..1) the segment `a`→`b` gets before a wall, a closed opening
/// or a floor/ceiling step blocks it.
pub(super) fn trace_clip(level: &Level, a: V3, b: V3) -> f32 {
    let bm = &level.blockmap;
    if bm.width == 0 {
        return 1.0;
    }
    let (ox, oy) = (fx(bm.org_x), fx(bm.org_y));
    let cell = |v: f32, o: f32, n: i32| (((v - o) / 128.0).floor() as i32).clamp(0, n - 1);
    let (cx0, cx1) = (cell(a[0].min(b[0]), ox, bm.width), cell(a[0].max(b[0]), ox, bm.width));
    let (cy0, cy1) = (cell(a[1].min(b[1]), oy, bm.height), cell(a[1].max(b[1]), oy, bm.height));
    let d = [b[0] - a[0], b[1] - a[1]];
    let mut best = 1.0f32;
    for cy in cy0..=cy1 {
        for cx in cx0..=cx1 {
            for &l in bm.block_list(cx, cy) {
                let Some(line) = level.lines.get(l as usize) else { continue };
                let (v1, v2) = (level.vertex(line.v1), level.vertex(line.v2));
                let (p1, p2) = ([fx(v1.x), fx(v1.y)], [fx(v2.x), fx(v2.y)]);
                let e = [p2[0] - p1[0], p2[1] - p1[1]];
                let den = d[0] * e[1] - d[1] * e[0];
                if den.abs() < 1e-6 {
                    continue;
                }
                let w = [p1[0] - a[0], p1[1] - a[1]];
                let t = (w[0] * e[1] - w[1] * e[0]) / den;
                let u = (w[0] * d[1] - w[1] * d[0]) / den;
                if !(0.0..=best).contains(&t) || !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let z = a[2] + (b[2] - a[2]) * t;
                let blocked = match (line.front_sector, line.back_sector) {
                    (Some(f), Some(bk)) => {
                        let (fs, bs) = (level.sector(f), level.sector(bk));
                        let lo = fx(fs.floor_height.max(bs.floor_height));
                        let hi = fx(fs.ceiling_height.min(bs.ceiling_height));
                        z < lo + 4.0 || z > hi - 4.0
                    }
                    _ => true,
                };
                if blocked {
                    best = t;
                }
            }
        }
    }
    best
}

/// A box: centre, unit axes (any orientation) and half sizes along them.
fn add_box(scene: &mut DebugScene, c: V3, ax: [V3; 3], half: [f32; 3], color: u8, light: i16) {
    let corner = |i: usize| {
        let s = |bit: usize| if i & bit != 0 { 1.0 } else { -1.0 };
        add(add(add(c, mul(ax[0], half[0] * s(1))), mul(ax[1], half[1] * s(2))), mul(ax[2], half[2] * s(4)))
    };
    // Light from above, slightly in front: tops brightest.
    let sun = norm([0.35, 0.25, 0.9]);
    for k in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let n = mul(ax[k], sign);
            let shade = ((1.0 - dot3(n, sun).max(0.0)) * 7.0).round() as i8 - 1;
            // The four corners with this axis bit fixed, in loop order.
            let bit = 1 << k;
            let (o1, o2) = match k {
                0 => (2, 4),
                1 => (1, 4),
                _ => (1, 2),
            };
            let base = if sign > 0.0 { bit } else { 0 };
            let q = [corner(base), corner(base | o1), corner(base | o1 | o2), corner(base | o2)];
            scene.tri([q[0], q[1], q[2]], color, shade, Some(light));
            scene.tri([q[0], q[2], q[3]], color, shade, Some(light));
        }
    }
}

/// The board's local axes (forward, right, up) from its pose.
fn board_axes(sk: &Skater) -> [V3; 3] {
    let b = &sk.board;
    let f0 = [b.yaw.cos(), b.yaw.sin(), 0.0];
    let r0 = [b.yaw.sin(), -b.yaw.cos(), 0.0];
    let u0 = [0.0, 0.0, 1.0];
    // Pitch about the right axis (nose up > 0).
    let (cp, sp) = (b.pitch.cos(), b.pitch.sin());
    let f1 = add(mul(f0, cp), mul(u0, sp));
    let u1 = sub(mul(u0, cp), mul(f0, sp));
    // Roll (and flips) about the forward axis.
    let roll = b.roll + b.flip;
    let (cr, sr) = (roll.cos(), roll.sin());
    let r2 = sub(mul(r0, cr), mul(u1, sr));
    let u2 = add(mul(u1, cr), mul(r0, sr));
    [f1, r2, u2]
}

/// The placeholder skateboard: deck with kicktails, grip tape, trucks,
/// wheels.
fn add_board(scene: &mut DebugScene, sk: &Skater, c: &ModelColors, light: i16) {
    let [f, r, u] = board_axes(sk);
    let len = sk.board.length;
    let base = sk.board.pos;
    let deck_c = add(base, mul(u, DECK_HEIGHT));
    let flat = len * 0.36;
    add_box(scene, deck_c, [f, r, u], [flat, 4.5, 0.6], c.deck, light);
    add_box(scene, add(deck_c, mul(u, 0.7)), [f, r, u], [flat - 0.5, 4.2, 0.15], c.grip, light);
    // Kicktails, tipped up at both ends.
    for sign in [1.0f32, -1.0] {
        let kick = 0.38 * sign;
        let tf = norm(add(mul(f, kick.cos()), mul(u, kick.sin() * sign)));
        let tu = cross(r, tf);
        let tail_len = len * 0.5 - flat;
        let tc = add(add(deck_c, mul(f, sign * flat)), mul(tf, sign * tail_len * 0.5));
        add_box(scene, tc, [tf, r, tu], [tail_len * 0.5, 4.3, 0.6], c.deck, light);
    }
    // Trucks and wheels.
    let wb = len * 0.3;
    for sign in [1.0f32, -1.0] {
        let tc = add(add(base, mul(f, sign * wb)), mul(u, 2.6));
        add_box(scene, tc, [f, r, u], [1.0, 3.8, 0.9], c.truck, light);
        for side in [1.0f32, -1.0] {
            let wc = add(add(add(base, mul(f, sign * wb)), mul(r, side * 4.2)), mul(u, 1.4));
            add_box(scene, wc, [f, r, u], [1.4, 0.9, 1.4], c.wheel, light);
        }
    }
}

/// The skateboard model on the board pose: back faces culled with the
/// model's normals, each face shaded by its angle to a light from above.
fn add_model_board(scene: &mut DebugScene, sk: &Skater, model: &BoardModel, light: i16, eye: V3) {
    let [f, r, u] = board_axes(sk);
    let k = sk.board.length / model.length.max(1e-3);
    let base = sk.board.pos;
    let tex = scene.texture(&model.texture);
    let sun = norm([0.35, 0.25, 0.9]);
    let world = |p: [f32; 3]| add(add(add(base, mul(f, p[0] * k)), mul(r, p[1] * k)), mul(u, p[2] * k));
    for t in &model.tris {
        let n = add(add(mul(f, t.n[0]), mul(r, t.n[1])), mul(u, t.n[2]));
        let p0 = world(t.p[0]);
        if dot3(n, sub(p0, eye)) >= 0.0 {
            continue; // facing away
        }
        let shade = ((1.0 - dot3(n, sun).max(0.0)) * 6.0).round() as i8 - 1;
        scene.tex_tris.push(TexTri { p: [p0, world(t.p[1]), world(t.p[2])], uv: t.uv, tex, shade, light: Some(light) });
    }
}

/// Doomguy standing on the deck: the `PLAY` sprite as a billboard, with
/// a frame for what the skater is doing.
fn add_rider_sprite(scene: &mut DebugScene, w: &doom_sim::World, sk: &Skater, mo: &doom_sim::mobj::Mobj, deck_top: f32, light: i16) {
    use doom_sim::info::{FF_FRAMEMASK, S_PLAY, S_PLAY_ATK1, S_PLAY_ATK2, S_PLAY_PAIN, S_PLAY_PAIN2};
    let sprite = w.info.state(S_PLAY).sprite.0;
    let k = sk.board.length / 36.0;
    let (frame, scale_y, pos) = if sk.state == SkateState::Bail {
        // Knocked down: the death frames (H..N), getting up at the end.
        let t = sk.state_tics as i32;
        let frame = if t > 44 { 0 } else { 7 + (t / 5).min(5) };
        (frame as u8, 1.0, sk.pos)
    } else {
        // Shooting and getting hurt show as in the game.
        let firing = [S_PLAY_ATK1, S_PLAY_ATK2, S_PLAY_PAIN, S_PLAY_PAIN2].contains(&mo.state);
        let frame = if firing {
            (mo.frame & FF_FRAMEMASK) as u8
        } else {
            match sk.state {
                // A push stroke walks the legs through the run cycle.
                SkateState::Pushing => ((sk.push_phase / 5) % 4) as u8,
                SkateState::Ollie | SkateState::Airborne => 1,
                SkateState::Braking => 2,
                _ => 0,
            }
        };
        // Crouching (charging an ollie, landing) squashes the sprite; the
        // pop and the air stretch it a little.
        let stretch = match sk.state {
            SkateState::Ollie => 1.06,
            SkateState::Airborne => 1.03,
            _ => 1.0,
        };
        let [_, _, u] = board_axes(sk);
        (frame, (1.0 - 0.3 * sk.crouch) * stretch, add(sk.board.pos, mul(u, deck_top * k)))
    };
    // A skater stands sideways on the deck, chest to the toe-side edge
    // (the board's right for a regular stance).
    let facing = sk.heading - std::f32::consts::FRAC_PI_2;
    let translation = ((mo.flags & doom_sim::mobj::flags::MF_TRANSLATION) >> doom_sim::mobj::flags::MF_TRANSSHIFT) as u8;
    scene.sprites.push(SceneSprite {
        pos,
        sprite,
        frame,
        facing,
        scale: [1.0 + 0.08 * sk.crouch, scale_y.max(0.4)],
        light: Some(light),
        translation,
        bias: 6.0,
    });
}

/// Interpolate two skater poses (`a` at the previous tic, `b` at the
/// latest) at `t` in 0..=1.
fn lerp_skater(a: &Skater, b: &Skater, t: f32) -> Skater {
    let mut out = b.clone();
    // A jump (teleport, warp) is not smoothed.
    let jump = ((b.pos[0] - a.pos[0]).powi(2) + (b.pos[1] - a.pos[1]).powi(2)).sqrt();
    if jump > 64.0 {
        return out;
    }
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let lerp_angle = |x: f32, y: f32| x + wrap(y - x) * t;
    for k in 0..3 {
        out.pos[k] = lerp(a.pos[k], b.pos[k]);
        out.vel[k] = lerp(a.vel[k], b.vel[k]);
        out.board.pos[k] = lerp(a.board.pos[k], b.board.pos[k]);
    }
    out.heading = lerp_angle(a.heading, b.heading);
    out.board.yaw = lerp_angle(a.board.yaw, b.board.yaw);
    out.board.pitch = lerp_angle(a.board.pitch, b.board.pitch);
    out.board.roll = lerp_angle(a.board.roll, b.board.roll);
    out.crouch = lerp(a.crouch, b.crouch);
    out.lean = lerp(a.lean, b.lean);
    out
}

impl Session {
    /// The skateboard is being ridden with the chase camera.
    pub(super) fn third_person(&self) -> bool {
        self.state == GameState::Level
            && self.world.console_player().skater.is_some()
            && self.world.console_player().playerstate != doom_sim::player::PlayerState::Dead
    }

    /// The skater as drawn this frame: between the last two tics' poses
    /// at the frame's position between them.
    pub(super) fn render_skater(&self) -> Option<Skater> {
        let cur = self.world.console_player().skater.as_deref()?;
        Some(match &self.skate_prev {
            Some(prev) if self.frame_timed => lerp_skater(prev, cur, self.frame_alpha),
            _ => cur.clone(),
        })
    }

    /// Remember the pose before a game tic (for interpolation).
    pub(super) fn snapshot_skater(&mut self) {
        self.skate_prev = self.world.console_player().skater.as_deref().cloned();
    }

    pub(super) fn camera_tuning(&self) -> CameraTuning {
        let c = &self.cvars;
        let min = c.get(cvar::SKATE_CAMERA_MIN_DISTANCE);
        let max = c.get(cvar::SKATE_CAMERA_MAX_DISTANCE).max(min);
        CameraTuning {
            distance: c.get(cvar::SKATE_CAMERA_DISTANCE),
            height: c.get(cvar::SKATE_CAMERA_HEIGHT),
            look_height: c.get(cvar::SKATE_CAMERA_LOOK_HEIGHT),
            follow: c.get(cvar::SKATE_CAMERA_FOLLOW),
            rotation: c.get(cvar::SKATE_CAMERA_ROTATION),
            radius: c.get(cvar::SKATE_CAMERA_RADIUS),
            min_distance: min,
            max_distance: max,
        }
    }

    /// Move the chase camera toward its target over `dt` seconds, following
    /// the interpolated pose.
    pub(super) fn update_chase(&mut self, dt: f32) {
        if !self.third_person() {
            self.chase.valid = false;
            return;
        }
        let Some(sk) = self.render_skater() else { return };
        let tune = self.camera_tuning();
        let focus = [sk.pos[0], sk.pos[1], sk.pos[2] + tune.look_height - 6.0 * sk.crouch];
        let hs = (sk.vel[0] * sk.vel[0] + sk.vel[1] * sk.vel[1]).sqrt();
        let travel = match (&sk.grind, &self.world.skate.grind) {
            (Some(g), Some(graph)) => {
                let e = &graph.edges[g.edge as usize];
                (e.tangent[1] * g.dir).atan2(e.tangent[0] * g.dir)
            }
            _ if hs > 2.0 && sk.state != SkateState::Bail => sk.vel[1].atan2(sk.vel[0]),
            _ if sk.fakie => sk.heading + std::f32::consts::PI,
            _ => sk.heading,
        };
        // Farther back at speed, within the configured range.
        let want = (tune.distance + hs * 35.0 * 0.04).clamp(tune.min_distance, tune.max_distance);
        // Frame-rate independent smoothing factors.
        let follow = 1.0 - (-tune.follow.max(0.1) * dt).exp();
        let turn = 1.0 - (-tune.rotation.max(0.1) * dt).exp();
        let level = &self.world.level;
        let c = &mut self.chase;
        if !c.valid {
            c.yaw = travel;
            c.dist = want;
        }
        // Turn toward the direction of travel, capped so a fakie switch or
        // a sharp corner swings round instead of snapping.
        let dyaw = (wrap(travel - c.yaw) * turn).clamp(-4.0 * dt.max(1e-3), 4.0 * dt.max(1e-3));
        c.yaw = wrap(c.yaw + dyaw);
        let back = [-c.yaw.cos(), -c.yaw.sin()];
        // How far back there is room for (walls pull the camera in).
        let full = [focus[0] + back[0] * want, focus[1] + back[1] * want, sk.pos[2] + tune.height];
        let room = trace_clip(level, focus, full) * want - tune.radius;
        let allowed = room.clamp(8.0, want);
        c.dist = if allowed < c.dist { allowed } else { c.dist + (allowed - c.dist) * follow };
        let height = tune.height * (c.dist / want.max(1.0)).clamp(0.4, 1.0);
        let ideal = [focus[0] + back[0] * c.dist, focus[1] + back[1] * c.dist, sk.pos[2] + height];
        if !c.valid {
            c.pos = ideal;
            c.valid = true;
        } else {
            for k in 0..3 {
                c.pos[k] += (ideal[k] - c.pos[k]) * follow;
            }
        }
        // Never through a wall: if smoothing put the camera behind one,
        // bring it in front at once.
        let t = trace_clip(level, focus, c.pos);
        if t < 1.0 {
            let d = sub(c.pos, focus);
            let len = dot3(d, d).sqrt().max(1.0);
            c.pos = add(focus, mul(d, ((t * len - tune.radius) / len).max(0.05)));
        }
        let (floor, ceil) = {
            let ss = doom_sim::geometry::point_in_subsector(level, to_fixed(c.pos[0]), to_fixed(c.pos[1]));
            let s = level.sector(level.subsectors[ss.idx()].sector);
            (fx(s.floor_height), fx(s.ceiling_height))
        };
        c.pos[2] = c.pos[2].clamp(floor + 6.0, (ceil - 6.0).max(floor + 6.0));
        // Look a little ahead of the skater, slightly above the board.
        c.look = [focus[0] + travel.cos() * 16.0, focus[1] + travel.sin() * 16.0, focus[2]];
    }

    /// The chase camera as a renderer camera.
    pub(super) fn chase_camera(&self) -> Option<Camera> {
        if !self.chase.valid {
            return None;
        }
        let c = &self.chase;
        let d = sub(c.look, c.pos);
        let h = (d[0] * d[0] + d[1] * d[1]).sqrt().max(0.01);
        let pitch = (d[2] * doom_render::PIXEL_ASPECT / h).atan().clamp(-0.75, 0.6);
        Some(Camera { x: c.pos[0], y: c.pos[1], z: c.pos[2], yaw: d[1].atan2(d[0]), pitch })
    }

    /// Fill `self.scene` with the model and debug geometry for this frame.
    pub(super) fn build_scene(&mut self, cam: &Camera) {
        let mut scene = std::mem::take(&mut self.scene);
        scene.clear();
        let w = &self.world;
        let cp = w.params.consoleplayer;
        let pal = self.con_palette;
        let d = self.debug;
        if d.wireframe {
            scene.clear_view = Some(pal.black);
            add_wireframe(&mut scene, &w.level, cam, &pal);
        }
        let player = &w.players[cp];
        let light = player
            .mo
            .and_then(|mo| w.thinkers.get_mobj(mo))
            .map_or(160, |m| w.level.sector(w.level.subsectors[m.subsector.idx()].sector).light_level);
        if let (Some(sk), Some(mo)) = (self.render_skater().filter(|_| self.third_person()), player.mo.and_then(|mo| w.thinkers.get_mobj(mo))) {
            let deck_top = match &self.board_model {
                Some(model) => {
                    add_model_board(&mut scene, &sk, model, light, [cam.x, cam.y, cam.z]);
                    model.deck_top
                }
                // No model asset: a plain box board stands in.
                None => {
                    add_board(&mut scene, &sk, &self.model_colors, light);
                    DECK_HEIGHT + 1.5
                }
            };
            add_rider_sprite(&mut scene, w, &sk, mo, deck_top, light);
        }
        if d.show_grindables {
            if let Some(graph) = &w.skate.grind {
                let active = player.skater.as_ref().and_then(|s| s.grind.map(|g| g.edge));
                let r = 1536.0;
                let mut drawn = 0usize;
                graph.for_each_in_box([cam.x - r, cam.y - r], [cam.x + r, cam.y + r], |e| {
                    if drawn > 6000 {
                        return;
                    }
                    drawn += 1;
                    let edge = &graph.edges[e as usize];
                    let z = edge.top_z(&w.level) + 0.5;
                    let color = if Some(e) == active {
                        pal.green
                    } else {
                        match edge.kind {
                            GrindKind::Ledge => pal.yellow,
                            GrindKind::Curb => pal.orange,
                            GrindKind::Bar => pal.magenta,
                            GrindKind::Rail => pal.cyan,
                        }
                    };
                    let (a, b) = ([edge.a[0], edge.a[1], z], [edge.b[0], edge.b[1], z]);
                    scene.line(a, b, color, false);
                    // Ticks toward the low side show which way the edge faces.
                    let m = edge.point(edge.length * 0.5);
                    scene.line([m[0], m[1], z], [m[0] + edge.normal[0] * 6.0, m[1] + edge.normal[1] * 6.0, z], pal.gray, false);
                    // Corner connections get a post at the joint.
                    for end in 0..2 {
                        let conns = graph.connections_at(e, end);
                        let p = if end == 0 { a } else { b };
                        if conns.iter().any(|c| c.kind == ConnKind::Corner) {
                            scene.line(p, [p[0], p[1], p[2] + 6.0], pal.white, false);
                        } else if conns.is_empty() {
                            scene.line(p, [p[0], p[1], p[2] + 3.0], pal.red, false);
                        }
                    }
                });
            }
        }
        if d.show_collision {
            add_collision(&mut scene, w, cp, cam, &pal);
        }
        if let Some(mo) = player.mo.and_then(|mo| w.thinkers.get_mobj(mo)) {
            let base = [fx(mo.x), fx(mo.y), fx(mo.z)];
            if d.show_velocity {
                let (v, heading) = match player.skater.as_deref() {
                    Some(sk) => (sk.vel, Some(sk.heading)),
                    None => ([fx(mo.momx), fx(mo.momy), fx(mo.momz)], None),
                };
                let o = add(base, [0.0, 0.0, 20.0]);
                let tip = add(o, mul(v, 6.0));
                scene.line(o, tip, pal.green, true);
                scene.cross(tip, 2.0, pal.green, true);
                if let Some(h) = heading {
                    scene.line(o, add(o, [h.cos() * 24.0, h.sin() * 24.0, 0.0]), pal.blue, true);
                }
            }
            if d.skate_debug {
                if let Some(sk) = player.skater.as_deref() {
                    let [f, r, u] = board_axes(sk);
                    let c = add(sk.board.pos, [0.0, 0.0, DECK_HEIGHT]);
                    scene.line(c, add(c, mul(f, 16.0)), pal.red, true);
                    scene.line(c, add(c, mul(r, 10.0)), pal.green, true);
                    scene.line(c, add(c, mul(u, 10.0)), pal.blue, true);
                    let half_wb = sk.board.length * 0.3;
                    let fl = [sk.heading.cos(), sk.heading.sin()];
                    let lf = [-fl[1], fl[0]];
                    for (i, (a, b)) in [(half_wb, 3.5), (half_wb, -3.5), (-half_wb, 3.5), (-half_wb, -3.5)].iter().enumerate() {
                        let p = [sk.pos[0] + fl[0] * a + lf[0] * b, sk.pos[1] + fl[1] * a + lf[1] * b, sk.board.wheel_floor[i]];
                        scene.cross(p, 1.5, pal.yellow, true);
                    }
                }
            }
        }
        self.scene = scene;
    }

    /// Text readouts for the debug switches, over the view.
    pub(super) fn draw_debug_text(&mut self) {
        let d = self.debug;
        let skate_text = d.show_skate_state || d.skate_debug;
        if !(d.show_player || d.show_velocity || skate_text) || self.state != GameState::Level || self.automap.active {
            return;
        }
        let w = &self.world;
        let cp = w.params.consoleplayer;
        let p = &w.players[cp];
        let Some(mo) = p.mo.and_then(|mo| w.thinkers.get_mobj(mo)) else { return };
        let pal = self.con_palette;
        let mut lines: Vec<(String, u8)> = Vec::new();
        if d.show_player {
            let sec_id = w.level.subsectors[mo.subsector.idx()].sector;
            let sec = w.level.sector(sec_id);
            let ang = mo.angle.0 as f64 / 4294967296.0 * 360.0;
            lines.push((format!("pos {:.1} {:.1} {:.1}  ang {:.0}  sector {} ({:.0}/{:.0})", fx(mo.x), fx(mo.y), fx(mo.z), ang, sec_id.0, fx(sec.floor_height), fx(sec.ceiling_height)), pal.white));
            lines.push((
                format!(
                    "health {}  armor {}  floorz {:.0} ceilz {:.0}  controller {}",
                    p.health,
                    p.armorpoints,
                    fx(mo.floorz),
                    fx(mo.ceilingz),
                    if p.skater.is_some() { "skate" } else { "classic" }
                ),
                pal.white,
            ));
        }
        if d.show_velocity {
            let v = match p.skater.as_deref() {
                Some(sk) => sk.vel,
                None => [fx(mo.momx), fx(mo.momy), fx(mo.momz)],
            };
            let hs = (v[0] * v[0] + v[1] * v[1]).sqrt();
            lines.push((format!("vel {:.2} {:.2} {:.2}  speed {:.1} u/tic = {:.0} u/s", v[0], v[1], v[2], hs, hs * 35.0), pal.green));
        }
        let mut balance = None;
        if skate_text {
            match p.skater.as_deref() {
                Some(sk) => {
                    lines.push((
                        format!(
                            "{}{}  speed {:.0} u/s  crouch {:.2}  lean {:+.2}  pitch {:+.2} roll {:+.2}",
                            sk.state.name().to_ascii_uppercase(),
                            if sk.fakie { " (fakie)" } else { "" },
                            sk.speed() * 35.0,
                            sk.crouch,
                            sk.lean,
                            sk.board.pitch,
                            sk.board.roll
                        ),
                        pal.yellow,
                    ));
                    if let (Some(g), Some(graph)) = (sk.grind, &w.skate.grind) {
                        let e = &graph.edges[g.edge as usize];
                        lines.push((
                            format!("{} on {} #{} ({:.0}/{:.0})  balance {:+.2}  {:.1}s  edges {}", g.style.name(), e.kind.name(), g.edge, g.s, e.length, g.balance, g.tics as f32 / 35.0, g.edges),
                            pal.cyan,
                        ));
                        balance = Some(g.balance);
                    }
                    if d.skate_debug {
                        let wf = sk.board.wheel_floor;
                        lines.push((
                            format!("wheels {:.0} {:.0} {:.0} {:.0}  air {}t spin {:.0}  impact {:.1}  charge {:.2}", wf[0], wf[1], wf[2], wf[3], sk.air_tics, sk.air_spin.to_degrees(), sk.last_impact, sk.charge),
                            pal.gray,
                        ));
                        if let Some(g) = &w.skate.grind {
                            lines.push((format!("grind graph: {} edges, {} nodes, {} connections", g.edges.len(), g.nodes.len(), g.stats.connections), pal.gray));
                        }
                    }
                }
                None => lines.push(("not skating (skate 1)".into(), pal.gray)),
            }
        }
        let top = self.renderer.viewwindowy + 2 + if self.hud_text.message().is_some() { 10 } else { 0 };
        let x = self.renderer.viewwindowx + 2;
        for (i, (text, color)) in lines.iter().enumerate() {
            draw_text_shadow(&mut self.screen, crate::ui::gfx::SCREENWIDTH, x, top + i as i32 * LINE_H, text, *color, pal.black);
        }
        // Balance meter while grinding.
        if let Some(b) = balance {
            let r = &self.renderer;
            let (cx, y) = (r.viewwindowx + r.scaledviewwidth / 2, r.viewwindowy + r.viewheight - 14);
            let half = 40;
            crate::ui::gfx::fill_rect(&mut self.screen, cx - half, y, half * 2, 3, pal.dark);
            crate::ui::gfx::fill_rect(&mut self.screen, cx - 1, y - 1, 2, 5, pal.white);
            let pos = cx + (b.clamp(-1.0, 1.0) * half as f32) as i32;
            let color = if b.abs() > 0.7 { pal.red } else { pal.green };
            crate::ui::gfx::fill_rect(&mut self.screen, pos - 2, y - 2, 4, 7, color);
        }
    }
}

#[inline]
fn to_fixed(v: f32) -> Fixed {
    Fixed((v * FRACUNIT as f32) as i32)
}

/// Level lines as a wireframe (near the camera).
fn add_wireframe(scene: &mut DebugScene, level: &Level, cam: &Camera, pal: &crate::console::Palette) {
    let r2 = 2048.0f32 * 2048.0;
    for line in &level.lines {
        let (v1, v2) = (level.vertex(line.v1), level.vertex(line.v2));
        let (a, b) = ([fx(v1.x), fx(v1.y)], [fx(v2.x), fx(v2.y)]);
        let mx = (a[0] + b[0]) * 0.5 - cam.x;
        let my = (a[1] + b[1]) * 0.5 - cam.y;
        if mx * mx + my * my > r2 {
            continue;
        }
        let Some(front) = line.front_sector else { continue };
        let fs = level.sector(front);
        let (ff, fc) = (fx(fs.floor_height), fx(fs.ceiling_height));
        match line.back_sector {
            None => {
                scene.line([a[0], a[1], ff], [b[0], b[1], ff], pal.white, true);
                scene.line([a[0], a[1], fc], [b[0], b[1], fc], pal.gray, true);
                scene.line([a[0], a[1], ff], [a[0], a[1], fc], pal.dark, true);
            }
            Some(back) => {
                let bs = level.sector(back);
                let (bf, bc) = (fx(bs.floor_height), fx(bs.ceiling_height));
                if ff != bf {
                    scene.line([a[0], a[1], ff], [b[0], b[1], ff], pal.cyan, true);
                    scene.line([a[0], a[1], bf], [b[0], b[1], bf], pal.cyan, true);
                } else {
                    scene.line([a[0], a[1], ff], [b[0], b[1], ff], pal.blue, true);
                }
                if fc != bc {
                    scene.line([a[0], a[1], fc.min(bc)], [b[0], b[1], fc.min(bc)], pal.dark, true);
                }
            }
        }
    }
}

/// Collision boxes of nearby things and the blocking lines around the
/// player.
fn add_collision(scene: &mut DebugScene, w: &doom_sim::World, cp: usize, cam: &Camera, pal: &crate::console::Palette) {
    let r2 = 1024.0f32 * 1024.0;
    let pmo = w.players[cp].mo;
    for (id, m) in w.thinkers.mobjs() {
        let (x, y, z) = (fx(m.x), fx(m.y), fx(m.z));
        if (x - cam.x).powi(2) + (y - cam.y).powi(2) > r2 {
            continue;
        }
        let color = if Some(id) == pmo {
            pal.green
        } else if m.flags & MF_SOLID != 0 {
            if m.flags & MF_SHOOTABLE != 0 {
                pal.red
            } else {
                pal.orange
            }
        } else if m.flags & MF_SPECIAL != 0 {
            pal.yellow
        } else {
            continue;
        };
        let (rad, h) = (fx(m.radius), fx(m.height));
        scene.aabb([x - rad, y - rad, z], [x + rad, y + rad, z + h], color, false);
    }
    // Lines that stop the player near them.
    let Some(mo) = pmo.and_then(|id| w.thinkers.get_mobj(id)) else { return };
    let (px, py) = (fx(mo.x), fx(mo.y));
    for line in &w.level.lines {
        let (v1, v2) = (w.level.vertex(line.v1), w.level.vertex(line.v2));
        let (a, b) = ([fx(v1.x), fx(v1.y)], [fx(v2.x), fx(v2.y)]);
        // Distance from the player to the segment.
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = (dx * dx + dy * dy).max(1e-6);
        let t = (((px - a[0]) * dx + (py - a[1]) * dy) / l2).clamp(0.0, 1.0);
        let (qx, qy) = (a[0] + dx * t - px, a[1] + dy * t - py);
        if qx * qx + qy * qy > 256.0 * 256.0 {
            continue;
        }
        let blocking = line.back_sector.is_none() || line.flags & doom_content::map::ml::BLOCKING != 0;
        if !blocking {
            continue;
        }
        let Some(front) = line.front_sector else { continue };
        let s = w.level.sector(front);
        let (f, c) = (fx(s.floor_height), fx(s.ceiling_height));
        let color = if line.back_sector.is_none() { pal.red } else { pal.magenta };
        scene.line([a[0], a[1], f + 0.5], [b[0], b[1], f + 0.5], color, false);
        scene.line([a[0], a[1], f], [a[0], a[1], c.min(f + 64.0)], color, false);
    }
}
