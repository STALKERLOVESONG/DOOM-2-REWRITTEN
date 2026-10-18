//! Hexen-style line specials (p_lnspec.cpp): the action set of Hexen-format
//! and UDMF maps, also called by ACS scripts and thing specials.
//!
//! A special is a number plus five arguments. Movers reuse the vanilla
//! thinkers (doors, floors, ceilings, plats, lights) with the special's own
//! speeds, heights and delays, so their movement, crushing and sounds are
//! Doom's. Specials not implemented yet are counted in
//! [`World::unimplemented`] rather than silently ignored.

use doom_content::map::{ml, spac, LineAction};
use doom_core::{Angle, Fixed, FRACUNIT};

use crate::ids::{LineId, MobjId, SectorId, ThinkerId};
use crate::info::*;
use crate::mobj::flags::*;
use crate::specials::ceilings::{add_active_ceiling, CeilingKind, CeilingMove};
use crate::specials::doors::{DoorKind, VerticalDoor};
use crate::specials::floors::{FloorKind, FloorMove};
use crate::specials::plats::{add_plat_unlimited, Plat, PlatKind, PlatStatus};
use crate::specials::{add_special, special, special_mut, SpecialThinker};
use crate::spec::{find_highest_ceiling_surrounding, find_highest_floor_surrounding, find_lowest_ceiling_surrounding, find_lowest_floor_surrounding};
use crate::world::{ExitTarget, GameExit, SoundSource, World};

/// What started a special: the activator and the line (and side) used.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpecialCtx {
    pub activator: Option<MobjId>,
    pub line: Option<LineId>,
    pub side: usize,
}

/// How a line was triggered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Cross,
    Use,
    Impact,
    Push,
}

/// Hexen speed units (1/8 map unit per tic).
fn speed(a: i32) -> Fixed {
    Fixed(a * (FRACUNIT / 8))
}

fn units(a: i32) -> Fixed {
    Fixed(a.wrapping_mul(FRACUNIT))
}

/// Deterministic random for specials and scripts (not `P_Random`).
pub fn script_random(w: &mut World) -> i32 {
    // xorshift32
    let mut x = w.script_rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    w.script_rng = x;
    (x >> 8) as i32 & 0xff
}

pub fn sectors_by_tag(w: &World, tag: i32) -> Vec<SectorId> {
    (0..w.level.num_map_sectors()).filter(|&i| w.level.sectors[i].tag as i32 == tag).map(|i| SectorId(i as u32)).collect()
}

pub fn lines_by_id(w: &World, id: i32) -> Vec<LineId> {
    w.level.lines.iter().enumerate().filter(|(_, l)| l.id == id).map(|(i, _)| LineId(i as u32)).collect()
}

/// Live map objects with a TID, in thinker order.
pub fn mobjs_by_tid(w: &World, tid: i32) -> Vec<MobjId> {
    w.thinkers.mobjs().filter(|(id, m)| m.tid == tid && w.thinkers.func(*id) == Some(crate::thinker::ThinkFn::Active)).map(|(id, _)| id).collect()
}

/// TID 0 means the activator.
fn targets(w: &World, tid: i32, ctx: &SpecialCtx) -> Vec<MobjId> {
    if tid == 0 {
        ctx.activator.into_iter().collect()
    } else {
        mobjs_by_tid(w, tid)
    }
}

/// Sectors a tag addresses; tag 0 on a line is the line's back sector
/// (manual doors and the like).
fn tagged(w: &World, tag: i32, ctx: &SpecialCtx) -> Vec<SectorId> {
    if tag == 0 {
        ctx.line.and_then(|l| w.level.line(l).back_sector).into_iter().collect()
    } else {
        sectors_by_tag(w, tag)
    }
}

/// `P_ActivateLine`: run a line's Hexen special if `how` and the activator
/// match its activation flags. Returns whether it ran.
pub fn activate_line(w: &mut World, line: LineId, side: usize, thing: Option<MobjId>, how: Trigger) -> bool {
    let Some(action) = w.level.line(line).action else { return false };
    let (is_player, is_missile) = match thing.and_then(|t| w.try_mo(Some(t))) {
        Some(m) => (m.player.is_some(), m.flags & MF_MISSILE != 0),
        None => (false, false),
    };
    let a = action.activation;
    let allowed = match how {
        Trigger::Cross => {
            (a & spac::ANYCROSS != 0)
                || if is_missile {
                    a & spac::PCROSS != 0
                } else if is_player {
                    a & spac::CROSS != 0
                } else {
                    a & spac::MCROSS != 0
                }
        }
        Trigger::Use => {
            (if is_player { a & spac::USE != 0 } else { a & spac::MUSE != 0 }) && (side == 0 || a & spac::USEBACK != 0)
        }
        Trigger::Impact => a & spac::IMPACT != 0,
        Trigger::Push => if is_player { a & spac::PUSH != 0 } else { a & spac::MPUSH != 0 },
    };
    if !allowed || (a & spac::FIRSTSIDEONLY != 0 && side != 0) {
        return false;
    }
    // Monsters never use secret lines.
    if !is_player && how == Trigger::Use && w.level.line(line).flags & ml::SECRET != 0 {
        return false;
    }
    if action.lock != 0 && !check_lock(w, thing, action.lock, true) {
        return false;
    }
    let ctx = SpecialCtx { activator: thing, line: Some(line), side };
    let ok = execute(w, action.special, action.args, &ctx);
    if ok {
        if !action.repeat {
            let l = &mut w.level.lines[line.idx()];
            l.action = None;
            l.special = 0;
        }
        if matches!(how, Trigger::Use | Trigger::Impact) {
            // Clears `special` for one-shot switches, as we already did.
            crate::spec::change_switch_texture(w, line, action.repeat);
        }
    }
    ok
}

/// ZDoom lock numbers against the activator's keys; on failure a player is
/// told which key is needed.
pub fn check_lock(w: &mut World, thing: Option<MobjId>, lock: i32, remote: bool) -> bool {
    let Some(p) = thing.and_then(|t| w.try_mo(Some(t))).and_then(|m| m.player) else { return false };
    let cards = w.players[p].cards; // blue, yellow, red card; blue, yellow, red skull
    let (ok, msg) = match lock {
        1 => (cards[2], "red card"),
        2 => (cards[0], "blue card"),
        3 => (cards[1], "yellow card"),
        4 => (cards[5], "red skull"),
        5 => (cards[3], "blue skull"),
        6 => (cards[4], "yellow skull"),
        100 | 229 => (cards.iter().any(|&c| c), "any key"),
        101 => (cards.iter().all(|&c| c), "all six keys"),
        129 => (cards[2] || cards[5], "red key"),
        130 => (cards[0] || cards[3], "blue key"),
        131 => (cards[1] || cards[4], "yellow key"),
        _ => (true, ""),
    };
    if !ok {
        let what = if remote { "activate this object" } else { "open this door" };
        w.script_events.push(crate::world::ScriptEvent::Print { player: Some(p), text: format!("You need a {msg} to {what}") });
        w.start_sound(SoundSource::Mobj(thing.unwrap()), SFX_OOF);
    }
    ok
}

/// Run a special. Returns whether it did anything (switches only change
/// and one-shot lines only clear when it did).
pub fn execute(w: &mut World, special: i32, args: [i32; 5], ctx: &SpecialCtx) -> bool {
    let [a0, a1, a2, a3, a4] = args;
    match special {
        0 => false,
        // ---- doors ----
        10 => do_door(w, ctx, DoorType::Close, a0, speed(a1), 0),
        11 => do_door(w, ctx, DoorType::Open, a0, speed(a1), 0),
        12 => do_door(w, ctx, DoorType::Raise, a0, speed(a1), a2),
        13 => check_lock(w, ctx.activator, a3, false) && do_door(w, ctx, DoorType::Raise, a0, speed(a1), a2),
        105 => do_door(w, ctx, DoorType::WaitRaise, a0, speed(a1), a2),
        249 => do_door(w, ctx, DoorType::CloseWaitOpen, a0, speed(a1), a2 * 35 / 8),
        // Generic_Door(tag, speed, kind, delay (octics), lock)
        202 => {
            if a4 != 0 && !check_lock(w, ctx.activator, a4, false) {
                return false;
            }
            let kind = match a2 & 3 {
                0 => DoorType::Raise,
                1 => DoorType::Open,
                2 => DoorType::CloseWaitOpen,
                _ => DoorType::Close,
            };
            do_door(w, ctx, kind, a0, speed(a1), a3 * 35 / 8)
        }
        // ---- floors ----
        20 => do_floor(w, ctx, a0, speed(a1), 0, |_, s| s.floor_height - units(a2)),
        21 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| find_lowest_floor_surrounding(w, s.id)),
        22 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| find_next_lowest_floor(w, s.id, s.floor_height)),
        23 => do_floor(w, ctx, a0, speed(a1), 0, |_, s| s.floor_height + units(a2)),
        24 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| find_highest_floor_surrounding(w, s.id)),
        25 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| next_highest_floor(w, s.id, s.floor_height)),
        28 => do_floor(w, ctx, a0, speed(a1), 1, |w, s| (find_lowest_ceiling_surrounding(w, s.id) - units(8)).min(s.ceiling_height - units(8))),
        35 => do_floor(w, ctx, a0, speed(a1), 0, |_, s| s.floor_height + units(a2 * 8)),
        36 => do_floor(w, ctx, a0, speed(a1), 0, |_, s| s.floor_height - units(a2 * 8)),
        37 => do_floor(w, ctx, a0, speed(a1), 0, |_, _| units(if a3 != 0 { -a2 } else { a2 })),
        66 => do_floor(w, ctx, a0, Fixed(0), 0, |_, s| s.floor_height - units(a2 * 8)),
        67 => do_floor(w, ctx, a0, Fixed(0), 0, |_, s| s.floor_height + units(a2 * 8)),
        68 => do_floor(w, ctx, a0, speed(a1), 0, |_, _| units(if a3 != 0 { -a2 * 8 } else { a2 * 8 })),
        238 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| find_lowest_ceiling_surrounding(w, s.id).min(s.ceiling_height)),
        239 => do_floor(w, ctx, a0, speed(a1), 0, |_, s| s.floor_height + units(a2)),
        240 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| s.floor_height + Fixed(shortest_lower_texture(w, s.id))),
        241 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| find_lowest_floor_surrounding(w, s.id)),
        // Floor_LowerToHighest(tag, speed, adjust, force_adjust)
        242 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| {
            let h = find_highest_floor_surrounding(w, s.id);
            if a3 != 0 || h != s.floor_height {
                h + units(a2 - 128)
            } else {
                h
            }
        }),
        // Generic_Floor(tag, speed, height, target, flags)
        200 => {
            let up = a4 & 8 != 0;
            let crush = (a4 & 16 != 0) as i32;
            do_floor(w, ctx, a0, speed(a1), crush, |w, s| {
                let dir = if up { 1 } else { -1 };
                match a3 {
                    1 => find_highest_floor_surrounding(w, s.id),
                    2 => find_lowest_floor_surrounding(w, s.id),
                    3 => {
                        if up {
                            next_highest_floor(w, s.id, s.floor_height)
                        } else {
                            find_next_lowest_floor(w, s.id, s.floor_height)
                        }
                    }
                    4 => find_lowest_ceiling_surrounding(w, s.id),
                    5 => s.ceiling_height,
                    6 => s.floor_height + Fixed(dir * shortest_lower_texture(w, s.id)),
                    _ => s.floor_height + Fixed(dir * a2 * FRACUNIT),
                }
            })
        }
        // ---- stairs ----
        26 => build_stairs(w, ctx, a0, speed(a1), -units(a2)),
        27 | 217 => build_stairs(w, ctx, a0, speed(a1), units(a2)),
        204 => build_stairs(w, ctx, a0, speed(a1), if a3 & 1 != 0 { units(a2) } else { -units(a2) }),
        // ---- ceilings ----
        40 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::LowerToFloor, |_, s| s.ceiling_height - units(a2)),
        41 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::RaiseToHighest, |_, s| s.ceiling_height + units(a2)),
        198 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::RaiseToHighest, |_, s| s.ceiling_height + units(a2 * 8)),
        199 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::LowerToFloor, |_, s| s.ceiling_height - units(a2 * 8)),
        43 | 97 => do_ceiling(w, ctx, a0, speed(a1), a2.max(1), CeilingKind::LowerAndCrush, |_, s| s.floor_height + units(8)),
        42 | 45 | 168 | 195..=197 | 205 => {
            let silent = special == 197 || (special == 205 && a3 != 0);
            let kind = if silent { CeilingKind::SilentCrushAndRaise } else { CeilingKind::CrushAndRaise };
            do_ceiling(w, ctx, a0, speed(a1), 1, kind, |_, s| s.floor_height + units(8))
        }
        44 => stop_crushers(w, a0),
        47 | 69 => {
            let v = if special == 69 { a2 * 8 } else { a2 };
            let dest = units(if a3 != 0 { -v } else { v });
            do_ceiling_to(w, ctx, a0, speed(a1), dest)
        }
        192 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::LowerToFloor, |w, s| find_highest_floor_surrounding(w, s.id)),
        193 => do_ceiling(w, ctx, a0, Fixed(0), 0, CeilingKind::LowerToFloor, |_, s| s.ceiling_height - units(a2 * 8)),
        194 => do_ceiling(w, ctx, a0, Fixed(0), 0, CeilingKind::RaiseToHighest, |_, s| s.ceiling_height + units(a2 * 8)),
        252 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::RaiseToHighest, |w, s| next_highest_ceiling(w, s.id, s.ceiling_height)),
        253 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::LowerToFloor, |w, s| lowest_ceiling_surrounding_any(w, s.id)),
        254 => do_ceiling(w, ctx, a0, speed(a1), 0, CeilingKind::LowerToFloor, |_, s| s.floor_height),
        // Generic_Ceiling(tag, speed, height, target, flags)
        201 => {
            let up = a4 & 8 != 0;
            let crush = (a4 & 16 != 0) as i32;
            let kind = if up { CeilingKind::RaiseToHighest } else { CeilingKind::LowerToFloor };
            do_ceiling(w, ctx, a0, speed(a1), crush, kind, |w, s| match a3 {
                1 => find_highest_ceiling_surrounding(w, s.id),
                2 => lowest_ceiling_surrounding_any(w, s.id),
                3 => next_highest_ceiling(w, s.id, s.ceiling_height),
                4 => find_highest_floor_surrounding(w, s.id),
                5 => s.floor_height,
                _ => s.ceiling_height + units(if up { a2 } else { -a2 }),
            })
        }
        // ---- plats ----
        60 => do_plat(w, ctx, a0, PlatKind::PerpetualRaise, speed(a1), a2, |w, s| {
            (find_lowest_floor_surrounding(w, s.id).min(s.floor_height), find_highest_floor_surrounding(w, s.id).max(s.floor_height))
        }),
        207 => do_plat(w, ctx, a0, PlatKind::PerpetualRaise, speed(a1), a2, |w, s| {
            ((find_lowest_floor_surrounding(w, s.id) + units(a3)).min(s.floor_height), find_highest_floor_surrounding(w, s.id).max(s.floor_height))
        }),
        61 => stop_plats(w, a0),
        62 | 206 => {
            let lip = if special == 206 { a3 } else { 8 };
            do_plat(w, ctx, a0, PlatKind::DownWaitUpStay, speed(a1), a2, |w, s| {
                ((find_lowest_floor_surrounding(w, s.id) + units(lip)).min(s.floor_height), s.floor_height)
            })
        }
        63 => do_plat(w, ctx, a0, PlatKind::DownWaitUpStay, speed(a1), a2, |_, s| (s.floor_height - units(a3 * 8), s.floor_height)),
        64 => do_plat(w, ctx, a0, PlatKind::UpWaitDownStay, speed(a1), a2, |w, s| {
            (s.floor_height, find_highest_floor_surrounding(w, s.id).max(s.floor_height))
        }),
        65 => do_plat(w, ctx, a0, PlatKind::UpWaitDownStay, speed(a1), a2, |_, s| (s.floor_height, s.floor_height + units(a3 * 8))),
        172 => do_plat(w, ctx, a0, PlatKind::UpWaitDownStay, speed(a1), a2, |w, s| (s.floor_height, next_highest_floor(w, s.id, s.floor_height))),
        // Generic_Lift(tag, speed, delay (octics), type, height)
        203 => {
            let delay = a2 * 35 / 8;
            match a3 {
                4 => execute(w, 60, [a0, a1, delay, 0, 0], ctx),
                1..=3 => do_plat(w, ctx, a0, PlatKind::DownWaitUpStay, speed(a1), delay, |w, s| {
                    let low = match a3 {
                        1 => find_lowest_floor_surrounding(w, s.id),
                        2 => find_next_lowest_floor(w, s.id, s.floor_height),
                        _ => lowest_ceiling_surrounding_any(w, s.id),
                    };
                    (low.min(s.floor_height), s.floor_height)
                }),
                _ => do_plat(w, ctx, a0, PlatKind::UpWaitDownStay, speed(a1), delay, |_, s| (s.floor_height, s.floor_height + units(a4 * 8))),
            }
        }
        // Plat_UpByValueStayTx(tag, speed, height): up and stay.
        230 => do_floor(w, ctx, a0, speed(a1), 0, |_, s| s.floor_height + units(a2 * 8)),
        228 => do_floor(w, ctx, a0, speed(a1), 0, |w, s| next_highest_floor(w, s.id, s.floor_height)),
        // ---- lights ----
        110 => change_lights(w, a0, |l| l + a1),
        111 => change_lights(w, a0, |l| l - a1),
        112 => change_lights(w, a0, |_| a1),
        113 => start_light(w, a0, |l| HexenLightKind::Fade { from: l, to: a1, tics: a2.max(1), t: 0 }),
        114 => start_light(w, a0, |_| HexenLightKind::Glow { upper: a1, lower: a2, tics: a3.max(1), t: 0, up: false }),
        115 => start_light(w, a0, |_| HexenLightKind::Flicker { upper: a1, lower: a2, count: 0 }),
        116 => start_light(w, a0, |_| HexenLightKind::Strobe { upper: a1, lower: a2, utics: a3.max(1), ltics: a4.max(1), count: 0, bright: true }),
        117 => stop_lights(w, a0),
        232 => {
            for s in sectors_by_tag(w, a0) {
                crate::specials::lights::spawn_strobe_flash(w, s, a2.max(1), false);
            }
            true
        }
        233 | 234 => {
            for s in sectors_by_tag(w, a0) {
                let lights: Vec<i32> = w.level.lines_of(s).iter().filter_map(|&l| crate::spec::next_sector(&w.level, l, s)).map(|o| w.level.sector(o).light_level as i32).collect();
                let v = if special == 233 { lights.iter().min() } else { lights.iter().max() };
                if let Some(&v) = v {
                    w.level.sector_mut(s).light_level = v as i16;
                }
            }
            true
        }
        // ---- teleports and exits ----
        70 => teleport(w, ctx, a0, a1, a2 == 0, false),
        71 => teleport(w, ctx, a0, a2, false, a1 == 0),
        154 => teleport(w, ctx, a0, a1, a2 == 0, false),
        74 => {
            w.exit_target = Some(ExitTarget::Number(a0));
            w.gameexit = Some(GameExit::Normal);
            true
        }
        75 => {
            w.exit_target = Some(ExitTarget::EndGame);
            w.gameexit = Some(GameExit::Normal);
            true
        }
        243 => {
            w.gameexit = Some(GameExit::Normal);
            true
        }
        244 => {
            w.gameexit = Some(GameExit::Secret);
            true
        }
        // ---- scripts ----
        80 | 83 | 84 | 85 | 226 => {
            if matches!(special, 83 | 85) && a4 != 0 && !check_lock(w, ctx.activator, a4, true) {
                return false;
            }
            let kind = if special == 226 { doom_script::acs::RequestKind::ExecuteAlways } else { doom_script::acs::RequestKind::Execute };
            let (map, a) = if special == 84 { (0, [a1, a2, a3, a4]) } else { (a1, [a2, a3, a4, 0]) };
            request_script(w, ctx, doom_script::acs::ScriptId::Number(a0), map, a, kind);
            true
        }
        81 | 82 => {
            let kind = if special == 81 { doom_script::acs::RequestKind::Suspend } else { doom_script::acs::RequestKind::Terminate };
            request_script(w, ctx, doom_script::acs::ScriptId::Number(a0), a1, [0; 4], kind);
            true
        }
        // ---- things ----
        72 => {
            // ThrustThing(angle (byte), force, nolimit, tid)
            let an = Angle((a0 as u32 & 0xff) << 24);
            let force = units(a1);
            for t in targets(w, a3, ctx) {
                let m = w.mo_mut(t);
                m.momx += force * doom_core::tables::cosine(an);
                m.momy += force * doom_core::tables::sine(an);
            }
            true
        }
        128 => {
            // ThrustThingZ(tid, speed, down, add)
            let v = Fixed(a1 * FRACUNIT / 4) * if a2 != 0 { -1 } else { 1 };
            for t in targets(w, a0, ctx) {
                let m = w.mo_mut(t);
                m.momz = if a3 != 0 { m.momz + v } else { v };
            }
            true
        }
        73 => damage(w, targets(w, 0, ctx), a0),
        119 => damage(w, targets(w, a0, ctx), a1),
        133 => {
            let things = targets(w, a0, ctx).into_iter().filter(|&t| w.mo(t).flags & MF_SHOOTABLE != 0).collect();
            damage(w, things, 10000)
        }
        248 => {
            // HealThing(amount, max)
            let Some(t) = ctx.activator else { return false };
            let max = if a1 > 0 { a1 } else if w.mo(t).player.is_some() { 100 } else { w.info.mobj(w.mo(t).mobj_type).spawnhealth };
            let m = w.mo_mut(t);
            m.health = (m.health + a0).min(max.max(m.health));
            let h = m.health;
            if let Some(p) = m.player {
                w.players[p].health = h;
            }
            true
        }
        132 => {
            for t in targets(w, a0, ctx) {
                crate::mobj::remove_mobj(w, t);
            }
            true
        }
        127 => {
            for t in targets(w, a0, ctx) {
                let m = w.mo_mut(t);
                m.special = a1;
                m.args = [a2, a3, a4, 0, 0];
            }
            true
        }
        176 => {
            for t in targets(w, a0, ctx) {
                w.mo_mut(t).tid = a1;
            }
            true
        }
        19 => {
            for t in targets(w, a0, ctx) {
                let m = w.mo_mut(t);
                (m.momx, m.momy, m.momz) = (Fixed(0), Fixed(0), Fixed(0));
            }
            true
        }
        125 => {
            // Thing_Move(tid, desttid, nofog)
            let Some(&dest) = mobjs_by_tid(w, a1).first() else { return false };
            let (x, y, an) = (w.mo(dest).x, w.mo(dest).y, w.mo(dest).angle);
            let mut any = false;
            for t in targets(w, a0, ctx) {
                any |= teleport_thing(w, t, x, y, an, a2 == 0, true);
            }
            any
        }
        135 | 137 | 139 => {
            // Thing_Spawn(tid, type, angle, newtid) / NoFog / Facing(tid, type, nofog, newtid)
            let Some(t) = spawn_id_type(w, a1) else {
                w.note_unimplemented("Thing_Spawn: unknown spawn number");
                return false;
            };
            let fog = special == 135 || (special == 139 && a2 == 0);
            let newtid = a3;
            let mut any = false;
            for spot in mobjs_by_tid(w, a0) {
                let s = w.mo(spot);
                let (x, y, z, sa) = (s.x, s.y, s.z, s.angle);
                let angle = if special == 139 { sa } else { Angle((a2 as u32 & 0xff) << 24) };
                any |= spawn_at(w, t, x, y, z, angle, newtid, fog).is_some();
            }
            any
        }
        // ---- lines ----
        55 => {
            // Line_SetBlocking(lineid, set, clear)
            let bits = |f: i32| {
                let mut b = 0u16;
                if f & (1 | 4 | 32) != 0 {
                    b |= ml::BLOCKING;
                }
                if f & 2 != 0 {
                    b |= ml::BLOCKMONSTERS;
                }
                if f & 1024 != 0 {
                    b |= ml::SOUNDBLOCK;
                }
                b
            };
            for l in lines_by_id(w, a0) {
                let line = &mut w.level.lines[l.idx()];
                line.flags = (line.flags | bits(a1)) & !bits(a2);
            }
            true
        }
        // ---- sectors ----
        214 => {
            // Sector_SetDamage(tag, amount, mod): nearest Doom damage type.
            let doom = match a1 {
                0 => 0,
                1..=5 => 7,
                6..=10 => 5,
                _ => 16,
            };
            for s in sectors_by_tag(w, a0) {
                w.level.sector_mut(s).special = doom;
            }
            true
        }
        // Map setup specials handled at load, and effects with no
        // gameplay consequence yet.
        121 | 208 | 9 | 50 | 160 | 181 | 190 | 209..=211 => true,
        _ => {
            w.note_unimplemented(special_name(special));
            false
        }
    }
}

fn request_script(w: &mut World, ctx: &SpecialCtx, script: doom_script::acs::ScriptId, map: i32, args: [i32; 4], kind: doom_script::acs::RequestKind) {
    let act = doom_script::acs::Activation {
        activator: ctx.activator.map(actor_handle),
        line: ctx.line.map(|l| l.0),
        side: ctx.side as u8,
    };
    let r = doom_script::acs::ScriptRequest { script, map, args, act, kind };
    match w.acs.as_mut() {
        Some(acs) => {
            acs.request(&r);
        }
        // Scripts are running (the runtime is lent out): queue it.
        None => w.acs_requests.push(r),
    }
}

/// ACS sees actors as opaque 32-bit handles: the thinker slot index + 1.
pub fn actor_handle(id: MobjId) -> u32 {
    id.index + 1
}

pub fn actor_from_handle(w: &World, h: u32) -> Option<MobjId> {
    let idx = h.checked_sub(1)?;
    w.thinkers.mobjs().find(|(id, _)| id.index == idx).map(|(id, _)| id)
}

// ---- movers --------------------------------------------------------------------

/// Sector fields a mover's destination is computed from.
struct SecInfo {
    id: SectorId,
    floor_height: Fixed,
    ceiling_height: Fixed,
}

fn sec_info(w: &World, s: SectorId) -> SecInfo {
    let sec = w.level.sector(s);
    SecInfo { id: s, floor_height: sec.floor_height, ceiling_height: sec.ceiling_height }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DoorType {
    Raise,
    Open,
    Close,
    CloseWaitOpen,
    WaitRaise,
}

fn do_door(w: &mut World, ctx: &SpecialCtx, kind: DoorType, tag: i32, speed: Fixed, delay: i32) -> bool {
    let blaze = speed >= Fixed(8 * FRACUNIT);
    let mut any = false;
    for sec in tagged(w, tag, ctx) {
        // A manual door already moving: reverse it, as EV_VerticalDoor does.
        if let Some(id) = w.level.sector(sec).special_data {
            let by_player = ctx.activator.is_some_and(|a| w.try_mo(Some(a)).is_some_and(|m| m.player.is_some()));
            if tag == 0 {
                if let Some(SpecialThinker::Door(d)) = special_mut(w, id) {
                    if d.direction == -1 {
                        d.direction = 1;
                    } else if kind == DoorType::Raise && by_player {
                        d.direction = -1;
                    }
                    any = true;
                }
            }
            continue;
        }
        let top = find_lowest_ceiling_surrounding(w, sec) - units(4);
        let ceiling = w.level.sector(sec).ceiling_height;
        let (dk, direction, sound) = match kind {
            DoorType::Raise => (if blaze { DoorKind::BlazeRaise } else { DoorKind::Normal }, 1, if blaze { SFX_BDOPN } else { SFX_DOROPN }),
            DoorType::Open => (if blaze { DoorKind::BlazeOpen } else { DoorKind::Open }, 1, if blaze { SFX_BDOPN } else { SFX_DOROPN }),
            DoorType::Close => (if blaze { DoorKind::BlazeClose } else { DoorKind::Close }, -1, if blaze { SFX_BDCLS } else { SFX_DORCLS }),
            DoorType::CloseWaitOpen => (DoorKind::Close30ThenOpen, -1, SFX_DORCLS),
            DoorType::WaitRaise => (DoorKind::RaiseIn5Mins, 2, SfxNum(0)),
        };
        let d = VerticalDoor {
            kind: dk,
            sector: sec,
            topheight: if kind == DoorType::CloseWaitOpen { ceiling } else { top },
            speed: if speed.0 > 0 { speed } else { crate::specials::doors::VDOORSPEED },
            direction,
            topwait: if delay > 0 { delay } else { crate::specials::doors::VDOORWAIT },
            topcountdown: if kind == DoorType::WaitRaise { delay.max(1) } else { 0 },
        };
        let id = add_special(w, SpecialThinker::Door(d));
        w.level.sector_mut(sec).special_data = Some(id);
        if direction == 1 && top == ceiling {
            // Already open: nothing to hear.
        } else if sound.0 != 0 {
            w.start_sound(SoundSource::Sector(sec), sound);
        }
        any = true;
    }
    any
}

/// Start a floor mover in every tagged sector that isn't moving, toward
/// `dest(sector)`. Zero speed moves it there in one tic (the speed is one
/// more than the distance, so the mover passes its destination and ends).
fn do_floor(w: &mut World, ctx: &SpecialCtx, tag: i32, speed: Fixed, crush: i32, dest: impl Fn(&World, &SecInfo) -> Fixed) -> bool {
    let mut any = false;
    for sec in tagged(w, tag, ctx) {
        if w.level.sector(sec).special_data.is_some() {
            continue;
        }
        let s = sec_info(w, sec);
        let d = dest(w, &s);
        if d == s.floor_height {
            continue;
        }
        let direction = if d > s.floor_height { 1 } else { -1 };
        let f = FloorMove {
            kind: if direction > 0 { FloorKind::RaiseFloor } else { FloorKind::LowerFloor },
            crush,
            sector: sec,
            direction,
            newspecial: 0,
            texture: crate::ids::FlatNum(0),
            floordestheight: d,
            speed: if speed.0 > 0 { speed } else { Fixed((d - s.floor_height).0.abs() + 1) },
        };
        let id = add_special(w, SpecialThinker::Floor(f));
        w.level.sector_mut(sec).special_data = Some(id);
        any = true;
    }
    any
}

fn ceiling_busy(w: &World, sec: SectorId) -> bool {
    let s = w.level.sector(sec);
    s.ceiling_data.is_some() || s.special_data.is_some_and(|id| matches!(special(w, id), Some(SpecialThinker::Ceiling(_) | SpecialThinker::Door(_))))
}

fn do_ceiling(
    w: &mut World,
    ctx: &SpecialCtx,
    tag: i32,
    speed: Fixed,
    crush: i32,
    kind: CeilingKind,
    dest: impl Fn(&World, &SecInfo) -> Fixed,
) -> bool {
    let mut any = false;
    for sec in tagged(w, tag, ctx) {
        if ceiling_busy(w, sec) {
            continue;
        }
        let s = sec_info(w, sec);
        let d = dest(w, &s);
        let crusher = matches!(kind, CeilingKind::CrushAndRaise | CeilingKind::SilentCrushAndRaise | CeilingKind::LowerAndCrush);
        if !crusher && d == s.ceiling_height {
            continue;
        }
        let direction = match kind {
            CeilingKind::RaiseToHighest => 1,
            _ => -1,
        };
        let c = CeilingMove {
            kind,
            sector: sec,
            bottomheight: if direction < 0 { d } else { s.floor_height },
            topheight: if direction > 0 { d } else { s.ceiling_height },
            speed: if speed.0 > 0 { speed } else { Fixed((d - s.ceiling_height).0.abs() + 1) },
            crush: crush != 0,
            direction,
            tag: w.level.sector(sec).tag,
            olddirection: direction,
        };
        let id = add_special(w, SpecialThinker::Ceiling(c));
        w.level.sector_mut(sec).ceiling_data = Some(id);
        add_active_ceiling(w, id);
        any = true;
    }
    any
}

/// `Ceiling_MoveToValue`: up or down, whichever reaches `dest`.
fn do_ceiling_to(w: &mut World, ctx: &SpecialCtx, tag: i32, speed: Fixed, dest: Fixed) -> bool {
    let mut any = false;
    for sec in tagged(w, tag, ctx) {
        if ceiling_busy(w, sec) {
            continue;
        }
        let up = dest > w.level.sector(sec).ceiling_height;
        let kind = if up { CeilingKind::RaiseToHighest } else { CeilingKind::LowerToFloor };
        any |= do_ceiling_sector(w, sec, speed, kind, dest);
    }
    any
}

fn do_ceiling_sector(w: &mut World, sec: SectorId, speed: Fixed, kind: CeilingKind, dest: Fixed) -> bool {
    let s = sec_info(w, sec);
    if dest == s.ceiling_height {
        return false;
    }
    let direction = if kind == CeilingKind::RaiseToHighest { 1 } else { -1 };
    let c = CeilingMove {
        kind,
        sector: sec,
        bottomheight: if direction < 0 { dest } else { s.floor_height },
        topheight: if direction > 0 { dest } else { s.ceiling_height },
        speed: if speed.0 > 0 { speed } else { Fixed((dest - s.ceiling_height).0.abs() + 1) },
        crush: false,
        direction,
        tag: w.level.sector(sec).tag,
        olddirection: direction,
    };
    let id = add_special(w, SpecialThinker::Ceiling(c));
    w.level.sector_mut(sec).ceiling_data = Some(id);
    add_active_ceiling(w, id);
    true
}

fn stop_crushers(w: &mut World, tag: i32) -> bool {
    let mut any = false;
    for sec in sectors_by_tag(w, tag) {
        for id in [w.level.sector(sec).ceiling_data, w.level.sector(sec).special_data].into_iter().flatten() {
            if let Some(SpecialThinker::Ceiling(c)) = special_mut(w, id) {
                if c.direction != 0 {
                    c.olddirection = c.direction;
                    c.direction = 0;
                    w.thinkers.set_func(id, crate::thinker::ThinkFn::Stasis);
                    any = true;
                }
            }
        }
    }
    any
}

fn do_plat(w: &mut World, ctx: &SpecialCtx, tag: i32, kind: PlatKind, speed: Fixed, wait: i32, range: impl Fn(&World, &SecInfo) -> (Fixed, Fixed)) -> bool {
    let mut any = false;
    for sec in tagged(w, tag, ctx) {
        if w.level.sector(sec).special_data.is_some() {
            continue;
        }
        let s = sec_info(w, sec);
        let (low, high) = range(w, &s);
        let status = match kind {
            PlatKind::UpWaitDownStay => PlatStatus::Up,
            PlatKind::PerpetualRaise => {
                if script_random(w) & 1 != 0 {
                    PlatStatus::Up
                } else {
                    PlatStatus::Down
                }
            }
            _ => PlatStatus::Down,
        };
        let p = Plat {
            sector: sec,
            speed: if speed.0 > 0 { speed } else { crate::specials::plats::PLATSPEED },
            low,
            high,
            wait: wait.max(1),
            count: 0,
            status,
            oldstatus: status,
            crush: false,
            tag: w.level.sector(sec).tag,
            kind,
        };
        let id = add_special(w, SpecialThinker::Plat(p));
        w.level.sector_mut(sec).special_data = Some(id);
        add_plat_unlimited(w, id);
        w.start_sound(SoundSource::Sector(sec), SFX_PSTART);
        any = true;
    }
    any
}

fn stop_plats(w: &mut World, tag: i32) -> bool {
    let mut any = false;
    for sec in sectors_by_tag(w, tag) {
        if let Some(id) = w.level.sector(sec).special_data {
            if let Some(SpecialThinker::Plat(p)) = special_mut(w, id) {
                if p.status != PlatStatus::InStasis {
                    p.oldstatus = p.status;
                    p.status = PlatStatus::InStasis;
                    w.thinkers.set_func(id, crate::thinker::ThinkFn::Stasis);
                    any = true;
                }
            }
        }
    }
    any
}

/// Doom-style stairs: each step is the next sector across a two-sided line
/// whose front is the current step, with the same floor flat.
fn build_stairs(w: &mut World, ctx: &SpecialCtx, tag: i32, speed: Fixed, step: Fixed) -> bool {
    let mut any = false;
    for start in tagged(w, tag, ctx) {
        if w.level.sector(start).special_data.is_some() {
            continue;
        }
        let mut sec = start;
        let mut height = w.level.sector(sec).floor_height + step;
        let texture = w.level.sector(sec).floor_pic;
        let mut visited = vec![sec];
        loop {
            let direction = if step.0 > 0 { 1 } else { -1 };
            let f = FloorMove {
                kind: if direction > 0 { FloorKind::RaiseFloor } else { FloorKind::LowerFloor },
                crush: 0,
                sector: sec,
                direction,
                newspecial: 0,
                texture: crate::ids::FlatNum(0),
                floordestheight: height,
                speed: if speed.0 > 0 { speed } else { Fixed(FRACUNIT / 4) },
            };
            let id = add_special(w, SpecialThinker::Floor(f));
            w.level.sector_mut(sec).special_data = Some(id);
            any = true;
            let next = w.level.lines_of(sec).iter().find_map(|&l| {
                let line = w.level.line(l);
                let t = line.back_sector?;
                (line.flags & ml::TWOSIDED != 0
                    && line.front_sector == Some(sec)
                    && w.level.sector(t).floor_pic == texture
                    && w.level.sector(t).special_data.is_none()
                    && !visited.contains(&t))
                .then_some(t)
            });
            let Some(n) = next else { break };
            sec = n;
            visited.push(n);
            height += step;
        }
    }
    any
}

fn find_next_lowest_floor(w: &World, sec: SectorId, height: Fixed) -> Fixed {
    w.level
        .lines_of(sec)
        .iter()
        .filter_map(|&l| crate::spec::next_sector(&w.level, l, sec))
        .map(|o| w.level.sector(o).floor_height)
        .filter(|&h| h < height)
        .max()
        .unwrap_or(height)
}

fn next_highest_floor(w: &World, sec: SectorId, height: Fixed) -> Fixed {
    w.level
        .lines_of(sec)
        .iter()
        .filter_map(|&l| crate::spec::next_sector(&w.level, l, sec))
        .map(|o| w.level.sector(o).floor_height)
        .filter(|&h| h > height)
        .min()
        .unwrap_or(height)
}

fn next_highest_ceiling(w: &World, sec: SectorId, height: Fixed) -> Fixed {
    w.level
        .lines_of(sec)
        .iter()
        .filter_map(|&l| crate::spec::next_sector(&w.level, l, sec))
        .map(|o| w.level.sector(o).ceiling_height)
        .filter(|&h| h > height)
        .min()
        .unwrap_or(height)
}

fn lowest_ceiling_surrounding_any(w: &World, sec: SectorId) -> Fixed {
    w.level
        .lines_of(sec)
        .iter()
        .filter_map(|&l| crate::spec::next_sector(&w.level, l, sec))
        .map(|o| w.level.sector(o).ceiling_height)
        .min()
        .unwrap_or(w.level.sector(sec).ceiling_height)
}

/// Height of the shortest lower texture around a sector (fixed).
fn shortest_lower_texture(w: &World, sec: SectorId) -> i32 {
    let mut min = i32::MAX;
    for &l in w.level.lines_of(sec) {
        let line = w.level.line(l);
        if line.flags & ml::TWOSIDED == 0 {
            continue;
        }
        for side in line.sidenum.iter().flatten() {
            let t = w.level.side(*side).bottom_texture.0 as usize;
            if t > 0 {
                if let Some(&h) = w.content.texture_heights.get(t) {
                    min = min.min(h);
                }
            }
        }
    }
    if min == i32::MAX {
        0
    } else {
        min
    }
}

// ---- lights ---------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum HexenLightKind {
    Fade { from: i32, to: i32, tics: i32, t: i32 },
    Glow { upper: i32, lower: i32, tics: i32, t: i32, up: bool },
    Flicker { upper: i32, lower: i32, count: i32 },
    Strobe { upper: i32, lower: i32, utics: i32, ltics: i32, count: i32, bright: bool },
}

/// A Hexen light effect on one sector.
#[derive(Clone, Debug)]
pub struct HexenLight {
    pub sector: SectorId,
    pub kind: HexenLightKind,
}

fn change_lights(w: &mut World, tag: i32, f: impl Fn(i32) -> i32) -> bool {
    for s in sectors_by_tag(w, tag) {
        let l = f(w.level.sector(s).light_level as i32).clamp(0, 255);
        w.level.sector_mut(s).light_level = l as i16;
    }
    true
}

fn start_light(w: &mut World, tag: i32, kind: impl Fn(i32) -> HexenLightKind) -> bool {
    for s in sectors_by_tag(w, tag) {
        stop_sector_lights(w, s);
        let k = kind(w.level.sector(s).light_level as i32);
        add_special(w, SpecialThinker::HexenLight(HexenLight { sector: s, kind: k }));
    }
    true
}

fn stop_sector_lights(w: &mut World, s: SectorId) {
    let ids: Vec<ThinkerId> = w
        .thinkers
        .iter()
        .filter(|(_, d, f)| *f == crate::thinker::ThinkFn::Active && matches!(d, crate::thinker::ThinkerData::Special(SpecialThinker::HexenLight(l)) if l.sector == s))
        .map(|(id, _, _)| id)
        .collect();
    for id in ids {
        w.thinkers.remove(id);
    }
}

fn stop_lights(w: &mut World, tag: i32) -> bool {
    for s in sectors_by_tag(w, tag) {
        stop_sector_lights(w, s);
    }
    true
}

/// One tic of a Hexen light effect.
pub fn t_hexen_light(w: &mut World, id: ThinkerId) {
    let Some(SpecialThinker::HexenLight(l)) = special(w, id) else { return };
    let mut l = l.clone();
    let sec = l.sector;
    let mut done = false;
    let level = match &mut l.kind {
        HexenLightKind::Fade { from, to, tics, t } => {
            *t += 1;
            if *t >= *tics {
                done = true;
            }
            *from + (*to - *from) * (*t).min(*tics) / *tics
        }
        HexenLightKind::Glow { upper, lower, tics, t, up } => {
            *t += 1;
            if *t >= *tics {
                *t = 0;
                *up = !*up;
            }
            let (a, b) = if *up { (*lower, *upper) } else { (*upper, *lower) };
            a + (b - a) * *t / *tics
        }
        HexenLightKind::Flicker { upper, lower, count } => {
            *count -= 1;
            let cur = w.level.sector(sec).light_level as i32;
            if *count <= 0 {
                let r = script_random(w);
                if cur == *upper {
                    *count = (r & 7) + 1;
                    *lower
                } else {
                    *count = (r & 31) + 1;
                    *upper
                }
            } else {
                cur
            }
        }
        HexenLightKind::Strobe { upper, lower, utics, ltics, count, bright } => {
            *count -= 1;
            if *count <= 0 {
                *bright = !*bright;
                *count = if *bright { *utics } else { *ltics };
            }
            if *bright {
                *upper
            } else {
                *lower
            }
        }
    };
    w.level.sector_mut(sec).light_level = level.clamp(0, 255) as i16;
    if done {
        w.thinkers.remove(id);
    } else if let Some(SpecialThinker::HexenLight(slot)) = special_mut(w, id) {
        *slot = l;
    }
}

// ---- things ---------------------------------------------------------------------

fn damage(w: &mut World, things: Vec<MobjId>, amount: i32) -> bool {
    let amount = if amount == 0 { 10000 } else { amount };
    for t in &things {
        if w.try_mo(Some(*t)).is_some_and(|m| m.health > 0) {
            crate::inter::damage_mobj(w, *t, None, None, amount);
        }
    }
    !things.is_empty()
}

/// Destination markers for `Teleport` by TID.
fn is_teleport_dest(w: &World, t: MobjType) -> bool {
    t == MT_TELEPORTMAN || matches!(w.info.class_name(t), "teleportdest2" | "teleportdest3" | "mapspot" | "mapspotgravity")
}

fn teleport(w: &mut World, ctx: &SpecialCtx, tid: i32, tag: i32, fog: bool, keep_angle: bool) -> bool {
    let Some(thing) = ctx.activator else { return false };
    // Not from the back of a line (so you can walk off a teleporter).
    if ctx.line.is_some() && ctx.side == 1 {
        return false;
    }
    if w.mo(thing).flags & MF_MISSILE != 0 {
        return false;
    }
    let in_tagged = |w: &World, m: MobjId| tag == 0 || w.level.sector(w.level.subsectors[w.mo(m).subsector.idx()].sector).tag as i32 == tag;
    let dest = if tid != 0 {
        mobjs_by_tid(w, tid).into_iter().find(|&m| is_teleport_dest(w, w.mo(m).mobj_type) && in_tagged(w, m))
    } else {
        w.thinkers.mobjs().map(|(id, _)| id).find(|&m| w.mo(m).mobj_type == MT_TELEPORTMAN && tag != 0 && in_tagged(w, m))
    };
    let Some(dest) = dest else { return false };
    let (x, y, an) = (w.mo(dest).x, w.mo(dest).y, w.mo(dest).angle);
    teleport_thing(w, thing, x, y, an, fog, keep_angle)
}

/// Move a thing to (x, y) on the floor there, as EV_Teleport does.
pub fn teleport_thing(w: &mut World, thing: MobjId, x: Fixed, y: Fixed, angle: Angle, fog: bool, keep_angle: bool) -> bool {
    let (oldx, oldy, oldz) = {
        let m = w.mo(thing);
        (m.x, m.y, m.z)
    };
    if !crate::map::teleport_move(w, thing, x, y) {
        return false;
    }
    let fz = w.mo(thing).floorz;
    w.mo_mut(thing).z = fz;
    if let Some(p) = w.mo(thing).player {
        w.players[p].viewz = fz + w.players[p].viewheight;
    }
    if fog {
        let f = crate::mobj::spawn_mobj(w, oldx, oldy, oldz, MT_TFOG);
        w.start_sound(SoundSource::Mobj(f), SFX_TELEPT);
        let an = angle.fine();
        let f = crate::mobj::spawn_mobj(w, x + doom_core::tables::finecosine(an) * 20, y + doom_core::tables::finesine(an) * 20, fz, MT_TFOG);
        w.start_sound(SoundSource::Mobj(f), SFX_TELEPT);
    }
    let is_player = w.mo(thing).player.is_some();
    let m = w.mo_mut(thing);
    if is_player && fog {
        m.reactiontime = 18;
    }
    if !keep_angle {
        m.angle = angle;
    }
    (m.momx, m.momy, m.momz) = (Fixed(0), Fixed(0), Fixed(0));
    true
}

/// Spawn an actor at a point (with fog), as scripts do. Returns the new
/// actor.
pub fn spawn_at(w: &mut World, t: MobjType, x: Fixed, y: Fixed, z: Fixed, angle: Angle, tid: i32, fog: bool) -> Option<MobjId> {
    let id = crate::mobj::spawn_mobj(w, x, y, z, t);
    let m = w.mo_mut(id);
    m.angle = angle;
    m.tid = tid;
    if w.mo(id).flags & MF_COUNTKILL != 0 {
        w.totalkills += 1;
    }
    if fog {
        let f = crate::mobj::spawn_mobj(w, x, y, z, MT_TFOG);
        w.start_sound(SoundSource::Mobj(f), SFX_TELEPT);
    }
    Some(id)
}

/// ZDoom spawn numbers (`Thing_Spawn`, `ThingCount`) for Doom's actors.
const SPAWN_IDS: &[(i32, &str)] = &[
    (1, "shotgunguy"),
    (2, "chaingunguy"),
    (3, "baronofhell"),
    (4, "zombieman"),
    (5, "doomimp"),
    (6, "arachnotron"),
    (7, "spidermastermind"),
    (8, "demon"),
    (9, "spectre"),
    (10, "doomimpball"),
    (11, "clip"),
    (12, "shell"),
    (19, "cacodemon"),
    (20, "revenant"),
    (22, "armorbonus"),
    (23, "stimpack"),
    (24, "medikit"),
    (25, "soulsphere"),
    (27, "shotgun"),
    (28, "chaingun"),
    (29, "rocketlauncher"),
    (30, "plasmarifle"),
    (31, "bfg9000"),
    (32, "chainsaw"),
    (33, "supershotgun"),
    (51, "plasmaball"),
    (53, "revenanttracer"),
    (68, "greenarmor"),
    (69, "bluearmor"),
    (75, "cell"),
    (110, "lostsoul"),
    (111, "archvile"),
    (112, "fatso"),
    (113, "hellknight"),
    (114, "cyberdemon"),
    (115, "painelemental"),
    (116, "wolfensteinss"),
    (125, "explosivebarrel"),
    (126, "cacodemonball"),
    (127, "rocket"),
    (128, "bfgball"),
    (129, "arachnotronplasma"),
    (132, "megasphere"),
    (133, "invulnerabilitysphere"),
    (134, "berserk"),
    (135, "blursphere"),
    (136, "radsuit"),
    (137, "allmap"),
    (138, "infrared"),
    (139, "clipbox"),
    (140, "rocketammo"),
    (141, "rocketbox"),
    (142, "cellpack"),
    (143, "shellbox"),
    (144, "backpack"),
    (152, "healthbonus"),
    (153, "fatshot"),
    (154, "baronball"),
];

pub fn spawn_id_type(w: &World, id: i32) -> Option<MobjType> {
    SPAWN_IDS.iter().find(|(n, _)| *n == id).and_then(|(_, name)| w.info.type_by_name(name))
}

/// The spawn number of a type, if it has one.
pub fn type_spawn_id(w: &World, t: MobjType) -> Option<i32> {
    let name = w.info.class_name(t);
    SPAWN_IDS.iter().find(|(_, n)| *n == name).map(|(id, _)| *id)
}

/// Line specials that configure the map rather than act (run once at level
/// setup): `TranslucentLine` and friends. Returns whether `action` was one.
pub fn is_static_special(action: &LineAction) -> bool {
    matches!(action.special, 9 | 50 | 121 | 160 | 181 | 190 | 208 | 209 | 210 | 211)
}

/// Special names for diagnostics.
pub fn special_name(s: i32) -> &'static str {
    match s {
        1..=8 | 59 | 86..=93 => "Polyobj_* (polyobjects)",
        14 => "Door_Animated",
        17 => "Thing_Raise",
        29 | 30 | 94 => "Pillar_*",
        38 | 138 => "Ceiling_Waggle / Floor_Waggle",
        46 => "Floor_CrushStop",
        57 | 107 | 156 => "portals (Sector_SetPortal / Line_SetPortal)",
        76..=78 => "TeleportOther / TeleportGroup / TeleportInSector",
        95 | 96 | 251 => "FloorAndCeiling_*",
        100..=103 | 221..=225 => "scrollers (Scroll_*)",
        106 => "Door_WaitClose",
        109 => "Light_ForceLightning",
        118 => "Plane_Copy",
        120 => "Radius_Quake",
        130 | 131 => "Thing_Activate / Thing_Deactivate",
        134 | 136 | 175 | 178 => "Thing_Projectile*",
        140 => "Sector_ChangeSound",
        177 => "Thing_Hate",
        212 | 213 => "Sector_SetColor / Sector_SetFade",
        215 => "Teleport_Line",
        216 => "Sector_SetGravity",
        218..=220 => "Sector_SetWind / Friction / Current",
        237 => "ChangeCamera",
        245..=247 => "Elevator_*",
        250 => "Floor_Donut",
        _ => "line special (other)",
    }
}
