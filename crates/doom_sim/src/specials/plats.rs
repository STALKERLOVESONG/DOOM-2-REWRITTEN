//! Lifts and platforms (p_plats.c).

use doom_content::game::GameVersion;
use doom_core::{Fixed, FRACUNIT};

use super::plane::{move_plane, MoveResult, Plane};
use super::{add_special, special_mut, SpecialThinker};
use crate::ids::{LineId, SectorId, ThinkerId};
use crate::info::*;
use crate::spec::{find_highest_floor_surrounding, find_lowest_floor_surrounding, find_next_highest_floor, find_sector_from_line_tag};
use crate::thinker::ThinkFn;
use crate::world::{SoundSource, World};

pub const PLATWAIT: i32 = 3;
pub const PLATSPEED: Fixed = Fixed(FRACUNIT);
pub const MAXPLATS: usize = 30;
const TICRATE: i32 = 35;

/// `plat_e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatStatus {
    Up = 0,
    Down = 1,
    Waiting = 2,
    InStasis = 3,
}

/// `plattype_e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatKind {
    PerpetualRaise,
    DownWaitUpStay,
    RaiseAndChange,
    RaiseToNearestAndChange,
    BlazeDwus,
    /// Hexen `Plat_UpWaitDownStay`: up, wait, back down, done.
    UpWaitDownStay,
}

/// `plat_t`.
#[derive(Clone, Debug)]
pub struct Plat {
    pub sector: SectorId,
    pub speed: Fixed,
    pub low: Fixed,
    pub high: Fixed,
    pub wait: i32,
    pub count: i32,
    pub status: PlatStatus,
    pub oldstatus: PlatStatus,
    pub crush: bool,
    pub tag: i16,
    pub kind: PlatKind,
}

fn plat(w: &mut World, id: ThinkerId) -> Option<&mut Plat> {
    match special_mut(w, id)? {
        SpecialThinker::Plat(p) => Some(p),
        _ => None,
    }
}

/// `T_PlatRaise`: move a plat up and down.
pub fn t_plat_raise(w: &mut World, id: ThinkerId) {
    let Some(p) = plat(w, id) else { return };
    let p = p.clone();
    let sec = p.sector;
    match p.status {
        PlatStatus::Up => {
            let res = move_plane(w, sec, p.speed, p.high, p.crush as i32, Plane::Floor, 1);
            if matches!(p.kind, PlatKind::RaiseAndChange | PlatKind::RaiseToNearestAndChange) && w.leveltime & 7 == 0 {
                w.start_sound(SoundSource::Sector(sec), SFX_STNMOV);
            }
            if res == MoveResult::Crushed && !p.crush {
                let pm = plat(w, id).unwrap();
                pm.count = pm.wait;
                pm.status = PlatStatus::Down;
                w.start_sound(SoundSource::Sector(sec), SFX_PSTART);
            } else if res == MoveResult::PastDest {
                let pm = plat(w, id).unwrap();
                pm.count = pm.wait;
                pm.status = PlatStatus::Waiting;
                w.start_sound(SoundSource::Sector(sec), SFX_PSTOP);
                match p.kind {
                    PlatKind::BlazeDwus | PlatKind::DownWaitUpStay | PlatKind::RaiseAndChange => remove_active_plat(w, id),
                    PlatKind::RaiseToNearestAndChange => {
                        if w.params.version > GameVersion::Doom1_2 {
                            remove_active_plat(w, id);
                        }
                    }
                    _ => {}
                }
            }
        }
        PlatStatus::Down => {
            let res = move_plane(w, sec, p.speed, p.low, 0, Plane::Floor, -1);
            if res == MoveResult::PastDest {
                let pm = plat(w, id).unwrap();
                pm.count = pm.wait;
                pm.status = PlatStatus::Waiting;
                w.start_sound(SoundSource::Sector(sec), SFX_PSTOP);
                if p.kind == PlatKind::UpWaitDownStay {
                    remove_active_plat(w, id);
                }
            }
        }
        PlatStatus::Waiting => {
            let floor = w.level.sector(sec).floor_height;
            let pm = plat(w, id).unwrap();
            pm.count -= 1;
            if pm.count == 0 {
                pm.status = if floor == pm.low { PlatStatus::Up } else { PlatStatus::Down };
                w.start_sound(SoundSource::Sector(sec), SFX_PSTART);
            }
        }
        PlatStatus::InStasis => {}
    }
}

/// `EV_DoPlat`: do platforms; `amount` is only used for some platforms.
pub fn ev_do_plat(w: &mut World, line: LineId, kind: PlatKind, amount: i32) -> bool {
    // Activate all <type> plats that are in_stasis.
    if kind == PlatKind::PerpetualRaise {
        let tag = w.level.line(line).tag;
        activate_in_stasis(w, tag);
    }
    let mut rtn = false;
    let mut secnum = -1;
    while let Some(sec) = find_sector_from_line_tag(w, line, secnum) {
        secnum = sec.0 as i32;
        if w.level.sector(sec).special_data.is_some() {
            continue;
        }
        // Find lowest & highest floors around sector.
        rtn = true;
        let tag = w.level.line(line).tag;
        let s = w.level.sector(sec).clone();
        let mut p = Plat {
            sector: sec,
            speed: PLATSPEED,
            low: Fixed(0),
            high: Fixed(0),
            wait: 0,
            count: 0,
            status: PlatStatus::Up,
            oldstatus: PlatStatus::Up,
            crush: false,
            tag,
            kind,
        };
        // The thinker is added before the type-specific setup, which can
        // consume P_Random (perpetual raise).
        let id = add_special(w, SpecialThinker::Plat(p.clone()));
        w.level.sector_mut(sec).special_data = Some(id);
        let front_side_floor = || {
            let side = w.level.line(line).sidenum[0].unwrap();
            w.level.sector(w.level.side(side).sector).floor_pic
        };
        let mut sound = SFX_PSTART;
        match kind {
            PlatKind::RaiseToNearestAndChange => {
                p.speed = PLATSPEED / 2;
                let pic = front_side_floor();
                w.level.sector_mut(sec).floor_pic = pic;
                p.high = find_next_highest_floor(w, sec, s.floor_height);
                p.wait = 0;
                p.status = PlatStatus::Up;
                // No more damage, if applicable.
                w.level.sector_mut(sec).special = 0;
                sound = SFX_STNMOV;
            }
            PlatKind::RaiseAndChange => {
                p.speed = PLATSPEED / 2;
                let pic = front_side_floor();
                w.level.sector_mut(sec).floor_pic = pic;
                p.high = s.floor_height + Fixed(amount * FRACUNIT);
                p.wait = 0;
                p.status = PlatStatus::Up;
                sound = SFX_STNMOV;
            }
            // Only Hexen-style specials make these.
            PlatKind::UpWaitDownStay => {}
            PlatKind::DownWaitUpStay | PlatKind::BlazeDwus => {
                p.speed = if kind == PlatKind::BlazeDwus { PLATSPEED * 8 } else { PLATSPEED * 4 };
                p.low = find_lowest_floor_surrounding(w, sec);
                if p.low > s.floor_height {
                    p.low = s.floor_height;
                }
                p.high = s.floor_height;
                p.wait = TICRATE * PLATWAIT;
                p.status = PlatStatus::Down;
            }
            PlatKind::PerpetualRaise => {
                p.low = find_lowest_floor_surrounding(w, sec);
                if p.low > s.floor_height {
                    p.low = s.floor_height;
                }
                p.high = find_highest_floor_surrounding(w, sec);
                if p.high < s.floor_height {
                    p.high = s.floor_height;
                }
                p.wait = TICRATE * PLATWAIT;
                p.status = if w.p_random() & 1 == 0 { PlatStatus::Up } else { PlatStatus::Down };
            }
        }
        if let Some(slot) = plat(w, id) {
            *slot = p;
        }
        w.start_sound(SoundSource::Sector(sec), sound);
        add_active_plat(w, id);
    }
    rtn
}

/// `P_ActivateInStasis`.
pub fn activate_in_stasis(w: &mut World, tag: i16) {
    for i in 0..MAXPLATS {
        let Some(id) = w.activeplats[i] else { continue };
        let Some(p) = plat(w, id) else { continue };
        if p.tag == tag && p.status == PlatStatus::InStasis {
            p.status = p.oldstatus;
            w.thinkers.set_func(id, ThinkFn::Active);
        }
    }
}

/// `EV_StopPlat`.
pub fn ev_stop_plat(w: &mut World, line: LineId) {
    let tag = w.level.line(line).tag;
    for j in 0..MAXPLATS {
        let Some(id) = w.activeplats[j] else { continue };
        let Some(p) = plat(w, id) else { continue };
        if p.status != PlatStatus::InStasis && p.tag == tag {
            p.oldstatus = p.status;
            p.status = PlatStatus::InStasis;
            w.thinkers.set_func(id, ThinkFn::Stasis);
        }
    }
}

/// `P_AddActivePlat` without vanilla's 30-plat limit: Hexen-style plats
/// beyond it run unregistered (they can't be stopped by tag).
pub fn add_plat_unlimited(w: &mut World, id: ThinkerId) {
    if let Some(slot) = w.activeplats.iter_mut().find(|s| s.is_none()) {
        *slot = Some(id);
    }
}

/// `P_AddActivePlat`.
pub fn add_active_plat(w: &mut World, id: ThinkerId) {
    match w.activeplats.iter_mut().find(|s| s.is_none()) {
        Some(slot) => *slot = Some(id),
        None => panic!("P_AddActivePlat: no more plats!"),
    }
}

/// `P_RemoveActivePlat`. A plat missing from the list (an unregistered
/// Hexen-style plat) is unlinked and freed; vanilla can't get here.
pub fn remove_active_plat(w: &mut World, id: ThinkerId) {
    let Some(i) = w.activeplats.iter().position(|s| *s == Some(id)) else {
        if let Some(SpecialThinker::Plat(p)) = super::special(w, id) {
            let sec = p.sector;
            super::finish(w, id, sec);
        }
        return;
    };
    if let Some(SpecialThinker::Plat(p)) = super::special(w, id) {
        let sec = p.sector;
        w.level.sector_mut(sec).special_data = None;
    }
    w.thinkers.remove(id);
    w.activeplats[i] = None;
}
