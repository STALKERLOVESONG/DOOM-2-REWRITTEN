//! Skateboard player controller (`player_controller Skate`).
//!
//! An alternative to the classic Doom movement in `p_user.c`: the player's
//! body rides a board with its own momentum, steering, ollies, landings,
//! bails and grinds on edges found in the map ([`crate::grind`]). The rest
//! of the playsim is untouched: the player is still an ordinary mobj that
//! moves through `P_TryMove` (so pickups, line triggers, teleporters and
//! damage keep working), shoots, and is shot at.
//!
//! The controller keeps its state in [`Skater`] (on the player) and runs in
//! place of `P_MovePlayer`/`P_CalcHeight` from [`crate::user::player_think`].
//! It owns the player's height: the mobj gets `MF_NOGRAVITY` while skating
//! and its `z` is written every tic.
//!
//! Physics are in floating point, map units per tic. Skating is not part
//! of vanilla demo compatibility (demos never enable it), but it is still
//! deterministic: no wall clock, and randomness from the skater's own
//! generator, never `P_Random`.
//!
//! # States
//!
//! [`SkateState`] is the base state (rolling, in the air, grinding,
//! bailed). Tricks layer on top: [`Skater::trick`] names what the skater is
//! doing in the air and [`GrindStyle`] the grind. New tricks (flips, grabs,
//! manuals, spins, wallrides, boneless, caveman) slot in as [`TrickKind`]
//! variants with their own input checks in [`air_tic`]/[`ground_tic`].

use std::sync::Arc;

use doom_core::{Angle, Fixed, FRACUNIT};

use crate::grind::{ConnKind, GrindGraph, GrindKind};
use crate::ids::MobjId;
use crate::maputl::{set_thing_position, unset_thing_position};
use crate::mobj::flags::{MF_NOCLIP, MF_NOGRAVITY};
use crate::player::{cheat, VIEWHEIGHT};
use crate::ticcmd::{ext, TicCmd};
use crate::world::World;

const TICRATE: f32 = 35.0;
/// Deck height above the ground (wheels and trucks).
pub const DECK_HEIGHT: f32 = 5.0;
/// Distance between the wheel pairs as a fraction of the board length.
const WHEELBASE: f32 = 0.6;
/// Distance between left and right wheels.
const TRACK: f32 = 7.0;
/// Drops up to this stay on the ground (stairs, kerbs); bigger ones launch.
const SNAP_DOWN: f32 = 12.0;
/// Tics a bail lasts before the skater gets up.
const BAIL_TICS: u32 = 52;
/// Tics after a grind ends before the same edge can be grabbed again.
const REGRIND_TICS: u32 = 10;

/// The tunables (`skate_*` cvars), in the units the cvars use: map units,
/// seconds, degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkateParams {
    /// Push acceleration (units/s²).
    pub acceleration: f32,
    /// Top speed reachable by pushing (units/s); hills and drops go faster.
    pub max_speed: f32,
    /// Steering rate at speed (degrees/s).
    pub turn_rate: f32,
    /// How hard carving (strafe keys) turns and how well wheels grip (0..2).
    pub carve_strength: f32,
    /// Rolling resistance (units/s² per unit of friction, x100).
    pub friction: f32,
    /// Braking deceleration (units/s²).
    pub brake_strength: f32,
    /// Ollie take-off speed (units/s).
    pub jump_strength: f32,
    /// Steering authority in the air (0..1).
    pub air_control: f32,
    /// Gravity (units/s²); Doom's is 1225.
    pub gravity: f32,
    /// Board length (units).
    pub board_length: f32,
    /// How far from an edge the board can snap onto it (units).
    pub grind_snap_distance: f32,
    /// Largest angle between the approach and the edge for a grind (degrees).
    pub grind_max_angle: f32,
    /// Highest ledge or rail kept as grindable (units above the floor).
    pub grind_max_height: f32,
    /// Magnetic snapping, self-balancing and corner following.
    pub grind_assist: bool,
}

impl Default for SkateParams {
    fn default() -> Self {
        SkateParams {
            acceleration: 620.0,
            max_speed: 720.0,
            turn_rate: 190.0,
            carve_strength: 1.0,
            friction: 0.4,
            brake_strength: 900.0,
            jump_strength: 330.0,
            air_control: 0.25,
            gravity: 1100.0,
            board_length: 36.0,
            grind_snap_distance: 14.0,
            grind_max_angle: 50.0,
            grind_max_height: 64.0,
            grind_assist: true,
        }
    }
}

/// Per-tic values derived from [`SkateParams`].
#[derive(Clone, Copy, Debug)]
struct Tuned {
    accel: f32,
    max_speed: f32,
    turn: f32,
    carve: f32,
    roll_friction: f32,
    brake: f32,
    jump: f32,
    air_control: f32,
    gravity: f32,
    board_length: f32,
    snap: f32,
    max_angle_cos: f32,
    assist: bool,
}

impl From<&SkateParams> for Tuned {
    fn from(p: &SkateParams) -> Tuned {
        let per_tic2 = 1.0 / (TICRATE * TICRATE);
        Tuned {
            accel: p.acceleration * per_tic2,
            max_speed: p.max_speed / TICRATE,
            turn: p.turn_rate.to_radians() / TICRATE,
            carve: p.carve_strength.max(0.0),
            roll_friction: p.friction * 100.0 * per_tic2,
            brake: p.brake_strength * per_tic2,
            jump: p.jump_strength / TICRATE,
            air_control: p.air_control.clamp(0.0, 1.0),
            gravity: p.gravity * per_tic2,
            board_length: p.board_length.clamp(16.0, 96.0),
            snap: p.grind_snap_distance.max(1.0),
            max_angle_cos: p.grind_max_angle.clamp(5.0, 89.0).to_radians().cos(),
            assist: p.grind_assist,
        }
    }
}

/// Skate data shared by the level: settings and the grind graph.
#[derive(Clone, Debug, Default)]
pub struct SkateWorld {
    pub params: SkateParams,
    /// The current level's grindable edges (built on demand, see
    /// [`ensure_grind_graph`]).
    pub grind: Option<Arc<GrindGraph>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SkateState {
    /// Standing on the board, not moving.
    Idle,
    /// A push stroke in progress.
    Pushing,
    /// Rolling.
    Cruising,
    /// Rolling and leaning into a turn.
    Carving,
    /// Foot brake / powerslide.
    Braking,
    /// Popping off the ground (the first tics of a jump).
    Ollie,
    Airborne,
    /// Absorbing a landing.
    Landing,
    Grinding,
    /// Fell off; the board is loose until the skater gets up.
    Bail,
}

impl SkateState {
    pub fn name(self) -> &'static str {
        match self {
            SkateState::Idle => "idle",
            SkateState::Pushing => "pushing",
            SkateState::Cruising => "cruising",
            SkateState::Carving => "carving",
            SkateState::Braking => "braking",
            SkateState::Ollie => "ollie",
            SkateState::Airborne => "airborne",
            SkateState::Landing => "landing",
            SkateState::Grinding => "grinding",
            SkateState::Bail => "bail",
        }
    }

    pub fn on_ground(self) -> bool {
        matches!(self, SkateState::Idle | SkateState::Pushing | SkateState::Cruising | SkateState::Carving | SkateState::Braking | SkateState::Landing)
    }
}

/// How the board sits on a grind edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GrindStyle {
    /// Both trucks on the edge.
    FiftyFifty,
    /// Front truck only (leaning forward).
    Nosegrind,
    /// Back truck only (leaning back).
    FiveO,
    /// Board across the edge, between the trucks.
    Boardslide,
    /// Front truck at an angle.
    Crooked,
}

impl GrindStyle {
    pub fn name(self) -> &'static str {
        match self {
            GrindStyle::FiftyFifty => "50-50",
            GrindStyle::Nosegrind => "nosegrind",
            GrindStyle::FiveO => "5-0",
            GrindStyle::Boardslide => "boardslide",
            GrindStyle::Crooked => "crooked grind",
        }
    }

    /// Board yaw relative to the direction of travel.
    pub fn board_angle(self) -> f32 {
        match self {
            GrindStyle::Boardslide => std::f32::consts::FRAC_PI_2,
            GrindStyle::Crooked => 0.35,
            _ => 0.0,
        }
    }
}

/// Air and flatground tricks. Only the ones marked implemented have inputs
/// yet; the rest are the planned set the state machine leaves room for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrickKind {
    /// Implemented: jump.
    Ollie,
    /// Implemented: rotation in the air, scored in half turns on landing.
    Spin,
    Kickflip,
    Heelflip,
    Grab,
    Manual,
    Boneless,
    Caveman,
    Wallride,
}

/// A trick in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveTrick {
    pub kind: TrickKind,
    pub tics: u32,
}

/// Something the skater is asked to do. Player input becomes these every
/// tic ([`think`]), and the console, scripts and tests send the same ones
/// with [`queue_action`]: every path goes through [`apply_action`] and the
/// controller's own state transitions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SkateAction {
    /// Ollie from the ground (with the charge built so far) or pop off a
    /// grind.
    Jump,
    /// Start a push stroke (`power` 1 = normal, 2 = running).
    Push { power: f32 },
    /// Brake for this many tics.
    Brake { tics: u32 },
    /// Fall off the board.
    Bail,
    /// Lock onto the nearest grindable edge in reach.
    Grind,
    /// Step off the grind (to the raised side of a ledge).
    Drop,
    /// Turn this many degrees in the air, at the spin rate.
    Spin { degrees: f32 },
    /// Horizontal speed along the board (units/s).
    SetSpeed(f32),
    /// Velocity (units/s); upward velocity on the ground launches.
    SetVelocity([f32; 3]),
}

impl SkateAction {
    pub fn name(&self) -> &'static str {
        match self {
            SkateAction::Jump => "jump",
            SkateAction::Push { .. } => "push",
            SkateAction::Brake { .. } => "brake",
            SkateAction::Bail => "bail",
            SkateAction::Grind => "grind",
            SkateAction::Drop => "drop",
            SkateAction::Spin { .. } => "spin",
            SkateAction::SetSpeed(_) => "set_speed",
            SkateAction::SetVelocity(_) => "set_velocity",
        }
    }
}

/// What the skater last completed, for the HUD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feat {
    Air { spin_halves: i32, airtime: u32, fakie: bool },
    Grind { style: GrindStyle, kind: GrindKind, tics: u32, edges: u32 },
    Bail,
}

/// The board's pose (for drawing, and loose during a bail).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Board {
    /// Centre of the board at ground level (wheel contact), map units.
    pub pos: [f32; 3],
    pub yaw: f32,
    /// Nose up > 0.
    pub pitch: f32,
    /// Right side down > 0.
    pub roll: f32,
    /// Spin about the board's long axis (flips, tumbling).
    pub flip: f32,
    pub length: f32,
    /// Ground height under each wheel: front-left, front-right,
    /// back-left, back-right.
    pub wheel_floor: [f32; 4],
}

/// Riding an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrindContact {
    pub edge: u32,
    /// Distance from the edge's `a` end.
    pub s: f32,
    /// +1: toward `b`.
    pub dir: f32,
    /// Speed along the edge (units/tic).
    pub speed: f32,
    /// -1..1; falling off at either end of the range (+ = left of travel).
    pub balance: f32,
    pub balance_vel: f32,
    pub style: GrindStyle,
    pub tics: u32,
    /// Edges ridden in this grind (corners and runs).
    pub edges: u32,
}

/// A loose board and tumbling body after a fall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BailState {
    pub board_vel: [f32; 3],
    pub board_spin: [f32; 3],
    /// Tumble of the body (radians), for the model.
    pub body_roll: f32,
}

/// The skateboard controller's state for one player.
#[derive(Clone, Debug)]
pub struct Skater {
    pub state: SkateState,
    pub state_tics: u32,
    /// Board contact point (= the mobj's x, y, z).
    pub pos: [f32; 3],
    /// Units per tic.
    pub vel: [f32; 3],
    /// Direction the board's nose points (radians, 0 = east).
    pub heading: f32,
    pub board: Board,
    /// 0..1: knees bent (charging an ollie, absorbing a landing).
    pub crouch: f32,
    /// -1..1: leaning into a carve (+ = left).
    pub lean: f32,
    /// Tics into the current push stroke (0 = not pushing).
    pub push_phase: u32,
    /// Riding backwards.
    pub fakie: bool,
    pub jump_held: bool,
    /// Ollie charge from holding jump (0..1).
    pub charge: f32,
    pub grind: Option<GrindContact>,
    pub trick: Option<ActiveTrick>,
    pub bail: Option<BailState>,
    pub last_feat: Option<Feat>,
    /// Tics since leaving the ground, and the turn accumulated meanwhile.
    pub air_tics: u32,
    pub air_spin: f32,
    /// Last grind edge and when it may be grabbed again.
    pub last_edge: Option<u32>,
    pub regrind_cooldown: u32,
    /// Impact speed of the last landing (units/tic), for debugging.
    pub last_impact: f32,
    /// Where this controller last put the mobj (to notice teleports).
    synced: (Fixed, Fixed, Fixed),
    rng: u32,
    /// Actions queued by [`queue_action`], applied next tic.
    pub pending: Vec<SkateAction>,
    /// Tics of braking left.
    pub brake_tics: u32,
    /// Strength of the current push stroke.
    pub push_power: f32,
    /// Air rotation still to do from a `Spin` action (radians, signed).
    pub spin_remaining: f32,
}

impl Skater {
    fn new(x: f32, y: f32, z: f32, heading: f32, board_length: f32, seed: u32) -> Skater {
        Skater {
            state: SkateState::Idle,
            state_tics: 0,
            pos: [x, y, z],
            vel: [0.0; 3],
            heading,
            board: Board { pos: [x, y, z], yaw: heading, pitch: 0.0, roll: 0.0, flip: 0.0, length: board_length, wheel_floor: [z; 4] },
            crouch: 0.0,
            lean: 0.0,
            push_phase: 0,
            fakie: false,
            jump_held: false,
            charge: 0.0,
            grind: None,
            trick: None,
            bail: None,
            last_feat: None,
            air_tics: 0,
            air_spin: 0.0,
            last_edge: None,
            regrind_cooldown: 0,
            last_impact: 0.0,
            synced: (Fixed(0), Fixed(0), Fixed(0)),
            rng: seed | 1,
            pending: Vec::new(),
            brake_tics: 0,
            push_power: 1.0,
            spin_remaining: 0.0,
        }
    }

    /// Horizontal speed (units/tic).
    pub fn speed(&self) -> f32 {
        if let Some(g) = &self.grind {
            return g.speed;
        }
        (self.vel[0] * self.vel[0] + self.vel[1] * self.vel[1]).sqrt()
    }

    pub fn grounded(&self) -> bool {
        self.state.on_ground()
    }

    fn set_state(&mut self, s: SkateState) {
        if self.state != s {
            self.state = s;
            self.state_tics = 0;
        }
    }

    /// Deterministic noise in -1..1.
    fn noise(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

#[inline]
fn fx(v: Fixed) -> f32 {
    v.0 as f32 / FRACUNIT as f32
}

#[inline]
fn to_fixed(v: f32) -> Fixed {
    Fixed((v * FRACUNIT as f32).round() as i32)
}

#[inline]
fn angle_to_rad(a: Angle) -> f32 {
    a.0 as f32 / 4294967296.0 * std::f32::consts::TAU
}

#[inline]
fn rad_to_angle(r: f32) -> Angle {
    let turns = r / std::f32::consts::TAU;
    Angle(((turns - turns.floor()) * 4294967296.0) as u64 as u32)
}

/// Wrap to -pi..pi.
#[inline]
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

#[inline]
fn dir(a: f32) -> [f32; 2] {
    [a.cos(), a.sin()]
}

#[inline]
fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

// ---- enabling ------------------------------------------------------------------

/// Whether `player` is on the board.
pub fn is_active(w: &World, player: usize) -> bool {
    w.players[player].skater.is_some()
}

/// Put `player` on a board where they stand (no-op if already on one or
/// without a body).
pub fn enable(w: &mut World, player: usize) {
    let Some(mo) = w.players[player].mo else { return };
    if w.players[player].skater.is_some() || w.thinkers.get_mobj(mo).is_none() {
        return;
    }
    let m = w.mo(mo);
    let (x, y, z, heading) = (fx(m.x), fx(m.y), fx(m.z), angle_to_rad(m.angle));
    let seed = (w.leveltime as u32).wrapping_mul(2654435761) ^ 0x5eed;
    let mut sk = Skater::new(x, y, z, heading, w.skate.params.board_length, seed);
    sk.synced = (m.x, m.y, m.z);
    let (momx, momy) = (fx(m.momx), fx(m.momy));
    sk.vel = [momx, momy, 0.0];
    let m = w.mo_mut(mo);
    m.flags |= MF_NOGRAVITY;
    m.momx = Fixed(0);
    m.momy = Fixed(0);
    m.momz = Fixed(0);
    w.players[player].skater = Some(Box::new(sk));
}

/// Back to walking: the board's momentum carries over to the body.
pub fn disable(w: &mut World, player: usize) {
    let Some(sk) = w.players[player].skater.take() else { return };
    if let Some(mo) = w.players[player].mo {
        if let Some(m) = w.thinkers.get_mobj_mut(mo) {
            m.flags &= !MF_NOGRAVITY;
            if sk.bail.is_none() {
                m.momx = to_fixed(sk.vel[0]);
                m.momy = to_fixed(sk.vel[1]);
            }
        }
        if w.players[player].viewheight.0 <= 0 {
            w.players[player].viewheight = VIEWHEIGHT;
        }
    }
}

/// Build (or rebuild, when the settings that shape it changed) the grind
/// graph for the current level.
pub fn ensure_grind_graph(w: &mut World) -> &GrindGraph {
    let wanted = grind_params(&w.skate.params);
    let stale = w.skate.grind.as_ref().is_none_or(|g| g.params != wanted || g.level != w.level.name || g.stats.lines_checked != w.level.lines.len());
    if stale {
        w.skate.grind = Some(Arc::new(GrindGraph::build(&w.level, &w.content.texture_heights, wanted)));
    }
    w.skate.grind.as_deref().expect("just built")
}

/// The graph settings implied by the skate settings.
pub fn grind_params(p: &SkateParams) -> crate::grind::GrindParams {
    crate::grind::GrindParams { max_height: p.grind_max_height, ..Default::default() }
}

/// Snap onto the nearest grindable edge within `radius` (the `skate_grind`
/// console command). Returns the edge.
pub fn force_grind(w: &mut World, player: usize, radius: f32) -> Result<u32, String> {
    let Some(mut sk) = w.players[player].skater.take() else { return Err("not skating (skate 1)".into()) };
    ensure_grind_graph(w);
    let graph = w.skate.grind.clone().expect("built");
    let found = graph.nearest(sk.pos[0], sk.pos[1], radius);
    let result = match found {
        Some((e, _)) => {
            let edge = &graph.edges[e as usize];
            let (s, _) = edge.local([sk.pos[0], sk.pos[1]]);
            let s = s.clamp(1.0, edge.length - 1.0);
            let along = dot([sk.vel[0], sk.vel[1]], edge.tangent);
            let heading_along = dot(dir(sk.heading), edge.tangent);
            let d = if along.abs() > 0.5 { along.signum() } else if heading_along < 0.0 { -1.0 } else { 1.0 };
            let speed = along.abs().max(6.0);
            sk.grind = Some(GrindContact { edge: e, s, dir: d, speed, balance: 0.0, balance_vel: 0.0, style: GrindStyle::FiftyFifty, tics: 0, edges: 1 });
            sk.heading = (edge.tangent[1] * d).atan2(edge.tangent[0] * d);
            sk.set_state(SkateState::Grinding);
            Ok(e)
        }
        None => Err(format!("no grindable edge within {radius} units")),
    };
    w.players[player].skater = Some(sk);
    if result.is_ok() {
        place_on_grind(w, player);
    }
    result
}

// ---- per tic -------------------------------------------------------------------

/// Run the controller for one tic (replaces `P_MovePlayer` and
/// `P_CalcHeight`).
pub fn think(w: &mut World, player: usize) {
    let Some(mut sk) = w.players[player].skater.take() else { return };
    let mo = w.players[player].mo.expect("skating without a body");
    let t = Tuned::from(&w.skate.params);
    sk.board.length = t.board_length;
    let mut cmd = w.players[player].cmd;
    sync_from_mobj(w, mo, &mut sk);
    // After a teleport the player can't move for a moment.
    if w.mo(mo).reactiontime != 0 {
        w.mo_mut(mo).reactiontime -= 1;
        cmd = TicCmd { buttons: cmd.buttons, ..TicCmd::default() };
    }
    sk.state_tics = sk.state_tics.saturating_add(1);
    sk.regrind_cooldown = sk.regrind_cooldown.saturating_sub(1);
    let jump = cmd.ext & ext::JUMP != 0;
    let jump_pressed = jump && !sk.jump_held;
    let jump_released = !jump && sk.jump_held;
    sk.jump_held = jump;

    // Input becomes actions, applied with the queued ones through the same
    // code: hold-and-release jump on the ground, press on a rail.
    let mut input: [Option<SkateAction>; 3] = [None; 3];
    if (jump_released && sk.grounded()) || (jump_pressed && sk.state == SkateState::Grinding) {
        input[0] = Some(SkateAction::Jump);
    }
    if cmd.forwardmove > 0 {
        input[1] = Some(SkateAction::Push { power: (cmd.forwardmove as f32 / 25.0).clamp(0.5, 2.0) });
    } else if cmd.forwardmove < 0 {
        input[1] = Some(SkateAction::Brake { tics: 1 });
    }
    let queued = std::mem::take(&mut sk.pending);
    for a in input.iter().flatten().chain(queued.iter()) {
        apply_action(w, mo, &mut sk, &t, *a, &cmd);
    }
    sk.pending = queued;
    sk.pending.clear();

    match sk.state {
        SkateState::Bail => bail_tic(w, mo, &mut sk, &t, &cmd),
        SkateState::Grinding => grind_tic(w, mo, &mut sk, &t, &cmd),
        _ if sk.grounded() => ground_tic(w, mo, &mut sk, &t, &cmd, jump),
        _ => air_tic(w, mo, &mut sk, &t, &cmd),
    }
    sk.brake_tics = sk.brake_tics.saturating_sub(1);
    if sk.state != SkateState::Bail {
        update_board(w, &mut sk);
    }
    // Hand the pose to the mobj and the view.
    let noclip = w.players[player].cheats & cheat::NOCLIP != 0;
    let aim = match (&sk.grind, &w.skate.grind) {
        (Some(g), Some(graph)) => {
            let e = &graph.edges[g.edge as usize];
            (e.tangent[1] * g.dir).atan2(e.tangent[0] * g.dir)
        }
        _ if sk.fakie => sk.heading + std::f32::consts::PI,
        _ => sk.heading,
    };
    {
        let m = w.mo_mut(mo);
        m.z = to_fixed(sk.pos[2]);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
        m.momz = Fixed(0);
        m.flags |= MF_NOGRAVITY;
        if noclip {
            m.flags |= MF_NOCLIP;
        }
        m.angle = rad_to_angle(aim);
        sk.synced = (m.x, m.y, m.z);
    }
    let (z, ceilingz) = (w.mo(mo).z, w.mo(mo).ceilingz);
    let p = &mut w.players[player];
    p.bob = Fixed(0);
    p.viewheight = VIEWHEIGHT;
    p.deltaviewheight = Fixed(0);
    let eye = (41.0 - 14.0 * sk.crouch).max(16.0) + DECK_HEIGHT;
    p.viewz = (z + to_fixed(eye)).min(ceilingz - Fixed(4 * FRACUNIT));
    w.onground = sk.grounded();
    w.players[player].skater = Some(sk);
}

/// Pick up changes made to the mobj by others since the last tic:
/// teleports, knockback, moving floors.
fn sync_from_mobj(w: &mut World, mo: MobjId, sk: &mut Skater) {
    let m = w.mo(mo);
    let (x, y, z) = (m.x, m.y, m.z);
    let moved = (x, y) != (sk.synced.0, sk.synced.1);
    if moved {
        let (nx, ny) = (fx(x), fx(y));
        let jump = ((nx - sk.pos[0]).powi(2) + (ny - sk.pos[1]).powi(2)).sqrt();
        sk.pos[0] = nx;
        sk.pos[1] = ny;
        if jump > 32.0 {
            // Teleported: arrive standing, facing the exit.
            sk.vel = [0.0; 3];
            sk.heading = angle_to_rad(m.angle);
            sk.grind = None;
            sk.bail = None;
            sk.fakie = false;
            sk.crouch = 0.0;
            sk.charge = 0.0;
            sk.trick = None;
            sk.set_state(SkateState::Idle);
            sk.pos[2] = fx(m.floorz);
            sk.board = Board { pos: sk.pos, yaw: sk.heading, pitch: 0.0, roll: 0.0, flip: 0.0, length: sk.board.length, wheel_floor: [sk.pos[2]; 4] };
        }
    }
    if z != sk.synced.2 {
        sk.pos[2] = fx(z);
    }
    // Knockback (damage thrust) lands on the board's momentum.
    let (momx, momy) = (fx(m.momx), fx(m.momy));
    if momx != 0.0 || momy != 0.0 {
        sk.vel[0] += momx;
        sk.vel[1] += momy;
        let m = w.mo_mut(mo);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
    }
}

/// Steering input this tic: (yaw change requested, carve lean -1..1).
fn steer_input(cmd: &TicCmd) -> (f32, f32) {
    let turn = cmd.angleturn as f32 / 65536.0 * std::f32::consts::TAU;
    let lean = -(cmd.sidemove as f32 / 40.0).clamp(-1.0, 1.0);
    (turn, lean)
}

fn ground_tic(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned, cmd: &TicCmd, jump: bool) {
    let fwd = dir(sk.heading);
    let left = [-fwd[1], fwd[0]];
    let mut v_along = dot([sk.vel[0], sk.vel[1]], fwd);
    let mut v_side = dot([sk.vel[0], sk.vel[1]], left);
    let speed = v_along.abs();

    // Rolling backwards on purpose or by a hill: ride fakie (switch the
    // board's nose instead of skating backwards).
    if v_along < -0.6 && cmd.forwardmove <= 0 {
        sk.heading = wrap(sk.heading + std::f32::consts::PI);
        sk.fakie = !sk.fakie;
        v_along = -v_along;
        v_side = -v_side;
    }
    let fwd = dir(sk.heading);
    let left = [-fwd[1], fwd[0]];

    // Steering: turning keys and mouse yaw the board (limited at speed),
    // the strafe keys lean into a carve.
    let (turn, lean_in) = steer_input(cmd);
    sk.lean += (lean_in - sk.lean) * 0.25;
    let speed_frac = (speed / t.max_speed).clamp(0.0, 1.0);
    let max_turn = if speed < 1.0 { t.turn * 3.0 } else { t.turn * (1.25 - 0.45 * speed_frac) };
    let mut dh = turn.clamp(-max_turn, max_turn);
    dh += sk.lean * t.turn * 0.8 * t.carve * (0.35 + 0.65 * speed_frac.min(0.7) / 0.7);
    sk.heading = wrap(sk.heading + dh);

    // Wheels grip: sideways velocity bleeds off, more with carve strength.
    let grip = (0.22 * t.carve).clamp(0.02, 0.9);
    v_side *= 1.0 - grip;

    let mut state = if speed < 0.25 { SkateState::Idle } else if sk.lean.abs() > 0.3 { SkateState::Carving } else { SkateState::Cruising };

    // Pushing: a stroke started by a Push action runs its course.
    let push_power = sk.push_power;
    if sk.push_phase > 0 {
        // A stroke: 12 tics of foot on the ground, then 10 to bring it back.
        if sk.push_phase <= 12 && v_along < t.max_speed {
            let a = t.accel * (0.6 + 0.4 * push_power.min(1.5)) * if v_along < t.max_speed * 0.25 { 1.4 } else { 1.0 };
            v_along = (v_along + a).min(t.max_speed.max(v_along));
        }
        state = SkateState::Pushing;
        sk.push_phase += 1;
        if sk.push_phase > 22 {
            sk.push_phase = 0;
        }
    }
    // Braking: foot drag at low speed, a powerslide at speed.
    if sk.brake_tics > 0 {
        v_along = (v_along - t.brake).max(0.0);
        sk.push_phase = 0;
        state = SkateState::Braking;
    }
    // Rolling resistance and air drag.
    let drag = t.roll_friction + v_along * v_along * 0.00035;
    v_along = (v_along - drag).max(0.0);
    // Hills: the board's pitch (from the wheels) pulls it down slopes and
    // stairs.
    v_along -= t.gravity * sk.board.pitch.sin() * 0.3;
    if v_along < 0.0 && v_along > -0.05 {
        v_along = 0.0;
    }

    sk.vel[0] = fwd[0] * v_along + left[0] * v_side;
    sk.vel[1] = fwd[1] * v_along + left[1] * v_side;
    sk.vel[2] = 0.0;

    // Ollie: hold to crouch and charge, release to pop.
    if jump {
        sk.charge = (sk.charge + 1.0 / 18.0).min(1.0);
        sk.crouch += (0.8 - sk.crouch) * 0.3;
    } else {
        let target = if sk.state == SkateState::Landing { sk.crouch } else { 0.0 };
        sk.crouch += (target - sk.crouch) * 0.25;
    }
    if sk.state == SkateState::Landing && sk.state_tics < 8 {
        state = SkateState::Landing;
        sk.crouch = sk.crouch.max(0.6 * (1.0 - sk.state_tics as f32 / 8.0));
    }
    sk.set_state(state);

    let z_before = sk.pos[2];
    let impact = move_xy(w, mo, sk, t);
    if impact.bail {
        start_bail(w, mo, sk);
        return;
    }
    // Follow the ground: small drops and steps stay grounded (with the
    // speed they give or cost); bigger drops launch the board.
    let floor = fx(w.mo(mo).floorz);
    if floor < z_before - SNAP_DOWN {
        sk.pos[2] = z_before;
        start_air(sk);
        sk.set_state(SkateState::Airborne);
    } else {
        let dz = floor - z_before;
        let along = dot([sk.vel[0], sk.vel[1]], dir(sk.heading));
        if dz != 0.0 && along > 0.01 {
            // Energy (half of it kept): v dv = -g dz. Steps down speed the
            // board up, steps up slow it, tall ones knock most speed off.
            let mut nv = (along * along - t.gravity * dz).max(0.0).sqrt();
            if dz > 12.0 {
                nv *= 0.5;
            }
            let k = nv / along;
            sk.vel[0] *= k;
            sk.vel[1] *= k;
        }
        sk.pos[2] = floor;
    }
}

// ---- actions -----------------------------------------------------------------------

/// Queue an action for `player`'s skater, applied on the next tic. Errors
/// when the action can't apply in the current state (with the reason).
pub fn queue_action(w: &mut World, player: usize, action: SkateAction) -> Result<(), String> {
    let Some(sk) = w.players[player].skater.as_deref() else { return Err("not skating (skate 1)".into()) };
    match action {
        SkateAction::Jump if !(sk.grounded() || sk.state == SkateState::Grinding) => {
            return Err(format!("can't jump while {}", sk.state.name()));
        }
        SkateAction::Push { .. } | SkateAction::Brake { .. } if !sk.grounded() => {
            return Err(format!("{} needs the board on the ground ({} now)", action.name(), sk.state.name()));
        }
        SkateAction::Drop if sk.state != SkateState::Grinding => return Err("not grinding".into()),
        SkateAction::Spin { .. } if sk.grounded() || sk.state == SkateState::Grinding || sk.state == SkateState::Bail => {
            return Err(format!("spins happen in the air ({} now)", sk.state.name()));
        }
        SkateAction::Bail if sk.state == SkateState::Bail => return Err("already bailing".into()),
        SkateAction::Grind => {
            ensure_grind_graph(w);
            let sk = w.players[player].skater.as_deref().expect("checked");
            if nearest_grind_target(w, sk).is_none() {
                return Err("no grindable edge in reach".into());
            }
        }
        _ => {}
    }
    w.players[player].skater.as_deref_mut().expect("checked").pending.push(action);
    Ok(())
}

/// The edge a `Grind` action locks onto: the nearest one within reach
/// whose height the board can get onto.
fn nearest_grind_target(w: &World, sk: &Skater) -> Option<u32> {
    let graph = w.skate.grind.as_ref()?;
    let reach = (w.skate.params.grind_snap_distance * 4.0).max(48.0);
    let mut best: Option<(u32, f32)> = None;
    for e in graph.near(sk.pos[0], sk.pos[1]) {
        let edge = &graph.edges[e as usize];
        let z = edge.top_z(&w.level);
        if z - sk.pos[2] > 56.0 || sk.pos[2] - z > 64.0 {
            continue;
        }
        let (s, _) = edge.local([sk.pos[0], sk.pos[1]]);
        let p = edge.point(s.clamp(0.0, edge.length));
        let d = (p[0] - sk.pos[0]).hypot(p[1] - sk.pos[1]);
        if d <= reach && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((e, d));
        }
    }
    best.map(|(e, _)| e)
}

/// Do an action now (the controller's state transitions).
fn apply_action(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned, action: SkateAction, cmd: &TicCmd) {
    match action {
        SkateAction::Jump => match sk.state {
            SkateState::Grinding => {
                let (_, lean) = steer_input(cmd);
                grind_pop(w, mo, sk, t, lean * 2.0);
            }
            s if s.on_ground() => {
                let pop = t.jump * (0.78 + 0.32 * sk.charge);
                sk.vel[2] = pop;
                sk.charge = 0.0;
                sk.push_phase = 0;
                sk.trick = Some(ActiveTrick { kind: TrickKind::Ollie, tics: 0 });
                start_air(sk);
                sk.set_state(SkateState::Ollie);
            }
            _ => {}
        },
        SkateAction::Push { power } => {
            if sk.grounded() && sk.crouch < 0.5 && sk.push_phase == 0 && sk.brake_tics == 0 {
                sk.push_phase = 1;
                sk.push_power = power.clamp(0.25, 2.0);
            }
        }
        SkateAction::Brake { tics } => {
            if sk.grounded() {
                sk.brake_tics = sk.brake_tics.max(tics);
                sk.push_phase = 0;
            }
        }
        SkateAction::Bail => {
            if sk.state != SkateState::Bail {
                start_bail(w, mo, sk);
            }
        }
        SkateAction::Grind => {
            if let Some(e) = nearest_grind_target(w, sk) {
                lock_onto(w, mo, sk, e);
            }
        }
        SkateAction::Drop => {
            if sk.state == SkateState::Grinding {
                if let (Some(g), Some(graph)) = (sk.grind, w.skate.grind.clone()) {
                    let e = &graph.edges[g.edge as usize];
                    let travel = [e.tangent[0] * g.dir, e.tangent[1] * g.dir];
                    sk.vel = [travel[0] * g.speed, travel[1] * g.speed, 0.5];
                    // Step to the raised side of a ledge, the right of a rail.
                    let left = [-travel[1], travel[0]];
                    let side = if e.kind == GrindKind::Rail { -1.0 } else { -dot(e.normal, left).signum() };
                    end_grind(w, mo, sk, &graph, side);
                    sk.set_state(SkateState::Airborne);
                }
            }
        }
        SkateAction::Spin { degrees } => {
            if !sk.grounded() && sk.state != SkateState::Grinding {
                sk.spin_remaining += degrees.to_radians();
            }
        }
        SkateAction::SetSpeed(units_per_second) => {
            let v = units_per_second / TICRATE;
            let travel = match sk.grind {
                Some(_) => None,
                None => Some(dir(sk.heading)),
            };
            match (travel, &mut sk.grind) {
                (_, Some(g)) => g.speed = v.max(0.0),
                (Some(f), None) => {
                    sk.vel[0] = f[0] * v;
                    sk.vel[1] = f[1] * v;
                    if sk.grounded() && v > 0.25 && sk.state == SkateState::Idle {
                        sk.set_state(SkateState::Cruising);
                    }
                }
                _ => {}
            }
        }
        SkateAction::SetVelocity(v) => {
            if let Some(g) = sk.grind.take() {
                // Leaving the edge with this velocity.
                sk.grind = Some(g);
                if let Some(graph) = w.skate.grind.clone() {
                    end_grind(w, mo, sk, &graph, 0.0);
                }
                sk.set_state(SkateState::Airborne);
            }
            sk.vel = [v[0] / TICRATE, v[1] / TICRATE, v[2] / TICRATE];
            if v[2] > 0.0 && sk.grounded() {
                start_air(sk);
                sk.set_state(SkateState::Airborne);
            }
        }
    }
}

/// Pop off a grind: keep the grind speed, hop up, drift to `side` (+ =
/// left of travel).
fn grind_pop(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned, side: f32) {
    let (Some(g), Some(graph)) = (sk.grind, w.skate.grind.clone()) else { return };
    let e = &graph.edges[g.edge as usize];
    let travel = [e.tangent[0] * g.dir, e.tangent[1] * g.dir];
    let left = [-travel[1], travel[0]];
    sk.vel = [travel[0] * g.speed + left[0] * side, travel[1] * g.speed + left[1] * side, t.jump * 0.85];
    end_grind(w, mo, sk, &graph, side);
    sk.trick = Some(ActiveTrick { kind: TrickKind::Ollie, tics: 0 });
    sk.set_state(SkateState::Ollie);
}

/// Lock onto edge `e` where the board is closest to it, keeping the speed
/// along it (at least a slow grind).
fn lock_onto(w: &mut World, mo: MobjId, sk: &mut Skater, e: u32) {
    let Some(graph) = w.skate.grind.clone() else { return };
    let edge = &graph.edges[e as usize];
    let (s, _) = edge.local([sk.pos[0], sk.pos[1]]);
    let s = s.clamp(1.0, (edge.length - 1.0).max(1.0));
    let along = dot([sk.vel[0], sk.vel[1]], edge.tangent);
    let heading_along = dot(dir(sk.heading), edge.tangent);
    let d = if along.abs() > 0.5 { along.signum() } else if heading_along < 0.0 { -1.0 } else { 1.0 };
    let speed = along.abs().max(6.0);
    sk.grind = Some(GrindContact { edge: e, s, dir: d, speed, balance: 0.0, balance_vel: 0.0, style: GrindStyle::FiftyFifty, tics: 0, edges: 1 });
    sk.heading = (edge.tangent[1] * d).atan2(edge.tangent[0] * d);
    sk.vel = [0.0; 3];
    sk.bail = None;
    sk.set_state(SkateState::Grinding);
    place_on_grind_mobj(w, mo, sk);
}

fn start_air(sk: &mut Skater) {
    sk.air_tics = 0;
    sk.air_spin = 0.0;
    sk.push_phase = 0;
}

fn air_tic(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned, cmd: &TicCmd) {
    sk.air_tics += 1;
    if let Some(tr) = &mut sk.trick {
        tr.tics += 1;
    }
    if sk.state == SkateState::Ollie && sk.state_tics >= 6 {
        sk.set_state(SkateState::Airborne);
    }
    // Spin the board and body with the turn keys (and Spin actions); air
    // control bends the flight path a little toward the board.
    let (turn, lean) = steer_input(cmd);
    // About 450 degrees a second at the default turn rate: a 180 fits in
    // an ordinary ollie.
    let max_spin = t.turn * 2.4;
    let queued_spin = sk.spin_remaining.clamp(-max_spin, max_spin);
    sk.spin_remaining -= queued_spin;
    let spin = (turn + lean * t.turn * 0.6 + queued_spin).clamp(-max_spin * 2.0, max_spin * 2.0);
    sk.heading = wrap(sk.heading + spin);
    sk.air_spin += spin;
    let hv = [sk.vel[0], sk.vel[1]];
    let hs = (hv[0] * hv[0] + hv[1] * hv[1]).sqrt();
    if hs > 0.5 && t.air_control > 0.0 {
        let cur = hv[1].atan2(hv[0]);
        // Steer the path toward the board nose (or tail, riding fakie later).
        let f = dir(sk.heading);
        let target = if dot(f, hv) >= 0.0 { sk.heading } else { sk.heading + std::f32::consts::PI };
        let bend = wrap(target - cur).clamp(-0.05, 0.05) * t.air_control;
        let n = cur + bend;
        sk.vel[0] = n.cos() * hs;
        sk.vel[1] = n.sin() * hs;
    }
    sk.lean *= 0.9;
    sk.crouch += ((if sk.vel[2] < 0.0 { 0.45 } else { 0.25 }) - sk.crouch) * 0.2;
    sk.vel[2] -= t.gravity;

    let prev = sk.pos;
    let impact = move_xy(w, mo, sk, t);
    if impact.bail {
        start_bail(w, mo, sk);
        return;
    }
    sk.pos[2] += sk.vel[2];
    let (floor, ceiling) = (fx(w.mo(mo).floorz), fx(w.mo(mo).ceilingz));
    let height = fx(w.mo(mo).height);
    if sk.pos[2] + height > ceiling {
        sk.pos[2] = (ceiling - height).max(floor);
        sk.vel[2] = sk.vel[2].min(0.0);
    }
    if try_start_grind(w, sk, t, prev) {
        place_on_grind_mobj(w, mo, sk);
        return;
    }
    if sk.pos[2] <= floor && sk.vel[2] <= 0.0 {
        sk.pos[2] = floor;
        land(w, mo, sk);
    }
}

/// Touch down: judge the landing.
fn land(w: &mut World, mo: MobjId, sk: &mut Skater) {
    let impact = -sk.vel[2];
    sk.last_impact = impact;
    sk.vel[2] = 0.0;
    let hv = [sk.vel[0], sk.vel[1]];
    let hs = (hv[0] * hv[0] + hv[1] * hv[1]).sqrt();
    let mut fakie_landing = false;
    if hs > 1.0 {
        let travel = hv[1].atan2(hv[0]);
        let mut off = wrap(sk.heading - travel).abs();
        if off > std::f32::consts::FRAC_PI_2 {
            // Landed backwards: ride away fakie.
            sk.heading = wrap(sk.heading + std::f32::consts::PI);
            sk.fakie = !sk.fakie;
            fakie_landing = true;
            off = std::f32::consts::PI - off;
        }
        if off > 1.2 && hs > 5.0 {
            start_bail(w, mo, sk);
            return;
        }
        // Sideways landings scrub speed; the wheels take the rest along the
        // board.
        let f = dir(sk.heading);
        let along = dot(hv, f).abs() * (1.0 - 0.25 * off / 1.2);
        sk.vel[0] = f[0] * along;
        sk.vel[1] = f[1] * along;
    }
    // Falls from very high knock the skater off the board.
    if impact > 26.0 {
        start_bail(w, mo, sk);
        return;
    }
    let spin_halves = (sk.air_spin / std::f32::consts::PI).round() as i32;
    if sk.air_tics > 8 {
        sk.last_feat = Some(Feat::Air { spin_halves, airtime: sk.air_tics, fakie: fakie_landing });
    }
    sk.trick = None;
    sk.crouch = (sk.crouch + impact / 20.0).min(1.0);
    sk.set_state(SkateState::Landing);
}

/// Moves the mobj by the skater's horizontal velocity (through
/// `P_TryMove`, so items and line specials work), sliding along walls.
/// Updates `sk.pos`/`sk.vel` from where the mobj ended up.
struct Impact {
    bail: bool,
}

fn move_xy(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned) -> Impact {
    let (vx, vy) = (sk.vel[0], sk.vel[1]);
    let hs = (vx * vx + vy * vy).sqrt();
    if hs < 0.001 {
        sk.pos[0] = fx(w.mo(mo).x);
        sk.pos[1] = fx(w.mo(mo).y);
        return Impact { bail: false };
    }
    let (x0, y0) = (fx(w.mo(mo).x), fx(w.mo(mo).y));
    // Air: the board has to clear a ledge's top to get onto it.
    let air = !sk.grounded();
    let steps = (hs / 15.0).ceil().max(1.0) as i32;
    let (sx, sy) = (vx / steps as f32, vy / steps as f32);
    let mut blocked = false;
    for _ in 0..steps {
        let m = w.mo(mo);
        let (tx, ty) = (m.x + to_fixed(sx), m.y + to_fixed(sy));
        if air && m.flags & MF_NOCLIP == 0 {
            if crate::map::check_position(w, mo, tx, ty) && fx(w.tm.tmfloorz) > sk.pos[2] + 2.0 {
                blocked = true;
                break;
            }
        }
        if !crate::map::try_move(w, mo, tx, ty) {
            blocked = true;
            break;
        }
    }
    let mut bail = false;
    if blocked {
        // Slide the rest of the way along the wall, as players do.
        let (x1, y1) = (fx(w.mo(mo).x), fx(w.mo(mo).y));
        let (rx, ry) = (x0 + vx - x1, y0 + vy - y1);
        {
            let m = w.mo_mut(mo);
            m.momx = to_fixed(rx);
            m.momy = to_fixed(ry);
        }
        crate::map::slide_move(w, mo);
        let m = w.mo_mut(mo);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
        let (x2, y2) = (fx(m.x), fx(m.y));
        let (ax, ay) = (x2 - x0, y2 - y0);
        // What the wall took away.
        let (lx, ly) = (vx - ax, vy - ay);
        let lost = (lx * lx + ly * ly).sqrt();
        let facing = dot(dir(sk.heading), [lx / lost.max(0.001), ly / lost.max(0.001)]).abs();
        if lost > t.max_speed * 0.55 && facing > 0.6 && sk.state != SkateState::Bail {
            bail = true;
        }
        sk.vel[0] = ax * 0.92;
        sk.vel[1] = ay * 0.92;
    }
    sk.pos[0] = fx(w.mo(mo).x);
    sk.pos[1] = fx(w.mo(mo).y);
    Impact { bail }
}

// ---- grinding ------------------------------------------------------------------

/// Look for an edge to lock onto after this tic's motion (from `prev`).
fn try_start_grind(w: &mut World, sk: &mut Skater, t: &Tuned, prev: [f32; 3]) -> bool {
    if sk.vel[2] > 2.5 {
        return false; // still rising fast
    }
    let Some(graph) = w.skate.grind.clone() else { return false };
    let snap = if t.assist { t.snap * 1.35 } else { t.snap };
    let hv = [sk.vel[0], sk.vel[1]];
    let hs = (hv[0] * hv[0] + hv[1] * hv[1]).sqrt();
    if hs < 1.5 {
        return false;
    }
    let mut best: Option<(u32, f32, f32)> = None; // edge, s, score
    for e in graph.near(sk.pos[0], sk.pos[1]) {
        if sk.regrind_cooldown > 0 && sk.last_edge == Some(e) {
            continue;
        }
        let edge = &graph.edges[e as usize];
        let z = edge.top_z(&w.level);
        // The board must come down onto the edge from above (a little
        // below counts, catching the lip).
        if sk.pos[2] < z - 6.0 || sk.pos[2] > z + 24.0 || prev[2] < z - 2.0 {
            continue;
        }
        let along = dot(hv, edge.tangent);
        if along.abs() < 1.5 || along.abs() / hs < t.max_angle_cos {
            continue;
        }
        let (s0, n0) = edge.local([prev[0], prev[1]]);
        let (s1, n1) = edge.local([sk.pos[0], sk.pos[1]]);
        let crossed = n0.signum() != n1.signum() && n0 != 0.0;
        let (s, n) = if crossed {
            let k = n0 / (n0 - n1);
            (s0 + (s1 - s0) * k, 0.0)
        } else {
            (s1, n1)
        };
        if n.abs() > snap || s < -2.0 || s > edge.length + 2.0 {
            continue;
        }
        // Coming from the low side needs the height to get up there.
        if edge.kind != GrindKind::Rail && n0 > 0.0 && !edge.approach_low {
            continue;
        }
        let score = n.abs() + (1.0 - along.abs() / hs) * 20.0 + (sk.pos[2] - z).abs() * 0.2;
        if best.is_none_or(|(_, _, b)| score < b) {
            best = Some((e, s.clamp(0.0, edge.length), score));
        }
    }
    let Some((e, s, _)) = best else { return false };
    let edge = &graph.edges[e as usize];
    let along = dot(hv, edge.tangent);
    let d = along.signum();
    let travel = (edge.tangent[1] * d).atan2(edge.tangent[0] * d);
    // The board's angle to the edge picks the grind.
    let rel = wrap(sk.heading - travel);
    let across = rel.sin().abs();
    let style = if across > 0.75 {
        GrindStyle::Boardslide
    } else if across > 0.3 {
        GrindStyle::Crooked
    } else {
        GrindStyle::FiftyFifty
    };
    // Snap the board onto the line: backwards boards ride fakie.
    let base = if rel.cos() < 0.0 && style != GrindStyle::Boardslide { travel + std::f32::consts::PI } else { travel };
    sk.heading = wrap(match style {
        GrindStyle::Boardslide => travel + std::f32::consts::FRAC_PI_2 * rel.sin().signum(),
        _ => base + style.board_angle() * rel.sin().signum(),
    });
    let lateral = dot(hv, edge.normal);
    sk.grind = Some(GrindContact {
        edge: e,
        s,
        dir: d,
        speed: along.abs() * 0.95,
        balance: (lateral * 0.08).clamp(-0.4, 0.4),
        balance_vel: 0.0,
        style,
        tics: 0,
        edges: 1,
    });
    sk.vel[2] = 0.0;
    sk.trick = None;
    sk.set_state(SkateState::Grinding);
    true
}

/// Put the mobj on the grind point (no collision: rails sit on blocking
/// lines).
fn place_on_grind(w: &mut World, player: usize) {
    let Some(mut sk) = w.players[player].skater.take() else { return };
    if let Some(mo) = w.players[player].mo {
        place_on_grind_mobj(w, mo, &mut sk);
    }
    w.players[player].skater = Some(sk);
}

fn place_on_grind_mobj(w: &mut World, mo: MobjId, sk: &mut Skater) {
    let Some(g) = sk.grind else { return };
    let Some(graph) = w.skate.grind.clone() else { return };
    let edge = &graph.edges[g.edge as usize];
    let p = edge.point(g.s);
    let z = edge.top_z(&w.level);
    relink(w, mo, p[0], p[1]);
    sk.pos = [p[0], p[1], z];
    let (floor, ceiling) = {
        let sec = w.level.subsectors[w.mo(mo).subsector.idx()].sector;
        let s = w.level.sector(sec);
        (s.floor_height, s.ceiling_height)
    };
    let m = w.mo_mut(mo);
    m.floorz = floor.min(to_fixed(z));
    m.ceilingz = ceiling;
    m.z = to_fixed(z);
}

/// Move a mobj without a collision check.
fn relink(w: &mut World, mo: MobjId, x: f32, y: f32) {
    unset_thing_position(w, mo);
    {
        let m = w.mo_mut(mo);
        m.x = to_fixed(x);
        m.y = to_fixed(y);
    }
    set_thing_position(w, mo);
}

fn grind_tic(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned, cmd: &TicCmd) {
    let Some(graph) = w.skate.grind.clone() else {
        sk.grind = None;
        sk.set_state(SkateState::Airborne);
        return;
    };
    let Some(mut g) = sk.grind else {
        sk.set_state(SkateState::Airborne);
        return;
    };
    if g.edge as usize >= graph.edges.len() {
        sk.grind = None;
        sk.set_state(SkateState::Airborne);
        return;
    }
    g.tics += 1;
    // Lean forward / back for nose and tail grinds.
    if matches!(g.style, GrindStyle::FiftyFifty | GrindStyle::Nosegrind | GrindStyle::FiveO) {
        g.style = match cmd.forwardmove {
            f if f > 0 => GrindStyle::Nosegrind,
            f if f < 0 => GrindStyle::FiveO,
            _ => GrindStyle::FiftyFifty,
        };
    }
    // Friction: slides drag more than truck grinds.
    let drag = match g.style {
        GrindStyle::Boardslide => 0.075,
        GrindStyle::Nosegrind | GrindStyle::FiveO | GrindStyle::Crooked => 0.06,
        GrindStyle::FiftyFifty => 0.045,
    };
    g.speed -= drag + g.speed * g.speed * 0.0002;

    // Balance: an inverted pendulum disturbed by speed and the edge, held
    // with the steering keys (assist helps).
    let (turn, lean) = steer_input(cmd);
    let steer = (lean + (turn / t.turn.max(1e-3)).clamp(-1.0, 1.0)).clamp(-1.0, 1.0);
    let wobble = sk.noise() * 0.0045 * (1.0 + g.speed / 12.0);
    g.balance_vel += wobble + g.balance * 0.012 + steer * 0.009;
    if t.assist {
        g.balance_vel -= g.balance * 0.025;
    }
    g.balance_vel *= 0.9;
    g.balance += g.balance_vel;
    sk.lean = g.balance;

    // Ride along, through connections at the ends.
    let mut edge_id = g.edge;
    let mut s = g.s + g.dir * g.speed;
    let mut turned = 0.0f32;
    let mut exit_end = None;
    for _ in 0..4 {
        let edge = &graph.edges[edge_id as usize];
        let end = if s > edge.length {
            1
        } else if s < 0.0 {
            0
        } else {
            break;
        };
        let over = if end == 1 { s - edge.length } else { -s };
        let follow = graph.connections_at(edge_id, end).iter().find(|c| match c.kind {
            ConnKind::Continuous => true,
            // Corners: assisted, or steered into, or taken slowly.
            ConnKind::Corner => t.assist || steer.signum() == c.turn.signum() || g.speed < 6.0,
        });
        match follow {
            Some(c) => {
                let next = &graph.edges[c.edge as usize];
                if c.enter_end == 0 {
                    s = over;
                    g.dir = 1.0;
                } else {
                    s = next.length - over;
                    g.dir = -1.0;
                }
                if c.kind == ConnKind::Corner {
                    g.speed *= 1.0 - c.turn.abs() / std::f32::consts::PI * 0.35;
                }
                turned += c.turn;
                edge_id = c.edge;
                g.edges += 1;
            }
            None => {
                exit_end = Some(end);
                s = s.clamp(0.0, edge.length);
                break;
            }
        }
    }
    g.edge = edge_id;
    g.s = s;
    sk.heading = wrap(sk.heading + turned);
    sk.grind = Some(g);
    let edge = &graph.edges[edge_id as usize];
    let travel = [edge.tangent[0] * g.dir, edge.tangent[1] * g.dir];
    let left = [-travel[1], travel[0]];

    if let Some(_end) = exit_end {
        // Off the end of the run.
        sk.vel = [travel[0] * g.speed, travel[1] * g.speed, 1.2];
        end_grind(w, mo, sk, &graph, 0.0);
        sk.set_state(SkateState::Airborne);
        return;
    }
    if g.balance.abs() >= 1.0 || g.speed < 0.8 {
        // Lost it (or stalled): step off to the side it leant.
        let side = if g.balance.abs() >= 1.0 { g.balance.signum() * 2.5 } else { 0.0 };
        sk.vel = [travel[0] * g.speed.max(0.0) + left[0] * side, travel[1] * g.speed.max(0.0) + left[1] * side, 0.5];
        // Stalled on a ledge: step back onto its top.
        let side = if side == 0.0 && edge.kind != GrindKind::Rail { -dot(edge.normal, left).signum() * 0.5 } else { side };
        end_grind(w, mo, sk, &graph, side);
        sk.set_state(SkateState::Airborne);
        return;
    }
    sk.set_state(SkateState::Grinding);
    place_on_grind_mobj(w, mo, sk);
}

/// Leave the edge: record the feat, and make sure the mobj is somewhere it
/// may stand (off a rail's blocking line), nudged toward `side` (+ = left
/// of travel).
fn end_grind(w: &mut World, mo: MobjId, sk: &mut Skater, graph: &GrindGraph, side: f32) {
    let Some(g) = sk.grind.take() else { return };
    sk.last_feat = Some(Feat::Grind { style: g.style, kind: graph.edges[g.edge as usize].kind, tics: g.tics, edges: g.edges });
    sk.last_edge = Some(g.edge);
    sk.regrind_cooldown = REGRIND_TICS;
    start_air(sk);
    let edge = &graph.edges[g.edge as usize];
    let travel = [edge.tangent[0] * g.dir, edge.tangent[1] * g.dir];
    let left = [-travel[1], travel[0]];
    let p = edge.point(g.s);
    let radius = fx(w.mo(mo).radius) + 1.5;
    let first = if side > 0.0 {
        1.0
    } else if side < 0.0 {
        -1.0
    } else {
        // Default: the raised side of a ledge, else the right.
        let top = [-edge.normal[0], -edge.normal[1]];
        if dot(top, left) >= 0.0 { 1.0 } else { -1.0 }
    };
    let z = sk.pos[2];
    for (k, off) in [(first, 0.0), (first, radius), (-first, radius), (first, radius * 2.0), (-first, radius * 2.0)] {
        let (x, y) = (p[0] + left[0] * k * off, p[1] + left[1] * k * off);
        let ok = crate::map::check_position(w, mo, to_fixed(x), to_fixed(y));
        if ok && fx(w.tm.tmfloorz) <= z + 1.0 && fx(w.tm.tmceilingz) - z >= fx(w.mo(mo).height) {
            let (fz, cz) = (w.tm.tmfloorz, w.tm.tmceilingz);
            relink(w, mo, x, y);
            let m = w.mo_mut(mo);
            m.floorz = fz;
            m.ceilingz = cz;
            sk.pos[0] = x;
            sk.pos[1] = y;
            return;
        }
    }
}

// ---- bails ---------------------------------------------------------------------

fn start_bail(w: &mut World, mo: MobjId, sk: &mut Skater) {
    let _ = (w, mo);
    let up = 3.0 + sk.speed() * 0.15;
    sk.bail = Some(BailState {
        board_vel: [sk.vel[0] * 0.9, sk.vel[1] * 0.9, up],
        board_spin: [0.25 + sk.noise() * 0.1, 0.18 * sk.noise(), 0.3 * sk.noise()],
        body_roll: 0.0,
    });
    if let Some(g) = sk.grind.take() {
        sk.last_edge = Some(g.edge);
        sk.regrind_cooldown = REGRIND_TICS;
    }
    sk.trick = None;
    sk.push_phase = 0;
    sk.last_feat = Some(Feat::Bail);
    sk.set_state(SkateState::Bail);
}

fn bail_tic(w: &mut World, mo: MobjId, sk: &mut Skater, t: &Tuned, cmd: &TicCmd) {
    // The body slides to a stop; the board tumbles away on its own.
    let floor = fx(w.mo(mo).floorz);
    if sk.pos[2] > floor || sk.vel[2] > 0.0 {
        sk.vel[2] -= t.gravity;
        sk.pos[2] = (sk.pos[2] + sk.vel[2]).max(floor);
        if sk.pos[2] <= floor {
            sk.vel[2] = 0.0;
        }
    }
    let damp = if sk.pos[2] <= floor { 0.86 } else { 0.98 };
    sk.vel[0] *= damp;
    sk.vel[1] *= damp;
    move_xy(w, mo, sk, t);
    if let Some(b) = &mut sk.bail {
        b.body_roll = (b.body_roll + 0.12).min(std::f32::consts::FRAC_PI_2);
        let bp = &mut sk.board.pos;
        bp[0] += b.board_vel[0];
        bp[1] += b.board_vel[1];
        bp[2] += b.board_vel[2];
        b.board_vel[2] -= t.gravity;
        let board_floor = floor_at_f(w, bp[0], bp[1]);
        if bp[2] <= board_floor {
            bp[2] = board_floor;
            b.board_vel[2] = -b.board_vel[2] * 0.35;
            b.board_vel[0] *= 0.7;
            b.board_vel[1] *= 0.7;
            for s in &mut b.board_spin {
                *s *= 0.6;
            }
        }
        sk.board.roll = wrap(sk.board.roll + b.board_spin[0]);
        sk.board.pitch = wrap(sk.board.pitch + b.board_spin[1]);
        sk.board.yaw = wrap(sk.board.yaw + b.board_spin[2]);
    }
    sk.crouch = 0.0;
    // Get up when the time is over (any movement key hurries it along).
    let hurry = cmd.forwardmove != 0 || cmd.ext & ext::JUMP != 0;
    if sk.state_tics >= BAIL_TICS || (hurry && sk.state_tics >= BAIL_TICS / 2) {
        sk.bail = None;
        sk.vel = [0.0; 3];
        sk.pos[2] = floor;
        sk.board = Board { pos: sk.pos, yaw: sk.heading, pitch: 0.0, roll: 0.0, flip: 0.0, length: sk.board.length, wheel_floor: [floor; 4] };
        sk.set_state(SkateState::Idle);
    }
}

// ---- board pose ------------------------------------------------------------------

fn floor_at_f(w: &World, x: f32, y: f32) -> f32 {
    let ss = crate::geometry::point_in_subsector(&w.level, to_fixed(x), to_fixed(y));
    let sec = w.level.subsectors[ss.idx()].sector;
    fx(w.level.sector(sec).floor_height)
}

/// Board position and tilt from the ground under its four wheels.
fn update_board(w: &World, sk: &mut Skater) {
    let f = dir(sk.heading);
    let l = [-f[1], f[0]];
    let half_wb = sk.board.length * WHEELBASE * 0.5;
    let half_tr = TRACK * 0.5;
    let wheels = [(half_wb, half_tr), (half_wb, -half_tr), (-half_wb, half_tr), (-half_wb, -half_tr)];
    for (i, (a, b)) in wheels.iter().enumerate() {
        let x = sk.pos[0] + f[0] * a + l[0] * b;
        let y = sk.pos[1] + f[1] * a + l[1] * b;
        sk.board.wheel_floor[i] = floor_at_f(w, x, y);
    }
    let wf = sk.board.wheel_floor;
    let (pitch_t, roll_t) = if sk.state.on_ground() {
        // Resting on the highest supports, tilted toward lower ones, but
        // never below the contact point.
        let front = wf[0].max(wf[1]).min(sk.pos[2] + 24.0);
        let back = wf[2].max(wf[3]).min(sk.pos[2] + 24.0);
        let lft = wf[0].max(wf[2]);
        let rgt = wf[1].max(wf[3]);
        let pitch = ((front - back) / (2.0 * half_wb)).atan().clamp(-0.6, 0.6);
        let roll = ((lft - rgt) / TRACK).atan().clamp(-0.3, 0.3) * 0.3;
        // Carving tips the deck toward the turn (lean + = left: left edge
        // down, which is negative roll).
        (pitch, roll - sk.lean * 0.22)
    } else if sk.state == SkateState::Ollie {
        // Tail snap: nose up, then level out.
        let k = sk.state_tics as f32 / 6.0;
        (0.45 * (k * std::f32::consts::PI).sin(), 0.0)
    } else if sk.state == SkateState::Grinding {
        // Nose and tail grinds tip the board onto one truck.
        let pitch = match sk.grind.map(|g| g.style) {
            Some(GrindStyle::Nosegrind) => -0.22,
            Some(GrindStyle::FiveO) => 0.22,
            _ => 0.0,
        };
        (pitch, -sk.lean * 0.12)
    } else {
        // In the air the nose follows the flight path a little.
        ((sk.vel[2] * 0.035).clamp(-0.3, 0.3), -sk.lean * 0.15)
    };
    let rate = if sk.state.on_ground() { 0.45 } else { 0.25 };
    sk.board.pitch += (pitch_t - sk.board.pitch) * rate;
    sk.board.roll += (roll_t - sk.board.roll) * rate;
    sk.board.yaw = sk.heading;
    sk.board.flip = 0.0;
    sk.board.pos = sk.pos;
}
