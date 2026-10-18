//! Player weapon sprites and weapon actions (p_pspr.c).

use doom_content::game::GameMode;
use doom_core::angle::{ANG180, ANG90, FINEANGLES, FINEMASK};
use doom_core::fixed::fixed_mul;
use doom_core::geom::point_to_angle2;
use doom_core::tables::{finecosine, finesine};
use doom_core::{Angle, Fixed, FRACUNIT};

use crate::enemy::noise_alert;
use crate::info::*;
use crate::items::ammo;
use crate::map::{aim_line_attack, line_attack, MELEERANGE, MISSILERANGE};
use crate::mobj::flags::*;
use crate::mobj::{set_mobj_state, spawn_mobj, spawn_player_missile};
use crate::player::{power, PlayerState, Weapon, NUMAMMO};
use crate::ticcmd::bt;
use crate::world::{SoundSource, World};

pub const PS_WEAPON: usize = 0;
pub const PS_FLASH: usize = 1;

const LOWERSPEED: Fixed = Fixed(FRACUNIT * 6);
const RAISESPEED: Fixed = Fixed(FRACUNIT * 6);
pub const WEAPONBOTTOM: Fixed = Fixed(128 * FRACUNIT);
pub const WEAPONTOP: Fixed = Fixed(32 * FRACUNIT);

fn pmo(w: &World, player: usize) -> crate::ids::MobjId {
    w.players[player].mo.expect("player without a body")
}

/// `P_SetPsprite`.
pub fn set_psprite(w: &mut World, player: usize, position: usize, mut stnum: StateNum) {
    loop {
        if stnum == S_NULL {
            // Object removed itself.
            w.players[player].psprites[position].state = None;
            break;
        }
        let state = *w.info.state(stnum);
        {
            let psp = &mut w.players[player].psprites[position];
            psp.state = Some(stnum);
            psp.tics = state.tics; // could be 0
            if state.misc1 != 0 {
                // Coordinate set.
                psp.sx = Fixed(state.misc1 << 16);
                psp.sy = Fixed(state.misc2 << 16);
            }
        }
        // Call action routine. Modified handling.
        if state.action != Action::None {
            if state.action == Action::WeaponReady {
                // Plain A_WeaponReady: no WRF_ flags.
                w.players[player].ready_flags = 0;
            }
            call_psprite_action(w, player, position, state.action);
            if w.players[player].psprites[position].state.is_none() {
                break;
            }
        }
        // Mod weapon states: action calls with arguments; a jump replaces
        // the state at once.
        if !w.info.state_calls.is_empty() {
            if let Some(call) = w.info.state_calls.get(&stnum.0).cloned() {
                if let Some(jump) = crate::actions::run_weapon(w, player, position, stnum, &call) {
                    stnum = jump;
                    continue;
                }
                if w.players[player].psprites[position].state.is_none() {
                    break;
                }
            }
        }
        // The action may have changed this psprite's state; continue from it.
        let cur = w.players[player].psprites[position].state.unwrap();
        stnum = w.info.state(cur).next;
        if w.players[player].psprites[position].tics != 0 {
            break;
        }
    }
}

/// `P_BringUpWeapon`: starts bringing the pending weapon up from the bottom
/// of the screen.
pub fn bring_up_weapon(w: &mut World, player: usize) {
    let p = &mut w.players[player];
    if p.pendingweapon == Weapon::NoChange {
        p.pendingweapon = p.readyweapon;
    }
    if p.pendingweapon == Weapon::Chainsaw {
        let mo = pmo(w, player);
        w.start_sound(SoundSource::Mobj(mo), SFX_SAWUP);
    }
    if w.players[player].pending_class.is_some() {
        crate::weapons::bring_up_class(w, player);
        return;
    }
    w.players[player].ready_class = None;
    let p = &mut w.players[player];
    let wpn = p.pendingweapon as usize;
    // A vanilla weapon whose class a mod replaced raises with its states.
    let newstate = if wpn < 9 && w.info.vanilla_weapon_replaced[wpn] {
        w.info.find_label(w.info.vanilla_weapon_classes[wpn], "select").unwrap_or(S_NULL)
    } else {
        w.weaponinfo[wpn].upstate
    };
    p.pendingweapon = Weapon::NoChange;
    p.psprites[PS_WEAPON].sy = WEAPONBOTTOM;
    set_psprite(w, player, PS_WEAPON, newstate);
}

/// `P_CheckAmmo`: returns true if there is enough ammo to shoot. If not,
/// selects the next weapon to use.
pub fn check_ammo(w: &mut World, player: usize) -> bool {
    if crate::weapons::active_class(w, player).is_some() {
        if crate::weapons::has_ammo(w, player) {
            return true;
        }
        // Out of ammo for a class weapon: switch to the owned weapon with
        // ammo and the best (lowest) selection order.
        match crate::weapons::best_weapon(w, player) {
            Some(s) => crate::weapons::select(w, player, s),
            None => {
                w.players[player].pending_class = None;
                let keep = w.players[player].readyweapon;
                w.players[player].readyweapon = Weapon::Fist;
                let _ = vanilla_check_ammo_pick(w, player);
                w.players[player].readyweapon = keep;
            }
        }
        let down = crate::weapons::weapon_state(w, player, crate::weapons::Which::Down);
        set_psprite(w, player, PS_WEAPON, down);
        return false;
    }
    let mode = w.params.mode;
    let bfg_cells = w.deh.bfg_cells_per_shot;
    let p = &mut w.players[player];
    let am = w.weaponinfo[p.readyweapon as usize].ammo;
    // Minimal amount for one shot varies.
    let count = match p.readyweapon {
        Weapon::Bfg => bfg_cells,
        Weapon::SuperShotgun => 2, // Double barrel.
        _ => 1,                    // Regular.
    };
    // Some do not need ammunition anyway. Return if current ammunition
    // sufficient.
    if am == ammo::NOAMMO || p.ammo[am] >= count {
        return true;
    }
    // Out of ammo, pick a weapon to change to. Preferences are set here.
    let owned = |wp: Weapon| p.weaponowned[wp as usize];
    p.pendingweapon = if owned(Weapon::Plasma) && p.ammo[ammo::CELL] != 0 && mode != GameMode::Shareware {
        Weapon::Plasma
    } else if owned(Weapon::SuperShotgun) && p.ammo[ammo::SHELL] > 2 && mode == GameMode::Commercial {
        Weapon::SuperShotgun
    } else if owned(Weapon::Chaingun) && p.ammo[ammo::CLIP] != 0 {
        Weapon::Chaingun
    } else if owned(Weapon::Shotgun) && p.ammo[ammo::SHELL] != 0 {
        Weapon::Shotgun
    } else if p.ammo[ammo::CLIP] != 0 {
        Weapon::Pistol
    } else if owned(Weapon::Chainsaw) {
        Weapon::Chainsaw
    } else if owned(Weapon::Missile) && p.ammo[ammo::MISL] != 0 {
        Weapon::Missile
    } else if owned(Weapon::Bfg) && p.ammo[ammo::CELL] > 40 && mode != GameMode::Shareware {
        Weapon::Bfg
    } else {
        // If everything fails.
        Weapon::Fist
    };
    // Now set appropriate weapon overlay.
    let down = w.weaponinfo[w.players[player].readyweapon as usize].downstate;
    set_psprite(w, player, PS_WEAPON, down);
    false
}

/// `P_CheckAmmo`'s choice of weapon when the current one is out of ammo
/// (sets `pendingweapon`).
fn vanilla_check_ammo_pick(w: &mut World, player: usize) -> Weapon {
    let mode = w.params.mode;
    let p = &mut w.players[player];
    let owned = |wp: Weapon| p.weaponowned[wp as usize];
    p.pendingweapon = if owned(Weapon::Plasma) && p.ammo[ammo::CELL] != 0 && mode != GameMode::Shareware {
        Weapon::Plasma
    } else if owned(Weapon::SuperShotgun) && p.ammo[ammo::SHELL] > 2 && mode == GameMode::Commercial {
        Weapon::SuperShotgun
    } else if owned(Weapon::Chaingun) && p.ammo[ammo::CLIP] != 0 {
        Weapon::Chaingun
    } else if owned(Weapon::Shotgun) && p.ammo[ammo::SHELL] != 0 {
        Weapon::Shotgun
    } else if p.ammo[ammo::CLIP] != 0 {
        Weapon::Pistol
    } else if owned(Weapon::Chainsaw) {
        Weapon::Chainsaw
    } else if owned(Weapon::Missile) && p.ammo[ammo::MISL] != 0 {
        Weapon::Missile
    } else if owned(Weapon::Bfg) && p.ammo[ammo::CELL] > 40 && mode != GameMode::Shareware {
        Weapon::Bfg
    } else {
        Weapon::Fist
    };
    p.pendingweapon
}

/// `P_FireWeapon`.
pub fn fire_weapon(w: &mut World, player: usize) {
    fire_weapon_mode(w, player, crate::weapons::FireMode::Primary, false);
}

/// `P_FireWeapon` for either attack: checks that mode's ammo, goes to
/// `Fire`/`AltFire` (or `Hold`/`AltHold` when refiring).
pub fn fire_weapon_mode(w: &mut World, player: usize, mode: crate::weapons::FireMode, hold: bool) {
    use crate::weapons::{FireMode, Which};
    w.players[player].fire_mode = mode;
    if mode == FireMode::Alt {
        // Secondary fire exists only on class weapons; out of its ammo it
        // just doesn't fire (the weapon stays up).
        if !crate::weapons::has_ammo_for(w, player, FireMode::Alt) {
            w.players[player].fire_mode = FireMode::Primary;
            return;
        }
    } else if !check_ammo(w, player) {
        return;
    }
    let mo = pmo(w, player);
    set_mobj_state(w, mo, S_PLAY_ATK1);
    let which = match (mode, hold) {
        (FireMode::Primary, false) => Which::Attack,
        (FireMode::Primary, true) => Which::Hold,
        (FireMode::Alt, false) => Which::AltAttack,
        (FireMode::Alt, true) => Which::AltHold,
    };
    let newstate = crate::weapons::weapon_state(w, player, which);
    set_psprite(w, player, PS_WEAPON, newstate);
    noise_alert(w, mo, mo);
}

/// Is `a` one of the weapon (psprite) actions?
pub fn is_psprite_action(a: Action) -> bool {
    matches!(
        a,
        Action::WeaponReady
            | Action::ReFire
            | Action::CheckReload
            | Action::Lower
            | Action::Raise
            | Action::GunFlash
            | Action::Punch
            | Action::Saw
            | Action::FireMissile
            | Action::FireBFG
            | Action::FirePlasma
            | Action::FirePistol
            | Action::FireShotgun
            | Action::FireShotgun2
            | Action::FireCGun
            | Action::Light0
            | Action::Light1
            | Action::Light2
            | Action::BFGsound
            | Action::OpenShotgun2
            | Action::LoadShotgun2
            | Action::CloseShotgun2
    )
}

/// `P_DropWeapon`: player died, so put the weapon away.
pub fn drop_weapon(w: &mut World, player: usize) {
    let down = crate::weapons::weapon_state(w, player, crate::weapons::Which::Down);
    set_psprite(w, player, PS_WEAPON, down);
}

/// `DecreaseAmmo`: Chocolate Doom's emulation of vanilla writing past
/// `ammo[]` into `maxammo[]` for `am_noammo` (and other out-of-range types).
fn decrease_ammo(w: &mut World, player: usize, ammonum: usize, amount: i32) {
    let p = &mut w.players[player];
    if ammonum < NUMAMMO {
        p.ammo[ammonum] -= amount;
    } else if ammonum - NUMAMMO < NUMAMMO {
        p.maxammo[ammonum - NUMAMMO] -= amount;
    }
}

fn ready_ammo(w: &World, player: usize) -> usize {
    w.weaponinfo[w.players[player].readyweapon as usize].ammo
}

fn flash_state(w: &World, player: usize) -> StateNum {
    crate::weapons::weapon_state(w, player, crate::weapons::Which::Flash)
}

/// `P_BulletSlope`: sets a slope so a near miss is at approximately the
/// height of the intended target.
fn bullet_slope(w: &mut World, mo: crate::ids::MobjId) {
    let mut an = w.mo(mo).angle;
    let range = Fixed(16 * 64 * FRACUNIT);
    w.bulletslope = aim_line_attack(w, Some(mo), an, range);
    if w.tm.linetarget.is_none() {
        an += Angle(1 << 26);
        w.bulletslope = aim_line_attack(w, Some(mo), an, range);
        if w.tm.linetarget.is_none() {
            an -= Angle(2 << 26);
            w.bulletslope = aim_line_attack(w, Some(mo), an, range);
        }
    }
}

/// `P_GunShot`.
fn gun_shot(w: &mut World, mo: crate::ids::MobjId, accurate: bool) {
    let damage = 5 * (w.p_random() % 3 + 1);
    let mut angle = w.mo(mo).angle;
    if !accurate {
        angle += Angle((w.p_sub_random() << 18) as u32);
    }
    let slope = w.bulletslope;
    line_attack(w, mo, angle, MISSILERANGE, slope, damage);
}

/// Dispatch a psprite state action (`acp2(player, psp)`).
pub fn call_psprite_action(w: &mut World, player: usize, psp: usize, action: Action) {
    let mo = pmo(w, player);
    match action {
        Action::WeaponReady => {
            // Get out of attack state.
            let st = w.mo(mo).state;
            if st == S_PLAY_ATK1 || st == S_PLAY_ATK2 {
                set_mobj_state(w, mo, S_PLAY);
            }
            if crate::weapons::active_class(w, player).is_none() && w.players[player].readyweapon == Weapon::Chainsaw && w.players[player].psprites[psp].state == Some(S_SAW) {
                w.start_sound(SoundSource::Mobj(mo), SFX_SAWIDL);
            }
            // Check for change; if player is dead, put the weapon away.
            // (WRF_NOSWITCH holds a pending change back.)
            const WRF_NOBOB: i32 = 1;
            const WRF_NOSWITCH: i32 = 2;
            const WRF_NOPRIMARY: i32 = 4;
            const WRF_NOSECONDARY: i32 = 8;
            let flags = w.players[player].ready_flags;
            let p = &w.players[player];
            if (p.pendingweapon != Weapon::NoChange && flags & WRF_NOSWITCH == 0) || p.health == 0 {
                // Change weapon (pending weapon should already be validated).
                let down = crate::weapons::weapon_state(w, player, crate::weapons::Which::Down);
                set_psprite(w, player, PS_WEAPON, down);
                return;
            }
            // Check for fire. The missile launcher and bfg do not
            // auto-fire.
            let class = crate::weapons::active_class(w, player).is_some();
            if w.players[player].cmd.buttons & bt::ATTACK != 0 && flags & WRF_NOPRIMARY == 0 {
                let p = &w.players[player];
                if !p.attackdown || class || (p.readyweapon != Weapon::Missile && p.readyweapon != Weapon::Bfg) {
                    w.players[player].attackdown = true;
                    fire_weapon(w, player);
                    return;
                }
            } else if w.players[player].cmd.buttons & bt::ATTACK == 0 {
                w.players[player].attackdown = false;
            }
            // Secondary fire (class weapons with an `AltFire` state).
            if w.players[player].cmd.ext & crate::ticcmd::ext::ALTATTACK != 0 && flags & WRF_NOSECONDARY == 0 {
                if crate::weapons::has_alt_fire(w, player) {
                    w.players[player].altattackdown = true;
                    fire_weapon_mode(w, player, crate::weapons::FireMode::Alt, false);
                    if w.players[player].fire_mode == crate::weapons::FireMode::Alt {
                        return;
                    }
                }
            } else {
                w.players[player].altattackdown = false;
            }
            if flags & WRF_NOBOB != 0 {
                return;
            }
            // Bob the weapon based on movement speed.
            let bob = w.players[player].bob;
            let mut angle = (128i32.wrapping_mul(w.leveltime)) as usize & FINEMASK;
            let ps = &mut w.players[player].psprites[psp];
            ps.sx = Fixed::ONE + fixed_mul(bob, finecosine(angle));
            angle &= FINEANGLES / 2 - 1;
            ps.sy = WEAPONTOP + fixed_mul(bob, finesine(angle));
        }
        Action::ReFire => a_refire(w, player),
        Action::CheckReload => {
            check_ammo(w, player);
        }
        Action::Lower => {
            let ps = &mut w.players[player].psprites[psp];
            ps.sy += LOWERSPEED;
            // Is already down.
            if ps.sy < WEAPONBOTTOM {
                return;
            }
            // Player is dead.
            if w.players[player].playerstate == PlayerState::Dead {
                w.players[player].psprites[psp].sy = WEAPONBOTTOM;
                // Don't bring weapon back up.
                return;
            }
            // The old weapon has been lowered off the screen, so change the
            // weapon and start raising it.
            if w.players[player].health == 0 {
                // Player is dead, so keep the weapon off screen.
                set_psprite(w, player, PS_WEAPON, S_NULL);
                return;
            }
            let p = &mut w.players[player];
            if p.pending_class.is_none() {
                p.readyweapon = p.pendingweapon;
            }
            bring_up_weapon(w, player);
        }
        Action::Raise => {
            let ps = &mut w.players[player].psprites[psp];
            ps.sy -= RAISESPEED;
            if ps.sy > WEAPONTOP {
                return;
            }
            ps.sy = WEAPONTOP;
            // The weapon has been raised all the way, so change to the
            // ready state.
            let ready = crate::weapons::weapon_state(w, player, crate::weapons::Which::Ready);
            set_psprite(w, player, PS_WEAPON, ready);
        }
        Action::GunFlash => {
            set_mobj_state(w, mo, S_PLAY_ATK2);
            let f = flash_state(w, player);
            set_psprite(w, player, PS_FLASH, f);
        }
        Action::Punch => {
            let mut damage = (w.p_random() % 10 + 1) << 1;
            if w.players[player].powers[power::STRENGTH] != 0 {
                damage *= 10;
            }
            let mut angle = w.mo(mo).angle;
            angle += Angle((w.p_sub_random() << 18) as u32);
            let slope = aim_line_attack(w, Some(mo), angle, MELEERANGE);
            line_attack(w, mo, angle, MELEERANGE, slope, damage);
            // Turn to face target.
            if let Some(t) = w.try_mo(w.tm.linetarget).map(|t| (t.x, t.y)) {
                w.start_sound(SoundSource::Mobj(mo), SFX_PUNCH);
                let (x, y) = (w.mo(mo).x, w.mo(mo).y);
                w.mo_mut(mo).angle = point_to_angle2(x, y, t.0, t.1);
            }
        }
        Action::Saw => {
            let damage = 2 * (w.p_random() % 10 + 1);
            let mut angle = w.mo(mo).angle;
            angle += Angle((w.p_sub_random() << 18) as u32);
            // Use meleerange + 1 so the puff doesn't skip the flash.
            let range = MELEERANGE + Fixed(1);
            let slope = aim_line_attack(w, Some(mo), angle, range);
            line_attack(w, mo, angle, range, slope, damage);
            let Some(t) = w.try_mo(w.tm.linetarget).map(|t| (t.x, t.y)) else {
                w.start_sound(SoundSource::Mobj(mo), SFX_SAWFUL);
                return;
            };
            w.start_sound(SoundSource::Mobj(mo), SFX_SAWHIT);
            // Turn to face target.
            let (x, y, ma) = {
                let m = w.mo(mo);
                (m.x, m.y, m.angle)
            };
            let angle = point_to_angle2(x, y, t.0, t.1);
            let m = w.mo_mut(mo);
            if (angle - ma).0 > ANG180 {
                if ((angle - ma).0 as i32) < -((ANG90 / 20) as i32) {
                    m.angle = angle + Angle(ANG90 / 21);
                } else {
                    m.angle -= Angle(ANG90 / 20);
                }
            } else if (angle - ma).0 > ANG90 / 20 {
                m.angle = angle - Angle(ANG90 / 21);
            } else {
                m.angle += Angle(ANG90 / 20);
            }
            m.flags |= MF_JUSTATTACKED;
        }
        Action::FireMissile => {
            let a = ready_ammo(w, player);
            decrease_ammo(w, player, a, 1);
            spawn_player_missile(w, mo, MT_ROCKET);
        }
        Action::FireBFG => {
            let a = ready_ammo(w, player);
            let n = w.deh.bfg_cells_per_shot;
            decrease_ammo(w, player, a, n);
            spawn_player_missile(w, mo, MT_BFG);
        }
        Action::FirePlasma => {
            let a = ready_ammo(w, player);
            decrease_ammo(w, player, a, 1);
            let f = flash_state(w, player);
            let r = w.p_random() & 1;
            set_psprite(w, player, PS_FLASH, StateNum(f.0 + r as u16));
            spawn_player_missile(w, mo, MT_PLASMA);
        }
        Action::FirePistol => {
            w.start_sound(SoundSource::Mobj(mo), SFX_PISTOL);
            set_mobj_state(w, mo, S_PLAY_ATK2);
            let a = ready_ammo(w, player);
            decrease_ammo(w, player, a, 1);
            let f = flash_state(w, player);
            set_psprite(w, player, PS_FLASH, f);
            bullet_slope(w, mo);
            let accurate = w.players[player].refire == 0;
            gun_shot(w, mo, accurate);
        }
        Action::FireShotgun => {
            w.start_sound(SoundSource::Mobj(mo), SFX_SHOTGN);
            set_mobj_state(w, mo, S_PLAY_ATK2);
            let a = ready_ammo(w, player);
            decrease_ammo(w, player, a, 1);
            let f = flash_state(w, player);
            set_psprite(w, player, PS_FLASH, f);
            bullet_slope(w, mo);
            for _ in 0..7 {
                gun_shot(w, mo, false);
            }
        }
        Action::FireShotgun2 => {
            w.start_sound(SoundSource::Mobj(mo), SFX_DSHTGN);
            set_mobj_state(w, mo, S_PLAY_ATK2);
            let a = ready_ammo(w, player);
            decrease_ammo(w, player, a, 2);
            let f = flash_state(w, player);
            set_psprite(w, player, PS_FLASH, f);
            bullet_slope(w, mo);
            for _ in 0..20 {
                let damage = 5 * (w.p_random() % 3 + 1);
                let mut angle = w.mo(mo).angle;
                angle += Angle((w.p_sub_random() << doom_core::angle::ANGLETOFINESHIFT) as u32);
                let slope = w.bulletslope + Fixed(w.p_sub_random() << 5);
                line_attack(w, mo, angle, MISSILERANGE, slope, damage);
            }
        }
        Action::FireCGun => {
            w.start_sound(SoundSource::Mobj(mo), SFX_PISTOL);
            let a = ready_ammo(w, player);
            if a < NUMAMMO && w.players[player].ammo[a] == 0 {
                return;
            }
            set_mobj_state(w, mo, S_PLAY_ATK2);
            decrease_ammo(w, player, a, 1);
            let f = flash_state(w, player);
            let cur = w.players[player].psprites[psp].state.unwrap();
            let st = StateNum((f.0 as i32 + cur.0 as i32 - S_CHAIN1.0 as i32) as u16);
            set_psprite(w, player, PS_FLASH, st);
            bullet_slope(w, mo);
            let accurate = w.players[player].refire == 0;
            gun_shot(w, mo, accurate);
        }
        Action::Light0 => w.players[player].extralight = 0,
        Action::Light1 => w.players[player].extralight = 1,
        Action::Light2 => w.players[player].extralight = 2,
        Action::BFGsound => w.start_sound(SoundSource::Mobj(mo), SFX_BFG),
        Action::OpenShotgun2 => w.start_sound(SoundSource::Mobj(mo), SFX_DBOPN),
        Action::LoadShotgun2 => w.start_sound(SoundSource::Mobj(mo), SFX_DBLOAD),
        Action::CloseShotgun2 => {
            w.start_sound(SoundSource::Mobj(mo), SFX_DBCLS);
            a_refire(w, player);
        }
        Action::None => {}
        // A mobj action on a psprite state (possible with DEHACKED): vanilla
        // would call it with the wrong argument types.
        other => w.note_unimplemented(other.name()),
    }
}

/// `A_ReFire`: the player can re-fire the weapon without lowering it
/// entirely.
fn a_refire(w: &mut World, player: usize) {
    if crate::weapons::active_class(w, player).is_some() {
        return refire(w, player, None);
    }
    let p = &w.players[player];
    // Check for fire (if a weaponchange is pending, let it go through
    // instead).
    if p.cmd.buttons & bt::ATTACK != 0 && p.pendingweapon == Weapon::NoChange && p.health != 0 {
        w.players[player].refire += 1;
        fire_weapon(w, player);
    } else {
        w.players[player].refire = 0;
        check_ammo(w, player);
    }
}

/// `A_ReFire` for class weapons: keep firing the attack that is running
/// while its button is held (into `Hold`/`AltHold`, or `state`).
pub fn refire(w: &mut World, player: usize, state: Option<StateNum>) {
    use crate::weapons::FireMode;
    let p = &w.players[player];
    let held = match p.fire_mode {
        FireMode::Primary => p.cmd.buttons & bt::ATTACK != 0,
        FireMode::Alt => p.cmd.ext & crate::ticcmd::ext::ALTATTACK != 0,
    };
    if held && p.pendingweapon == Weapon::NoChange && p.health != 0 {
        w.players[player].refire += 1;
        let mode = w.players[player].fire_mode;
        match state {
            Some(s) => {
                if crate::weapons::has_ammo_for(w, player, mode) {
                    set_psprite(w, player, PS_WEAPON, s);
                }
            }
            None => fire_weapon_mode(w, player, mode, true),
        }
    } else {
        w.players[player].refire = 0;
        check_ammo(w, player);
    }
}

/// `A_BFGSpray`: spawn a BFG explosion on every monster in view.
pub fn a_bfg_spray(w: &mut World, mo: crate::ids::MobjId) {
    let (angle, target) = (w.mo(mo).angle, w.mo(mo).target);
    // Offset angles from its attack angle.
    for i in 0..40u32 {
        let an = angle - Angle(ANG90 / 2) + Angle(ANG90 / 40 * i);
        // mo->target is the originator (player) of the missile.
        aim_line_attack(w, target, an, Fixed(16 * 64 * FRACUNIT));
        let Some(lt) = w.tm.linetarget else { continue };
        let (x, y, z, h) = {
            let t = w.mo(lt);
            (t.x, t.y, t.z, t.height)
        };
        spawn_mobj(w, x, y, z + (h >> 2), MT_EXTRABFG);
        let mut damage = 0;
        for _ in 0..15 {
            damage += (w.p_random() & 7) + 1;
        }
        crate::inter::damage_mobj(w, lt, target, target, damage);
    }
}

/// `P_SetupPsprites`: called at start of level for each player.
pub fn setup_psprites(w: &mut World, player: usize) {
    // Remove all psprites.
    for psp in &mut w.players[player].psprites {
        psp.state = None;
    }
    // Spawn the gun.
    let p = &mut w.players[player];
    p.pendingweapon = p.readyweapon;
    bring_up_weapon(w, player);
}

/// `P_MovePsprites`: called every tic by player thinking routine.
pub fn move_psprites(w: &mut World, player: usize) {
    for i in 0..crate::player::NUMPSPRITES {
        let psp = w.players[player].psprites[i];
        // A null state means not active.
        let Some(st) = psp.state else { continue };
        // Drop tic count and possibly change state; a -1 tic count never
        // changes.
        if psp.tics != -1 {
            w.players[player].psprites[i].tics -= 1;
            if w.players[player].psprites[i].tics == 0 {
                let next = w.info.state(st).next;
                set_psprite(w, player, i, next);
            }
        }
    }
    let p = &mut w.players[player];
    p.psprites[PS_FLASH].sx = p.psprites[PS_WEAPON].sx;
    p.psprites[PS_FLASH].sy = p.psprites[PS_WEAPON].sy;
}
