//! Weapons as classes: mods' `Weapon` classes (DECORATE/ZScript) and
//! Doom's nine, running in the same psprite machinery.
//!
//! A player owns class weapons alongside the vanilla ones
//! (`Player::class_weapons`). The weapon in hand runs a class's states when
//! it is a mod weapon (`Player::ready_class`) or a vanilla weapon whose
//! class a mod replaced (`Info::vanilla_weapon_replaced`); see
//! [`active_class`]. Otherwise it runs `weaponinfo`, exactly as vanilla
//! (demos are untouched).
//!
//! Classes have primary and secondary fire (`Fire`/`Hold`,
//! `AltFire`/`AltHold`) with their own ammo (`Weapon.AmmoType1/2`,
//! `AmmoUse1/2`). Which one is running is `Player::fire_mode`; ammo is
//! taken from that mode's type.
//!
//! The weapon action functions themselves (`A_FireBullets`, ...) live in
//! `crate::actions`, so they work from plain state calls, code blocks and
//! ZScript methods alike. Randomness uses the script RNG, never `P_Random`.

use doom_core::{Angle, Fixed, FRACUNIT};

use crate::hexen::script_random;
use crate::info::*;
use crate::player::Weapon;
use crate::pspr::{set_psprite, PS_FLASH, PS_WEAPON, WEAPONBOTTOM};
use crate::world::{SoundSource, World};

/// What a class weapon uses as ammunition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AmmoRef {
    /// One of Doom's four (clip, shell, cell, rocket).
    Vanilla(usize),
    /// A mod's own ammo, counted in the named inventory.
    Named(String),
}

/// `Weapon.*` properties of a class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponDef {
    /// `Weapon.SlotNumber` (-1 / unset = none: only by pickup or script).
    pub slot: i32,
    /// Primary fire: `Weapon.AmmoType1`, `AmmoUse1`, `AmmoGive1`.
    pub ammo: Option<AmmoRef>,
    pub ammo_use: i32,
    pub ammo_give: i32,
    /// Lower is preferred when switching away from an empty weapon.
    pub selection_order: i32,
    /// Secondary fire: `Weapon.AmmoType2`, `AmmoUse2`, `AmmoGive2`.
    pub ammo2: Option<AmmoRef>,
    pub ammo_use2: i32,
    pub ammo_give2: i32,
}

impl Default for WeaponDef {
    /// No slot, no ammo.
    fn default() -> Self {
        WeaponDef { slot: -1, ammo: None, ammo_use: 0, ammo_give: 0, selection_order: 0, ammo2: None, ammo_use2: 0, ammo_give2: 0 }
    }
}

/// Which attack is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FireMode {
    #[default]
    Primary,
    Alt,
}

/// Which weapon state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Up,
    Down,
    Ready,
    Attack,
    /// Primary refire (`Hold`, else `Fire`).
    Hold,
    AltAttack,
    /// Secondary refire (`AltHold`, else `AltFire`).
    AltHold,
    Flash,
    AltFlash,
}

pub fn def(w: &World, t: MobjType) -> Option<&WeaponDef> {
    w.info.weapons.get(&t.0)
}

/// The class whose states and ammo the weapon in hand uses: the mod weapon
/// that is up, or the class replacing the vanilla weapon that is up.
/// `None`: a plain vanilla weapon (`weaponinfo`).
pub fn active_class(w: &World, player: usize) -> Option<MobjType> {
    let p = &w.players[player];
    p.ready_class.or_else(|| {
        let i = p.readyweapon as usize;
        (i < 9 && w.info.vanilla_weapon_replaced[i]).then(|| w.info.vanilla_weapon_classes[i])
    })
}

/// The state for `which` of the player's current weapon.
pub fn weapon_state(w: &World, player: usize, which: Which) -> StateNum {
    match active_class(w, player) {
        Some(t) => {
            let find = |l: &str| w.info.find_label(t, l);
            let s = match which {
                Which::Up => find("select"),
                Which::Down => find("deselect"),
                Which::Ready => find("ready"),
                Which::Attack => find("fire"),
                Which::Hold => find("hold").or_else(|| find("fire")),
                Which::AltAttack => find("altfire"),
                Which::AltHold => find("althold").or_else(|| find("altfire")),
                Which::Flash => find("flash"),
                Which::AltFlash => find("altflash").or_else(|| find("flash")),
            };
            s.unwrap_or(S_NULL)
        }
        None => {
            let p = &w.players[player];
            let wi = &w.weaponinfo[p.readyweapon as usize];
            match which {
                Which::Up => wi.upstate,
                Which::Down => wi.downstate,
                Which::Ready => wi.readystate,
                Which::Attack | Which::Hold => wi.atkstate,
                Which::Flash | Which::AltFlash => wi.flashstate,
                // Vanilla weapons have no secondary attack.
                Which::AltAttack | Which::AltHold => S_NULL,
            }
        }
    }
}

/// Does the weapon in hand have a secondary attack?
pub fn has_alt_fire(w: &World, player: usize) -> bool {
    active_class(w, player).is_some_and(|t| w.info.find_label(t, "altfire").is_some())
}

fn ammo_count(w: &World, player: usize, a: &AmmoRef) -> i32 {
    let p = &w.players[player];
    match a {
        AmmoRef::Vanilla(i) => p.ammo[*i],
        AmmoRef::Named(n) => p.inventory.get(n).copied().unwrap_or(0),
    }
}

/// (ammo type, amount per use) of a fire mode.
fn mode_ammo(d: &WeaponDef, mode: FireMode) -> (Option<&AmmoRef>, i32) {
    match mode {
        FireMode::Primary => (d.ammo.as_ref(), d.ammo_use),
        FireMode::Alt => (d.ammo2.as_ref(), d.ammo_use2),
    }
}

/// Enough ammo for one use of `mode` of the class weapon in hand?
pub fn has_ammo_for(w: &World, player: usize, mode: FireMode) -> bool {
    let Some(t) = active_class(w, player) else { return true };
    let Some(d) = def(w, t) else { return true };
    let optional = match mode {
        FireMode::Primary => zflags::AMMO_OPTIONAL,
        FireMode::Alt => zflags::ALT_AMMO_OPTIONAL,
    };
    if w.info.has_zflag(t, optional) {
        return true;
    }
    match mode_ammo(d, mode) {
        (Some(a), n) => ammo_count(w, player, a) >= n.max(1),
        (None, _) => true,
    }
}

/// Enough ammo for the attack that is running?
pub fn has_ammo(w: &World, player: usize) -> bool {
    has_ammo_for(w, player, w.players[player].fire_mode)
}

/// `DepleteAmmo` for the attack that is running.
pub fn use_ammo(w: &mut World, player: usize) {
    let mode = w.players[player].fire_mode;
    use_ammo_mode(w, player, mode, None);
}

/// Take `amount` (default: the mode's `AmmoUse`) of a fire mode's ammo.
pub fn use_ammo_mode(w: &mut World, player: usize, mode: FireMode, amount: Option<i32>) {
    let Some(t) = active_class(w, player) else { return };
    let Some(d) = def(w, t).cloned() else { return };
    let (Some(a), n) = mode_ammo(&d, mode) else { return };
    let n = amount.unwrap_or(n);
    let p = &mut w.players[player];
    match a {
        AmmoRef::Vanilla(i) => p.ammo[*i] = (p.ammo[*i] - n).max(0),
        AmmoRef::Named(name) => {
            if let Some(c) = p.inventory.get_mut(name) {
                *c = (*c - n).max(0);
            }
        }
    }
}

/// Give ammo of a type (capped like a pickup for Doom's four).
fn give_ammo_ref(w: &mut World, player: usize, a: &AmmoRef, n: i32) -> bool {
    if n <= 0 {
        return false;
    }
    let p = &mut w.players[player];
    match a {
        AmmoRef::Vanilla(i) => {
            if p.ammo[*i] >= p.maxammo[*i] {
                return false;
            }
            p.ammo[*i] = (p.ammo[*i] + n).min(p.maxammo[*i]);
            true
        }
        AmmoRef::Named(name) => {
            *p.inventory.entry(name.clone()).or_default() += n;
            true
        }
    }
}

/// Give a class weapon (and its `AmmoGive1/2`). With `switch`, a newly
/// owned weapon is brought up. Doom's own weapon classes give the vanilla
/// weapon. Returns whether anything was given.
pub fn give_weapon(w: &mut World, player: usize, t: MobjType, switch: bool) -> bool {
    if let Some(wpn) = w.info.vanilla_weapon_of(t) {
        let gave = crate::inter::give_weapon(w, player, wpn, false);
        if !switch && w.players[player].pendingweapon == wpn {
            w.players[player].pendingweapon = Weapon::NoChange;
        }
        return gave;
    }
    let d = def(w, t).cloned().unwrap_or_default();
    let new = !w.players[player].class_weapons.contains(&t);
    let mut gave = new;
    if let Some(a) = &d.ammo {
        gave |= give_ammo_ref(w, player, a, d.ammo_give);
    }
    if let Some(a) = &d.ammo2 {
        if d.ammo2 != d.ammo || d.ammo_give2 > 0 {
            gave |= give_ammo_ref(w, player, a, d.ammo_give2);
        }
    }
    if new {
        w.players[player].class_weapons.push(t);
        if switch {
            select(w, player, Selection::Class(t));
        }
    }
    gave
}

/// Every mod weapon that has a slot (`IDKFA`/`IDFA`), with ammo.
pub fn give_all(w: &mut World, player: usize) {
    let mut all: Vec<(MobjType, WeaponDef)> = w
        .info
        .weapons
        .iter()
        .filter(|(&t, d)| d.slot >= 0 && d.slot <= 9 && slot_set(d) && w.info.vanilla_weapon_of(MobjType(t)).is_none())
        .map(|(&t, d)| (MobjType(t), d.clone()))
        .collect();
    all.sort_by_key(|(t, _)| t.0);
    for (t, d) in all {
        if !w.players[player].class_weapons.contains(&t) {
            w.players[player].class_weapons.push(t);
        }
        for (a, give) in [(&d.ammo, d.ammo_give), (&d.ammo2, d.ammo_give2)] {
            if let Some(AmmoRef::Named(n)) = a {
                let e = w.players[player].inventory.entry(n.clone()).or_default();
                *e = (*e).max(give.max(1) * 10);
            }
        }
    }
}

/// A class gave `Weapon.SlotNumber` (0 is a real slot; unset is -1).
fn slot_set(d: &WeaponDef) -> bool {
    d.slot >= 0
}

/// A weapon to switch to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Vanilla(Weapon),
    Class(MobjType),
}

/// Ask for a weapon change; the current weapon lowers first.
pub fn select(w: &mut World, player: usize, s: Selection) {
    let p = &mut w.players[player];
    match s {
        Selection::Class(t) => {
            if p.ready_class == Some(t) {
                return;
            }
            p.pending_class = Some(t);
            // Any value other than NoChange makes the current weapon lower.
            p.pendingweapon = p.readyweapon;
        }
        Selection::Vanilla(wp) => {
            if p.ready_class.is_none() && p.readyweapon == wp {
                return;
            }
            p.pending_class = None;
            p.pendingweapon = wp;
        }
    }
}

/// Doom's weapons in each slot (GZDoom's default slot layout).
fn vanilla_slot(slot: u8) -> &'static [Weapon] {
    match slot {
        1 => &[Weapon::Fist, Weapon::Chainsaw],
        2 => &[Weapon::Pistol],
        3 => &[Weapon::Shotgun, Weapon::SuperShotgun],
        4 => &[Weapon::Chaingun],
        5 => &[Weapon::Missile],
        6 => &[Weapon::Plasma],
        7 => &[Weapon::Bfg],
        _ => &[],
    }
}

/// Mod weapons the player owns in `slot`, in pickup order.
fn class_weapons_in_slot(w: &World, player: usize, slot: u8) -> Vec<MobjType> {
    w.players[player].class_weapons.iter().copied().filter(|t| def(w, *t).is_some_and(|d| d.slot == slot as i32)).collect()
}

/// A weapon slot key (`slot` 0..=9). With no mod weapon involved, slots
/// 1-7 follow vanilla's key rules (fist/chainsaw and shotgun/super shotgun
/// preferences, no plasma or BFG in shareware); otherwise the key cycles
/// through the slot's owned weapons, vanilla ones first.
pub fn select_slot(w: &mut World, player: usize, slot: u8) {
    let class = class_weapons_in_slot(w, player, slot);
    if class.is_empty() {
        if (1..=7).contains(&slot) {
            vanilla_change(w, player, slot - 1);
        }
        return;
    }
    let p = &w.players[player];
    let shareware = w.params.mode == doom_content::game::GameMode::Shareware;
    let ssg_ok = w.params.mode == doom_content::game::GameMode::Commercial;
    let mut cands: Vec<Selection> = vanilla_slot(slot)
        .iter()
        .filter(|&&wp| p.weaponowned[wp as usize])
        .filter(|&&wp| wp != Weapon::SuperShotgun || ssg_ok)
        .filter(|&&wp| !(shareware && matches!(wp, Weapon::Plasma | Weapon::Bfg)))
        .map(|&wp| Selection::Vanilla(wp))
        .collect();
    cands.extend(class.into_iter().map(Selection::Class));
    if cands.is_empty() {
        return;
    }
    let current = match p.pending_class.or(p.ready_class) {
        Some(t) => Selection::Class(t),
        None if p.pendingweapon != Weapon::NoChange => Selection::Vanilla(p.pendingweapon),
        None => Selection::Vanilla(p.readyweapon),
    };
    let next = match cands.iter().position(|c| *c == current) {
        Some(i) => cands[(i + 1) % cands.len()],
        None => cands[0],
    };
    select(w, player, next);
}

/// Vanilla's weapon change for `BT_CHANGE` with weapon index `index`
/// (`P_PlayerThink`): the rules keys 1-7 have in Doom.
pub fn vanilla_change(w: &mut World, player: usize, index: u8) {
    let mode = w.params.mode;
    let p = &mut w.players[player];
    let mut newweapon = Weapon::from_index(index);
    if newweapon == Weapon::Fist && p.weaponowned[Weapon::Chainsaw as usize] && !(p.readyweapon == Weapon::Chainsaw && p.powers[crate::player::power::STRENGTH] != 0) {
        newweapon = Weapon::Chainsaw;
    }
    if mode == doom_content::game::GameMode::Commercial
        && newweapon == Weapon::Shotgun
        && p.weaponowned[Weapon::SuperShotgun as usize]
        && p.readyweapon != Weapon::SuperShotgun
    {
        newweapon = Weapon::SuperShotgun;
    }
    // `weaponowned[newweapon]`: slot 8 (wp_nochange) can't be selected
    // from a 3-bit field, so the index is always in range.
    let current_is_vanilla = p.ready_class.is_none();
    if (newweapon as usize) < p.weaponowned.len() && p.weaponowned[newweapon as usize] && (newweapon != p.readyweapon || !current_is_vanilla) {
        // Do not go to plasma or BFG in shareware, even if cheated.
        if (newweapon != Weapon::Plasma && newweapon != Weapon::Bfg) || mode != doom_content::game::GameMode::Shareware {
            p.pending_class = None;
            p.pendingweapon = newweapon;
        }
    }
}

/// Weapon slot key `slot` (1-based) given through vanilla's `BT_CHANGE`
/// (demos and old callers): mod weapons cycle like [`select_slot`].
/// Returns false to leave the key to vanilla handling.
pub fn slot_key(w: &mut World, player: usize, slot: i32) -> bool {
    let s = slot as u8;
    if class_weapons_in_slot(w, player, s).is_empty() && w.players[player].ready_class.is_none() {
        // Plain vanilla: P_PlayerThink's own code runs.
        return false;
    }
    select_slot(w, player, s);
    true
}

/// Weapon priority: the owned weapon with ammo and the lowest
/// `SelectionOrder` (vanilla weapons use GZDoom's orders), other than the
/// one in hand. Used when a class weapon runs dry.
pub fn best_weapon(w: &World, player: usize) -> Option<Selection> {
    let p = &w.players[player];
    let shareware = w.params.mode == doom_content::game::GameMode::Shareware;
    let mut best: Option<(i32, Selection)> = None;
    let mut consider = |order: i32, s: Selection| {
        if best.is_none_or(|(o, _)| order < o) {
            best = Some((order, s));
        }
    };
    for (i, &t) in w.info.vanilla_weapon_classes.iter().enumerate() {
        let wp = Weapon::from_index(i as u8);
        if !p.weaponowned[i] || (shareware && matches!(wp, Weapon::Plasma | Weapon::Bfg)) {
            continue;
        }
        if p.ready_class.is_none() && p.readyweapon == wp {
            continue;
        }
        let Some(d) = def(w, t) else { continue };
        let ok = match &d.ammo {
            Some(a) => ammo_count(w, player, a) >= d.ammo_use.max(1),
            None => true,
        };
        if ok {
            consider(d.selection_order, Selection::Vanilla(wp));
        }
    }
    for &t in &p.class_weapons {
        if p.ready_class == Some(t) {
            continue;
        }
        let Some(d) = def(w, t) else { continue };
        let ok = match &d.ammo {
            Some(a) => ammo_count(w, player, a) >= d.ammo_use.max(1),
            None => true,
        };
        if ok {
            consider(d.selection_order, Selection::Class(t));
        }
    }
    best.map(|(_, s)| s)
}

/// Bring up the pending class weapon (`P_BringUpWeapon` for classes).
pub fn bring_up_class(w: &mut World, player: usize) {
    let p = &mut w.players[player];
    p.ready_class = p.pending_class.take();
    p.pendingweapon = Weapon::NoChange;
    p.psprites[PS_WEAPON].sy = WEAPONBOTTOM;
    let up = weapon_state(w, player, Which::Up);
    set_psprite(w, player, PS_WEAPON, up);
}

// ---- helpers for the weapon action functions (crate::actions) ------------------------

pub(crate) fn crandom(w: &mut World) -> f64 {
    (script_random(w) as f64 - 127.5) / 127.5
}

pub(crate) fn deg_to_bam(d: f64) -> Angle {
    Angle((d.rem_euclid(360.0) / 360.0 * 4294967296.0) as u64 as u32)
}

/// `A_GunFlash`: the flash psprite (and the player's attack frame).
pub fn gun_flash(w: &mut World, player: usize, mo: crate::ids::MobjId, label: Option<String>) {
    crate::mobj::set_mobj_state(w, mo, S_PLAY_ATK2);
    let alt = w.players[player].fire_mode == FireMode::Alt;
    let st = match (label, active_class(w, player)) {
        (Some(l), Some(t)) => w.info.find_label(t, &l).unwrap_or(S_NULL),
        _ => weapon_state(w, player, if alt { Which::AltFlash } else { Which::Flash }),
    };
    set_psprite(w, player, PS_FLASH, st);
}

/// The attack sound of the weapon in hand (`AttackSound`), if any.
pub(crate) fn attack_sound(w: &World, player: usize) -> SfxNum {
    active_class(w, player).map(|t| w.info.mobj(t).attacksound).unwrap_or(SfxNum(0))
}

/// `A_FireBullets`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire_bullets(w: &mut World, player: usize, spread_xy: f64, spread_z: f64, n: i32, damage: i32, flags: i32, range: f64, puff: Option<MobjType>) {
    const FBF_USEAMMO: i32 = 1;
    const FBF_NORANDOM: i32 = 2;
    const FBF_NOPITCH: i32 = 8;
    const FBF_NOFLASH: i32 = 16;
    let Some(mo) = w.players[player].mo else { return };
    if flags & FBF_USEAMMO != 0 {
        use_ammo(w, player);
    }
    let attack = attack_sound(w, player);
    w.start_sound(SoundSource::Mobj(mo), attack);
    if flags & FBF_NOFLASH == 0 {
        crate::mobj::set_mobj_state(w, mo, S_PLAY_ATK2);
    }
    let range = Fixed(((if range <= 0.0 { 8192.0 } else { range }) * FRACUNIT as f64) as i32);
    let base = w.mo(mo).angle;
    let slope = if flags & FBF_NOPITCH != 0 { Fixed(0) } else { crate::map::aim_line_attack(w, Some(mo), base, Fixed(16 * 64 * FRACUNIT)) };
    let roll = |w: &mut World| if flags & FBF_NORANDOM != 0 { damage } else { damage * ((script_random(w) % 3) + 1) };
    if n == -1 || (n == 1 && w.players[player].refire == 0) {
        let d = roll(w);
        crate::map::line_attack_puff(w, mo, base, range, slope, d, puff);
        return;
    }
    for _ in 0..n.max(1) {
        let a = base + deg_to_bam(spread_xy * crandom(w));
        let s = slope + Fixed((spread_z * crandom(w) / 64.0 * FRACUNIT as f64) as i32);
        let d = roll(w);
        crate::map::line_attack_puff(w, mo, a, range, s, d, puff);
    }
}

/// `Weapon.*` properties from a class's defaults: (lower-case name, first
/// value as text, last value as a number), inheriting `parent`'s.
pub fn def_from_properties(parent: Option<&WeaponDef>, props: &[(String, Option<String>, Option<f64>)]) -> Option<WeaponDef> {
    let mut d = parent.cloned();
    let ammo_ref = |text: &Option<String>| {
        let t = text.clone().unwrap_or_default().to_ascii_lowercase();
        match t.as_str() {
            "" | "none" => None,
            "clip" | "shell" | "cell" | "rocketammo" => crate::inventory::ammo(&t).map(|(i, _)| AmmoRef::Vanilla(i)),
            _ => Some(AmmoRef::Named(t)),
        }
    };
    for (name, text, num) in props {
        if !name.starts_with("weapon.") {
            continue;
        }
        let dd = d.get_or_insert_with(WeaponDef::default);
        let n = num.unwrap_or(0.0) as i32;
        match name.as_str() {
            "weapon.slotnumber" => dd.slot = n,
            "weapon.ammouse" | "weapon.ammouse1" => dd.ammo_use = n,
            "weapon.ammogive" | "weapon.ammogive1" => dd.ammo_give = n,
            "weapon.selectionorder" => dd.selection_order = n,
            "weapon.ammotype" | "weapon.ammotype1" => dd.ammo = ammo_ref(text),
            "weapon.ammouse2" => dd.ammo_use2 = n,
            "weapon.ammogive2" => dd.ammo_give2 = n,
            "weapon.ammotype2" => dd.ammo2 = ammo_ref(text),
            _ => {}
        }
    }
    d
}

/// The `Weapon.*` properties this engine uses (others are presentation).
pub const KNOWN_PROPERTIES: &[&str] = &[
    "weapon.slotnumber",
    "weapon.ammotype",
    "weapon.ammotype1",
    "weapon.ammouse",
    "weapon.ammouse1",
    "weapon.ammogive",
    "weapon.ammogive1",
    "weapon.selectionorder",
    "weapon.ammotype2",
    "weapon.ammouse2",
    "weapon.ammogive2",
];

/// The current weapon's ammo as (count, maximum), for the status bar;
/// `None` for weapons that use none.
pub fn ready_ammo(w: &World, player: usize) -> Option<(i32, i32)> {
    let p = &w.players[player];
    match active_class(w, player).and_then(|t| def(w, t)).map(|d| d.ammo.clone()) {
        Some(Some(AmmoRef::Vanilla(i))) => Some((p.ammo[i], p.maxammo[i])),
        Some(Some(a @ AmmoRef::Named(_))) => Some((ammo_count(w, player, &a), 999)),
        Some(None) => None,
        None => {
            let t = w.weaponinfo.get(p.readyweapon as usize).map_or(usize::MAX, |wi| wi.ammo);
            (t < 4).then(|| (p.ammo[t], p.maxammo[t]))
        }
    }
}
