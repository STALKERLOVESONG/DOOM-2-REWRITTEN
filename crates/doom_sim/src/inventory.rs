//! Items by class name, for scripts and mod actions: Doom's ammo, keys,
//! weapons and health map onto the player's fields; anything else (mod
//! tokens, keys, quest items) is counted by name, on players and monsters
//! alike.

use crate::ids::MobjId;
use crate::player::Weapon;
use crate::world::World;

const AMMO: &[(&str, usize, i32)] = &[
    ("clip", 0, 10),
    ("clipbox", 0, 50),
    ("shell", 1, 4),
    ("shellbox", 1, 20),
    ("cell", 2, 20),
    ("cellpack", 2, 100),
    ("rocketammo", 3, 1),
    ("rocketbox", 3, 5),
];

const KEYS: &[&str] = &["bluecard", "yellowcard", "redcard", "blueskull", "yellowskull", "redskull"];

pub const WEAPONS: &[(&str, Weapon)] = &[
    ("fist", Weapon::Fist),
    ("pistol", Weapon::Pistol),
    ("shotgun", Weapon::Shotgun),
    ("chaingun", Weapon::Chaingun),
    ("rocketlauncher", Weapon::Missile),
    ("plasmarifle", Weapon::Plasma),
    ("bfg9000", Weapon::Bfg),
    ("chainsaw", Weapon::Chainsaw),
    ("supershotgun", Weapon::SuperShotgun),
];

/// Ammo type index and units per item for an ammo class name.
pub fn ammo(name: &str) -> Option<(usize, i32)> {
    AMMO.iter().find(|(a, ..)| a.eq_ignore_ascii_case(name)).map(|&(_, i, u)| (i, u))
}

pub fn give(w: &mut World, mo: MobjId, name: &str, amount: i32) {
    let n = name.to_ascii_lowercase();
    let Some(p) = w.mo(mo).player else {
        *w.mo_mut(mo).inventory.entry(n).or_default() += amount;
        return;
    };
    let pl = &mut w.players[p];
    if let Some((ammo, unit)) = ammo(&n) {
        pl.ammo[ammo] = (pl.ammo[ammo] + unit * amount).min(pl.maxammo[ammo]);
    } else if let Some(k) = KEYS.iter().position(|k| *k == n) {
        pl.cards[k] = true;
    } else if let Some(&(_, wpn)) = WEAPONS.iter().find(|(x, _)| *x == n) {
        pl.weaponowned[wpn as usize] = true;
    } else if n == "health" {
        pl.health += amount;
        let h = pl.health;
        w.mo_mut(mo).health = h;
    } else if n == "backpack" {
        if !pl.backpack {
            pl.maxammo = pl.maxammo.map(|m| m * 2);
            pl.backpack = true;
        }
    } else {
        *pl.inventory.entry(n).or_default() += amount;
    }
}

pub fn take(w: &mut World, mo: MobjId, name: &str, amount: i32) {
    let n = name.to_ascii_lowercase();
    let take_named = |inv: &mut std::collections::BTreeMap<String, i32>| {
        if let Some(c) = inv.get_mut(&n) {
            // 0 takes everything, as in ZDoom.
            *c -= if amount == 0 { *c } else { amount };
            if *c <= 0 {
                inv.remove(&n);
            }
        }
    };
    let Some(p) = w.mo(mo).player else {
        take_named(&mut w.mo_mut(mo).inventory);
        return;
    };
    let pl = &mut w.players[p];
    if let Some((ammo, unit)) = ammo(&n) {
        pl.ammo[ammo] = (pl.ammo[ammo] - unit * amount).max(0);
    } else if let Some(k) = KEYS.iter().position(|k| *k == n) {
        pl.cards[k] = false;
    } else if let Some(&(_, wpn)) = WEAPONS.iter().find(|(x, _)| *x == n) {
        if pl.readyweapon != wpn {
            pl.weaponowned[wpn as usize] = false;
        }
    } else {
        take_named(&mut pl.inventory);
    }
}

pub fn count(w: &World, mo: MobjId, name: &str) -> i32 {
    let n = name.to_ascii_lowercase();
    let m = w.mo(mo);
    let Some(p) = m.player else {
        return m.inventory.get(&n).copied().unwrap_or(0);
    };
    let pl = &w.players[p];
    if let Some((ammo, _)) = ammo(&n) {
        pl.ammo[ammo]
    } else if let Some(k) = KEYS.iter().position(|k| *k == n) {
        pl.cards[k] as i32
    } else if let Some(&(_, wpn)) = WEAPONS.iter().find(|(x, _)| *x == n) {
        pl.weaponowned[wpn as usize] as i32
    } else if n == "health" {
        pl.health
    } else if n == "armor" || n == "basicarmor" {
        pl.armorpoints
    } else {
        pl.inventory.get(&n).copied().unwrap_or(0)
    }
}

pub fn clear(w: &mut World, mo: MobjId) {
    match w.mo(mo).player {
        Some(p) => w.players[p].inventory.clear(),
        None => w.mo_mut(mo).inventory.clear(),
    }
}
