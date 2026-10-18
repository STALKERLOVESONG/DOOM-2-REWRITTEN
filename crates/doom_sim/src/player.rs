//! `player_t` (d_player.h) and related enums (doomdef.h).

use doom_core::Fixed;

use crate::ids::MobjId;
use crate::info::StateNum;
use crate::ticcmd::TicCmd;

pub const MAXPLAYERS: usize = 4;
pub const VIEWHEIGHT: Fixed = Fixed(41 * doom_core::FRACUNIT);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PlayerState {
    /// Playing or camping.
    #[default]
    Live,
    /// Dead on the ground, view follows killer.
    Dead,
    /// Ready to restart/respawn.
    Reborn,
}

/// `cheat_t`.
pub mod cheat {
    pub const NOCLIP: i32 = 1;
    pub const GODMODE: i32 = 2;
    pub const NOMOMENTUM: i32 = 4;
    /// Not in vanilla (GZDoom's `notarget`): monsters do not look for this
    /// player or wake to their noise.
    pub const NOTARGET: i32 = 8;
}

pub const NUMCARDS: usize = 6;
pub const NUMWEAPONS: usize = 9;
pub const NUMAMMO: usize = 4;
pub const NUMPOWERS: usize = 6;
pub const NUMPSPRITES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum Weapon {
    Fist = 0,
    #[default]
    Pistol,
    Shotgun,
    Chaingun,
    Missile,
    Plasma,
    Bfg,
    Chainsaw,
    SuperShotgun,
    /// `wp_nochange`.
    NoChange,
}

impl Weapon {
    pub fn from_index(i: u8) -> Weapon {
        use Weapon::*;
        [Fist, Pistol, Shotgun, Chaingun, Missile, Plasma, Bfg, Chainsaw, SuperShotgun]
            .get(i as usize)
            .copied()
            .unwrap_or(NoChange)
    }
}

pub mod power {
    pub const INVULNERABILITY: usize = 0;
    pub const STRENGTH: usize = 1;
    pub const INVISIBILITY: usize = 2;
    pub const IRONFEET: usize = 3;
    pub const ALLMAP: usize = 4;
    pub const INFRARED: usize = 5;
}

/// `pspdef_t`: a weapon/flash overlay sprite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PspDef {
    /// `None` = not active.
    pub state: Option<StateNum>,
    pub tics: i32,
    pub sx: Fixed,
    pub sy: Fixed,
}

#[derive(Clone, Debug, Default)]
pub struct Player {
    pub mo: Option<MobjId>,
    pub playerstate: PlayerState,
    pub cmd: TicCmd,
    /// Focal origin above `mo.z`.
    pub viewz: Fixed,
    /// Base height above floor for `viewz`.
    pub viewheight: Fixed,
    /// Bob/squat speed.
    pub deltaviewheight: Fixed,
    /// Bounded/scaled total momentum.
    pub bob: Fixed,
    /// Mirrors `mo.health`; kept separately as in vanilla.
    pub health: i32,
    pub armorpoints: i32,
    /// 0 = none, 1 = green, 2 = blue.
    pub armortype: i32,
    pub powers: [i32; NUMPOWERS],
    pub cards: [bool; NUMCARDS],
    pub backpack: bool,
    pub frags: [i32; MAXPLAYERS],
    pub readyweapon: Weapon,
    pub pendingweapon: Weapon,
    pub weaponowned: [bool; NUMWEAPONS],
    pub ammo: [i32; NUMAMMO],
    pub maxammo: [i32; NUMAMMO],
    /// True if the button is down last tic.
    pub attackdown: bool,
    pub usedown: bool,
    pub cheats: i32,
    /// Refired shots are less accurate.
    pub refire: i32,
    pub killcount: i32,
    pub itemcount: i32,
    pub secretcount: i32,
    /// Hint messages (static strings in vanilla).
    pub message: Option<&'static str>,
    /// For screen flashing (red or bright).
    pub damagecount: i32,
    pub bonuscount: i32,
    /// Who did damage (`None` for floors/ceilings).
    pub attacker: Option<MobjId>,
    /// So gun flashes light up areas.
    pub extralight: i32,
    /// Current PLAYPAL-independent fixed colormap (infrared/invuln), 0 = none.
    pub fixedcolormap: i32,
    /// Player skin colorshift (0-3).
    pub colormap: i32,
    pub psprites: [PspDef; NUMPSPRITES],
    /// True if secret level has been done.
    pub didsecret: bool,
    /// Items Doom has no field for (mod tokens, keys...) by lower-case
    /// class name, as given and taken by scripts.
    pub inventory: std::collections::BTreeMap<String, i32>,
    /// Mod weapons owned (`crate::weapons`), in pickup order.
    pub class_weapons: Vec<crate::info::MobjType>,
    /// The mod weapon that is up (`None`: `readyweapon` is).
    pub ready_class: Option<crate::info::MobjType>,
    /// A mod weapon to switch to once the current one is lowered.
    pub pending_class: Option<crate::info::MobjType>,
    /// Secondary fire was held last tic.
    pub altattackdown: bool,
    /// Which attack of the weapon in hand is running (ammo comes from it).
    pub fire_mode: crate::weapons::FireMode,
    /// `A_WeaponReady` flags of the current ready state (`WRF_*`).
    pub ready_flags: i32,
    /// Fields of the player's weapons as ZScript sees them through
    /// `invoker` (`invoker.shots`), by "class.field" (lower case).
    pub weapon_vars: std::collections::BTreeMap<String, f64>,
    /// Skateboard controller state (`player_controller Skate`); `None`
    /// moves with the classic Doom controller.
    pub skater: Option<Box<crate::skate::Skater>>,
}

/// `maxammo[]` (p_inter.c).
pub const MAXAMMO: [i32; NUMAMMO] = [200, 50, 300, 50];
