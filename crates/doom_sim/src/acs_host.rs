//! The playsim as an ACS host: scripts run inside [`World::tick`] and act on
//! the world through the same specials and helpers the map uses.
//!
//! The runtime lives in [`World::acs`]; while it runs it is taken out of the
//! world (so the world can be lent to it as the host), and anything that
//! wants to start a script meanwhile queues a request in
//! [`World::acs_requests`].

use doom_content::map::{ml, LineAction};
use doom_core::{Angle, Fixed};
use doom_script::acs::{AcsHost, AcsRuntime, Activation, Builtin, Module, PrintKind, ScriptKind, ScriptRequest, Strings};

use crate::hexen::{self, actor_from_handle, actor_handle, mobjs_by_tid, SpecialCtx};
use crate::ids::{FlatNum, LineId, MobjId, SectorId, TexNum};
use crate::level::HEXEN_LINE;
use crate::mobj::flags::*;
use crate::player::{Weapon, MAXPLAYERS};
use crate::world::{ExitTarget, GameExit, ScriptEvent, World};

/// Load a map's ACS: its BEHAVIOR (if any) and every library named in
/// `LOADACS` lumps, plus libraries those modules `#import`.
pub fn load_modules(stack: &doom_content::ResourceStack, map: &str, behavior: Option<Vec<u8>>, warnings: &mut Vec<String>) -> (Option<Module>, Vec<Module>) {
    let map_module = behavior.filter(|b| Module::is_acs(b)).and_then(|b| match Module::load(map, b) {
        Ok(m) => Some(m),
        Err(e) => {
            warnings.push(format!("ACS: {e}"));
            None
        }
    });
    let find = |name: &str| -> Option<Module> {
        let lump = doom_content::LumpName::new(name);
        let id = stack.find_in(doom_content::Namespace::Acs, lump).or_else(|| stack.check_num(lump))?;
        Module::load(name, stack.data(id).to_vec()).ok()
    };
    let mut libs: Vec<Module> = Vec::new();
    let mut wanted: Vec<String> = Vec::new();
    for &id in stack.all_named("LOADACS") {
        let text = String::from_utf8_lossy(stack.data(id)).into_owned();
        wanted.extend(text.split_whitespace().filter(|w| !w.starts_with("//")).map(str::to_string));
    }
    wanted.extend(map_module.iter().flat_map(|m| m.libraries.clone()));
    let mut i = 0;
    while i < wanted.len() {
        let name = wanted[i].clone();
        i += 1;
        if libs.iter().any(|l| l.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        match find(&name) {
            Some(m) => {
                wanted.extend(m.libraries.clone());
                libs.push(m);
            }
            None => warnings.push(format!("ACS: library {name} not found")),
        }
    }
    (map_module, libs)
}

/// The level number scripts use for `ACS_Execute(script, map)`.
fn level_number(w: &World) -> i32 {
    if w.params.mode == doom_content::game::GameMode::Commercial {
        w.params.map
    } else {
        (w.params.episode - 1) * 10 + w.params.map
    }
}

/// Install the level's scripts (keeping world/global variables) and start
/// OPEN and ENTER scripts. Called by `setup_level` once things exist.
pub fn start_level(w: &mut World) {
    let globals = match w.acs.take() {
        Some(old) => old.globals,
        None => std::mem::take(&mut w.acs_globals),
    };
    let (map, libs) = (w.level.acs_map.take(), std::mem::take(&mut w.level.acs_libs));
    if map.is_none() && libs.is_empty() {
        w.acs_globals = globals;
        return;
    }
    let mut rt = AcsRuntime::new(map, libs, |_| None, globals);
    rt.map_number = level_number(w);
    for warning in rt.warnings.drain(..) {
        w.load_warnings.push(format!("ACS: {warning}"));
    }
    rt.run_deferred();
    rt.start_kind(ScriptKind::Open, Activation::default(), &[]);
    for p in 0..MAXPLAYERS {
        if let (true, Some(mo)) = (w.playeringame[p], w.players[p].mo) {
            rt.start_kind(ScriptKind::Enter, Activation { activator: Some(actor_handle(mo)), ..Default::default() }, &[]);
        }
    }
    w.acs = Some(Box::new(rt));
}

/// Start every script of a kind (DEATH, RESPAWN...) with an activator.
pub fn start_kind(w: &mut World, kind: ScriptKind, activator: Option<MobjId>) {
    let act = Activation { activator: activator.map(actor_handle), ..Default::default() };
    // While scripts run the runtime is lent out; such starts (a script
    // killing the player) are dropped.
    if let Some(rt) = w.acs.as_mut() {
        rt.start_kind(kind, act, &[]);
    }
}

/// One tic of scripts.
pub fn tick(w: &mut World) {
    let Some(mut rt) = w.acs.take() else { return };
    rt.tick(w);
    for (what, n) in std::mem::take(&mut rt.unimplemented) {
        *w.unimplemented.entry(leak_name(what)).or_default() += n;
    }
    for warning in rt.warnings.drain(..) {
        if w.load_warnings.len() < 200 {
            w.load_warnings.push(format!("ACS: {warning}"));
        }
    }
    // Requests made while the runtime was out.
    for r in std::mem::take(&mut w.acs_requests) {
        rt.request(&r);
    }
    w.acs = Some(rt);
}

/// `World::unimplemented` keys are `&'static str`; script diagnostics are
/// few and repetitive, so intern them.
fn leak_name(s: String) -> &'static str {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut g = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    if let Some(&n) = set.get(s.as_str()) {
        return n;
    }
    let n: &'static str = Box::leak(s.into_boxed_str());
    set.insert(n);
    n
}

// ---- inventory by class name ----------------------------------------------------

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

const WEAPONS: &[(&str, Weapon)] = &[
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

fn give_item(w: &mut World, p: usize, name: &str, amount: i32) {
    let n = name.to_ascii_lowercase();
    let pl = &mut w.players[p];
    if let Some(&(_, ammo, unit)) = AMMO.iter().find(|(a, ..)| *a == n) {
        pl.ammo[ammo] = (pl.ammo[ammo] + unit * amount).min(pl.maxammo[ammo]);
    } else if let Some(k) = KEYS.iter().position(|k| *k == n) {
        pl.cards[k] = true;
    } else if let Some(&(_, wpn)) = WEAPONS.iter().find(|(x, _)| *x == n) {
        pl.weaponowned[wpn as usize] = true;
    } else if n == "health" {
        pl.health += amount;
        let h = pl.health;
        if let Some(mo) = pl.mo {
            w.mo_mut(mo).health = h;
        }
    } else if n == "backpack" {
        if !pl.backpack {
            pl.maxammo = pl.maxammo.map(|m| m * 2);
            pl.backpack = true;
        }
    } else {
        *pl.inventory.entry(n).or_default() += amount;
    }
}

fn take_item(w: &mut World, p: usize, name: &str, amount: i32) {
    let n = name.to_ascii_lowercase();
    let pl = &mut w.players[p];
    if let Some(&(_, ammo, unit)) = AMMO.iter().find(|(a, ..)| *a == n) {
        pl.ammo[ammo] = (pl.ammo[ammo] - unit * amount).max(0);
    } else if let Some(k) = KEYS.iter().position(|k| *k == n) {
        pl.cards[k] = false;
    } else if let Some(&(_, wpn)) = WEAPONS.iter().find(|(x, _)| *x == n) {
        if pl.readyweapon != wpn {
            pl.weaponowned[wpn as usize] = false;
        }
    } else if let Some(c) = pl.inventory.get_mut(&n) {
        *c -= amount;
        if *c <= 0 {
            pl.inventory.remove(&n);
        }
    }
}

fn count_item(w: &World, p: usize, name: &str) -> i32 {
    let n = name.to_ascii_lowercase();
    let pl = &w.players[p];
    if let Some(&(_, ammo, _)) = AMMO.iter().find(|(a, ..)| *a == n) {
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

// ---- the host -----------------------------------------------------------------------

impl World {
    fn act_mo(&self, act: &Activation) -> Option<MobjId> {
        act.activator.and_then(|h| actor_from_handle(self, h))
    }

    fn act_player(&self, act: &Activation) -> Option<usize> {
        self.act_mo(act).and_then(|m| self.try_mo(Some(m))).and_then(|m| m.player)
    }

    /// Players an inventory function applies to: the activator's, or every
    /// player when there is none (OPEN scripts).
    fn inventory_players(&self, act: &Activation) -> Vec<usize> {
        match self.act_player(act) {
            Some(p) => vec![p],
            None if act.activator.is_none() => (0..MAXPLAYERS).filter(|&p| self.playeringame[p]).collect(),
            None => vec![],
        }
    }

    fn tid_targets(&self, tid: i32, act: &Activation) -> Vec<MobjId> {
        if tid == 0 {
            self.act_mo(act).into_iter().collect()
        } else {
            mobjs_by_tid(self, tid)
        }
    }

    fn ctx(&self, act: &Activation) -> SpecialCtx {
        SpecialCtx { activator: self.act_mo(act), line: act.line.map(LineId), side: act.side as usize }
    }

    fn sector_of(&self, m: MobjId) -> SectorId {
        self.level.subsectors[self.mo(m).subsector.idx()].sector
    }

    /// `ThingCount`-style: live actors of a class (or any) with a TID (or
    /// any), optionally in tagged sectors.
    fn count_things(&self, class: Option<crate::info::MobjType>, any_class: bool, tid: i32, tag: Option<i32>) -> i32 {
        self.thinkers
            .mobjs()
            .filter(|(id, m)| {
                self.thinkers.func(*id) == Some(crate::thinker::ThinkFn::Active)
                    && (tid == 0 || m.tid == tid)
                    && (any_class || Some(m.mobj_type) == class)
                    && !(m.flags & MF_COUNTKILL != 0 && m.health <= 0)
                    && tag.is_none_or(|t| self.level.sector(self.level.subsectors[m.subsector.idx()].sector).tag as i32 == t)
            })
            .count() as i32
    }

    fn spawn_named(&mut self, class: &str, x: Fixed, y: Fixed, z: Fixed, tid: i32, angle: Angle) -> i32 {
        let Some(t) = self.info.type_by_name(class) else {
            self.note_unimplemented("ACS Spawn: unknown class");
            return 0;
        };
        let fog = false;
        hexen::spawn_at(self, t, x, y, z, angle, tid, fog).is_some() as i32
    }
}

fn byte_angle(a: i32) -> Angle {
    Angle((a as u32 & 0xff) << 24)
}

fn fixed_angle(a: i32) -> Angle {
    Angle((a as u32) << 16)
}

impl AcsHost for World {
    fn line_special(&mut self, act: &Activation, special: i32, args: [i32; 5], _strings: &Strings) -> i32 {
        let ctx = self.ctx(act);
        hexen::execute(self, special, args, &ctx) as i32
    }

    fn tag_busy(&self, tag: i32) -> bool {
        hexen::sectors_by_tag(self, tag).iter().any(|&s| {
            let sec = self.level.sector(s);
            sec.special_data.is_some() || sec.ceiling_data.is_some()
        })
    }

    fn random(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (max as i64 - min as i64 + 1) as u64;
        let r = ((hexen::script_random(self) as u64) << 16 | (hexen::script_random(self) as u64) << 8 | hexen::script_random(self) as u64) % span;
        (min as i64 + r as i64) as i32
    }

    fn level_time(&self) -> i32 {
        self.leveltime
    }

    fn print(&mut self, act: &Activation, kind: PrintKind, text: &str) {
        let player = match kind {
            PrintKind::Bold | PrintKind::HudMessage { bold: true, .. } => None,
            PrintKind::Log => return,
            _ => self.act_player(act).or(Some(self.params.consoleplayer)),
        };
        if !text.is_empty() {
            self.script_events.push(ScriptEvent::Print { player, text: text.to_string() });
        }
    }

    fn take_requests(&mut self) -> Vec<ScriptRequest> {
        std::mem::take(&mut self.acs_requests)
    }

    fn builtin(&mut self, act: &mut Activation, f: Builtin, a: &[i32], strings: &mut Strings) -> i32 {
        let arg = |i: usize| a.get(i).copied().unwrap_or(0);
        let s = |i: usize| strings.get(arg(i)).to_string();
        use Builtin as B;
        match f {
            // ---- things ----
            B::ThingCount => {
                let class = hexen::spawn_id_type(self, arg(0));
                self.count_things(class, arg(0) == 0, arg(1), None)
            }
            B::ThingCountName => {
                let class = self.info.type_by_name(&s(0));
                if class.is_none() {
                    return 0;
                }
                self.count_things(class, false, arg(1), None)
            }
            B::ThingCountSector => self.count_things(hexen::spawn_id_type(self, arg(0)), arg(0) == 0, arg(1), Some(arg(2))),
            B::ThingCountNameSector => self.count_things(self.info.type_by_name(&s(0)), false, arg(1), Some(arg(2))),
            B::Spawn => self.spawn_named(&s(0), Fixed(arg(1)), Fixed(arg(2)), Fixed(arg(3)), arg(4), byte_angle(arg(5))),
            B::SpawnSpot | B::SpawnSpotFacing => {
                let class = s(0);
                let mut n = 0;
                for spot in self.tid_targets(arg(1), act) {
                    let m = self.mo(spot);
                    let (x, y, z, sa) = (m.x, m.y, m.z, m.angle);
                    let angle = if f == B::SpawnSpotFacing { sa } else { byte_angle(arg(3)) };
                    n += self.spawn_named(&class, x, y, z, arg(2), angle);
                }
                n
            }
            B::SetThingSpecial => {
                for t in self.tid_targets(arg(0), act) {
                    let m = self.mo_mut(t);
                    m.special = arg(1);
                    m.args = [arg(2), arg(3), arg(4), arg(5), arg(6)];
                }
                0
            }
            B::ThingDamage2 => {
                let targets = self.tid_targets(arg(0), act);
                let n = targets.len() as i32;
                for t in targets {
                    if self.mo(t).health > 0 {
                        crate::inter::damage_mobj(self, t, None, None, arg(1));
                    }
                }
                n
            }
            B::ActivatorTid => self.act_mo(act).map_or(0, |m| self.mo(m).tid),
            B::GetActorX | B::GetActorY | B::GetActorZ | B::GetActorFloorZ | B::GetActorCeilingZ | B::GetActorAngle | B::GetActorLightLevel => {
                let Some(&t) = self.tid_targets(arg(0), act).first() else { return 0 };
                let m = self.mo(t);
                match f {
                    B::GetActorX => m.x.0,
                    B::GetActorY => m.y.0,
                    B::GetActorZ => m.z.0,
                    B::GetActorFloorZ => m.floorz.0,
                    B::GetActorCeilingZ => m.ceilingz.0,
                    B::GetActorAngle => (m.angle.0 >> 16) as i32,
                    _ => self.level.sector(self.sector_of(t)).light_level as i32,
                }
            }
            B::GetActorPitch => 0,
            B::SetActorPosition => {
                let Some(&t) = self.tid_targets(arg(0), act).first() else { return 0 };
                let an = self.mo(t).angle;
                let ok = hexen::teleport_thing(self, t, Fixed(arg(1)), Fixed(arg(2)), an, arg(4) != 0, true);
                if ok {
                    self.mo_mut(t).z = Fixed(arg(3));
                }
                ok as i32
            }
            B::SetActorAngle => {
                for t in self.tid_targets(arg(0), act) {
                    self.mo_mut(t).angle = fixed_angle(arg(1));
                }
                0
            }
            B::SetActorPitch => 0,
            B::SetActorState => {
                let label = s(1).to_ascii_lowercase();
                let mut n = 0;
                for t in self.tid_targets(arg(0), act) {
                    let mi = *self.info.mobj(self.mo(t).mobj_type);
                    let state = match label.as_str() {
                        "spawn" => mi.spawnstate,
                        "see" => mi.seestate,
                        "pain" => mi.painstate,
                        "melee" => mi.meleestate,
                        "missile" => mi.missilestate,
                        "death" => mi.deathstate,
                        "xdeath" => mi.xdeathstate,
                        "raise" => mi.raisestate,
                        _ => {
                            self.note_unimplemented("ACS SetActorState: custom state labels");
                            continue;
                        }
                    };
                    if state.0 != 0 {
                        crate::mobj::set_mobj_state(self, t, state);
                        n += 1;
                    }
                }
                n
            }
            B::ClassifyActor => {
                let Some(&t) = self.tid_targets(arg(0), act).first() else { return if arg(0) == 0 { 1 } else { 0 } };
                let m = self.mo(t);
                let mut c = 256;
                if m.player.is_some() {
                    c |= 2;
                }
                if m.flags & MF_COUNTKILL != 0 {
                    c |= 16;
                }
                if m.flags & MF_MISSILE != 0 {
                    c |= 128;
                }
                c | if m.health > 0 { 32 } else { 64 }
            }
            // ---- inventory ----
            B::GiveInventory | B::TakeInventory => {
                let (item, amount) = (s(0), arg(1));
                for p in self.inventory_players(act) {
                    if f == B::GiveInventory {
                        give_item(self, p, &item, amount);
                    } else {
                        take_item(self, p, &item, amount);
                    }
                }
                0
            }
            B::CheckInventory => self.act_player(act).map_or(0, |p| count_item(self, p, &s(0))),
            B::ClearInventory => {
                for p in self.inventory_players(act) {
                    self.players[p].inventory.clear();
                }
                0
            }
            B::GiveActorInventory | B::TakeActorInventory | B::ClearActorInventory | B::CheckActorInventory => {
                let players: Vec<usize> = self.tid_targets(arg(0), act).into_iter().filter_map(|t| self.mo(t).player).collect();
                let (item, amount) = (s(1), arg(2));
                match f {
                    B::CheckActorInventory => players.first().map_or(0, |&p| count_item(self, p, &item)),
                    _ => {
                        for p in players {
                            match f {
                                B::GiveActorInventory => give_item(self, p, &item, amount),
                                B::TakeActorInventory => take_item(self, p, &item, amount),
                                _ => self.players[p].inventory.clear(),
                            }
                        }
                        0
                    }
                }
            }
            B::UseInventory | B::UseActorInventory => 0,
            B::CheckWeapon => {
                let name = s(0).to_ascii_lowercase();
                self.act_player(act).map_or(0, |p| WEAPONS.iter().any(|(n, wpn)| *n == name && self.players[p].readyweapon == *wpn) as i32)
            }
            B::SetWeapon => {
                let name = s(0).to_ascii_lowercase();
                let Some(p) = self.act_player(act) else { return 0 };
                match WEAPONS.iter().find(|(n, _)| *n == name) {
                    Some(&(_, wpn)) if self.players[p].weaponowned[wpn as usize] => {
                        self.players[p].pendingweapon = wpn;
                        1
                    }
                    _ => 0,
                }
            }
            B::GetAmmoCapacity => {
                let name = s(0).to_ascii_lowercase();
                let Some(p) = self.act_player(act) else { return 0 };
                AMMO.iter().find(|(n, ..)| *n == name).map_or(0, |&(_, i, _)| self.players[p].maxammo[i])
            }
            B::SetAmmoCapacity => {
                let name = s(0).to_ascii_lowercase();
                if let (Some(p), Some(&(_, i, _))) = (self.act_player(act), AMMO.iter().find(|(n, ..)| *n == name)) {
                    self.players[p].maxammo[i] = arg(1);
                }
                0
            }
            // ---- players ----
            B::PlayerCount => self.playeringame.iter().filter(|&&p| p).count() as i32,
            B::PlayerNumber => self.act_player(act).map_or(-1, |p| p as i32),
            B::PlayerInGame => self.playeringame.get(arg(0) as usize).copied().unwrap_or(false) as i32,
            B::PlayerIsBot | B::PlayerClass => 0,
            B::PlayerHealth => self.act_player(act).map_or(0, |p| self.players[p].health),
            B::PlayerArmorPoints => self.act_player(act).map_or(0, |p| self.players[p].armorpoints),
            B::PlayerFrags => self.act_player(act).map_or(0, |p| self.players[p].frags.iter().sum()),
            B::PlayerKey => {
                let Some(p) = self.act_player(act) else { return 0 };
                let c = self.players[p].cards; // blue, yellow, red card; blue, yellow, red skull
                (match arg(0) {
                    0 => c[3],
                    1 => c[5],
                    2 => c[4],
                    3 => c[3] || c[4] || c[5],
                    4 => c[0],
                    5 => c[2],
                    6 => c[1],
                    7 => c[0] || c[1] || c[2],
                    _ => false,
                }) as i32
            }
            B::GetPlayerInput | B::GetPlayerInfo => 0,
            // ---- game ----
            B::GameType => {
                if !self.params.netgame {
                    0
                } else if self.params.deathmatch != 0 {
                    2
                } else {
                    1
                }
            }
            B::GameSkill => self.params.skill as i32,
            B::SinglePlayer => (!self.params.netgame) as i32,
            B::IsNetworkGame => self.params.netgame as i32,
            B::GetLevelInfo => match arg(0) {
                2 => level_number(self),
                3 => self.totalsecret,
                4 => self.players.iter().map(|p| p.secretcount).sum(),
                5 => self.totalitems,
                6 => self.players.iter().map(|p| p.itemcount).sum(),
                7 => self.totalkills,
                8 => self.players.iter().map(|p| p.killcount).sum(),
                _ => 0,
            },
            B::GetCvar => 0,
            B::ChangeLevel => {
                self.exit_target = Some(ExitTarget::Name(s(0)));
                self.gameexit = Some(GameExit::Normal);
                0
            }
            // ---- map ----
            B::ChangeFloor | B::ChangeCeiling => {
                let Some(flat) = self.content.flat_num(&s(1)) else { return 0 };
                for sec in hexen::sectors_by_tag(self, arg(0)) {
                    let sm = self.level.sector_mut(sec);
                    if f == B::ChangeFloor {
                        sm.floor_pic = FlatNum(flat as u16);
                    } else {
                        sm.ceiling_pic = FlatNum(flat as u16);
                    }
                }
                0
            }
            B::SetLineTexture => {
                let Some(tex) = self.content.texture_num(&s(3)) else { return 0 };
                for l in hexen::lines_by_id(self, arg(0)) {
                    let Some(side) = self.level.line(l).sidenum.get(arg(1) as usize & 1).copied().flatten() else { continue };
                    let sd = &mut self.level.sides[side.idx()];
                    let t = TexNum(tex as u16);
                    match arg(2) {
                        0 => sd.top_texture = t,
                        1 => sd.mid_texture = t,
                        _ => sd.bottom_texture = t,
                    }
                }
                0
            }
            B::SetLineBlocking | B::SetLineMonsterBlocking => {
                let bit = if f == B::SetLineBlocking { ml::BLOCKING } else { ml::BLOCKMONSTERS };
                for l in hexen::lines_by_id(self, arg(0)) {
                    let line = &mut self.level.lines[l.idx()];
                    if arg(1) != 0 {
                        line.flags |= bit;
                    } else {
                        line.flags &= !bit;
                    }
                }
                0
            }
            B::SetLineSpecial => {
                for l in hexen::lines_by_id(self, arg(0)) {
                    set_line_special(self, l, arg(1), [arg(2), arg(3), arg(4), arg(5), arg(6)]);
                }
                0
            }
            B::ClearLineSpecial => {
                if let Some(l) = (arg(0) >= 0).then(|| LineId(arg(0) as u32)).filter(|l| l.idx() < self.level.lines.len()) {
                    set_line_special(self, l, 0, [0; 5]);
                }
                0
            }
            B::GetLineRowOffset => {
                let Some(l) = (arg(0) >= 0).then(|| LineId(arg(0) as u32)).filter(|l| l.idx() < self.level.lines.len()) else { return 0 };
                self.level.line(l).sidenum[0].map_or(0, |s| self.level.side(s).row_offset.0 >> 16)
            }
            B::GetSectorFloorZ | B::GetSectorCeilingZ => {
                let Some(&sec) = hexen::sectors_by_tag(self, arg(0)).first() else { return 0 };
                let s = self.level.sector(sec);
                if f == B::GetSectorFloorZ {
                    s.floor_height.0
                } else {
                    s.ceiling_height.0
                }
            }
            B::GetSectorLightLevel => hexen::sectors_by_tag(self, arg(0)).first().map_or(0, |&s| self.level.sector(s).light_level as i32),
            B::SectorDamage => 0,
            // ---- music ----
            B::SetMusic | B::LocalSetMusic => {
                self.script_events.push(ScriptEvent::Music { name: s(0) });
                0
            }
            // ---- extension functions ----
            B::Func(n) => self.acs_func(act, n, a, strings),
            // Presentation state that has no effect yet.
            B::SetFont | B::SetHudSize | B::FadeTo | B::FadeRange | B::CancelFade | B::SetMugShotState | B::SetCameraToTexture => 0,
            B::GetScreenWidth => 320,
            B::GetScreenHeight => 200,
            B::CheckPlayerCamera => -1,
            _ => {
                self.note_unimplemented(builtin_name(f));
                0
            }
        }
    }
}

impl World {
    /// `CallFunc` extension functions the VM leaves to the engine.
    fn acs_func(&mut self, act: &mut Activation, n: u16, a: &[i32], strings: &mut Strings) -> i32 {
        let arg = |i: usize| a.get(i).copied().unwrap_or(0);
        let s = |i: usize| strings.get(arg(i)).to_string();
        match n {
            0 => 0,
            // GetActorVelX/Y/Z(tid)
            9..=11 => self.tid_targets(arg(0), act).first().map_or(0, |&t| {
                let m = self.mo(t);
                [m.momx, m.momy, m.momz][(n - 9) as usize].0
            }),
            // SetActivator(tid[, ptr])
            12 => {
                let t = self.tid_targets(arg(0), act).first().copied();
                act.activator = t.map(actor_handle);
                t.is_some() as i32
            }
            // SetActivatorToTarget(tid)
            13 => {
                let t = self.tid_targets(arg(0), act).first().and_then(|&t| self.mo(t).target);
                if t.is_some() {
                    act.activator = t.map(actor_handle);
                }
                t.is_some() as i32
            }
            // SpawnSpotForced / SpawnSpotFacingForced
            20 | 21 => {
                let mut n2 = 0;
                for spot in self.tid_targets(arg(1), act) {
                    let m = self.mo(spot);
                    let (x, y, z, sa) = (m.x, m.y, m.z, m.angle);
                    let angle = if n == 21 { sa } else { byte_angle(arg(3)) };
                    n2 += self.spawn_named(&s(0), x, y, z, arg(2), angle);
                }
                n2
            }
            // SetActorVelocity(tid, vx, vy, vz, add, setbob)
            23 => {
                for t in self.tid_targets(arg(0), act) {
                    let m = self.mo_mut(t);
                    if arg(4) != 0 {
                        m.momx += Fixed(arg(1));
                        m.momy += Fixed(arg(2));
                        m.momz += Fixed(arg(3));
                    } else {
                        (m.momx, m.momy, m.momz) = (Fixed(arg(1)), Fixed(arg(2)), Fixed(arg(3)));
                    }
                }
                1
            }
            // CheckActorClass(tid, class)
            27 => self.tid_targets(arg(0), act).first().is_some_and(|&t| self.info.class_name(self.mo(t).mobj_type).eq_ignore_ascii_case(&s(1))) as i32,
            // CheckSight(source, dest, flags)
            35 => {
                let (Some(&a1), Some(&b1)) = (self.tid_targets(arg(0), act).first(), self.tid_targets(arg(1), act).first()) else { return 0 };
                crate::sight::check_sight(self, a1, b1) as i32
            }
            // SpawnForced(class, x, y, z, tid, angle)
            36 => self.spawn_named(&s(0), Fixed(arg(1)), Fixed(arg(2)), Fixed(arg(3)), arg(4), byte_angle(arg(5))),
            // UniqueTID(start, limit) / IsTIDUsed(tid)
            46 => {
                let start = if arg(0) > 0 { arg(0) } else { 1 };
                (start..start.saturating_add(65536)).find(|&t| mobjs_by_tid(self, t).is_empty()).unwrap_or(0)
            }
            47 => (!mobjs_by_tid(self, arg(0)).is_empty()) as i32,
            // GetActorClass(tid)
            68 => {
                let name = self.tid_targets(arg(0), act).first().map(|&t| self.info.class_name(self.mo(t).mobj_type).to_string()).unwrap_or_default();
                strings.add(name)
            }
            // SetMusicVolume(volume)
            97 => {
                self.script_events.push(ScriptEvent::MusicVolume((arg(0) as f32 / 65536.0).clamp(0.0, 1.0)));
                0
            }
            _ => {
                self.note_unimplemented(func_name(n));
                0
            }
        }
    }
}

fn set_line_special(w: &mut World, l: LineId, special: i32, args: [i32; 5]) {
    let line = &mut w.level.lines[l.idx()];
    if special == 0 {
        line.action = None;
        line.special = 0;
        return;
    }
    let activation = line.action.map_or(doom_content::map::spac::USE, |a| a.activation);
    let repeat = line.action.is_some_and(|a| a.repeat);
    line.action = Some(LineAction { special, args, activation, repeat, lock: 0 });
    line.special = HEXEN_LINE;
}

fn builtin_name(f: Builtin) -> &'static str {
    match f {
        Builtin::SpawnProjectile | Builtin::ThingProjectile2 => "ACS SpawnProjectile / Thing_Projectile2",
        Builtin::SectorSound | Builtin::AmbientSound | Builtin::LocalAmbientSound | Builtin::ThingSound | Builtin::ActivatorSound | Builtin::SoundSequence => {
            "ACS named sounds (needs SNDINFO)"
        }
        Builtin::SetActorProperty | Builtin::GetActorProperty => "ACS Set/GetActorProperty",
        Builtin::MorphActor | Builtin::UnmorphActor => "ACS MorphActor",
        Builtin::SetFloorTrigger | Builtin::SetCeilingTrigger => "ACS SetFloorTrigger",
        Builtin::ReplaceTextures => "ACS ReplaceTextures",
        Builtin::ChangeSky => "ACS ChangeSky",
        Builtin::SetGravity | Builtin::SetAirControl => "ACS SetGravity / SetAirControl",
        Builtin::CheckActorFloorTexture | Builtin::CheckActorCeilingTexture => "ACS CheckActorFloorTexture",
        Builtin::StartTranslation
        | Builtin::TranslationRange1
        | Builtin::TranslationRange2
        | Builtin::TranslationRange3
        | Builtin::TranslationRange4
        | Builtin::TranslationRange5
        | Builtin::EndTranslation => "ACS translations",
        _ => "ACS builtin (other)",
    }
}

fn func_name(n: u16) -> &'static str {
    match n {
        1..=8 => "ACSF Get*UDMF*",
        24 | 25 | 28 | 29 => "ACSF user variables",
        26 | 91 => "ACSF quakes",
        53..=59 => "ACSF CVars",
        60 => "ACSF LineAttack",
        61 | 62 | 70 | 71 => "ACSF sounds",
        72 => "ACSF SpawnDecal",
        74 | 82 => "ACSF DropItem / DropInventory",
        75 => "ACSF CheckFlag",
        79 | 80 | 88..=90 => "ACSF ChangeActorAngle/Pitch/Roll",
        83 => "ACSF PickActor",
        92 => "ACSF Warp",
        96 => "ACSF SpawnParticle",
        98 => "ACSF CheckProximity",
        99 => "ACSF CheckActorState",
        _ => "ACSF (other)",
    }
}
