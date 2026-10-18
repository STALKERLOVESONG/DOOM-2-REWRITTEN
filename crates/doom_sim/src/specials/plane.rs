//! `T_MovePlane` (p_floor.c): move a floor or ceiling towards a destination,
//! undoing the move if things would be crushed.

use doom_core::Fixed;

use crate::ids::SectorId;
use crate::map::change_sector;
use crate::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveResult {
    Ok,
    Crushed,
    PastDest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    Floor,
    Ceiling,
}

/// `crush` is the C `boolean` *value*: callers pass 0/1, except stairs,
/// which pass the uninitialised value Chocolate Doom emulates (10). The
/// distinction matters: `crush == true` tests compare against 1, while
/// `P_ChangeSector(sector, crush)` only tests for non-zero.
pub fn move_plane(w: &mut World, sector: SectorId, speed: Fixed, dest: Fixed, crush: i32, plane: Plane, direction: i32) -> MoveResult {
    let crunch = crush != 0;
    let crush_is_true = crush == 1;
    let get = |w: &World| match plane {
        Plane::Floor => w.level.sector(sector).floor_height,
        Plane::Ceiling => w.level.sector(sector).ceiling_height,
    };
    let set = |w: &mut World, v: Fixed| match plane {
        Plane::Floor => w.level.sector_mut(sector).floor_height = v,
        Plane::Ceiling => w.level.sector_mut(sector).ceiling_height = v,
    };

    let lastpos = get(w);
    match (plane, direction) {
        // DOWN (floor or ceiling).
        (_, -1) => {
            if lastpos - speed < dest {
                set(w, dest);
                if change_sector(w, sector, crunch) {
                    set(w, lastpos);
                    change_sector(w, sector, crunch);
                }
                return MoveResult::PastDest;
            }
            set(w, lastpos - speed);
            if change_sector(w, sector, crunch) {
                // Ceilings moving down keep crushing; floors moving down
                // always restore.
                if plane == Plane::Ceiling && crush_is_true {
                    return MoveResult::Crushed;
                }
                set(w, lastpos);
                change_sector(w, sector, crunch);
                return MoveResult::Crushed;
            }
        }
        // UP.
        (_, 1) => {
            if lastpos + speed > dest {
                set(w, dest);
                if change_sector(w, sector, crunch) {
                    set(w, lastpos);
                    change_sector(w, sector, crunch);
                }
                return MoveResult::PastDest;
            }
            set(w, lastpos + speed);
            let flag = change_sector(w, sector, crunch);
            // Ceilings moving up never crush (that code is `#if 0` in the
            // original).
            if flag && plane == Plane::Floor {
                if crush_is_true {
                    return MoveResult::Crushed;
                }
                set(w, lastpos);
                change_sector(w, sector, crunch);
                return MoveResult::Crushed;
            }
        }
        _ => {}
    }
    MoveResult::Ok
}
