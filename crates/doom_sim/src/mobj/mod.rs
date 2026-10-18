//! Map objects (p_mobj.h / p_mobj.c).

mod spawn;
mod think;

pub use spawn::*;
pub use think::*;

use doom_content::map::MapThing;
use doom_core::{Angle, Fixed};

use crate::ids::{MobjId, SubsectorId};
use crate::info::{MobjType, SpriteNum, StateNum};

/// `mobjflag_t`.
pub mod flags {
    /// Call `P_SpecialThing` when touched.
    pub const MF_SPECIAL: u32 = 1;
    /// Blocks.
    pub const MF_SOLID: u32 = 2;
    /// Can be hit.
    pub const MF_SHOOTABLE: u32 = 4;
    /// Don't use the sector links (invisible but touchable).
    pub const MF_NOSECTOR: u32 = 8;
    /// Don't use the blocklinks (inert but displayable).
    pub const MF_NOBLOCKMAP: u32 = 16;
    /// Not to be activated by sound, deaf monster.
    pub const MF_AMBUSH: u32 = 32;
    /// Will try to attack right back.
    pub const MF_JUSTHIT: u32 = 64;
    /// Will take at least one step before attacking.
    pub const MF_JUSTATTACKED: u32 = 128;
    /// On level spawning (initial position), hang from ceiling instead of
    /// stand on floor.
    pub const MF_SPAWNCEILING: u32 = 256;
    /// Don't apply gravity (every tic).
    pub const MF_NOGRAVITY: u32 = 512;
    /// Allow jumps from high places.
    pub const MF_DROPOFF: u32 = 0x400;
    /// For players, will pick up items.
    pub const MF_PICKUP: u32 = 0x800;
    /// Player cheat.
    pub const MF_NOCLIP: u32 = 0x1000;
    /// Player: keep info about sliding along walls.
    pub const MF_SLIDE: u32 = 0x2000;
    /// Allow moves to any height, no gravity (floating monsters).
    pub const MF_FLOAT: u32 = 0x4000;
    /// Don't cross lines or look at heights on teleport.
    pub const MF_TELEPORT: u32 = 0x8000;
    /// Don't hit same species, explode on block.
    pub const MF_MISSILE: u32 = 0x10000;
    /// Dropped by a demon, not level spawned.
    pub const MF_DROPPED: u32 = 0x20000;
    /// Use fuzzy draw (shadow demons or spectres).
    pub const MF_SHADOW: u32 = 0x40000;
    /// Don't bleed when shot (use puff).
    pub const MF_NOBLOOD: u32 = 0x80000;
    /// Don't stop moving halfway off a step.
    pub const MF_CORPSE: u32 = 0x100000;
    /// Floating to a height for a move.
    pub const MF_INFLOAT: u32 = 0x200000;
    /// Counts towards the kill total.
    pub const MF_COUNTKILL: u32 = 0x400000;
    /// Counts towards the item total.
    pub const MF_COUNTITEM: u32 = 0x800000;
    /// Special handling: skull in flight.
    pub const MF_SKULLFLY: u32 = 0x1000000;
    /// Don't spawn this object in deathmatch mode (e.g. key cards).
    pub const MF_NOTDMATCH: u32 = 0x2000000;
    /// Player sprite colour translation (2 bits).
    pub const MF_TRANSLATION: u32 = 0xc000000;
    pub const MF_TRANSSHIFT: u32 = 26;
}

/// Spawn heights for `P_SpawnMobj` (`ONFLOORZ`, `ONCEILINGZ`).
pub const ONFLOORZ: Fixed = Fixed::MIN;
pub const ONCEILINGZ: Fixed = Fixed::MAX;

/// `mobj_t`.
#[derive(Clone, Debug, Default)]
pub struct Mobj {
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
    /// Sector thing-list links.
    pub snext: Option<MobjId>,
    pub sprev: Option<MobjId>,
    pub angle: Angle,
    pub sprite: SpriteNum,
    /// Frame, possibly ORed with `FF_FULLBRIGHT`.
    pub frame: i32,
    /// Blockmap links.
    pub bnext: Option<MobjId>,
    pub bprev: Option<MobjId>,
    pub subsector: SubsectorId,
    /// Highest floor / lowest ceiling contacted (from the last position check).
    pub floorz: Fixed,
    pub ceilingz: Fixed,
    pub radius: Fixed,
    pub height: Fixed,
    pub momx: Fixed,
    pub momy: Fixed,
    pub momz: Fixed,
    pub mobj_type: MobjType,
    /// State tic counter.
    pub tics: i32,
    pub state: StateNum,
    pub flags: u32,
    pub health: i32,
    /// Movement direction (0-7, 8 = none).
    pub movedir: i32,
    /// When 0, select a new direction.
    pub movecount: i32,
    /// Thing being chased/attacked, or the source of a missile.
    pub target: Option<MobjId>,
    /// Reaction time before attacking; also used as a post-teleport freeze.
    pub reactiontime: i32,
    /// Chase this target without looking for others while > 0.
    pub threshold: i32,
    /// Player number if this is a player's body.
    pub player: Option<usize>,
    /// Player number last looked for.
    pub lastlook: i32,
    /// For nightmare respawn.
    pub spawnpoint: MapThing,
    /// Thing being chased by a homing missile (revenant rockets) or archvile target.
    pub tracer: Option<MobjId>,
    /// Thing ID for scripts and specials (0 = none).
    pub tid: i32,
    /// Hexen-style special run when it dies or is picked up, with its
    /// arguments.
    pub special: i32,
    pub args: [i32; 5],
    /// Translucency from mod actions (0 = opaque, 1 = invisible).
    pub fade: f32,
    /// Sprite scale from mod actions (`None` = 1).
    pub scale: Option<(f32, f32)>,
    /// DECORATE/ZScript user variables (`user_x`), by lower-case name.
    pub user_vars: Vec<(String, f64)>,
    /// Named items given to this actor (mod tokens), lower-case class name.
    pub inventory: std::collections::BTreeMap<String, i32>,
}
