//! Damage, death and item pickup (p_inter.c).

use doom_content::game::{GameMode, GameVersion};
use doom_core::fixed::fixed_mul;
use doom_core::geom::point_to_angle2;
use doom_core::tables::{finecosine, finesine};
use doom_core::{Angle, Fixed, FRACUNIT};

use crate::ids::MobjId;
use crate::info::*;
use crate::items::ammo;
use crate::mobj::flags::*;
use crate::mobj::{remove_mobj, set_mobj_state, spawn_mobj, ONFLOORZ};
use crate::player::{cheat, power, PlayerState, Weapon, NUMAMMO};
use crate::world::{Skill, SoundSource, World};

const BONUSADD: i32 = 6;
pub const MAXHEALTH: i32 = 100;
const BASETHRESHOLD: i32 = 100;
const TICRATE: i32 = 35;
pub const INVULNTICS: i32 = 30 * TICRATE;
pub const INVISTICS: i32 = 60 * TICRATE;
pub const INFRATICS: i32 = 120 * TICRATE;
pub const IRONTICS: i32 = 60 * TICRATE;

/// A weapon's ammo per item: a clip loaded by the gun.
pub const CLIPAMMO: [i32; NUMAMMO] = [10, 4, 20, 1];

// Pickup messages (d_englsh.h).
const GOTARMOR: &str = "Picked up the armor.";
const GOTMEGA: &str = "Picked up the MegaArmor!";
const GOTHTHBONUS: &str = "Picked up a health bonus.";
const GOTARMBONUS: &str = "Picked up an armor bonus.";
const GOTSTIM: &str = "Picked up a stimpack.";
const GOTMEDINEED: &str = "Picked up a medikit that you REALLY need!";
const GOTMEDIKIT: &str = "Picked up a medikit.";
const GOTSUPER: &str = "Supercharge!";
const GOTBLUECARD: &str = "Picked up a blue keycard.";
const GOTYELWCARD: &str = "Picked up a yellow keycard.";
const GOTREDCARD: &str = "Picked up a red keycard.";
const GOTBLUESKUL: &str = "Picked up a blue skull key.";
const GOTYELWSKUL: &str = "Picked up a yellow skull key.";
const GOTREDSKULL: &str = "Picked up a red skull key.";
const GOTINVUL: &str = "Invulnerability!";
const GOTBERSERK: &str = "Berserk!";
const GOTINVIS: &str = "Partial Invisibility";
const GOTSUIT: &str = "Radiation Shielding Suit";
const GOTMAP: &str = "Computer Area Map";
const GOTVISOR: &str = "Light Amplification Visor";
const GOTMSPHERE: &str = "MegaSphere!";
const GOTCLIP: &str = "Picked up a clip.";
const GOTCLIPBOX: &str = "Picked up a box of bullets.";
const GOTROCKET: &str = "Picked up a rocket.";
const GOTROCKBOX: &str = "Picked up a box of rockets.";
const GOTCELL: &str = "Picked up an energy cell.";
const GOTCELLBOX: &str = "Picked up an energy cell pack.";
const GOTSHELLS: &str = "Picked up 4 shotgun shells.";
const GOTSHELLBOX: &str = "Picked up a box of shotgun shells.";
const GOTBACKPACK: &str = "Picked up a backpack full of ammo!";
const GOTBFG9000: &str = "You got the BFG9000!  Oh, yes.";
const GOTCHAINGUN: &str = "You got the chaingun!";
const GOTCHAINSAW: &str = "A chainsaw!  Find some meat!";
const GOTLAUNCHER: &str = "You got the rocket launcher!";
const GOTPLASMA: &str = "You got the plasma gun!";
const GOTSHOTGUN: &str = "You got the shotgun!";
const GOTSHOTGUN2: &str = "You got the super shotgun!";

/// `P_GiveAmmo`: `num` is the number of clip loads, not the individual count
/// (0 = 1/2 clip). Returns false if the ammo can't be picked up at all.
pub fn give_ammo(w: &mut World, player: usize, am: usize, num: i32) -> bool {
    if am == ammo::NOAMMO {
        return false;
    }
    if am >= NUMAMMO {
        panic!("P_GiveAmmo: bad type {am}");
    }
    let skill = w.params.skill;
    let p = &mut w.players[player];
    if p.ammo[am] == p.maxammo[am] {
        return false;
    }
    let mut num = if num != 0 { num * CLIPAMMO[am] } else { CLIPAMMO[am] / 2 };
    if skill == Skill::Baby || skill == Skill::Nightmare {
        // Give double ammo in trainer mode, you'll need in nightmare.
        num <<= 1;
    }
    let oldammo = p.ammo[am];
    p.ammo[am] += num;
    if p.ammo[am] > p.maxammo[am] {
        p.ammo[am] = p.maxammo[am];
    }
    // If non zero ammo, don't change up weapons, player was lower on
    // purpose.
    if oldammo != 0 {
        return true;
    }
    // We were down to zero, so select a new weapon. Preferences are not
    // user selectable.
    match am {
        ammo::CLIP => {
            if p.readyweapon == Weapon::Fist {
                p.pendingweapon = if p.weaponowned[Weapon::Chaingun as usize] { Weapon::Chaingun } else { Weapon::Pistol };
            }
        }
        ammo::SHELL => {
            if (p.readyweapon == Weapon::Fist || p.readyweapon == Weapon::Pistol) && p.weaponowned[Weapon::Shotgun as usize] {
                p.pendingweapon = Weapon::Shotgun;
            }
        }
        ammo::CELL => {
            if (p.readyweapon == Weapon::Fist || p.readyweapon == Weapon::Pistol) && p.weaponowned[Weapon::Plasma as usize] {
                p.pendingweapon = Weapon::Plasma;
            }
        }
        ammo::MISL => {
            if p.readyweapon == Weapon::Fist && p.weaponowned[Weapon::Missile as usize] {
                p.pendingweapon = Weapon::Missile;
            }
        }
        _ => {}
    }
    true
}

/// `P_GiveWeapon`: the weapon name may have a MF_DROPPED flag ored in.
pub fn give_weapon(w: &mut World, player: usize, weapon: Weapon, dropped: bool) -> bool {
    let wi = w.weaponinfo[weapon as usize];
    if w.params.netgame && w.params.deathmatch != 2 && !dropped {
        // Leave placed weapons forever on net games.
        if w.players[player].weaponowned[weapon as usize] {
            return false;
        }
        w.players[player].bonuscount += BONUSADD;
        w.players[player].weaponowned[weapon as usize] = true;
        let n = if w.params.deathmatch != 0 { 5 } else { 2 };
        give_ammo(w, player, wi.ammo, n);
        w.players[player].pendingweapon = weapon;
        if player == w.params.consoleplayer {
            w.start_sound(SoundSource::None, SFX_WPNUP);
        }
        return false;
    }
    let gaveammo = if wi.ammo != ammo::NOAMMO {
        // Give one clip with a dropped weapon, two clips with a found weapon.
        give_ammo(w, player, wi.ammo, if dropped { 1 } else { 2 })
    } else {
        false
    };
    let p = &mut w.players[player];
    let gaveweapon = if p.weaponowned[weapon as usize] {
        false
    } else {
        p.weaponowned[weapon as usize] = true;
        p.pendingweapon = weapon;
        true
    };
    gaveweapon || gaveammo
}

/// `P_GiveBody`: returns false if the body isn't needed at all.
pub fn give_body(w: &mut World, player: usize, num: i32) -> bool {
    let p = &mut w.players[player];
    if p.health >= MAXHEALTH {
        return false;
    }
    p.health = (p.health + num).min(MAXHEALTH);
    let (h, mo) = (p.health, p.mo);
    if let Some(mo) = mo {
        w.mo_mut(mo).health = h;
    }
    true
}

/// `P_GiveArmor`: returns false if the armor is worse than the current
/// armor.
pub fn give_armor(w: &mut World, player: usize, armortype: i32) -> bool {
    let hits = armortype * 100;
    let p = &mut w.players[player];
    if p.armorpoints >= hits {
        return false; // don't pick up
    }
    p.armortype = armortype;
    p.armorpoints = hits;
    true
}

/// `P_GiveCard`.
pub fn give_card(w: &mut World, player: usize, card: usize) {
    let p = &mut w.players[player];
    if p.cards[card] {
        return;
    }
    p.bonuscount = BONUSADD;
    p.cards[card] = true;
}

/// `P_GivePower`.
pub fn give_power(w: &mut World, player: usize, pw: usize) -> bool {
    match pw {
        power::INVULNERABILITY => {
            w.players[player].powers[pw] = INVULNTICS;
            true
        }
        power::INVISIBILITY => {
            w.players[player].powers[pw] = INVISTICS;
            let mo = w.players[player].mo.unwrap();
            w.mo_mut(mo).flags |= MF_SHADOW;
            true
        }
        power::INFRARED => {
            w.players[player].powers[pw] = INFRATICS;
            true
        }
        power::IRONFEET => {
            w.players[player].powers[pw] = IRONTICS;
            true
        }
        power::STRENGTH => {
            give_body(w, player, 100);
            w.players[player].powers[pw] = 1;
            true
        }
        _ => {
            if w.players[player].powers[pw] != 0 {
                return false; // already got it
            }
            w.players[player].powers[pw] = 1;
            true
        }
    }
}

/// `P_TouchSpecialThing`.
pub fn touch_special_thing(w: &mut World, special: MobjId, toucher: MobjId) {
    let (sz, sprite, sflags) = {
        let s = w.mo(special);
        (s.z, s.sprite, s.flags)
    };
    let (tz, theight, thealth, tplayer) = {
        let t = w.mo(toucher);
        (t.z, t.height, t.health, t.player)
    };
    let delta = sz - tz;
    if delta > theight || delta.0 < -8 * FRACUNIT {
        return; // out of reach
    }
    let mut sound = SFX_ITEMUP;
    // Dead thing touching. Can happen with a sliding player corpse.
    if thealth <= 0 {
        return;
    }
    let Some(player) = tplayer else {
        // Vanilla dereferences NULL here; only players have MF_PICKUP.
        return;
    };
    let deh = w.deh;
    let version = w.params.version;
    let getpow = |s: &mut SfxNum| {
        if version > GameVersion::Doom1_2 {
            *s = SFX_GETPOW;
        }
    };
    let netgame = w.params.netgame;
    let mut msg: Option<&'static str> = None;
    let dropped = sflags & MF_DROPPED != 0;

    macro_rules! key {
        ($card:expr, $text:expr) => {{
            if !w.players[player].cards[$card] {
                msg = Some($text);
            }
            give_card(w, player, $card);
            if netgame {
                if let Some(m) = msg {
                    w.players[player].message = Some(m);
                }
                return;
            }
        }};
    }

    // Identify by sprite.
    match sprite {
        // armor
        SPR_ARM1 => {
            if !give_armor(w, player, deh.green_armor_class) {
                return;
            }
            msg = Some(GOTARMOR);
        }
        SPR_ARM2 => {
            if !give_armor(w, player, deh.blue_armor_class) {
                return;
            }
            msg = Some(GOTMEGA);
        }
        // bonus items
        SPR_BON1 => {
            let p = &mut w.players[player];
            p.health += 1; // can go over 100%
            if p.health > deh.max_health {
                p.health = deh.max_health;
            }
            let h = p.health;
            w.mo_mut(toucher).health = h;
            msg = Some(GOTHTHBONUS);
        }
        SPR_BON2 => {
            let p = &mut w.players[player];
            p.armorpoints += 1; // can go over 100%
            if p.armorpoints > deh.max_armor {
                p.armorpoints = deh.max_armor;
            }
            if p.armortype == 0 {
                p.armortype = 1;
            }
            msg = Some(GOTARMBONUS);
        }
        SPR_SOUL => {
            let p = &mut w.players[player];
            p.health += deh.soulsphere_health;
            if p.health > deh.max_soulsphere {
                p.health = deh.max_soulsphere;
            }
            let h = p.health;
            w.mo_mut(toucher).health = h;
            msg = Some(GOTSUPER);
            getpow(&mut sound);
        }
        SPR_MEGA => {
            if w.params.mode != GameMode::Commercial {
                return;
            }
            w.players[player].health = deh.megasphere_health;
            w.mo_mut(toucher).health = deh.megasphere_health;
            give_armor(w, player, 2);
            msg = Some(GOTMSPHERE);
            getpow(&mut sound);
        }
        // cards (leave cards for everyone)
        SPR_BKEY => key!(0, GOTBLUECARD),
        SPR_YKEY => key!(1, GOTYELWCARD),
        SPR_RKEY => key!(2, GOTREDCARD),
        SPR_BSKU => key!(3, GOTBLUESKUL),
        SPR_YSKU => key!(4, GOTYELWSKUL),
        SPR_RSKU => key!(5, GOTREDSKULL),
        // medikits, heals
        SPR_STIM => {
            if !give_body(w, player, 10) {
                return;
            }
            msg = Some(GOTSTIM);
        }
        SPR_MEDI => {
            if !give_body(w, player, 25) {
                return;
            }
            // Vanilla bug: health is checked after it was given, so the
            // "REALLY need" message only shows below 25 *after* +25.
            msg = Some(if w.players[player].health < 25 { GOTMEDINEED } else { GOTMEDIKIT });
        }
        // power ups
        SPR_PINV => {
            if !give_power(w, player, power::INVULNERABILITY) {
                return;
            }
            msg = Some(GOTINVUL);
            getpow(&mut sound);
        }
        SPR_PSTR => {
            if !give_power(w, player, power::STRENGTH) {
                return;
            }
            msg = Some(GOTBERSERK);
            if w.players[player].readyweapon != Weapon::Fist {
                w.players[player].pendingweapon = Weapon::Fist;
            }
            getpow(&mut sound);
        }
        SPR_PINS => {
            if !give_power(w, player, power::INVISIBILITY) {
                return;
            }
            msg = Some(GOTINVIS);
            getpow(&mut sound);
        }
        SPR_SUIT => {
            if !give_power(w, player, power::IRONFEET) {
                return;
            }
            msg = Some(GOTSUIT);
            getpow(&mut sound);
        }
        SPR_PMAP => {
            if !give_power(w, player, power::ALLMAP) {
                return;
            }
            msg = Some(GOTMAP);
            getpow(&mut sound);
        }
        SPR_PVIS => {
            if !give_power(w, player, power::INFRARED) {
                return;
            }
            msg = Some(GOTVISOR);
            getpow(&mut sound);
        }
        // ammo
        SPR_CLIP => {
            if !give_ammo(w, player, ammo::CLIP, if dropped { 0 } else { 1 }) {
                return;
            }
            msg = Some(GOTCLIP);
        }
        SPR_AMMO => {
            if !give_ammo(w, player, ammo::CLIP, 5) {
                return;
            }
            msg = Some(GOTCLIPBOX);
        }
        SPR_ROCK => {
            if !give_ammo(w, player, ammo::MISL, 1) {
                return;
            }
            msg = Some(GOTROCKET);
        }
        SPR_BROK => {
            if !give_ammo(w, player, ammo::MISL, 5) {
                return;
            }
            msg = Some(GOTROCKBOX);
        }
        SPR_CELL => {
            if !give_ammo(w, player, ammo::CELL, 1) {
                return;
            }
            msg = Some(GOTCELL);
        }
        SPR_CELP => {
            if !give_ammo(w, player, ammo::CELL, 5) {
                return;
            }
            msg = Some(GOTCELLBOX);
        }
        SPR_SHEL => {
            if !give_ammo(w, player, ammo::SHELL, 1) {
                return;
            }
            msg = Some(GOTSHELLS);
        }
        SPR_SBOX => {
            if !give_ammo(w, player, ammo::SHELL, 5) {
                return;
            }
            msg = Some(GOTSHELLBOX);
        }
        SPR_BPAK => {
            if !w.players[player].backpack {
                let p = &mut w.players[player];
                for m in &mut p.maxammo {
                    *m *= 2;
                }
                p.backpack = true;
            }
            for i in 0..NUMAMMO {
                give_ammo(w, player, i, 1);
            }
            msg = Some(GOTBACKPACK);
        }
        // weapons
        SPR_BFUG => {
            if !give_weapon(w, player, Weapon::Bfg, false) {
                return;
            }
            msg = Some(GOTBFG9000);
            sound = SFX_WPNUP;
        }
        SPR_MGUN => {
            if !give_weapon(w, player, Weapon::Chaingun, dropped) {
                return;
            }
            msg = Some(GOTCHAINGUN);
            sound = SFX_WPNUP;
        }
        SPR_CSAW => {
            if !give_weapon(w, player, Weapon::Chainsaw, false) {
                return;
            }
            msg = Some(GOTCHAINSAW);
            sound = SFX_WPNUP;
        }
        SPR_LAUN => {
            if !give_weapon(w, player, Weapon::Missile, false) {
                return;
            }
            msg = Some(GOTLAUNCHER);
            sound = SFX_WPNUP;
        }
        SPR_PLAS => {
            if !give_weapon(w, player, Weapon::Plasma, false) {
                return;
            }
            msg = Some(GOTPLASMA);
            sound = SFX_WPNUP;
        }
        SPR_SHOT => {
            if !give_weapon(w, player, Weapon::Shotgun, dropped) {
                return;
            }
            msg = Some(GOTSHOTGUN);
            sound = SFX_WPNUP;
        }
        SPR_SGN2 => {
            if !give_weapon(w, player, Weapon::SuperShotgun, dropped) {
                return;
            }
            msg = Some(GOTSHOTGUN2);
            sound = SFX_WPNUP;
        }
        _ => {
            // Vanilla: I_Error("P_SpecialThing: Unknown gettable thing").
            // A mod's own pickup (DECORATE/ZScript item): until inventory
            // classes exist, it becomes a named inventory token, which is
            // how scripts test for most mod items.
            let t = w.mo(special).mobj_type;
            let name = w.info.class_name(t).to_string();
            if name.is_empty() {
                w.note_overflow("P_TouchSpecialThing: unknown gettable thing (vanilla I_Error)");
                return;
            }
            if w.info.weapons.contains_key(&t.0) {
                // A mod weapon: own it (switching to it when new) and take
                // its ammo; a weapon already owned with full ammo stays.
                if !crate::weapons::give_weapon(w, player, t, true) {
                    return;
                }
                sound = SFX_WPNUP;
            } else {
                *w.players[player].inventory.entry(name).or_default() += 1;
                sound = SFX_ITEMUP;
            }
        }
    }
    if let Some(m) = msg {
        w.players[player].message = Some(m);
    }
    if sflags & MF_COUNTITEM != 0 {
        w.players[player].itemcount += 1;
    }
    // Hexen-style pickup special (often ACS_Execute), run by the picker.
    let (sp, args) = (w.mo(special).special, w.mo(special).args);
    if sp != 0 {
        let ctx = crate::hexen::SpecialCtx { activator: Some(toucher), line: None, side: 0 };
        crate::hexen::execute(w, sp, args, &ctx);
    }
    remove_mobj(w, special);
    w.players[player].bonuscount += BONUSADD;
    if player == w.params.consoleplayer {
        w.start_sound(SoundSource::None, sound);
    }
}

/// `P_KillMobj`.
pub fn kill_mobj(w: &mut World, source: Option<MobjId>, target: MobjId) {
    {
        let t = w.mo_mut(target);
        t.flags &= !(MF_SHOOTABLE | MF_FLOAT | MF_SKULLFLY);
        if t.mobj_type != MT_SKULL {
            t.flags &= !MF_NOGRAVITY;
        }
        t.flags |= MF_CORPSE | MF_DROPOFF;
        t.height = t.height >> 2;
    }
    let (tflags, tplayer) = (w.mo(target).flags, w.mo(target).player);
    // Hexen-style death special, run by the killer (once).
    let (sp, args) = (w.mo(target).special, w.mo(target).args);
    if sp != 0 {
        w.mo_mut(target).special = 0;
        let ctx = crate::hexen::SpecialCtx { activator: source, line: None, side: 0 };
        crate::hexen::execute(w, sp, args, &ctx);
    }
    if tplayer.is_some() {
        crate::acs_host::start_kind(w, doom_script::acs::ScriptKind::Death, Some(target));
    }
    match w.try_mo(source).and_then(|s| s.player) {
        Some(sp) => {
            // Count for intermission.
            if tflags & MF_COUNTKILL != 0 {
                w.players[sp].killcount += 1;
            }
            if let Some(tp) = tplayer {
                w.players[sp].frags[tp] += 1;
            }
        }
        None => {
            if !w.params.netgame && tflags & MF_COUNTKILL != 0 {
                // Count all monster deaths, even those caused by other
                // monsters.
                w.players[0].killcount += 1;
            }
        }
    }
    if let Some(tp) = tplayer {
        // Count environment kills against you.
        if source.is_none() {
            w.players[tp].frags[tp] += 1;
        }
        w.mo_mut(target).flags &= !MF_SOLID;
        w.players[tp].playerstate = PlayerState::Dead;
        crate::pspr::drop_weapon(w, tp);
        // (AM_Stop when the console player's automap is open is the host's
        // business.)
    }
    let (health, t) = (w.mo(target).health, w.mo(target).mobj_type);
    let info = *w.info.mobj(t);
    if health < -info.spawnhealth && info.xdeathstate != S_NULL {
        set_mobj_state(w, target, info.xdeathstate);
    } else {
        set_mobj_state(w, target, info.deathstate);
    }
    let r = w.p_random() & 3;
    {
        let m = w.mo_mut(target);
        m.tics -= r;
        if m.tics < 1 {
            m.tics = 1;
        }
    }
    // In Chex Quest, monsters don't drop items.
    if w.params.version == GameVersion::Chex {
        return;
    }
    // Drop stuff. This determines the kind of object spawned during the
    // death frame of a thing.
    let item = match t {
        MT_WOLFSS | MT_POSSESSED => MT_CLIP,
        MT_SHOTGUY => MT_SHOTGUN,
        MT_CHAINGUY => MT_CHAINGUN,
        _ => return,
    };
    let (x, y) = (w.mo(target).x, w.mo(target).y);
    let mo = spawn_mobj(w, x, y, ONFLOORZ, item);
    w.mo_mut(mo).flags |= MF_DROPPED; // special versions of items
}

/// `P_DamageMobj`: damages both enemies and players. `inflictor` is the
/// thing that caused the damage (creature or missile); `source` is the thing
/// to target after taking damage (creature or `None`).
pub fn damage_mobj(w: &mut World, target: MobjId, inflictor: Option<MobjId>, source: Option<MobjId>, mut damage: i32) {
    let t = w.mo(target);
    if t.flags & MF_SHOOTABLE == 0 {
        return; // shouldn't happen...
    }
    if t.health <= 0 {
        return;
    }
    if t.flags & MF_SKULLFLY != 0 {
        let m = w.mo_mut(target);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
        m.momz = Fixed(0);
    }
    let player = w.mo(target).player;
    if player.is_some() && w.params.skill == Skill::Baby {
        damage >>= 1; // take half damage in trainer mode
    }

    // Some close combat weapons should not inflict thrust and push the
    // victim out of reach, thus kick away unless using the chainsaw.
    let source_chainsaw = w
        .try_mo(source)
        .and_then(|s| s.player)
        .is_some_and(|sp| w.players[sp].readyweapon == Weapon::Chainsaw);
    if let Some(inf) = w.try_mo(inflictor).map(|i| (i.x, i.y, i.z)) {
        if w.mo(target).flags & MF_NOCLIP == 0 && !source_chainsaw {
            let tm = w.mo(target).clone();
            let mut ang = point_to_angle2(inf.0, inf.1, tm.x, tm.y);
            let mass = w.info.mobj(tm.mobj_type).mass;
            let mut thrust = if mass != 0 {
                damage.wrapping_mul(FRACUNIT >> 3).wrapping_mul(100) / mass
            } else {
                // Vanilla divides by zero (DEHACKED mass 0).
                w.note_overflow("P_DamageMobj: mass 0 (vanilla divide by zero)");
                0
            };
            // Make fall forwards sometimes.
            if damage < 40 && damage > tm.health && (tm.z - inf.2).0 > 64 * FRACUNIT && (w.p_random() & 1) != 0 {
                ang += Angle::A180;
                thrust = thrust.wrapping_mul(4);
            }
            let a = ang.fine();
            let m = w.mo_mut(target);
            m.momx += fixed_mul(Fixed(thrust), finecosine(a));
            m.momy += fixed_mul(Fixed(thrust), finesine(a));
        }
    }

    // Player specific.
    if let Some(p) = player {
        // End of game hell hack.
        let sec = w.level.subsectors[w.mo(target).subsector.idx()].sector;
        if w.level.sector(sec).special == 11 && damage >= w.mo(target).health {
            damage = w.mo(target).health - 1;
        }
        // Below certain threshold, ignore damage in GOD mode, or with
        // INVUL power.
        let pl = &w.players[p];
        if damage < 1000 && (pl.cheats & cheat::GODMODE != 0 || pl.powers[power::INVULNERABILITY] != 0) {
            return;
        }
        let pl = &mut w.players[p];
        if pl.armortype != 0 {
            let mut saved = if pl.armortype == 1 { damage / 3 } else { damage / 2 };
            if pl.armorpoints <= saved {
                // Armor is used up.
                saved = pl.armorpoints;
                pl.armortype = 0;
            }
            pl.armorpoints -= saved;
            damage -= saved;
        }
        pl.health -= damage; // mirror mobj health here for Dave
        if pl.health < 0 {
            pl.health = 0;
        }
        pl.attacker = source;
        pl.damagecount += damage; // add damage after armor / invuln
        if pl.damagecount > 100 {
            pl.damagecount = 100; // teleport stomp does 10k points...
        }
        // (I_Tactile: rumble, presentation only.)
    }

    // Do the damage.
    w.mo_mut(target).health -= damage;
    if w.mo(target).health <= 0 {
        kill_mobj(w, source, target);
        return;
    }

    let tm = w.mo(target).clone();
    let info = *w.info.mobj(tm.mobj_type);
    if w.p_random() < info.painchance && tm.flags & MF_SKULLFLY == 0 {
        w.mo_mut(target).flags |= MF_JUSTHIT; // fight back!
        set_mobj_state(w, target, info.painstate);
    }
    w.mo_mut(target).reactiontime = 0; // we're awake now...

    let tm = w.mo(target).clone();
    let src = w.try_mo(source).map(|s| s.mobj_type);
    if (tm.threshold == 0 || tm.mobj_type == MT_VILE)
        && src.is_some()
        && (source != Some(target) || w.params.version < GameVersion::Doom1_5)
        && src != Some(MT_VILE)
    {
        // If not intent on another player, chase after this one.
        let m = w.mo_mut(target);
        m.target = source;
        m.threshold = BASETHRESHOLD;
        if m.state == info.spawnstate && info.seestate != S_NULL {
            set_mobj_state(w, target, info.seestate);
        }
    }
}
