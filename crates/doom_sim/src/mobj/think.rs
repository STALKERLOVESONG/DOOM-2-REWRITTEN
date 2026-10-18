//! Per-tic mobj movement and state cycling (p_mobj.c `P_MobjThinker`,
//! `P_XYMovement`, `P_ZMovement`, `P_NightmareRespawn`).

use doom_content::game::GameVersion;
use doom_content::map::mtf;
use doom_core::fixed::fixed_mul;
use doom_core::geom::aprox_distance;
use doom_core::{Angle, Fixed, FRACUNIT};

use super::flags::*;
use super::spawn::*;
use super::{ONCEILINGZ, ONFLOORZ};
use crate::ids::MobjId;
use crate::info::*;
use crate::map::{check_position, slide_move, try_move, MAXMOVE};
use crate::player::{cheat, VIEWHEIGHT};
use crate::world::{SoundSource, World};

pub const STOPSPEED: i32 = 0x1000;
pub const FRICTION: Fixed = Fixed(0xe800);
pub const GRAVITY: Fixed = Fixed(FRACUNIT);
pub const FLOATSPEED: Fixed = Fixed(FRACUNIT * 4);
const TICRATE: i32 = 35;

/// `P_XYMovement`.
pub fn xy_movement(w: &mut World, id: MobjId) {
    let (momx, momy, flags, mobj_type) = {
        let m = w.mo(id);
        (m.momx, m.momy, m.flags, m.mobj_type)
    };
    {
        if momx.0 == 0 && momy.0 == 0 {
            if flags & MF_SKULLFLY != 0 {
                // The skull slammed into something.
                let spawn = w.info.mobj(mobj_type).spawnstate;
                let m = w.mo_mut(id);
                m.flags &= !MF_SKULLFLY;
                m.momx = Fixed(0);
                m.momy = Fixed(0);
                m.momz = Fixed(0);
                set_mobj_state(w, id, spawn);
            }
            return;
        }
    }
    let player = w.mo(id).player;
    let (mut xmove, mut ymove) = {
        let m = w.mo_mut(id);
        m.momx = m.momx.clamp(-MAXMOVE, MAXMOVE);
        m.momy = m.momy.clamp(-MAXMOVE, MAXMOVE);
        (m.momx, m.momy)
    };

    loop {
        let (x, y) = (w.mo(id).x, w.mo(id).y);
        let (ptryx, ptryy);
        // Note: only the positive direction is checked, as in vanilla.
        if xmove.0 > MAXMOVE.0 / 2 || ymove.0 > MAXMOVE.0 / 2 {
            ptryx = x + xmove / 2;
            ptryy = y + ymove / 2;
            xmove = xmove >> 1;
            ymove = ymove >> 1;
        } else {
            ptryx = x + xmove;
            ptryy = y + ymove;
            xmove = Fixed(0);
            ymove = Fixed(0);
        }

        if !try_move(w, id, ptryx, ptryy) {
            // Blocked move.
            if w.mo(id).player.is_some() {
                // Try to slide along it.
                slide_move(w, id);
            } else if w.mo(id).flags & MF_MISSILE != 0 {
                // Explode a missile, unless it hit a sky ceiling (hack to
                // prevent missiles exploding against the sky; does not
                // handle sky floors).
                if let Some(cl) = w.tm.ceilingline {
                    if let Some(back) = w.level.line(cl).back_sector {
                        if w.level.sector(back).ceiling_pic == w.sky_flat {
                            remove_mobj(w, id);
                            return;
                        }
                    }
                }
                explode_missile(w, id);
            } else {
                let m = w.mo_mut(id);
                m.momx = Fixed(0);
                m.momy = Fixed(0);
            }
        }
        if xmove.0 == 0 && ymove.0 == 0 {
            break;
        }
    }

    // Slow down.
    if let Some(p) = player {
        if w.players[p].cheats & cheat::NOMOMENTUM != 0 {
            // Debug option for no sliding at all.
            let m = w.mo_mut(id);
            m.momx = Fixed(0);
            m.momy = Fixed(0);
            return;
        }
    }
    let m = w.mo(id);
    if m.flags & (MF_MISSILE | MF_SKULLFLY) != 0 {
        return; // no friction for missiles ever
    }
    if m.z > m.floorz {
        return; // no friction when airborne
    }
    if m.flags & MF_CORPSE != 0 {
        // Do not stop sliding if halfway off a step with some momentum.
        let q = FRACUNIT / 4;
        if m.momx.0 > q || m.momx.0 < -q || m.momy.0 > q || m.momy.0 < -q {
            let sec = w.level.subsectors[m.subsector.idx()].sector;
            if m.floorz != w.level.sector(sec).floor_height {
                return;
            }
        }
    }

    let stopped = m.momx.0 > -STOPSPEED
        && m.momx.0 < STOPSPEED
        && m.momy.0 > -STOPSPEED
        && m.momy.0 < STOPSPEED
        && player.is_none_or(|p| w.players[p].cmd.forwardmove == 0 && w.players[p].cmd.sidemove == 0);
    if stopped {
        // If in a walking frame, stop moving. Note this tests the *player's*
        // body (`player->mo`), which is normally this mobj.
        if let Some(p) = player {
            if let Some(pmo) = w.players[p].mo {
                let st = w.mo(pmo).state;
                if (st.0 as u32).wrapping_sub(S_PLAY_RUN1.0 as u32) < 4 {
                    set_mobj_state(w, pmo, S_PLAY);
                }
            }
        }
        let m = w.mo_mut(id);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
    } else {
        let m = w.mo_mut(id);
        m.momx = fixed_mul(m.momx, FRICTION);
        m.momy = fixed_mul(m.momy, FRICTION);
    }
}

/// `P_ZMovement`.
pub fn z_movement(w: &mut World, id: MobjId) {
    // Check for smooth step up.
    {
        let m = w.mo(id);
        if let Some(p) = m.player {
            if m.z < m.floorz {
                let d = m.floorz - m.z;
                let pl = &mut w.players[p];
                pl.viewheight -= d;
                pl.deltaviewheight = (VIEWHEIGHT - pl.viewheight) >> 3;
            }
        }
    }

    // Adjust height.
    {
        let m = w.mo_mut(id);
        m.z += m.momz;
    }

    let (flags, target) = (w.mo(id).flags, w.mo(id).target);
    if flags & MF_FLOAT != 0 {
        if let Some(t) = w.try_mo(target).map(|t| (t.x, t.y, t.z)) {
            // Float down towards target if too close.
            if flags & MF_SKULLFLY == 0 && flags & MF_INFLOAT == 0 {
                let m = w.mo(id);
                let dist = aprox_distance(m.x - t.0, m.y - t.1);
                let delta = (t.2 + (m.height >> 1)) - m.z;
                let m = w.mo_mut(id);
                if delta.0 < 0 && dist.0 < -(delta.0.wrapping_mul(3)) {
                    m.z -= FLOATSPEED;
                } else if delta.0 > 0 && dist.0 < delta.0.wrapping_mul(3) {
                    m.z += FLOATSPEED;
                }
            }
        }
    }

    // Clip movement.
    let correct_lost_soul_bounce = w.params.version >= GameVersion::Ultimate;
    if w.mo(id).z <= w.mo(id).floorz {
        // Hit the floor.
        let m = w.mo_mut(id);
        if correct_lost_soul_bounce && m.flags & MF_SKULLFLY != 0 {
            // The skull slammed into something.
            m.momz = -m.momz;
        }
        if m.momz.0 < 0 {
            let (player, momz) = (m.player, m.momz);
            if let Some(p) = player {
                if momz.0 < -GRAVITY.0 * 8 {
                    // Squat down: decrease viewheight for a moment after
                    // hitting the ground (hard), and utter appropriate sound.
                    w.players[p].deltaviewheight = momz >> 3;
                    w.start_sound(SoundSource::Mobj(id), SFX_OOF);
                }
            }
            w.mo_mut(id).momz = Fixed(0);
        }
        let m = w.mo_mut(id);
        m.z = m.floorz;
        // Doom 2 v1.9 bug compatibility: a charging soul hit by a raising
        // floor incorrectly reverses its momentum here.
        if !correct_lost_soul_bounce && m.flags & MF_SKULLFLY != 0 {
            m.momz = -m.momz;
        }
        if m.flags & MF_MISSILE != 0 && m.flags & MF_NOCLIP == 0 {
            explode_missile(w, id);
            return;
        }
    } else if w.mo(id).flags & MF_NOGRAVITY == 0 {
        let m = w.mo_mut(id);
        if m.momz.0 == 0 {
            m.momz = -GRAVITY * 2;
        } else {
            m.momz -= GRAVITY;
        }
    }

    let m = w.mo(id);
    if m.z + m.height > m.ceilingz {
        // Hit the ceiling.
        let m = w.mo_mut(id);
        if m.momz.0 > 0 {
            m.momz = Fixed(0);
        }
        m.z = m.ceilingz - m.height;
        if m.flags & MF_SKULLFLY != 0 {
            // The skull slammed into something.
            m.momz = -m.momz;
        }
        if m.flags & MF_MISSILE != 0 && m.flags & MF_NOCLIP == 0 {
            explode_missile(w, id);
        }
    }
}

/// `P_NightmareRespawn`.
pub fn nightmare_respawn(w: &mut World, id: MobjId) {
    let sp = w.mo(id).spawnpoint;
    let (x, y) = (Fixed(sp.x), Fixed(sp.y));
    // Something is occupying its position?
    if !check_position(w, id, x, y) {
        return; // no respawn
    }
    // Spawn a teleport fog at the old spot because of removal of the body.
    let (ox, oy, osec) = {
        let m = w.mo(id);
        (m.x, m.y, w.level.subsectors[m.subsector.idx()].sector)
    };
    let ofloor = w.level.sector(osec).floor_height;
    let fog = spawn_mobj(w, ox, oy, ofloor, MT_TFOG);
    w.start_sound(SoundSource::Mobj(fog), SFX_TELEPT);
    // Spawn a teleport fog at the new spot.
    let nfloor = floor_at(w, x, y);
    let fog = spawn_mobj(w, x, y, nfloor, MT_TFOG);
    w.start_sound(SoundSource::Mobj(fog), SFX_TELEPT);

    // Spawn the new monster, inheriting attributes from the deceased one.
    let t = w.mo(id).mobj_type;
    let z = if w.info.mobj(t).flags & MF_SPAWNCEILING != 0 { ONCEILINGZ } else { ONFLOORZ };
    let mo = spawn_mobj(w, x, y, z, t);
    let m = w.mo_mut(mo);
    m.spawnpoint = sp;
    m.angle = Angle::from_mapthing_degrees(sp.angle);
    if sp.options & mtf::AMBUSH != 0 {
        m.flags |= MF_AMBUSH;
    }
    m.reactiontime = 18;
    // Remove the old monster.
    remove_mobj(w, id);
}

/// `P_MobjThinker`.
pub fn mobj_thinker(w: &mut World, id: MobjId) {
    // Momentum movement.
    let m = w.mo(id);
    if m.momx.0 != 0 || m.momy.0 != 0 || m.flags & MF_SKULLFLY != 0 {
        xy_movement(w, id);
        if w.thinkers.is_removed(id) {
            return; // mobj was removed
        }
    }
    let m = w.mo(id);
    if m.z != m.floorz || m.momz.0 != 0 {
        z_movement(w, id);
        if w.thinkers.is_removed(id) {
            return; // mobj was removed
        }
    }

    // Cycle through states, calling action functions at transitions.
    let m = w.mo(id);
    if m.tics != -1 {
        let m = w.mo_mut(id);
        m.tics -= 1;
        // You can cycle through multiple states in a tic.
        if m.tics == 0 {
            let state = m.state;
            let next = w.info.state(state).next;
            set_mobj_state(w, id, next);
        }
    } else {
        // Check for nightmare respawn.
        if m.flags & MF_COUNTKILL == 0 || !w.params.respawnmonsters {
            return;
        }
        let m = w.mo_mut(id);
        m.movecount += 1;
        if m.movecount < 12 * TICRATE {
            return;
        }
        if w.leveltime & 31 != 0 {
            return;
        }
        if w.p_random() > 4 {
            return;
        }
        nightmare_respawn(w, id);
    }
}
