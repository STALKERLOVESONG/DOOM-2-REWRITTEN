//! Sector special thinkers (p_doors.c, p_floor.c, p_ceilng.c, p_plats.c,
//! p_lights.c) and the shared plane mover (`T_MovePlane`).

pub mod ceilings;
pub mod doors;
pub mod floors;
pub mod lights;
pub mod plats;
pub mod plane;

use crate::ids::{SectorId, ThinkerId};
use crate::thinker::{ThinkFn, ThinkerData};
use crate::world::World;

pub use ceilings::{CeilingKind, CeilingMove};
pub use doors::{DoorKind, VerticalDoor};
pub use floors::{FloorKind, FloorMove};
pub use lights::{FireFlicker, Glow, LightFlash, Strobe};
pub use plats::{Plat, PlatKind, PlatStatus};

#[derive(Clone, Debug)]
pub enum SpecialThinker {
    Door(VerticalDoor),
    Floor(FloorMove),
    Ceiling(CeilingMove),
    Plat(Plat),
    FireFlicker(FireFlicker),
    LightFlash(LightFlash),
    Strobe(Strobe),
    Glow(Glow),
    /// Hexen light effects (`Light_Fade`, `Light_Glow`, ...).
    HexenLight(crate::hexen::HexenLight),
}

impl SpecialThinker {
    /// The `int` at byte offset 28 of the vanilla (32-bit) struct, which is
    /// what `EV_VerticalDoor` reads as `door->direction` when a sector's
    /// `specialdata` is some other mover (C type punning).
    pub fn punned_direction(&self) -> i32 {
        match self {
            SpecialThinker::Door(d) => d.direction,
            SpecialThinker::Plat(p) => p.wait,
            SpecialThinker::Floor(f) => f.newspecial,
            SpecialThinker::Ceiling(c) => c.speed.0,
            _ => 0,
        }
    }

    pub fn set_punned_direction(&mut self, v: i32) {
        match self {
            SpecialThinker::Door(d) => d.direction = v,
            SpecialThinker::Plat(p) => p.wait = v,
            SpecialThinker::Floor(f) => f.newspecial = v,
            SpecialThinker::Ceiling(c) => c.speed = doom_core::Fixed(v),
            _ => {}
        }
    }
}

/// Add a special thinker (`P_AddThinker`) running its think routine.
pub fn add_special(w: &mut World, s: SpecialThinker) -> ThinkerId {
    w.thinkers.add(ThinkerData::Special(s), ThinkFn::Active)
}

pub fn special_mut(w: &mut World, id: ThinkerId) -> Option<&mut SpecialThinker> {
    match w.thinkers.data_mut(id)? {
        ThinkerData::Special(s) => Some(s),
        _ => None,
    }
}

pub fn special(w: &World, id: ThinkerId) -> Option<&SpecialThinker> {
    match w.thinkers.data(id)? {
        ThinkerData::Special(s) => Some(s),
        _ => None,
    }
}

/// Dispatch one tic of a special thinker.
pub fn run_special(w: &mut World, id: ThinkerId) {
    let Some(s) = special(w, id) else { return };
    match s {
        SpecialThinker::Door(_) => doors::t_vertical_door(w, id),
        SpecialThinker::Floor(_) => floors::t_move_floor(w, id),
        SpecialThinker::Ceiling(_) => ceilings::t_move_ceiling(w, id),
        SpecialThinker::Plat(_) => plats::t_plat_raise(w, id),
        SpecialThinker::FireFlicker(_) => lights::t_fire_flicker(w, id),
        SpecialThinker::LightFlash(_) => lights::t_light_flash(w, id),
        SpecialThinker::Strobe(_) => lights::t_strobe_flash(w, id),
        SpecialThinker::Glow(_) => lights::t_glow(w, id),
        SpecialThinker::HexenLight(_) => crate::hexen::t_hexen_light(w, id),
    }
}

/// `sector->specialdata = NULL; P_RemoveThinker(thinker)`. Hexen-style
/// ceiling movers live in `ceiling_data` instead.
pub fn finish(w: &mut World, id: ThinkerId, sector: SectorId) {
    let s = w.level.sector_mut(sector);
    if s.ceiling_data == Some(id) {
        s.ceiling_data = None;
    } else {
        s.special_data = None;
    }
    w.thinkers.remove(id);
}
