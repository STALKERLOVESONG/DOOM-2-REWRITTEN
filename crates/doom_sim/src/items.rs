//! Weapon definitions (d_items.c) and DEHACKED-tunable misc values
//! (Chocolate Doom's `deh_misc`).

use crate::info::*;

pub mod ammo {
    pub const CLIP: usize = 0;
    pub const SHELL: usize = 1;
    pub const CELL: usize = 2;
    pub const MISL: usize = 3;
    /// `am_noammo` (unlimited, fist and chainsaw). Note it is 5, not 4:
    /// `NUMAMMO` sits between them in the C enum.
    pub const NOAMMO: usize = 5;
}

/// `weaponinfo_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeaponInfo {
    pub ammo: usize,
    pub upstate: StateNum,
    pub downstate: StateNum,
    pub readystate: StateNum,
    pub atkstate: StateNum,
    pub flashstate: StateNum,
}

pub const DEFAULT_WEAPONINFO: [WeaponInfo; 9] = [
    // fist
    WeaponInfo { ammo: ammo::NOAMMO, upstate: S_PUNCHUP, downstate: S_PUNCHDOWN, readystate: S_PUNCH, atkstate: S_PUNCH1, flashstate: S_NULL },
    // pistol
    WeaponInfo { ammo: ammo::CLIP, upstate: S_PISTOLUP, downstate: S_PISTOLDOWN, readystate: S_PISTOL, atkstate: S_PISTOL1, flashstate: S_PISTOLFLASH },
    // shotgun
    WeaponInfo { ammo: ammo::SHELL, upstate: S_SGUNUP, downstate: S_SGUNDOWN, readystate: S_SGUN, atkstate: S_SGUN1, flashstate: S_SGUNFLASH1 },
    // chaingun
    WeaponInfo { ammo: ammo::CLIP, upstate: S_CHAINUP, downstate: S_CHAINDOWN, readystate: S_CHAIN, atkstate: S_CHAIN1, flashstate: S_CHAINFLASH1 },
    // missile launcher
    WeaponInfo { ammo: ammo::MISL, upstate: S_MISSILEUP, downstate: S_MISSILEDOWN, readystate: S_MISSILE, atkstate: S_MISSILE1, flashstate: S_MISSILEFLASH1 },
    // plasma rifle
    WeaponInfo { ammo: ammo::CELL, upstate: S_PLASMAUP, downstate: S_PLASMADOWN, readystate: S_PLASMA, atkstate: S_PLASMA1, flashstate: S_PLASMAFLASH1 },
    // bfg 9000
    WeaponInfo { ammo: ammo::CELL, upstate: S_BFGUP, downstate: S_BFGDOWN, readystate: S_BFG, atkstate: S_BFG1, flashstate: S_BFGFLASH1 },
    // chainsaw
    WeaponInfo { ammo: ammo::NOAMMO, upstate: S_SAWUP, downstate: S_SAWDOWN, readystate: S_SAW, atkstate: S_SAW1, flashstate: S_NULL },
    // super shotgun
    WeaponInfo { ammo: ammo::SHELL, upstate: S_DSGUNUP, downstate: S_DSGUNDOWN, readystate: S_DSGUN, atkstate: S_DSGUN1, flashstate: S_DSGUNFLASH1 },
];

/// DEHACKED "Misc" values with their vanilla defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DehMisc {
    pub initial_health: i32,
    pub initial_bullets: i32,
    pub max_health: i32,
    pub max_armor: i32,
    pub green_armor_class: i32,
    pub blue_armor_class: i32,
    pub max_soulsphere: i32,
    pub soulsphere_health: i32,
    pub megasphere_health: i32,
    pub god_mode_health: i32,
    pub idfa_armor: i32,
    pub idfa_armor_class: i32,
    pub idkfa_armor: i32,
    pub idkfa_armor_class: i32,
    pub bfg_cells_per_shot: i32,
    pub species_infighting: bool,
}

impl Default for DehMisc {
    fn default() -> Self {
        Self {
            initial_health: 100,
            initial_bullets: 50,
            max_health: 200,
            max_armor: 200,
            green_armor_class: 1,
            blue_armor_class: 2,
            max_soulsphere: 200,
            soulsphere_health: 100,
            megasphere_health: 200,
            god_mode_health: 100,
            idfa_armor: 200,
            idfa_armor_class: 2,
            idkfa_armor: 200,
            idkfa_armor_class: 2,
            bfg_cells_per_shot: 40,
            species_infighting: false,
        }
    }
}
