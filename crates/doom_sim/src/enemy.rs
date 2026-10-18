//! Monster AI and mobj actions (p_enemy.c).

use doom_content::game::{GameMode, GameVersion};
use doom_core::angle::{ANG180, ANG270, ANG90};
use doom_core::fixed::fixed_mul;
use doom_core::geom::{aprox_distance, point_to_angle2};
use doom_core::tables::{finecosine, finesine};
use doom_core::{Angle, Fixed, FRACUNIT};

use crate::geometry::line_opening;
use crate::ids::*;
use crate::info::*;
use crate::level::{MAPBLOCKSHIFT, MAXRADIUS};
use crate::map::{check_position, line_attack, radius_attack, teleport_move, try_move, MELEERANGE, MISSILERANGE};
use crate::maputl::{block_things_iterator, set_thing_position, unset_thing_position};
use crate::mobj::flags::*;
use crate::mobj::{remove_mobj, set_mobj_state, spawn_missile, spawn_mobj, spawn_puff, FLOATSPEED};
use crate::player::MAXPLAYERS;
use crate::sight::check_sight;
use crate::spec::use_special_line;
use crate::specials::doors::{ev_do_door_tag, DoorKind};
use crate::specials::floors::{ev_do_floor_tag, FloorKind};
use crate::world::{GameExit, Skill, SoundSource, World};
use doom_content::map::ml;

// dirtype_t
pub const DI_EAST: i32 = 0;
pub const DI_NORTHEAST: i32 = 1;
pub const DI_NORTH: i32 = 2;
pub const DI_NORTHWEST: i32 = 3;
pub const DI_WEST: i32 = 4;
pub const DI_SOUTHWEST: i32 = 5;
pub const DI_SOUTH: i32 = 6;
pub const DI_SOUTHEAST: i32 = 7;
pub const DI_NODIR: i32 = 8;

const OPPOSITE: [i32; 9] = [DI_WEST, DI_SOUTHWEST, DI_SOUTH, DI_SOUTHEAST, DI_EAST, DI_NORTHEAST, DI_NORTH, DI_NORTHWEST, DI_NODIR];
const DIAGS: [i32; 4] = [DI_NORTHWEST, DI_NORTHEAST, DI_SOUTHWEST, DI_SOUTHEAST];
const XSPEED: [i32; 8] = [FRACUNIT, 47000, 0, -47000, -FRACUNIT, -47000, 0, 47000];
const YSPEED: [i32; 8] = [0, 47000, FRACUNIT, 47000, 0, -47000, -FRACUNIT, -47000];

fn info(w: &World, mo: MobjId) -> MobjInfo {
    *w.info.mobj(w.mo(mo).mobj_type)
}

fn snd(w: &mut World, mo: MobjId, sfx: SfxNum) {
    w.start_sound(SoundSource::Mobj(mo), sfx);
}

// ---------------------------------------------------------------------------
// Sound propagation
// ---------------------------------------------------------------------------

/// `P_RecursiveSound`: called by `P_NoiseAlert`. Recursively traverse
/// adjacent sectors; sound blocking lines cut off traversal.
fn recursive_sound(w: &mut World, sec: SectorId, soundblocks: i32, soundtarget: Option<MobjId>) {
    let s = sec.idx();
    // Wake up all monsters in this sector.
    if w.level.valid.sectors[s] == w.level.valid.current && w.level.sectors[s].sound_traversed <= soundblocks + 1 {
        return; // already flooded
    }
    w.level.valid.sectors[s] = w.level.valid.current;
    w.level.sectors[s].sound_traversed = soundblocks + 1;
    w.level.sectors[s].sound_target = soundtarget;

    let lines: Vec<LineId> = w.level.lines_of(sec).to_vec();
    for l in lines {
        let check = w.level.line(l).clone();
        if check.flags & ml::TWOSIDED == 0 {
            continue;
        }
        let op = line_opening(&w.level, &check, w.tm.opening);
        w.tm.opening = op;
        if op.range.0 <= 0 {
            continue; // closed door
        }
        let s0 = w.level.side(check.sidenum[0].unwrap()).sector;
        let other = if s0 == sec { w.level.side(check.sidenum[1].unwrap()).sector } else { s0 };
        if check.flags & ml::SOUNDBLOCK != 0 {
            if soundblocks == 0 {
                recursive_sound(w, other, 1, soundtarget);
            }
        } else {
            recursive_sound(w, other, soundblocks, soundtarget);
        }
    }
}

/// `P_NoiseAlert`: if a monster yells at a player, it will alert other
/// monsters to the player.
pub fn noise_alert(w: &mut World, target: MobjId, emitter: MobjId) {
    w.level.valid.bump();
    let sec = w.level.subsectors[w.mo(emitter).subsector.idx()].sector;
    recursive_sound(w, sec, 0, Some(target));
}

// ---------------------------------------------------------------------------
// Movement
// ---------------------------------------------------------------------------

/// `P_CheckMeleeRange`.
pub(crate) fn check_melee_range(w: &mut World, actor: MobjId) -> bool {
    let Some(target) = w.mo(actor).target else { return false };
    let Some(pl) = w.try_mo(Some(target)).cloned() else { return false };
    let a = w.mo(actor);
    let dist = aprox_distance(pl.x - a.x, pl.y - a.y);
    let range = if w.params.version < GameVersion::Doom1_5 {
        MELEERANGE
    } else {
        MELEERANGE - Fixed(20 * FRACUNIT) + w.info.mobj(pl.mobj_type).radius
    };
    if dist >= range {
        return false;
    }
    check_sight(w, actor, target)
}

/// `P_CheckMissileRange`.
fn check_missile_range(w: &mut World, actor: MobjId) -> bool {
    let target = w.mo(actor).target.unwrap();
    if !check_sight(w, actor, target) {
        return false;
    }
    if w.mo(actor).flags & MF_JUSTHIT != 0 {
        // The target just hit the enemy, so fight back!
        w.mo_mut(actor).flags &= !MF_JUSTHIT;
        return true;
    }
    if w.mo(actor).reactiontime != 0 {
        return false; // do not attack yet
    }
    // OPTIMIZE: get this from a global checksight.
    let (a, t) = (w.mo(actor).clone(), w.mo(target).clone());
    let mut dist = aprox_distance(a.x - t.x, a.y - t.y) - Fixed(64 * FRACUNIT);
    if w.info.mobj(a.mobj_type).meleestate == S_NULL {
        dist -= Fixed(128 * FRACUNIT); // no melee attack, so fire more
    }
    let mut dist = dist.0 >> 16;
    if a.mobj_type == MT_VILE && dist > 14 * 64 {
        return false; // too far away
    }
    if a.mobj_type == MT_UNDEAD {
        if dist < 196 {
            return false; // close for fist attack
        }
        dist >>= 1;
    }
    if a.mobj_type == MT_CYBORG || a.mobj_type == MT_SPIDER || a.mobj_type == MT_SKULL {
        dist >>= 1;
    }
    if dist > 200 {
        dist = 200;
    }
    if a.mobj_type == MT_CYBORG && dist > 160 {
        dist = 160;
    }
    w.p_random() >= dist
}

/// `P_Move`: move in the current direction; returns false if the move is
/// blocked.
fn p_move(w: &mut World, actor: MobjId) -> bool {
    let a = w.mo(actor).clone();
    if a.movedir == DI_NODIR {
        return false;
    }
    if !(0..8).contains(&a.movedir) {
        panic!("Weird actor->movedir!");
    }
    let speed = w.info.mobj(a.mobj_type).speed;
    let tryx = a.x + Fixed(speed.wrapping_mul(XSPEED[a.movedir as usize]));
    let tryy = a.y + Fixed(speed.wrapping_mul(YSPEED[a.movedir as usize]));
    if !try_move(w, actor, tryx, tryy) {
        // Open any specials.
        if a.flags & MF_FLOAT != 0 && w.tm.floatok {
            // Must adjust height.
            let tmfloorz = w.tm.tmfloorz;
            let m = w.mo_mut(actor);
            if m.z < tmfloorz {
                m.z += FLOATSPEED;
            } else {
                m.z -= FLOATSPEED;
            }
            m.flags |= MF_INFLOAT;
            return true;
        }
        if w.tm.spechit.is_empty() {
            return false;
        }
        w.mo_mut(actor).movedir = DI_NODIR;
        let mut good = false;
        while let Some(ld) = w.tm.spechit.pop() {
            // If the special is not a door that can be opened, return false.
            if use_special_line(w, actor, ld, 0) {
                good = true;
            }
        }
        return good;
    }
    let m = w.mo_mut(actor);
    m.flags &= !MF_INFLOAT;
    if m.flags & MF_FLOAT == 0 {
        m.z = m.floorz;
    }
    true
}

/// `P_TryWalk`: attempts to move actor in its current (ob->moveangle)
/// direction. If blocked by either a wall or an actor returns false; if
/// move is either clear or blocked only by a door, returns true and sets.
/// If a door is in the way, an OpenDoor call is made to start it opening.
pub(crate) fn try_walk(w: &mut World, actor: MobjId) -> bool {
    if !p_move(w, actor) {
        return false;
    }
    let r = w.p_random() & 15;
    w.mo_mut(actor).movecount = r;
    true
}

/// `P_NewChaseDir`.
pub(crate) fn new_chase_dir(w: &mut World, actor: MobjId) {
    let Some(target) = w.try_mo(w.mo(actor).target).cloned() else {
        // Vanilla I_Errors here (its own actors never get here without a
        // target). ZDoom actors do (A_Wander, chasing after the target
        // died): walk off in a random direction that is open. The script
        // RNG keeps vanilla's P_Random sequence untouched.
        let start = (crate::hexen::script_random(w) & 7) as i32;
        for k in 0..8 {
            w.mo_mut(actor).movedir = (start + k) & 7;
            if try_walk(w, actor) {
                return;
            }
        }
        w.mo_mut(actor).movedir = DI_NODIR;
        return;
    };
    let a = w.mo(actor).clone();
    let olddir = a.movedir;
    let turnaround = OPPOSITE[olddir as usize];
    let deltax = target.x - a.x;
    let deltay = target.y - a.y;
    let mut d = [0i32; 3];
    d[1] = if deltax.0 > 10 * FRACUNIT {
        DI_EAST
    } else if deltax.0 < -10 * FRACUNIT {
        DI_WEST
    } else {
        DI_NODIR
    };
    d[2] = if deltay.0 < -10 * FRACUNIT {
        DI_SOUTH
    } else if deltay.0 > 10 * FRACUNIT {
        DI_NORTH
    } else {
        DI_NODIR
    };

    // Try direct route.
    if d[1] != DI_NODIR && d[2] != DI_NODIR {
        let dir = DIAGS[(((deltay.0 < 0) as usize) << 1) + (deltax.0 > 0) as usize];
        w.mo_mut(actor).movedir = dir;
        if dir != turnaround && try_walk(w, actor) {
            return;
        }
    }

    // Try other directions.
    if w.p_random() > 200 || deltay.abs() > deltax.abs() {
        d.swap(1, 2);
    }
    if d[1] == turnaround {
        d[1] = DI_NODIR;
    }
    if d[2] == turnaround {
        d[2] = DI_NODIR;
    }
    if d[1] != DI_NODIR {
        w.mo_mut(actor).movedir = d[1];
        if try_walk(w, actor) {
            return; // either moved forward or attacked
        }
    }
    if d[2] != DI_NODIR {
        w.mo_mut(actor).movedir = d[2];
        if try_walk(w, actor) {
            return;
        }
    }

    // There is no direct path to the player, so pick another direction.
    if olddir != DI_NODIR {
        w.mo_mut(actor).movedir = olddir;
        if try_walk(w, actor) {
            return;
        }
    }

    // Randomly determine direction of search.
    if w.p_random() & 1 != 0 {
        for tdir in DI_EAST..=DI_SOUTHEAST {
            if tdir != turnaround {
                w.mo_mut(actor).movedir = tdir;
                if try_walk(w, actor) {
                    return;
                }
            }
        }
    } else {
        for tdir in (DI_EAST..=DI_SOUTHEAST).rev() {
            if tdir != turnaround {
                w.mo_mut(actor).movedir = tdir;
                if try_walk(w, actor) {
                    return;
                }
            }
        }
    }

    if turnaround != DI_NODIR {
        w.mo_mut(actor).movedir = turnaround;
        if try_walk(w, actor) {
            return;
        }
    }
    w.mo_mut(actor).movedir = DI_NODIR; // can not move
}

/// `P_LookForPlayers`: if `allaround` is false, only look 180 degrees in
/// front. Returns true if a player is targeted.
pub fn look_for_players(w: &mut World, actor: MobjId, allaround: bool) -> bool {
    let mut c = 0;
    let stop = (w.mo(actor).lastlook - 1) & 3;
    loop {
        let ll = w.mo(actor).lastlook;
        'next: {
            if !w.playeringame[ll as usize] {
                break 'next;
            }
            // Done looking.
            let done = c == 2 || ll == stop;
            c += 1;
            if done {
                return false;
            }
            let p = &w.players[ll as usize];
            if p.health <= 0 {
                break 'next; // dead
            }
            let Some(pmo) = p.mo else { break 'next };
            if p.cheats & crate::player::cheat::NOTARGET != 0 {
                break 'next; // `notarget`: monsters ignore this player
            }
            if !check_sight(w, actor, pmo) {
                break 'next; // out of sight
            }
            if !allaround {
                let (a, pm) = (w.mo(actor).clone(), w.mo(pmo).clone());
                let an = point_to_angle2(a.x, a.y, pm.x, pm.y) - a.angle;
                if an.0 > ANG90 && an.0 < ANG270 {
                    let dist = aprox_distance(pm.x - a.x, pm.y - a.y);
                    // If real close, react anyway.
                    if dist > MELEERANGE {
                        break 'next; // behind back
                    }
                }
            }
            w.mo_mut(actor).target = Some(pmo);
            return true;
        }
        let m = w.mo_mut(actor);
        m.lastlook = (m.lastlook + 1) & 3;
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// `A_KeenDie`: DOOM II special, map 32. Uses special tag 666.
fn a_keen_die(w: &mut World, mo: MobjId) {
    a_fall(w, mo);
    // Scan the remaining thinkers to see if all Keens are dead.
    let t = w.mo(mo).mobj_type;
    let alive = w.thinkers.mobjs().any(|(id, m2)| id != mo && m2.mobj_type == t && m2.health > 0);
    if alive {
        return; // other Keen not dead
    }
    ev_do_door_tag(w, 666, DoorKind::Open);
}

/// A player with the `notarget` cheat: monsters do not notice them.
fn is_notarget(w: &World, mo: Option<MobjId>) -> bool {
    let player = w.try_mo(mo).and_then(|m| m.player);
    player.is_some_and(|p| w.players[p].cheats & crate::player::cheat::NOTARGET != 0)
}

/// `A_Look`: stay in state until a player is sighted.
fn a_look(w: &mut World, actor: MobjId) {
    w.mo_mut(actor).threshold = 0; // any shot will wake up
    let sec = w.level.subsectors[w.mo(actor).subsector.idx()].sector;
    let targ = w.level.sector(sec).sound_target;
    let mut seen = false;
    if let Some(t) = w.try_mo(targ).map(|m| m.flags) {
        if t & MF_SHOOTABLE != 0 && !is_notarget(w, targ) {
            w.mo_mut(actor).target = targ;
            if w.mo(actor).flags & MF_AMBUSH != 0 {
                if check_sight(w, actor, targ.unwrap()) {
                    seen = true;
                }
            } else {
                seen = true;
            }
        }
    }
    if !seen && !look_for_players(w, actor, false) {
        return;
    }

    // Go into chase state. `seeyou:`
    let inf = info(w, actor);
    if inf.seesound != SFX_NONE {
        let sound = match inf.seesound {
            SFX_POSIT1 | SFX_POSIT2 | SFX_POSIT3 => SfxNum(SFX_POSIT1.0 + (w.p_random() % 3) as u16),
            SFX_BGSIT1 | SFX_BGSIT2 => SfxNum(SFX_BGSIT1.0 + (w.p_random() % 2) as u16),
            s => s,
        };
        let t = w.mo(actor).mobj_type;
        if t == MT_SPIDER || t == MT_CYBORG {
            // Full volume.
            w.start_sound(SoundSource::None, sound);
        } else {
            snd(w, actor, sound);
        }
    }
    set_mobj_state(w, actor, inf.seestate);
}

/// `A_Chase`: actor has a melee attack, so it tries to close as fast as
/// possible.
fn a_chase(w: &mut World, actor: MobjId) {
    {
        let m = w.mo_mut(actor);
        if m.reactiontime != 0 {
            m.reactiontime -= 1;
        }
    }
    // Modify target threshold.
    if w.mo(actor).threshold != 0 {
        let target_dead = w.try_mo(w.mo(actor).target).is_none_or(|t| t.health <= 0);
        if w.params.version > GameVersion::Doom1_2 && target_dead {
            w.mo_mut(actor).threshold = 0;
        } else {
            w.mo_mut(actor).threshold -= 1;
        }
    }

    // Turn towards movement direction if not there yet.
    {
        let m = w.mo_mut(actor);
        if m.movedir < 8 {
            m.angle = Angle(m.angle.0 & (7u32 << 29));
            let delta = m.angle.0.wrapping_sub((m.movedir as u32) << 29) as i32;
            if delta > 0 {
                m.angle -= Angle(ANG90 / 2);
            } else if delta < 0 {
                m.angle += Angle(ANG90 / 2);
            }
        }
    }

    let target = w.mo(actor).target;
    if w.try_mo(target).is_none_or(|t| t.flags & MF_SHOOTABLE == 0) {
        // Look for a new target.
        if look_for_players(w, actor, true) {
            return; // got a new target
        }
        let spawn = info(w, actor).spawnstate;
        set_mobj_state(w, actor, spawn);
        return;
    }

    // Do not attack twice in a row.
    if w.mo(actor).flags & MF_JUSTATTACKED != 0 {
        w.mo_mut(actor).flags &= !MF_JUSTATTACKED;
        if w.params.skill != Skill::Nightmare && !w.params.fastparm {
            new_chase_dir(w, actor);
        }
        return;
    }

    let inf = info(w, actor);
    // Check for melee attack.
    if inf.meleestate != S_NULL && check_melee_range(w, actor) {
        if inf.attacksound != SFX_NONE {
            snd(w, actor, inf.attacksound);
        }
        set_mobj_state(w, actor, inf.meleestate);
        return;
    }

    // Check for missile attack.
    'nomissile: {
        if inf.missilestate != S_NULL {
            if w.params.skill < Skill::Nightmare && !w.params.fastparm && w.mo(actor).movecount != 0 {
                break 'nomissile;
            }
            if !check_missile_range(w, actor) {
                break 'nomissile;
            }
            set_mobj_state(w, actor, inf.missilestate);
            w.mo_mut(actor).flags |= MF_JUSTATTACKED;
            return;
        }
    }

    // Possibly choose another target.
    if w.params.netgame && w.mo(actor).threshold == 0 {
        let t = w.mo(actor).target.unwrap();
        if !check_sight(w, actor, t) && look_for_players(w, actor, true) {
            return; // got a new target
        }
    }

    // Chase towards player.
    w.mo_mut(actor).movecount -= 1;
    if w.mo(actor).movecount < 0 || !p_move(w, actor) {
        new_chase_dir(w, actor);
    }

    // Make active sound.
    if inf.activesound != SFX_NONE && w.p_random() < 3 {
        snd(w, actor, inf.activesound);
    }
}

/// `A_FaceTarget`.
fn a_face_target(w: &mut World, actor: MobjId) {
    let Some(t) = w.try_mo(w.mo(actor).target).map(|t| (t.x, t.y, t.flags)) else { return };
    let (x, y) = (w.mo(actor).x, w.mo(actor).y);
    w.mo_mut(actor).flags &= !MF_AMBUSH;
    let mut angle = point_to_angle2(x, y, t.0, t.1);
    if t.2 & MF_SHADOW != 0 {
        angle += Angle((w.p_sub_random() << 21) as u32);
    }
    w.mo_mut(actor).angle = angle;
}

fn has_target(w: &World, actor: MobjId) -> bool {
    w.try_mo(w.mo(actor).target).is_some()
}

/// `A_PosAttack`.
fn a_pos_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    let mut angle = w.mo(actor).angle;
    let slope = crate::map::aim_line_attack(w, Some(actor), angle, MISSILERANGE);
    snd(w, actor, SFX_PISTOL);
    angle += Angle((w.p_sub_random() << 20) as u32);
    let damage = ((w.p_random() % 5) + 1) * 3;
    line_attack(w, actor, angle, MISSILERANGE, slope, damage);
}

/// `A_SPosAttack`.
fn a_spos_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    snd(w, actor, SFX_SHOTGN);
    a_face_target(w, actor);
    let bangle = w.mo(actor).angle;
    let slope = crate::map::aim_line_attack(w, Some(actor), bangle, MISSILERANGE);
    for _ in 0..3 {
        let angle = bangle + Angle((w.p_sub_random() << 20) as u32);
        let damage = ((w.p_random() % 5) + 1) * 3;
        line_attack(w, actor, angle, MISSILERANGE, slope, damage);
    }
}

/// `A_CPosAttack`.
fn a_cpos_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    snd(w, actor, SFX_SHOTGN);
    a_face_target(w, actor);
    let bangle = w.mo(actor).angle;
    let slope = crate::map::aim_line_attack(w, Some(actor), bangle, MISSILERANGE);
    let angle = bangle + Angle((w.p_sub_random() << 20) as u32);
    let damage = ((w.p_random() % 5) + 1) * 3;
    line_attack(w, actor, angle, MISSILERANGE, slope, damage);
}

/// `A_CPosRefire` / `A_SpidRefire`: keep firing unless target got out of
/// sight.
fn a_refire_monster(w: &mut World, actor: MobjId, chance: i32) {
    a_face_target(w, actor);
    if w.p_random() < chance {
        return;
    }
    let target = w.mo(actor).target;
    let lost = match w.try_mo(target).map(|t| t.health) {
        None => true,
        Some(h) if h <= 0 => true,
        Some(_) => !check_sight(w, actor, target.unwrap()),
    };
    if lost {
        let see = info(w, actor).seestate;
        set_mobj_state(w, actor, see);
    }
}

fn a_missile_at_target(w: &mut World, actor: MobjId, t: MobjType) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    let target = w.mo(actor).target;
    spawn_missile(w, actor, target, t);
}

/// `A_TroopAttack`.
fn a_troop_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    if check_melee_range(w, actor) {
        snd(w, actor, SFX_CLAW);
        let damage = (w.p_random() % 8 + 1) * 3;
        let t = w.mo(actor).target.unwrap();
        crate::inter::damage_mobj(w, t, Some(actor), Some(actor), damage);
        return;
    }
    // Launch a missile.
    let target = w.mo(actor).target;
    spawn_missile(w, actor, target, MT_TROOPSHOT);
}

/// `A_SargAttack`.
fn a_sarg_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    if w.params.version >= GameVersion::Doom1_5 && !check_melee_range(w, actor) {
        return;
    }
    let damage = ((w.p_random() % 10) + 1) * 4;
    if w.params.version <= GameVersion::Doom1_2 {
        let angle = w.mo(actor).angle;
        line_attack(w, actor, angle, MELEERANGE, Fixed(0), damage);
    } else {
        let t = w.mo(actor).target.unwrap();
        crate::inter::damage_mobj(w, t, Some(actor), Some(actor), damage);
    }
}

/// `A_HeadAttack`.
fn a_head_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    if check_melee_range(w, actor) {
        let damage = (w.p_random() % 6 + 1) * 10;
        let t = w.mo(actor).target.unwrap();
        crate::inter::damage_mobj(w, t, Some(actor), Some(actor), damage);
        return;
    }
    // Launch a missile.
    let target = w.mo(actor).target;
    spawn_missile(w, actor, target, MT_HEADSHOT);
}

/// `A_BruisAttack`.
fn a_bruis_attack(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    if check_melee_range(w, actor) {
        snd(w, actor, SFX_CLAW);
        let damage = (w.p_random() % 8 + 1) * 10;
        let t = w.mo(actor).target.unwrap();
        crate::inter::damage_mobj(w, t, Some(actor), Some(actor), damage);
        return;
    }
    // Launch a missile.
    let target = w.mo(actor).target;
    spawn_missile(w, actor, target, MT_BRUISERSHOT);
}

/// `A_SkelMissile`.
fn a_skel_missile(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    w.mo_mut(actor).z += Fixed(16 * FRACUNIT); // so missile spawns higher
    let target = w.mo(actor).target;
    let mo = spawn_missile(w, actor, target, MT_TRACER);
    w.mo_mut(actor).z -= Fixed(16 * FRACUNIT); // back to normal
    let m = w.mo_mut(mo);
    m.x += m.momx;
    m.y += m.momy;
    m.tracer = target;
}

const TRACEANGLE: u32 = 0xc000000;

/// `A_Tracer`.
fn a_tracer(w: &mut World, actor: MobjId) {
    if w.gametic & 3 != 0 {
        return;
    }
    // Spawn a puff of smoke behind the rocket.
    let a = w.mo(actor).clone();
    spawn_puff(w, a.x, a.y, a.z);
    let th = spawn_mobj(w, a.x - a.momx, a.y - a.momy, a.z, MT_SMOKE);
    w.mo_mut(th).momz = Fixed::ONE;
    let r = w.p_random() & 3;
    {
        let m = w.mo_mut(th);
        m.tics -= r;
        if m.tics < 1 {
            m.tics = 1;
        }
    }
    // Adjust direction.
    let Some(dest) = w.try_mo(a.tracer).cloned() else { return };
    if dest.health <= 0 {
        return;
    }
    // Change angle.
    let exact = point_to_angle2(a.x, a.y, dest.x, dest.y);
    let mut angle = a.angle;
    if exact != angle {
        if (exact - angle).0 > 0x8000_0000 {
            angle -= Angle(TRACEANGLE);
            if (exact - angle).0 < 0x8000_0000 {
                angle = exact;
            }
        } else {
            angle += Angle(TRACEANGLE);
            if (exact - angle).0 > 0x8000_0000 {
                angle = exact;
            }
        }
    }
    let speed = w.info.mobj(a.mobj_type).speed;
    let an = angle.fine();
    let dist = (aprox_distance(dest.x - a.x, dest.y - a.y).0 / speed).max(1);
    let slope = Fixed((dest.z + Fixed(40 * FRACUNIT) - a.z).0 / dist);
    let m = w.mo_mut(actor);
    m.angle = angle;
    m.momx = fixed_mul(Fixed(speed), finecosine(an));
    m.momy = fixed_mul(Fixed(speed), finesine(an));
    if slope < m.momz {
        m.momz -= Fixed(FRACUNIT / 8);
    } else {
        m.momz += Fixed(FRACUNIT / 8);
    }
}

/// `A_SkelWhoosh`.
fn a_skel_whoosh(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    snd(w, actor, SFX_SKESWG);
}

/// `A_SkelFist`.
fn a_skel_fist(w: &mut World, actor: MobjId) {
    if !has_target(w, actor) {
        return;
    }
    a_face_target(w, actor);
    if check_melee_range(w, actor) {
        let damage = ((w.p_random() % 10) + 1) * 6;
        snd(w, actor, SFX_SKEPCH);
        let t = w.mo(actor).target.unwrap();
        crate::inter::damage_mobj(w, t, Some(actor), Some(actor), damage);
    }
}

/// `PIT_VileCheck`: detect a corpse that could be raised.
fn pit_vile_check(w: &mut World, thing: MobjId, tryx: Fixed, tryy: Fixed) -> bool {
    let t = w.mo(thing).clone();
    if t.flags & MF_CORPSE == 0 {
        return true; // not a monster
    }
    if t.tics != -1 {
        return true; // not lying still yet
    }
    let tinfo = *w.info.mobj(t.mobj_type);
    if tinfo.raisestate == S_NULL {
        return true; // monster doesn't have a raise state
    }
    let maxdist = tinfo.radius + w.info.mobj(MT_VILE).radius;
    if (t.x - tryx).abs() > maxdist || (t.y - tryy).abs() > maxdist {
        return true; // not actually touching
    }
    w.corpsehit = Some(thing);
    {
        let c = w.mo_mut(thing);
        c.momx = Fixed(0);
        c.momy = Fixed(0);
        c.height = c.height << 2;
    }
    let check = check_position(w, thing, t.x, t.y);
    let c = w.mo_mut(thing);
    c.height = c.height >> 2;
    // Doesn't fit here / got one, so stop checking.
    !check
}

/// `A_VileChase`: check for resurrecting a body.
fn a_vile_chase(w: &mut World, actor: MobjId) {
    let a = w.mo(actor).clone();
    if a.movedir != DI_NODIR {
        // Check for corpses to raise.
        let speed = w.info.mobj(a.mobj_type).speed;
        let tryx = a.x + Fixed(speed.wrapping_mul(XSPEED[a.movedir as usize]));
        let tryy = a.y + Fixed(speed.wrapping_mul(YSPEED[a.movedir as usize]));
        let (ox, oy) = (w.level.blockmap.org_x, w.level.blockmap.org_y);
        let r2 = MAXRADIUS * 2;
        let xl = (tryx - ox - r2).0 >> MAPBLOCKSHIFT;
        let xh = (tryx - ox + r2).0 >> MAPBLOCKSHIFT;
        let yl = (tryy - oy - r2).0 >> MAPBLOCKSHIFT;
        let yh = (tryy - oy + r2).0 >> MAPBLOCKSHIFT;
        for bx in xl..=xh {
            for by in yl..=yh {
                // Call PIT_VileCheck to check whether object is a corpse
                // that can be raised.
                if !block_things_iterator(w, bx, by, |w, t| pit_vile_check(w, t, tryx, tryy)) {
                    // Got one!
                    let corpse = w.corpsehit.unwrap();
                    let temp = w.mo(actor).target;
                    w.mo_mut(actor).target = Some(corpse);
                    a_face_target(w, actor);
                    w.mo_mut(actor).target = temp;
                    set_mobj_state(w, actor, S_VILE_HEAL1);
                    snd(w, corpse, SFX_SLOP);
                    let cinfo = info(w, corpse);
                    set_mobj_state(w, corpse, cinfo.raisestate);
                    let c = w.mo_mut(corpse);
                    c.height = c.height << 2;
                    c.flags = cinfo.flags;
                    c.health = cinfo.spawnhealth;
                    c.target = None;
                    return;
                }
            }
        }
    }
    // Return to normal attack.
    a_chase(w, actor);
}

/// `A_Fire`: keep fire in front of player unless out of sight.
fn a_fire(w: &mut World, actor: MobjId) {
    let Some(dest) = w.try_mo(w.mo(actor).tracer).cloned() else { return };
    let dest_id = w.mo(actor).tracer.unwrap();
    // Don't move it if the vile lost sight. Chocolate substitutes a dummy
    // mobj for a NULL target.
    let visible = match w.mo(actor).target.filter(|&t| w.thinkers.exists(t)) {
        Some(t) => check_sight(w, t, dest_id),
        None => {
            w.note_overflow("A_Fire with NULL target (P_SubstNullMobj sight check not emulated)");
            false
        }
    };
    if !visible {
        return;
    }
    let an = dest.angle.fine();
    unset_thing_position(w, actor);
    {
        let m = w.mo_mut(actor);
        m.x = dest.x + fixed_mul(Fixed(24 * FRACUNIT), finecosine(an));
        m.y = dest.y + fixed_mul(Fixed(24 * FRACUNIT), finesine(an));
        m.z = dest.z;
    }
    set_thing_position(w, actor);
}

/// `A_VileTarget`: spawn the hellfire.
fn a_vile_target(w: &mut World, actor: MobjId) {
    let Some(t) = w.try_mo(w.mo(actor).target).cloned() else { return };
    a_face_target(w, actor);
    // Vanilla bug preserved: the fire spawns at (target->x, target->x).
    let fog = spawn_mobj(w, t.x, t.x, t.z, MT_FIRE);
    let target = w.mo(actor).target;
    w.mo_mut(actor).tracer = Some(fog);
    let f = w.mo_mut(fog);
    f.target = Some(actor);
    f.tracer = target;
    a_fire(w, fog);
}

/// `A_VileAttack`.
fn a_vile_attack(w: &mut World, actor: MobjId) {
    let Some(target) = w.mo(actor).target.filter(|&t| w.thinkers.exists(t)) else { return };
    a_face_target(w, actor);
    if !check_sight(w, actor, target) {
        return;
    }
    snd(w, actor, SFX_BAREXP);
    crate::inter::damage_mobj(w, target, Some(actor), Some(actor), 20);
    let mass = info(w, target).mass;
    w.mo_mut(target).momz = Fixed((1000 * FRACUNIT) / mass);
    let an = w.mo(actor).angle.fine();
    let Some(fire) = w.mo(actor).tracer.filter(|&f| w.thinkers.exists(f)) else { return };
    let (tx, ty) = (w.mo(target).x, w.mo(target).y);
    // Move the fire between the vile and the player.
    let f = w.mo_mut(fire);
    f.x = tx - fixed_mul(Fixed(24 * FRACUNIT), finecosine(an));
    f.y = ty - fixed_mul(Fixed(24 * FRACUNIT), finesine(an));
    radius_attack(w, fire, Some(actor), 70);
}

/// Mancubus attack: firing three missiles (bruisers) in three different
/// directions? Doesn't look like it.
const FATSPREAD: u32 = ANG90 / 8;

fn fat_missile(w: &mut World, actor: MobjId, spread: i64) {
    let target = w.mo(actor).target;
    let mo = spawn_missile(w, actor, target, MT_FATSHOT);
    let speed = info(w, mo).speed;
    let m = w.mo_mut(mo);
    m.angle = Angle((m.angle.0 as i64 + spread) as u32);
    let an = m.angle.fine();
    m.momx = fixed_mul(Fixed(speed), finecosine(an));
    m.momy = fixed_mul(Fixed(speed), finesine(an));
}

fn subst_target_missile(w: &mut World, actor: MobjId) {
    let target = w.mo(actor).target;
    spawn_missile(w, actor, target, MT_FATSHOT);
}

/// `A_FatAttack1`.
fn a_fat_attack1(w: &mut World, actor: MobjId) {
    a_face_target(w, actor);
    // Change direction to...
    w.mo_mut(actor).angle += Angle(FATSPREAD);
    subst_target_missile(w, actor);
    fat_missile(w, actor, FATSPREAD as i64);
}

/// `A_FatAttack2`.
fn a_fat_attack2(w: &mut World, actor: MobjId) {
    a_face_target(w, actor);
    // Now here choose opposite deviation.
    w.mo_mut(actor).angle -= Angle(FATSPREAD);
    subst_target_missile(w, actor);
    fat_missile(w, actor, -(FATSPREAD as i64 * 2));
}

/// `A_FatAttack3`.
fn a_fat_attack3(w: &mut World, actor: MobjId) {
    a_face_target(w, actor);
    fat_missile(w, actor, -(FATSPREAD as i64 / 2));
    fat_missile(w, actor, FATSPREAD as i64 / 2);
}

const SKULLSPEED: Fixed = Fixed(20 * FRACUNIT);

/// `A_SkullAttack`: fly at the player like a missile.
fn a_skull_attack(w: &mut World, actor: MobjId) {
    let Some(dest) = w.try_mo(w.mo(actor).target).cloned() else { return };
    w.mo_mut(actor).flags |= MF_SKULLFLY;
    let atk = info(w, actor).attacksound;
    snd(w, actor, atk);
    a_face_target(w, actor);
    let a = w.mo(actor).clone();
    let an = a.angle.fine();
    let dist = (aprox_distance(dest.x - a.x, dest.y - a.y).0 / SKULLSPEED.0).max(1);
    let m = w.mo_mut(actor);
    m.momx = fixed_mul(SKULLSPEED, finecosine(an));
    m.momy = fixed_mul(SKULLSPEED, finesine(an));
    m.momz = Fixed((dest.z + (dest.height >> 1) - a.z).0 / dist);
}

/// `A_PainShootSkull`: spawn a lost soul and launch it at the target.
fn a_pain_shoot_skull(w: &mut World, actor: MobjId, angle: Angle) {
    // Count total number of skulls currently on the level.
    let count = w.thinkers.mobjs().filter(|(_, m)| m.mobj_type == MT_SKULL).count();
    // If there are already 20 skulls on the level, don't spit another one.
    if count > 20 {
        return;
    }
    // Okay, there's place for another one.
    let an = angle.fine();
    let a = w.mo(actor).clone();
    let prestep = Fixed(4 * FRACUNIT + 3 * (info(w, actor).radius.0 + w.info.mobj(MT_SKULL).radius.0) / 2);
    let x = a.x + fixed_mul(prestep, finecosine(an));
    let y = a.y + fixed_mul(prestep, finesine(an));
    let z = a.z + Fixed(8 * FRACUNIT);
    let newmobj = spawn_mobj(w, x, y, z, MT_SKULL);
    // Check for movements.
    let (nx, ny) = (w.mo(newmobj).x, w.mo(newmobj).y);
    if !try_move(w, newmobj, nx, ny) {
        // Kill it immediately.
        crate::inter::damage_mobj(w, newmobj, Some(actor), Some(actor), 10000);
        return;
    }
    w.mo_mut(newmobj).target = a.target;
    a_skull_attack(w, newmobj);
}

/// `A_Scream`.
fn a_scream(w: &mut World, actor: MobjId) {
    let ds = info(w, actor).deathsound;
    let sound = match ds {
        SFX_NONE => return,
        SFX_PODTH1 | SFX_PODTH2 | SFX_PODTH3 => SfxNum(SFX_PODTH1.0 + (w.p_random() % 3) as u16),
        SFX_BGDTH1 | SFX_BGDTH2 => SfxNum(SFX_BGDTH1.0 + (w.p_random() % 2) as u16),
        s => s,
    };
    // Check for bosses.
    let t = w.mo(actor).mobj_type;
    if t == MT_SPIDER || t == MT_CYBORG {
        // Full volume.
        w.start_sound(SoundSource::None, sound);
    } else {
        snd(w, actor, sound);
    }
}

/// `A_Fall`: actor is on ground, it can be walked over.
fn a_fall(w: &mut World, actor: MobjId) {
    w.mo_mut(actor).flags &= !MF_SOLID;
}

/// `CheckBossEnd`: is this the boss for this level?
fn check_boss_end(w: &World, t: MobjType) -> bool {
    let (ep, map) = (w.params.episode, w.params.map);
    if w.params.version < GameVersion::Ultimate {
        if map != 8 {
            return false;
        }
        if t == MT_BRUISER && ep != 1 {
            return false;
        }
        true
    } else {
        // New logic that appeared in Ultimate Doom. Looks like the logic
        // was overhauled while adding in the episode 4 support. Now bosses
        // only trigger on their specific episode.
        match ep {
            1 => map == 8 && t == MT_BRUISER,
            2 => map == 8 && t == MT_CYBORG,
            3 => map == 8 && t == MT_SPIDER,
            4 => (map == 6 && t == MT_CYBORG) || (map == 8 && t == MT_SPIDER),
            _ => map == 8,
        }
    }
}

/// `A_BossDeath`: possibly trigger special effects if on first boss level.
fn a_boss_death(w: &mut World, mo: MobjId) {
    let t = w.mo(mo).mobj_type;
    if w.params.mode == GameMode::Commercial {
        if w.params.map != 7 {
            return;
        }
        if t != MT_FATSO && t != MT_BABY {
            return;
        }
    } else if !check_boss_end(w, t) {
        return;
    }
    // Make sure there is a player alive for victory.
    if !(0..MAXPLAYERS).any(|i| w.playeringame[i] && w.players[i].health > 0) {
        return; // no one left alive, so do not end game
    }
    // Scan the remaining thinkers to see if all bosses are dead.
    if w.thinkers.mobjs().any(|(id, m2)| id != mo && m2.mobj_type == t && m2.health > 0) {
        return; // other boss not dead
    }
    // Victory!
    if w.params.mode == GameMode::Commercial {
        if w.params.map == 7 {
            if t == MT_FATSO {
                ev_do_floor_tag(w, 666, None, FloorKind::LowerFloorToLowest);
                return;
            }
            if t == MT_BABY {
                ev_do_floor_tag(w, 667, None, FloorKind::RaiseToTexture);
                return;
            }
        }
    } else {
        match w.params.episode {
            1 => {
                ev_do_floor_tag(w, 666, None, FloorKind::LowerFloorToLowest);
                return;
            }
            4 => match w.params.map {
                6 => {
                    ev_do_door_tag(w, 666, DoorKind::BlazeOpen);
                    return;
                }
                8 => {
                    ev_do_floor_tag(w, 666, None, FloorKind::LowerFloorToLowest);
                    return;
                }
                _ => {}
            },
            _ => {}
        }
    }
    w.gameexit = Some(GameExit::Normal);
}

/// `A_BrainAwake`.
fn a_brain_awake(w: &mut World) {
    // Find all the target spots.
    w.braintargets = w.thinkers.mobjs().filter(|(_, m)| m.mobj_type == MT_BOSSTARGET).map(|(id, _)| id).collect();
    w.braintargeton = 0;
    if w.braintargets.len() > 32 {
        w.note_overflow("more than 32 boss targets (vanilla overflows braintargets[])");
    }
    w.start_sound(SoundSource::None, SFX_BOSSIT);
}

/// `A_BrainScream`.
fn a_brain_scream(w: &mut World, mo: MobjId) {
    let (mx, my) = (w.mo(mo).x, w.mo(mo).y);
    let mut x = mx - Fixed(196 * FRACUNIT);
    while x < mx + Fixed(320 * FRACUNIT) {
        let y = my - Fixed(320 * FRACUNIT);
        let z = Fixed(128 + w.p_random() * 2 * FRACUNIT);
        let th = spawn_mobj(w, x, y, z, MT_ROCKET);
        w.mo_mut(th).momz = Fixed(w.p_random() * 512);
        set_mobj_state(w, th, S_BRAINEXPLODE1);
        let r = w.p_random() & 7;
        let m = w.mo_mut(th);
        m.tics -= r;
        if m.tics < 1 {
            m.tics = 1;
        }
        x += Fixed(FRACUNIT * 8);
    }
    w.start_sound(SoundSource::None, SFX_BOSDTH);
}

/// `A_BrainExplode`.
fn a_brain_explode(w: &mut World, mo: MobjId) {
    let (mx, my) = (w.mo(mo).x, w.mo(mo).y);
    let x = mx + Fixed(w.p_sub_random() * 2048);
    let y = my;
    let z = Fixed(128 + w.p_random() * 2 * FRACUNIT);
    let th = spawn_mobj(w, x, y, z, MT_ROCKET);
    w.mo_mut(th).momz = Fixed(w.p_random() * 512);
    set_mobj_state(w, th, S_BRAINEXPLODE1);
    let r = w.p_random() & 7;
    let m = w.mo_mut(th);
    m.tics -= r;
    if m.tics < 1 {
        m.tics = 1;
    }
}

/// `A_BrainSpit`.
fn a_brain_spit(w: &mut World, mo: MobjId) {
    w.brain_easy ^= true;
    if w.params.skill <= Skill::Easy && !w.brain_easy {
        return;
    }
    if w.braintargets.is_empty() {
        // Chocolate: "A_BrainSpit: numbraintargets was 0 (vanilla crashes
        // here)".
        w.note_overflow("A_BrainSpit with no targets (vanilla crashes)");
        return;
    }
    // Shoot a cube at current target.
    let targ = w.braintargets[w.braintargeton];
    w.braintargeton = (w.braintargeton + 1) % w.braintargets.len();
    // Spawn brain missile.
    let newmobj = spawn_missile(w, mo, Some(targ), MT_SPAWNSHOT);
    w.mo_mut(newmobj).target = Some(targ);
    let (ty, my) = (w.mo(targ).y, w.mo(mo).y);
    let nm = w.mo(newmobj).clone();
    let tics = w.info.state(nm.state).tics;
    if nm.momy.0 == 0 || tics == 0 {
        // Vanilla divides by zero here.
        w.note_overflow("A_BrainSpit division by zero (vanilla crashes)");
        return;
    }
    w.mo_mut(newmobj).reactiontime = ((ty - my).0 / nm.momy.0) / tics;
    w.start_sound(SoundSource::None, SFX_BOSPIT);
}

/// `A_SpawnFly`: cube flying, then spawning a monster.
fn a_spawn_fly(w: &mut World, mo: MobjId) {
    w.mo_mut(mo).reactiontime -= 1;
    if w.mo(mo).reactiontime != 0 {
        return; // still flying
    }
    let (tx, ty, tz) = match w.try_mo(w.mo(mo).target) {
        Some(t) => (t.x, t.y, t.z),
        None => (Fixed(0), Fixed(0), Fixed(0)), // P_SubstNullMobj
    };
    // First spawn teleport fog.
    let fog = spawn_mobj(w, tx, ty, tz, MT_SPAWNFIRE);
    snd(w, fog, SFX_TELEPT);
    // Randomly select monster to spawn.
    let r = w.p_random();
    // Probability distribution (kind of :), decreasing likelihood.
    let t = match r {
        0..=49 => MT_TROOP,
        50..=89 => MT_SERGEANT,
        90..=119 => MT_SHADOWS,
        120..=129 => MT_PAIN,
        130..=159 => MT_HEAD,
        160..=161 => MT_VILE,
        162..=171 => MT_UNDEAD,
        172..=191 => MT_BABY,
        192..=221 => MT_FATSO,
        222..=245 => MT_KNIGHT,
        _ => MT_BRUISER,
    };
    let newmobj = spawn_mobj(w, tx, ty, tz, t);
    if look_for_players(w, newmobj, true) {
        let see = info(w, newmobj).seestate;
        set_mobj_state(w, newmobj, see);
    }
    // Telefrag anything in this spot.
    let (nx, ny) = (w.mo(newmobj).x, w.mo(newmobj).y);
    teleport_move(w, newmobj, nx, ny);
    // Remove self (i.e., cube).
    remove_mobj(w, mo);
}

/// `A_PlayerScream`.
fn a_player_scream(w: &mut World, mo: MobjId) {
    // Default death sound.
    let sound = if w.params.mode == GameMode::Commercial && w.mo(mo).health < -50 {
        // If the player dies less with less than -50% without gibbing.
        SFX_PDIEHI
    } else {
        SFX_PLDETH
    };
    snd(w, mo, sound);
}

/// Dispatch a mobj state action (`acp1(mobj)`).
pub fn call_mobj_action(w: &mut World, actor: MobjId, action: Action) {
    match action {
        Action::None => {}
        Action::Look => a_look(w, actor),
        Action::Chase => a_chase(w, actor),
        Action::FaceTarget => a_face_target(w, actor),
        Action::PosAttack => a_pos_attack(w, actor),
        Action::SPosAttack => a_spos_attack(w, actor),
        Action::CPosAttack => a_cpos_attack(w, actor),
        Action::CPosRefire => a_refire_monster(w, actor, 40),
        Action::SpidRefire => a_refire_monster(w, actor, 10),
        Action::BspiAttack => a_missile_at_target(w, actor, MT_ARACHPLAZ),
        Action::TroopAttack => a_troop_attack(w, actor),
        Action::SargAttack => a_sarg_attack(w, actor),
        Action::HeadAttack => a_head_attack(w, actor),
        Action::CyberAttack => a_missile_at_target(w, actor, MT_ROCKET),
        Action::BruisAttack => a_bruis_attack(w, actor),
        Action::SkelMissile => a_skel_missile(w, actor),
        Action::Tracer => a_tracer(w, actor),
        Action::SkelWhoosh => a_skel_whoosh(w, actor),
        Action::SkelFist => a_skel_fist(w, actor),
        Action::VileChase => a_vile_chase(w, actor),
        Action::VileStart => snd(w, actor, SFX_VILATK),
        Action::StartFire => {
            snd(w, actor, SFX_FLAMST);
            a_fire(w, actor);
        }
        Action::FireCrackle => {
            snd(w, actor, SFX_FLAME);
            a_fire(w, actor);
        }
        Action::Fire => a_fire(w, actor),
        Action::VileTarget => a_vile_target(w, actor),
        Action::VileAttack => a_vile_attack(w, actor),
        Action::FatRaise => {
            a_face_target(w, actor);
            snd(w, actor, SFX_MANATK);
        }
        Action::FatAttack1 => a_fat_attack1(w, actor),
        Action::FatAttack2 => a_fat_attack2(w, actor),
        Action::FatAttack3 => a_fat_attack3(w, actor),
        Action::SkullAttack => a_skull_attack(w, actor),
        Action::PainAttack => {
            if !has_target(w, actor) {
                return;
            }
            a_face_target(w, actor);
            let a = w.mo(actor).angle;
            a_pain_shoot_skull(w, actor, a);
        }
        Action::PainDie => {
            a_fall(w, actor);
            let a = w.mo(actor).angle;
            a_pain_shoot_skull(w, actor, a + Angle(ANG90));
            a_pain_shoot_skull(w, actor, a + Angle(ANG180));
            a_pain_shoot_skull(w, actor, a + Angle(ANG270));
        }
        Action::Scream => a_scream(w, actor),
        Action::XScream => snd(w, actor, SFX_SLOP),
        Action::Pain => {
            let ps = info(w, actor).painsound;
            if ps != SFX_NONE {
                snd(w, actor, ps);
            }
        }
        Action::Fall => a_fall(w, actor),
        Action::Explode => {
            let t = w.mo(actor).target;
            radius_attack(w, actor, t, 128);
        }
        Action::BossDeath => a_boss_death(w, actor),
        Action::Hoof => {
            snd(w, actor, SFX_HOOF);
            a_chase(w, actor);
        }
        Action::Metal => {
            snd(w, actor, SFX_METAL);
            a_chase(w, actor);
        }
        Action::BabyMetal => {
            snd(w, actor, SFX_BSPWLK);
            a_chase(w, actor);
        }
        Action::KeenDie => a_keen_die(w, actor),
        Action::BrainAwake => a_brain_awake(w),
        Action::BrainPain => w.start_sound(SoundSource::None, SFX_BOSPN),
        Action::BrainScream => a_brain_scream(w, actor),
        Action::BrainExplode => a_brain_explode(w, actor),
        Action::BrainDie => w.gameexit = Some(GameExit::Normal),
        Action::BrainSpit => a_brain_spit(w, actor),
        Action::SpawnSound => {
            snd(w, actor, SFX_BOSCUB);
            a_spawn_fly(w, actor);
        }
        Action::SpawnFly => a_spawn_fly(w, actor),
        Action::PlayerScream => a_player_scream(w, actor),
        Action::BFGSpray => crate::pspr::a_bfg_spray(w, actor),
        // Weapon (psprite) actions placed on a mobj state, e.g. by DEHACKED.
        // Vanilla would call them with a mobj as the player argument.
        other => w.note_unimplemented(other.name()),
    }
}
