//! Player thinking (p_user.c).

use doom_core::fixed::fixed_mul;
use doom_core::geom::point_to_angle2;
use doom_core::tables::{finecosine, finesine};
use doom_core::{Angle, Fixed, FRACUNIT};

use crate::info::*;
use crate::mobj::flags::*;
use crate::mobj::set_mobj_state;
use crate::player::{cheat, power, PlayerState, Weapon, VIEWHEIGHT};
use crate::ticcmd::bt;
use crate::world::World;
use doom_content::game::GameMode;
use doom_core::angle::{ANG180, ANG90, FINEANGLES, FINEMASK};

/// 16 pixels of bob.
const MAXBOB: Fixed = Fixed(0x100000);
const ANG5: u32 = ANG90 / 18;
const INVERSECOLORMAP: i32 = 32;

/// `P_Thrust`: moves the given origin along a given angle.
pub fn thrust(w: &mut World, player: usize, angle: Angle, mv: Fixed) {
    let mo = w.players[player].mo.unwrap();
    let a = angle.fine();
    let m = w.mo_mut(mo);
    m.momx += fixed_mul(mv, finecosine(a));
    m.momy += fixed_mul(mv, finesine(a));
}

/// `P_CalcHeight`: calculate the walking / running height adjustment.
pub fn calc_height(w: &mut World, player: usize) {
    let mo = w.players[player].mo.unwrap();
    let (momx, momy, z, ceilingz) = {
        let m = w.mo(mo);
        (m.momx, m.momy, m.z, m.ceilingz)
    };
    let leveltime = w.leveltime;
    let onground = w.onground;
    let p = &mut w.players[player];

    // Regular movement bobbing (needs to be calculated for gun swing even if
    // not on ground).
    p.bob = fixed_mul(momx, momx) + fixed_mul(momy, momy);
    p.bob = p.bob >> 2;
    if p.bob > MAXBOB {
        p.bob = MAXBOB;
    }

    if p.cheats & cheat::NOMOMENTUM != 0 || !onground {
        p.viewz = z + VIEWHEIGHT;
        if p.viewz > ceilingz - Fixed(4 * FRACUNIT) {
            p.viewz = ceilingz - Fixed(4 * FRACUNIT);
        }
        // Vanilla overwrites the clamped value immediately.
        p.viewz = z + p.viewheight;
        return;
    }

    let angle = ((FINEANGLES / 20) as i32).wrapping_mul(leveltime) as usize & FINEMASK;
    let bob = fixed_mul(p.bob / 2, finesine(angle));

    // Move viewheight.
    if p.playerstate == PlayerState::Live {
        p.viewheight += p.deltaviewheight;
        if p.viewheight > VIEWHEIGHT {
            p.viewheight = VIEWHEIGHT;
            p.deltaviewheight = Fixed(0);
        }
        if p.viewheight < VIEWHEIGHT / 2 {
            p.viewheight = VIEWHEIGHT / 2;
            if p.deltaviewheight.0 <= 0 {
                p.deltaviewheight = Fixed(1);
            }
        }
        if p.deltaviewheight.0 != 0 {
            p.deltaviewheight += Fixed(FRACUNIT / 4);
            if p.deltaviewheight.0 == 0 {
                p.deltaviewheight = Fixed(1);
            }
        }
    }
    p.viewz = z + p.viewheight + bob;
    if p.viewz > ceilingz - Fixed(4 * FRACUNIT) {
        p.viewz = ceilingz - Fixed(4 * FRACUNIT);
    }
}

/// `P_MovePlayer`.
pub fn move_player(w: &mut World, player: usize) {
    let cmd = w.players[player].cmd;
    let mo = w.players[player].mo.unwrap();
    {
        let m = w.mo_mut(mo);
        m.angle += Angle(((cmd.angleturn as i32) << 16) as u32);
    }
    // Do not let the player control movement if not onground.
    let (onground, angle) = {
        let m = w.mo(mo);
        (m.z <= m.floorz, m.angle)
    };
    w.onground = onground;
    if cmd.forwardmove != 0 && w.onground {
        thrust(w, player, angle, Fixed(cmd.forwardmove as i32 * 2048));
    }
    if cmd.sidemove != 0 && w.onground {
        thrust(w, player, angle - Angle::A90, Fixed(cmd.sidemove as i32 * 2048));
    }
    if (cmd.forwardmove != 0 || cmd.sidemove != 0) && w.mo(mo).state == S_PLAY {
        set_mobj_state(w, mo, S_PLAY_RUN1);
    }
}

/// `P_DeathThink`: fall on your face when dying; decrease POV height to
/// floor height.
pub fn death_think(w: &mut World, player: usize) {
    crate::pspr::move_psprites(w, player);
    let mo = w.players[player].mo.unwrap();
    {
        let p = &mut w.players[player];
        // Fall to the ground.
        if p.viewheight.0 > 6 * FRACUNIT {
            p.viewheight -= Fixed::ONE;
        }
        if p.viewheight.0 < 6 * FRACUNIT {
            p.viewheight = Fixed(6 * FRACUNIT);
        }
        p.deltaviewheight = Fixed(0);
    }
    w.onground = {
        let m = w.mo(mo);
        m.z <= m.floorz
    };
    calc_height(w, player);

    let attacker = w.players[player].attacker;
    let (mx, my, mangle) = {
        let m = w.mo(mo);
        (m.x, m.y, m.angle)
    };
    match w.try_mo(attacker).map(|a| (a.x, a.y)) {
        Some((ax, ay)) if attacker != Some(mo) => {
            let angle = point_to_angle2(mx, my, ax, ay);
            let delta = angle - mangle;
            if delta.0 < ANG5 || delta.0 > ANG5.wrapping_neg() {
                // Looking at killer, so fade damage flash down.
                w.mo_mut(mo).angle = angle;
                let p = &mut w.players[player];
                if p.damagecount != 0 {
                    p.damagecount -= 1;
                }
            } else if delta.0 < ANG180 {
                w.mo_mut(mo).angle += Angle(ANG5);
            } else {
                w.mo_mut(mo).angle -= Angle(ANG5);
            }
        }
        _ => {
            let p = &mut w.players[player];
            if p.damagecount != 0 {
                p.damagecount -= 1;
            }
        }
    }
    if w.players[player].cmd.buttons & bt::USE != 0 {
        w.players[player].playerstate = PlayerState::Reborn;
    }
}

/// `P_PlayerThink`.
pub fn player_think(w: &mut World, player: usize) {
    let mo = w.players[player].mo.expect("player in game without a body");

    // Fixme: do this in the cheat code.
    {
        let noclip = w.players[player].cheats & cheat::NOCLIP != 0;
        let m = w.mo_mut(mo);
        if noclip {
            m.flags |= MF_NOCLIP;
        } else {
            m.flags &= !MF_NOCLIP;
        }
    }

    // Chain saw run forward.
    if w.mo(mo).flags & MF_JUSTATTACKED != 0 {
        let cmd = &mut w.players[player].cmd;
        cmd.angleturn = 0;
        cmd.forwardmove = (0xc800 / 512) as i8;
        cmd.sidemove = 0;
        w.mo_mut(mo).flags &= !MF_JUSTATTACKED;
    }

    if w.players[player].playerstate == PlayerState::Dead {
        // A dead skater falls off the board (and under gravity again).
        crate::skate::disable(w, player);
        death_think(w, player);
        return;
    }

    if w.players[player].skater.is_some() {
        // The skateboard controller moves the player instead.
        crate::skate::think(w, player);
    } else {
        // Move around. Reactiontime is used to prevent movement for a bit
        // after a teleport.
        if w.mo(mo).reactiontime != 0 {
            w.mo_mut(mo).reactiontime -= 1;
        } else {
            move_player(w, player);
        }
        calc_height(w, player);
    }

    let sec = w.level.subsectors[w.mo(mo).subsector.idx()].sector;
    if w.level.sector(sec).special != 0 {
        crate::spec::player_in_special_sector(w, player);
    }

    // Check for weapon change. A special event has no other buttons.
    if w.players[player].cmd.buttons & bt::SPECIAL != 0 {
        w.players[player].cmd.buttons = 0;
    }
    let mode = w.params.mode;
    let cmd = w.players[player].cmd;
    // Live input names a weapon slot, 0..=9 (`TicCmd::weapon_slot`).
    if let Some(slot) = cmd.weapon_slot {
        crate::weapons::select_slot(w, player, slot);
    }
    // Vanilla's 3-bit change field (demos). Mod weapons in the slot (or
    // one in hand): slot cycling.
    let class_handled = cmd.buttons & bt::CHANGE != 0
        && crate::weapons::slot_key(w, player, ((cmd.buttons & bt::WEAPONMASK) >> bt::WEAPONSHIFT) as i32 + 1);
    let p = &mut w.players[player];
    if cmd.buttons & bt::CHANGE != 0 && !class_handled {
        // The actual changing of the weapon is done when the weapon psprite
        // can do it (read: not in the middle of an attack).
        let mut newweapon = Weapon::from_index((cmd.buttons & bt::WEAPONMASK) >> bt::WEAPONSHIFT);
        if newweapon == Weapon::Fist
            && p.weaponowned[Weapon::Chainsaw as usize]
            && !(p.readyweapon == Weapon::Chainsaw && p.powers[power::STRENGTH] != 0)
        {
            newweapon = Weapon::Chainsaw;
        }
        if mode == GameMode::Commercial
            && newweapon == Weapon::Shotgun
            && p.weaponowned[Weapon::SuperShotgun as usize]
            && p.readyweapon != Weapon::SuperShotgun
        {
            newweapon = Weapon::SuperShotgun;
        }
        // `weaponowned[newweapon]`: slot 8 (wp_nochange) can't be selected
        // from a 3-bit field, so the index is always in range.
        if (newweapon as usize) < p.weaponowned.len()
            && p.weaponowned[newweapon as usize]
            && newweapon != p.readyweapon
        {
            // Do not go to plasma or BFG in shareware, even if cheated.
            if (newweapon != Weapon::Plasma && newweapon != Weapon::Bfg) || mode != GameMode::Shareware {
                p.pendingweapon = newweapon;
            }
        }
    }

    // Check for use.
    if cmd.buttons & bt::USE != 0 {
        if !w.players[player].usedown {
            crate::map::use_lines(w, player);
            w.players[player].usedown = true;
        }
    } else {
        w.players[player].usedown = false;
    }

    // Cycle psprites.
    crate::pspr::move_psprites(w, player);

    // Counters, time dependent power ups.
    let p = &mut w.players[player];
    // Strength counts up to diminish fade.
    if p.powers[power::STRENGTH] != 0 {
        p.powers[power::STRENGTH] += 1;
    }
    if p.powers[power::INVULNERABILITY] != 0 {
        p.powers[power::INVULNERABILITY] -= 1;
    }
    let mut lose_shadow = false;
    if p.powers[power::INVISIBILITY] != 0 {
        p.powers[power::INVISIBILITY] -= 1;
        lose_shadow = p.powers[power::INVISIBILITY] == 0;
    }
    if p.powers[power::INFRARED] != 0 {
        p.powers[power::INFRARED] -= 1;
    }
    if p.powers[power::IRONFEET] != 0 {
        p.powers[power::IRONFEET] -= 1;
    }
    if p.damagecount != 0 {
        p.damagecount -= 1;
    }
    if p.bonuscount != 0 {
        p.bonuscount -= 1;
    }

    // Handling colormaps.
    let inv = p.powers[power::INVULNERABILITY];
    let ir = p.powers[power::INFRARED];
    p.fixedcolormap = if inv != 0 {
        if inv > 4 * 32 || inv & 8 != 0 {
            INVERSECOLORMAP
        } else {
            0
        }
    } else if ir != 0 {
        // Almost full bright.
        if ir > 4 * 32 || ir & 8 != 0 {
            1
        } else {
            0
        }
    } else {
        0
    };
    if lose_shadow {
        w.mo_mut(mo).flags &= !MF_SHADOW;
    }
}
