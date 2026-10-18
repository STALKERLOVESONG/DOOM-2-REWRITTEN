//! Actor definitions: sprites, states, mobj types, sounds (info.c/info.h,
//! sounds.c/sounds.h).
//!
//! The default tables are generated from the reference source
//! (`tools/gen_info.py`). The simulation uses an [`Info`] instance — a
//! mutable copy — so content patches (DEHACKED now, DECORATE later) can
//! modify or extend it without touching gameplay code.

mod generated;

pub use generated::*;

use doom_core::Fixed;

macro_rules! num_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
        pub struct $name(pub u16);

        impl $name {
            #[inline]
            pub const fn idx(self) -> usize {
                self.0 as usize
            }
        }
    };
}

num_type!(SpriteNum);
num_type!(StateNum);
num_type!(MobjType);
num_type!(SfxNum);
num_type!(MusicNum);

/// Frame flag: draw at full brightness (`FF_FULLBRIGHT`).
pub const FF_FULLBRIGHT: i32 = 0x8000;
/// Frame number mask (`FF_FRAMEMASK`).
pub const FF_FRAMEMASK: i32 = 0x7fff;

/// `state_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub sprite: SpriteNum,
    /// Frame index, possibly ORed with [`FF_FULLBRIGHT`].
    pub frame: i32,
    /// Duration in tics; -1 = forever.
    pub tics: i32,
    pub action: Action,
    pub next: StateNum,
    pub misc1: i32,
    pub misc2: i32,
}

/// `mobjinfo_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MobjInfo {
    /// Editor number used in THINGS (-1 = not placeable).
    pub doomednum: i32,
    pub spawnstate: StateNum,
    pub spawnhealth: i32,
    pub seestate: StateNum,
    pub seesound: SfxNum,
    pub reactiontime: i32,
    pub attacksound: SfxNum,
    pub painstate: StateNum,
    pub painchance: i32,
    pub painsound: SfxNum,
    pub meleestate: StateNum,
    pub missilestate: StateNum,
    pub deathstate: StateNum,
    pub xdeathstate: StateNum,
    pub deathsound: SfxNum,
    /// Map units per step for monsters, fixed point per tic for missiles
    /// (the C code uses the same `int` both ways).
    pub speed: i32,
    pub radius: Fixed,
    pub height: Fixed,
    pub mass: i32,
    pub damage: i32,
    pub activesound: SfxNum,
    pub flags: u32,
    pub raisestate: StateNum,
}

/// The labels a vanilla `mobjinfo_t` defines.
pub fn standard_labels(m: &MobjInfo) -> std::collections::HashMap<String, StateNum> {
    let mut l = std::collections::HashMap::new();
    for (name, s) in [
        ("spawn", m.spawnstate),
        ("see", m.seestate),
        ("pain", m.painstate),
        ("melee", m.meleestate),
        ("missile", m.missilestate),
        ("death", m.deathstate),
        ("xdeath", m.xdeathstate),
        ("raise", m.raisestate),
    ] {
        if s.0 != 0 {
            l.insert(name.to_string(), s);
        }
    }
    l
}

/// `sfxinfo_t` (static part).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SfxInfo {
    /// Lump name without the `DS`/`DP` prefix.
    pub name: &'static str,
    pub priority: i32,
    /// Linked sound (e.g. `chgun` plays `pistol` at a different pitch).
    pub link: Option<SfxNum>,
    pub pitch: i32,
    pub volume: i32,
}

/// Runtime actor tables.
#[derive(Clone, Debug)]
pub struct Info {
    pub sprnames: Vec<String>,
    pub states: Vec<State>,
    pub mobjinfo: Vec<MobjInfo>,
    pub sfx: Vec<SfxInfo>,
    /// Class name per mobj type (lower case, ZDoom names; "" for vanilla
    /// types ZDoom has no name for). Scripts spawn and test actors by name.
    pub class_names: Vec<String>,
    /// Parent class per mobj type (DECORATE/ZScript inheritance).
    pub class_parents: Vec<Option<MobjType>>,
    /// State labels per mobj type (lower case: `spawn`, `death.fire`, ...),
    /// for jumps by name and scripts' `SetActorState`.
    pub class_labels: Vec<std::collections::HashMap<String, StateNum>>,
    /// User variables per mobj type (`var int user_x;`), lower case.
    pub class_vars: Vec<Vec<String>>,
    /// Action calls with arguments (DECORATE/ZScript) by state number;
    /// vanilla states use `State::action` instead.
    pub state_calls: std::collections::HashMap<u16, std::sync::Arc<ActionCall>>,
    /// Logical sound names (SNDINFO and Doom's, lower case) to sounds, and
    /// the lump each sound added by SNDINFO plays.
    pub sound_names: std::collections::HashMap<String, SfxNum>,
    /// Sounds a mod defined or redefined (SNDINFO), by sound number: the
    /// resource each plays, resolved at load ([`crate::sounds`]). Doom's
    /// sounds without a definition play their `DS` lumps.
    pub sound_defs: std::collections::HashMap<u16, crate::sounds::SoundDef>,
    /// `$playersound` sounds by "class/*name".
    pub player_sounds: std::collections::HashMap<String, SfxNum>,
    /// `$ambient` definitions by number.
    pub ambient_sounds: std::collections::HashMap<u32, crate::sounds::AmbientDef>,
    /// States whose duration is `random(min, max)`.
    pub state_random_tics: std::collections::HashMap<u16, (i32, i32)>,
    /// Weapon classes (`Weapon.*` properties), by mobj type: mods' and
    /// Doom's nine ([`Info::vanilla_weapon_classes`]).
    pub weapons: std::collections::HashMap<u16, crate::weapons::WeaponDef>,
    /// ZDoom actor flags with no vanilla bit ([`zflags`]), per mobj type.
    pub class_zflags: Vec<u32>,
    /// `VSpeed` per mobj type (puffs), map units per tic.
    pub class_vspeed: Vec<Option<f32>>,
    /// ZScript methods per mobj type, by lower-case name (inherited ones
    /// are found through the parent chain, see [`Info::find_method`]).
    pub class_methods: Vec<std::collections::HashMap<String, std::sync::Arc<doom_script::zscript::ast::FuncDecl>>>,
    /// The class of each of Doom's nine weapons (`Fist`, `Pistol`, ...),
    /// indexed by `Weapon`. Their states are `weaponinfo`'s.
    pub vanilla_weapon_classes: [MobjType; 9],
    /// A mod replaced that vanilla weapon's class (`replaces Pistol`): the
    /// player's weapon of that kind runs the class's states and ammo.
    pub vanilla_weapon_replaced: [bool; 9],
}

/// ZDoom actor flags without a vanilla `MF_` bit ([`Info::class_zflags`]).
pub mod zflags {
    /// A puff that spawns on actors too (not only blood).
    pub const PUFFONACTORS: u32 = 1;
    /// A puff spawned even when the attack hits nothing.
    pub const ALWAYSPUFF: u32 = 2;
    /// The puff's `target` is the shooter.
    pub const PUFFGETSOWNER: u32 = 4;
    /// Randomise the first state's duration.
    pub const RANDOMIZE: u32 = 8;
    /// The puff's `tracer` is the actor hit.
    pub const HITTRACER: u32 = 16;
    /// Weapon: never fire automatically on pickup.
    pub const NOAUTOFIRE: u32 = 32;
    /// Weapon: firing doesn't need ammo (primary / secondary).
    pub const AMMO_OPTIONAL: u32 = 64;
    pub const ALT_AMMO_OPTIONAL: u32 = 128;
    /// Names as written in DECORATE/ZScript (lower case, no prefix).
    pub const NAMES: &[(&str, u32)] = &[
        ("puffonactors", PUFFONACTORS),
        ("alwayspuff", ALWAYSPUFF),
        ("puffgetsowner", PUFFGETSOWNER),
        ("randomize", RANDOMIZE),
        ("hittracer", HITTRACER),
        ("noautofire", NOAUTOFIRE),
        ("ammo_optional", AMMO_OPTIONAL),
        ("alt_ammo_optional", ALT_AMMO_OPTIONAL),
    ];
}

/// An action function call in a state: `A_Jump(128, "See")`, or a ZScript
/// anonymous function (`{ ... }`).
#[derive(Clone, Debug)]
pub struct ActionCall {
    /// Function name, lower case (`a_jump`); empty for a code block.
    pub name: String,
    pub args: Vec<doom_script::zscript::ast::Arg>,
    pub block: Option<doom_script::zscript::ast::Block>,
    /// The class whose `States` contains the call (labels resolve there).
    pub owner: MobjType,
}

/// ZDoom engine things placed by maps: invisible markers that scripts and
/// specials find by TID. (doomednum, class name.)
pub const ZDOOM_MARKERS: &[(i32, &str)] = &[
    (9001, "mapspot"),
    (9013, "mapspotgravity"),
    (9043, "teleportdest3"),
    (9044, "teleportdest2"),
    (9024, "patrolpoint"),
    (9047, "patrolspecial"),
    (9070, "interpolationpoint"),
    (9072, "movingcamera"),
    (9025, "securitycamera"),
    (9074, "actormover"),
    (9075, "interpolationspecial"),
    (14066, "soundsequence"),
];

impl Info {
    /// The vanilla Doom tables, plus ZDoom's invisible map markers.
    pub fn doom() -> Self {
        let mut info = Self {
            sprnames: SPRNAMES.iter().map(|s| s.to_string()).collect(),
            states: DEFAULT_STATES.to_vec(),
            mobjinfo: DEFAULT_MOBJINFO.to_vec(),
            sfx: DEFAULT_SFX.to_vec(),
            class_names: vec![String::new(); NUMMOBJTYPES],
            class_parents: vec![None; NUMMOBJTYPES],
            class_labels: Vec::new(),
            class_vars: vec![Vec::new(); NUMMOBJTYPES],
            state_calls: Default::default(),
            sound_names: Default::default(),
            sound_defs: Default::default(),
            player_sounds: Default::default(),
            ambient_sounds: Default::default(),
            state_random_tics: Default::default(),
            weapons: Default::default(),
            class_zflags: vec![0; NUMMOBJTYPES],
            class_vspeed: vec![None; NUMMOBJTYPES],
            class_methods: vec![Default::default(); NUMMOBJTYPES],
            vanilla_weapon_classes: [MobjType(0); 9],
            vanilla_weapon_replaced: [false; 9],
        };
        // BulletPuff rises one unit per tic (vanilla P_SpawnPuff's momz).
        info.class_vspeed[MT_PUFF.idx()] = Some(1.0);
        for &(name, t) in crate::actors::CLASSES {
            info.class_names[t.idx()] = name.to_string();
        }
        info.class_labels = info.mobjinfo.iter().map(standard_labels).collect();
        for (i, s) in info.sfx.iter().enumerate() {
            info.sound_names.insert(s.name.to_string(), SfxNum(i as u16));
        }
        // ZDoom's names for Doom's sounds (`weapons/pistol`, `imp/sight1`).
        crate::sounds::add_logical_names(&mut info);
        // Markers: one shared invisible state; not in sector lists or the
        // blockmap, so nothing draws or touches them.
        info.sprnames.push("TNT1".to_string());
        let tnt1 = SpriteNum((info.sprnames.len() - 1) as u16);
        info.states.push(State { sprite: tnt1, frame: 0, tics: -1, action: Action::None, next: S_NULL, misc1: 0, misc2: 0 });
        let marker_state = StateNum((info.states.len() - 1) as u16);
        for &(doomednum, name) in ZDOOM_MARKERS {
            info.mobjinfo.push(MobjInfo {
                doomednum,
                spawnstate: marker_state,
                spawnhealth: 1000,
                seestate: S_NULL,
                seesound: SfxNum(0),
                reactiontime: 8,
                attacksound: SfxNum(0),
                painstate: S_NULL,
                painchance: 0,
                painsound: SfxNum(0),
                meleestate: S_NULL,
                missilestate: S_NULL,
                deathstate: S_NULL,
                xdeathstate: S_NULL,
                deathsound: SfxNum(0),
                speed: 0,
                radius: Fixed::from_int(20),
                height: Fixed::from_int(16),
                mass: 100,
                damage: 0,
                activesound: SfxNum(0),
                flags: crate::mobj::flags::MF_NOBLOCKMAP | crate::mobj::flags::MF_NOSECTOR | crate::mobj::flags::MF_NOGRAVITY,
                raisestate: S_NULL,
            });
            info.class_names.push(name.to_string());
            info.class_parents.push(None);
            info.class_labels.push(std::collections::HashMap::from([("spawn".to_string(), marker_state)]));
            info.class_vars.push(Vec::new());
            info.class_zflags.push(0);
            info.class_vspeed.push(None);
            info.class_methods.push(Default::default());
        }
        // ZDoom's weapon base classes, with their built-in `LightDone`
        // state (flash states end with `Goto LightDone`).
        info.states.push(State { sprite: tnt1, frame: 0, tics: 0, action: Action::Light0, next: S_NULL, misc1: 0, misc2: 0 });
        let light_done = StateNum((info.states.len() - 1) as u16);
        let mut base = DEFAULT_MOBJINFO[MT_SHOTGUN.idx()];
        base.doomednum = -1;
        base.spawnstate = marker_state;
        let weapon = info.add_type(base, "weapon", None);
        info.class_labels[weapon.idx()].insert("lightdone".to_string(), light_done);
        let doomweapon = info.add_type(base, "doomweapon", Some(weapon));
        info.class_labels[doomweapon.idx()].insert("lightdone".to_string(), light_done);
        info.add_vanilla_weapon_classes(base, doomweapon);
        info
    }

    /// Doom's nine weapons as `DoomWeapon` classes with `weaponinfo`'s
    /// states under the standard labels, so mods can inherit from them
    /// (`MyShotgun : Shotgun`) or replace them (`replaces Pistol`). Fist and
    /// Pistol get classes of their own; the others are their pickups.
    fn add_vanilla_weapon_classes(&mut self, base: MobjInfo, doomweapon: MobjType) {
        use crate::player::Weapon;
        use crate::weapons::{AmmoRef, WeaponDef};
        // (weapon, class, existing pickup type, slot, ammo per shot, GZDoom selection order)
        let table: [(Weapon, &str, Option<MobjType>, i32, i32, i32); 9] = [
            (Weapon::Fist, "fist", None, 1, 0, 3700),
            (Weapon::Pistol, "pistol", None, 2, 1, 1900),
            (Weapon::Shotgun, "shotgun", Some(MT_SHOTGUN), 3, 1, 1300),
            (Weapon::Chaingun, "chaingun", Some(MT_CHAINGUN), 4, 1, 700),
            (Weapon::Missile, "rocketlauncher", Some(MT_MISC27), 5, 1, 2500),
            (Weapon::Plasma, "plasmarifle", Some(MT_MISC28), 6, 1, 100),
            (Weapon::Bfg, "bfg9000", Some(MT_MISC25), 7, 40, 2800),
            (Weapon::Chainsaw, "chainsaw", Some(MT_MISC26), 1, 0, 2200),
            (Weapon::SuperShotgun, "supershotgun", Some(MT_SUPERSHOTGUN), 3, 2, 400),
        ];
        for (wpn, name, pickup, slot, ammo_use, order) in table {
            let t = match pickup {
                Some(t) => {
                    self.class_parents[t.idx()] = Some(doomweapon);
                    t
                }
                None => self.add_type(base, name, Some(doomweapon)),
            };
            let wi = crate::items::DEFAULT_WEAPONINFO[wpn as usize];
            let labels = &mut self.class_labels[t.idx()];
            for (l, st) in [("select", wi.upstate), ("deselect", wi.downstate), ("ready", wi.readystate), ("fire", wi.atkstate), ("flash", wi.flashstate)] {
                if st != S_NULL {
                    labels.insert(l.to_string(), st);
                }
            }
            let ammo = (wi.ammo < 4).then_some(AmmoRef::Vanilla(wi.ammo));
            self.weapons.insert(t.0, WeaponDef { slot, ammo, ammo_use, ammo_give: 0, selection_order: order, ..Default::default() });
            self.vanilla_weapon_classes[wpn as usize] = t;
        }
    }

    /// Which of Doom's nine weapons class `t` is, if any.
    pub fn vanilla_weapon_of(&self, t: MobjType) -> Option<crate::player::Weapon> {
        self.vanilla_weapon_classes.iter().position(|&c| c == t).map(|i| crate::player::Weapon::from_index(i as u8))
    }

    /// A ZScript method of `class` (or the nearest ancestor defining it).
    pub fn find_method(&self, class: MobjType, name: &str) -> Option<(MobjType, std::sync::Arc<doom_script::zscript::ast::FuncDecl>)> {
        let mut c = Some(class);
        for _ in 0..64 {
            let t = c?;
            if let Some(f) = self.class_methods.get(t.idx()).and_then(|m| m.get(name)) {
                return Some((t, f.clone()));
            }
            c = self.class_parents.get(t.idx()).copied().flatten();
        }
        None
    }

    /// Does `class` have this ZDoom flag ([`zflags`])?
    pub fn has_zflag(&self, class: MobjType, flag: u32) -> bool {
        self.class_zflags.get(class.idx()).is_some_and(|f| f & flag != 0)
    }

    /// Add a mobj type with its class data; returns its number.
    pub fn add_type(&mut self, mi: MobjInfo, name: &str, parent: Option<MobjType>) -> MobjType {
        self.mobjinfo.push(mi);
        self.class_names.push(name.to_ascii_lowercase());
        self.class_parents.push(parent);
        self.class_labels.push(Default::default());
        self.class_vars.push(parent.map(|p| self.class_vars[p.idx()].clone()).unwrap_or_default());
        self.class_zflags.push(parent.map_or(0, |p| self.class_zflags[p.idx()]));
        self.class_vspeed.push(parent.and_then(|p| self.class_vspeed[p.idx()]));
        self.class_methods.push(Default::default());
        MobjType((self.mobjinfo.len() - 1) as u16)
    }

    /// Is `t` the class `ancestor` or derived from it?
    pub fn is_a(&self, mut t: MobjType, ancestor: MobjType) -> bool {
        for _ in 0..64 {
            if t == ancestor {
                return true;
            }
            match self.class_parents.get(t.idx()).copied().flatten() {
                Some(p) => t = p,
                None => return false,
            }
        }
        false
    }

    /// A state by label in a class (`see`, `death.fire`, `super::see`),
    /// searching parents for labels the class doesn't define. Dotted labels
    /// fall back to their first part (`death.fire` -> `death`), as in ZDoom.
    pub fn find_label(&self, class: MobjType, label: &str) -> Option<StateNum> {
        let label = label.to_ascii_lowercase();
        let (class, label) = match label.strip_prefix("super::") {
            Some(l) => (self.class_parents.get(class.idx()).copied().flatten()?, l.to_string()),
            None => (class, label),
        };
        let mut l = label.as_str();
        loop {
            let mut c = Some(class);
            // Bounded: a malformed replacement chain must not hang.
            for _ in 0..64 {
                let Some(t) = c else { break };
                if let Some(&s) = self.class_labels.get(t.idx()).and_then(|m| m.get(l)) {
                    return Some(s);
                }
                c = self.class_parents.get(t.idx()).copied().flatten();
            }
            l = l.rsplit_once('.')?.0;
        }
    }

    /// A sound by SNDINFO or Doom name (`imp/sight`, `pistol`).
    pub fn sound(&self, name: &str) -> Option<SfxNum> {
        self.sound_names.get(&name.to_ascii_lowercase()).copied()
    }

    /// A class by name (case-insensitive). Replacements were applied when
    /// DECORATE loaded, so this spawns what the mod intends.
    pub fn type_by_name(&self, name: &str) -> Option<MobjType> {
        self.class_names.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| MobjType(i as u16))
    }

    pub fn class_name(&self, t: MobjType) -> &str {
        self.class_names.get(t.idx()).map_or("", String::as_str)
    }

    #[inline]
    pub fn state(&self, s: StateNum) -> &State {
        &self.states[s.idx()]
    }

    #[inline]
    pub fn mobj(&self, t: MobjType) -> &MobjInfo {
        &self.mobjinfo[t.idx()]
    }

    /// Linear search by editor number, like `P_SpawnMapThing` (first match).
    pub fn type_for_doomednum(&self, doomednum: i32) -> Option<MobjType> {
        self.mobjinfo.iter().position(|m| m.doomednum == doomednum).map(|i| MobjType(i as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_line_up() {
        let info = Info::doom();
        // Vanilla's states, plus the invisible state ZDoom's markers share
        // and the weapon classes' `LightDone`.
        assert_eq!(info.states.len(), NUMSTATES + 2);
        // Vanilla's types, the markers, `Weapon`, `DoomWeapon`, and the
        // `Fist` and `Pistol` classes (Doom's other weapons are pickups).
        assert_eq!(info.mobjinfo.len(), NUMMOBJTYPES + ZDOOM_MARKERS.len() + 4);
        for (i, &t) in info.vanilla_weapon_classes.iter().enumerate() {
            let wi = crate::items::DEFAULT_WEAPONINFO[i];
            assert_eq!(info.find_label(t, "ready"), Some(wi.readystate), "weapon {i}");
            assert_eq!(info.find_label(t, "fire"), Some(wi.atkstate), "weapon {i}");
            assert!(info.weapons[&t.0].slot >= 1);
        }
        assert_eq!(info.class_zflags.len(), info.mobjinfo.len());
        assert_eq!(info.class_methods.len(), info.mobjinfo.len());
        assert!(info.find_label(info.type_by_name("doomweapon").unwrap(), "lightdone").is_some());
        assert_eq!(info.class_names.len(), info.mobjinfo.len());
        assert_eq!(info.type_by_name("DoomImp"), Some(MT_TROOP));
        assert_eq!(info.class_name(info.type_for_doomednum(9001).unwrap()), "mapspot");
        assert_eq!(info.mobj(MT_PLAYER).spawnstate, S_PLAY);
        assert_eq!(info.mobj(MT_PLAYER).radius, Fixed::from_int(16));
        assert_eq!(info.type_for_doomednum(3004), Some(MT_POSSESSED));
        assert_eq!(info.sprnames[SPR_TROO.idx()], "TROO");
        assert_eq!(info.state(S_PLAY_RUN1).next, S_PLAY_RUN2);
        assert_eq!(info.sfx[SFX_CHGUN.idx()].link, Some(SFX_PISTOL));
    }
}
