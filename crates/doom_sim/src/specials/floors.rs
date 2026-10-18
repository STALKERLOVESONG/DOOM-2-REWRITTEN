//! Floor movers and stairs (p_floor.c).

use doom_content::game::GameVersion;
use doom_core::{Fixed, FRACUNIT};

use super::plane::{move_plane, MoveResult, Plane};
use super::{add_special, finish, special_mut, SpecialThinker};
use crate::ids::{FlatNum, LineId, SectorId, ThinkerId};
use crate::info::*;
use crate::spec::{
    find_highest_floor_surrounding, find_lowest_ceiling_surrounding, find_lowest_floor_surrounding,
    find_next_highest_floor, find_sector_from_line_tag, next_sector,
};
use crate::world::{SoundSource, World};
use doom_content::map::ml;

pub const FLOORSPEED: Fixed = Fixed(FRACUNIT);
/// Chocolate Doom's emulation of the uninitialised `crush` field of stair
/// steps in vanilla.
const STAIRS_UNINITIALIZED_CRUSH_FIELD_VALUE: i32 = 10;

/// `floor_e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorKind {
    /// Lower floor to highest surrounding floor.
    LowerFloor,
    /// Lower floor to lowest surrounding floor.
    LowerFloorToLowest,
    /// Lower floor to highest surrounding floor VERY FAST.
    TurboLower,
    /// Raise floor to lowest surrounding CEILING.
    RaiseFloor,
    /// Raise floor to next highest surrounding floor.
    RaiseFloorToNearest,
    /// Raise floor to shortest height texture around it.
    RaiseToTexture,
    /// Lower floor to lowest surrounding floor and change floorpic.
    LowerAndChange,
    RaiseFloor24,
    RaiseFloor24AndChange,
    RaiseFloorCrush,
    /// Raise to next highest floor, turbo-speed.
    RaiseFloorTurbo,
    DonutRaise,
    RaiseFloor512,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StairKind {
    /// Slowly build by 8.
    Build8,
    /// Quickly build by 16.
    Turbo16,
}

/// `floormove_t`.
#[derive(Clone, Debug)]
pub struct FloorMove {
    pub kind: FloorKind,
    /// C `boolean` value (0/1, or 10 for stairs; see `plane::move_plane`).
    pub crush: i32,
    pub sector: SectorId,
    pub direction: i32,
    pub newspecial: i32,
    pub texture: FlatNum,
    pub floordestheight: Fixed,
    pub speed: Fixed,
}

/// `T_MoveFloor`: move a floor to its destination (up or down).
pub fn t_move_floor(w: &mut World, id: ThinkerId) {
    let Some(SpecialThinker::Floor(f)) = special_mut(w, id) else { return };
    let f = f.clone();
    let res = move_plane(w, f.sector, f.speed, f.floordestheight, f.crush, Plane::Floor, f.direction);
    if w.leveltime & 7 == 0 {
        w.start_sound(SoundSource::Sector(f.sector), SFX_STNMOV);
    }
    if res == MoveResult::PastDest {
        w.level.sector_mut(f.sector).special_data = None;
        if (f.direction == 1 && f.kind == FloorKind::DonutRaise)
            || (f.direction == -1 && f.kind == FloorKind::LowerAndChange)
        {
            let s = w.level.sector_mut(f.sector);
            s.special = f.newspecial as i16;
            s.floor_pic = f.texture;
        }
        finish(w, id, f.sector);
        if w.params.version > GameVersion::Doom1_2 {
            w.start_sound(SoundSource::Sector(f.sector), SFX_PSTOP);
        }
    }
}

/// `textureheight[]` for a sidedef's bottom texture.
fn bottom_texture_height(w: &World, side: crate::ids::SideId) -> i32 {
    let t = w.level.side(side).bottom_texture.0 as usize;
    // Texture 0 ("-") still has a height in vanilla: missing lower textures
    // count as the first texture's height. Preserved.
    w.content.texture_heights.get(t).copied().unwrap_or(0)
}

/// `EV_DoFloor`: handle floor types.
pub fn ev_do_floor(w: &mut World, line: LineId, kind: FloorKind) -> bool {
    let tag = w.level.line(line).tag;
    ev_do_floor_tag(w, tag, Some(line), kind)
}

/// `EV_DoFloor` on a tag; `line` is only needed for
/// `RaiseFloor24AndChange` (front sector texture/special).
pub fn ev_do_floor_tag(w: &mut World, tag: i16, line: Option<LineId>, kind: FloorKind) -> bool {
    let mut rtn = false;
    let mut secnum = -1;
    while let Some(sec) = crate::spec::find_sector_from_tag(w, tag, secnum) {
        secnum = sec.0 as i32;
        // ALREADY MOVING? IF SO, KEEP GOING...
        if w.level.sector(sec).special_data.is_some() {
            continue;
        }
        // New floor thinker.
        rtn = true;
        let s = w.level.sector(sec).clone();
        let mut f = FloorMove {
            kind,
            crush: 0,
            sector: sec,
            direction: 0,
            newspecial: 0,
            texture: FlatNum(0),
            floordestheight: Fixed(0),
            speed: FLOORSPEED,
        };
        match kind {
            FloorKind::LowerFloor => {
                f.direction = -1;
                f.floordestheight = find_highest_floor_surrounding(w, sec);
            }
            FloorKind::LowerFloorToLowest => {
                f.direction = -1;
                f.floordestheight = find_lowest_floor_surrounding(w, sec);
            }
            FloorKind::TurboLower => {
                f.direction = -1;
                f.speed = FLOORSPEED * 4;
                f.floordestheight = find_highest_floor_surrounding(w, sec);
                if w.params.version <= GameVersion::Doom1_2 || f.floordestheight != s.floor_height {
                    f.floordestheight += Fixed(8 * FRACUNIT);
                }
            }
            FloorKind::RaiseFloorCrush | FloorKind::RaiseFloor => {
                if kind == FloorKind::RaiseFloorCrush {
                    f.crush = 1;
                }
                f.direction = 1;
                f.floordestheight = find_lowest_ceiling_surrounding(w, sec);
                if f.floordestheight > s.ceiling_height {
                    f.floordestheight = s.ceiling_height;
                }
                if kind == FloorKind::RaiseFloorCrush {
                    f.floordestheight -= Fixed(8 * FRACUNIT);
                }
            }
            FloorKind::RaiseFloorTurbo => {
                f.direction = 1;
                f.speed = FLOORSPEED * 4;
                f.floordestheight = find_next_highest_floor(w, sec, s.floor_height);
            }
            FloorKind::RaiseFloorToNearest => {
                f.direction = 1;
                f.floordestheight = find_next_highest_floor(w, sec, s.floor_height);
            }
            FloorKind::RaiseFloor24 => {
                f.direction = 1;
                f.floordestheight = s.floor_height + Fixed(24 * FRACUNIT);
            }
            FloorKind::RaiseFloor512 => {
                f.direction = 1;
                f.floordestheight = s.floor_height + Fixed(512 * FRACUNIT);
            }
            FloorKind::RaiseFloor24AndChange => {
                f.direction = 1;
                f.floordestheight = s.floor_height + Fixed(24 * FRACUNIT);
                let front = w.level.line(line.expect("RaiseFloor24AndChange needs a line")).front_sector.unwrap();
                let (pic, special) = (w.level.sector(front).floor_pic, w.level.sector(front).special);
                let ss = w.level.sector_mut(sec);
                ss.floor_pic = pic;
                ss.special = special;
            }
            FloorKind::RaiseToTexture => {
                let mut minsize = i32::MAX;
                f.direction = 1;
                for &l in w.level.lines_of(sec) {
                    let line = w.level.line(l);
                    if line.flags & ml::TWOSIDED != 0 {
                        for side in line.sidenum.iter().flatten() {
                            let h = bottom_texture_height(w, *side);
                            if h < minsize {
                                minsize = h;
                            }
                        }
                    }
                }
                f.floordestheight = s.floor_height + Fixed(minsize);
            }
            FloorKind::LowerAndChange => {
                f.direction = -1;
                f.floordestheight = find_lowest_floor_surrounding(w, sec);
                f.texture = s.floor_pic;
                for &l in w.level.lines_of(sec) {
                    let line = w.level.line(l);
                    if line.flags & ml::TWOSIDED == 0 {
                        continue;
                    }
                    let (Some(s0), Some(s1)) = (line.sidenum[0], line.sidenum[1]) else { continue };
                    let other = if w.level.side(s0).sector == sec { w.level.side(s1).sector } else { w.level.side(s0).sector };
                    let o = w.level.sector(other);
                    if w.params.version < GameVersion::Doom1_5 || o.floor_height == f.floordestheight {
                        f.texture = o.floor_pic;
                        f.newspecial = o.special as i32;
                        break;
                    }
                }
            }
            FloorKind::DonutRaise => {}
        }
        let id = add_special(w, SpecialThinker::Floor(f));
        w.level.sector_mut(sec).special_data = Some(id);
    }
    rtn
}

/// `EV_BuildStairs`.
pub fn ev_build_stairs(w: &mut World, line: LineId, kind: StairKind) -> bool {
    let (speed, stairsize) = match kind {
        StairKind::Build8 => (FLOORSPEED / 4, Fixed(8 * FRACUNIT)),
        StairKind::Turbo16 => (FLOORSPEED * 4, Fixed(16 * FRACUNIT)),
    };
    let mut rtn = false;
    let mut secnum = -1;
    while let Some(start) = find_sector_from_line_tag(w, line, secnum) {
        secnum = start.0 as i32;
        // ALREADY MOVING? IF SO, KEEP GOING...
        if w.level.sector(start).special_data.is_some() {
            continue;
        }
        // New floor thinker.
        rtn = true;
        let mut sec = start;
        let mut height = w.level.sector(sec).floor_height + stairsize;
        let step = |sector: SectorId, height: Fixed| FloorMove {
            kind: FloorKind::LowerFloor,
            crush: STAIRS_UNINITIALIZED_CRUSH_FIELD_VALUE,
            sector,
            direction: 1,
            newspecial: 0,
            texture: FlatNum(0),
            floordestheight: height,
            speed,
        };
        let id = add_special(w, SpecialThinker::Floor(step(sec, height)));
        w.level.sector_mut(sec).special_data = Some(id);
        let texture = w.level.sector(sec).floor_pic;

        // Find next sector to raise:
        // 1. Find 2-sided line with same sector side[0]
        // 2. Other side is the next sector to raise
        loop {
            let mut ok = false;
            let lines: Vec<LineId> = w.level.lines_of(sec).to_vec();
            for l in lines {
                let line = w.level.line(l);
                if line.flags & ml::TWOSIDED == 0 {
                    continue;
                }
                if line.front_sector != Some(sec) {
                    continue;
                }
                // Vanilla dereferences backsector unconditionally here.
                let Some(tsec) = line.back_sector else { continue };
                if w.level.sector(tsec).floor_pic != texture {
                    continue;
                }
                height += stairsize;
                if w.level.sector(tsec).special_data.is_some() {
                    continue;
                }
                sec = tsec;
                secnum = tsec.0 as i32;
                let id = add_special(w, SpecialThinker::Floor(step(sec, height)));
                w.level.sector_mut(sec).special_data = Some(id);
                ok = true;
                break;
            }
            if !ok {
                break;
            }
        }
    }
    rtn
}

/// `EV_DoDonut`: special stuff that can't be categorized.
pub fn ev_do_donut(w: &mut World, line: LineId) -> bool {
    let mut rtn = false;
    let mut secnum = -1;
    while let Some(s1) = find_sector_from_line_tag(w, line, secnum) {
        secnum = s1.0 as i32;
        // ALREADY MOVING? IF SO, KEEP GOING...
        if w.level.sector(s1).special_data.is_some() {
            continue;
        }
        rtn = true;
        let first = w.level.lines_of(s1)[0];
        let Some(s2) = next_sector(&w.level, first, s1) else {
            // Chocolate: "linedef had no second sidedef! Unexpected behavior
            // may occur in Vanilla Doom."
            break;
        };
        let lines: Vec<LineId> = w.level.lines_of(s2).to_vec();
        for l in lines {
            let s3 = w.level.line(l).back_sector;
            if s3 == Some(s1) {
                continue;
            }
            let (s3_floorheight, s3_floorpic) = match s3 {
                Some(s3) => (w.level.sector(s3).floor_height, w.level.sector(s3).floor_pic),
                None => {
                    // Chocolate's DonutOverrun defaults (vanilla reads
                    // memory past a NULL back sector).
                    w.note_overflow("EV_DoDonut NULL back sector (DonutOverrun defaults emulated)");
                    (Fixed(0), FlatNum(0x16))
                }
            };
            // Spawn rising slime.
            let id = add_special(
                w,
                SpecialThinker::Floor(FloorMove {
                    kind: FloorKind::DonutRaise,
                    crush: 0,
                    sector: s2,
                    direction: 1,
                    newspecial: 0,
                    texture: s3_floorpic,
                    floordestheight: s3_floorheight,
                    speed: FLOORSPEED / 2,
                }),
            );
            w.level.sector_mut(s2).special_data = Some(id);
            // Spawn lowering donut-hole.
            let id = add_special(
                w,
                SpecialThinker::Floor(FloorMove {
                    kind: FloorKind::LowerFloor,
                    crush: 0,
                    sector: s1,
                    direction: -1,
                    newspecial: 0,
                    texture: FlatNum(0),
                    floordestheight: s3_floorheight,
                    speed: FLOORSPEED / 2,
                }),
            );
            w.level.sector_mut(s1).special_data = Some(id);
            break;
        }
    }
    rtn
}
