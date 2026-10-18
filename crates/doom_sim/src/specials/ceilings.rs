//! Ceilings and crushers (p_ceilng.c).

use doom_core::{Fixed, FRACUNIT};

use super::plane::{move_plane, MoveResult, Plane};
use super::{add_special, special_mut, SpecialThinker};
use crate::ids::{LineId, SectorId, ThinkerId};
use crate::info::*;
use crate::spec::{find_highest_ceiling_surrounding, find_sector_from_line_tag};
use crate::thinker::ThinkFn;
use crate::world::{SoundSource, World};

pub const CEILSPEED: Fixed = Fixed(FRACUNIT);
pub const MAXCEILINGS: usize = 30;

/// `ceiling_e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeilingKind {
    LowerToFloor,
    RaiseToHighest,
    LowerAndCrush,
    CrushAndRaise,
    FastCrushAndRaise,
    SilentCrushAndRaise,
}

/// `ceiling_t`.
#[derive(Clone, Debug)]
pub struct CeilingMove {
    pub kind: CeilingKind,
    pub sector: SectorId,
    pub bottomheight: Fixed,
    pub topheight: Fixed,
    pub speed: Fixed,
    pub crush: bool,
    /// 1 = up, 0 = waiting (in stasis), -1 = down.
    pub direction: i32,
    /// ID.
    pub tag: i16,
    pub olddirection: i32,
}

fn ceiling(w: &mut World, id: ThinkerId) -> Option<&mut CeilingMove> {
    match special_mut(w, id)? {
        SpecialThinker::Ceiling(c) => Some(c),
        _ => None,
    }
}

/// `T_MoveCeiling`.
pub fn t_move_ceiling(w: &mut World, id: ThinkerId) {
    let Some(c) = ceiling(w, id) else { return };
    let c = c.clone();
    let sec = c.sector;
    match c.direction {
        0 => {} // IN STASIS
        1 => {
            // UP
            let res = move_plane(w, sec, c.speed, c.topheight, 0, Plane::Ceiling, c.direction);
            if w.leveltime & 7 == 0 && c.kind != CeilingKind::SilentCrushAndRaise {
                w.start_sound(SoundSource::Sector(sec), SFX_STNMOV);
            }
            if res == MoveResult::PastDest {
                match c.kind {
                    CeilingKind::RaiseToHighest => remove_active_ceiling(w, id),
                    CeilingKind::SilentCrushAndRaise | CeilingKind::FastCrushAndRaise | CeilingKind::CrushAndRaise => {
                        if c.kind == CeilingKind::SilentCrushAndRaise {
                            w.start_sound(SoundSource::Sector(sec), SFX_PSTOP);
                        }
                        ceiling(w, id).unwrap().direction = -1;
                    }
                    _ => {}
                }
            }
        }
        -1 => {
            // DOWN
            let res = move_plane(w, sec, c.speed, c.bottomheight, c.crush as i32, Plane::Ceiling, c.direction);
            if w.leveltime & 7 == 0 && c.kind != CeilingKind::SilentCrushAndRaise {
                w.start_sound(SoundSource::Sector(sec), SFX_STNMOV);
            }
            if res == MoveResult::PastDest {
                match c.kind {
                    CeilingKind::SilentCrushAndRaise | CeilingKind::CrushAndRaise | CeilingKind::FastCrushAndRaise => {
                        if c.kind == CeilingKind::SilentCrushAndRaise {
                            w.start_sound(SoundSource::Sector(sec), SFX_PSTOP);
                        }
                        let cm = ceiling(w, id).unwrap();
                        if matches!(c.kind, CeilingKind::SilentCrushAndRaise | CeilingKind::CrushAndRaise) {
                            cm.speed = CEILSPEED;
                        }
                        cm.direction = 1;
                    }
                    CeilingKind::LowerAndCrush | CeilingKind::LowerToFloor => remove_active_ceiling(w, id),
                    _ => {}
                }
            } else if res == MoveResult::Crushed
                && matches!(
                    c.kind,
                    CeilingKind::SilentCrushAndRaise | CeilingKind::CrushAndRaise | CeilingKind::LowerAndCrush
                )
            {
                ceiling(w, id).unwrap().speed = CEILSPEED / 8;
            }
        }
        _ => {}
    }
}

/// `EV_DoCeiling`: move a ceiling up/down and all around!
pub fn ev_do_ceiling(w: &mut World, line: LineId, kind: CeilingKind) -> bool {
    // Reactivate in-stasis ceilings... for certain types.
    if matches!(kind, CeilingKind::FastCrushAndRaise | CeilingKind::SilentCrushAndRaise | CeilingKind::CrushAndRaise) {
        activate_in_stasis_ceiling(w, line);
    }
    let mut rtn = false;
    let mut secnum = -1;
    while let Some(sec) = find_sector_from_line_tag(w, line, secnum) {
        secnum = sec.0 as i32;
        if w.level.sector(sec).special_data.is_some() {
            continue;
        }
        // New ceiling thinker.
        rtn = true;
        let s = w.level.sector(sec).clone();
        let mut c = CeilingMove {
            kind,
            sector: sec,
            bottomheight: Fixed(0),
            topheight: Fixed(0),
            speed: CEILSPEED,
            crush: false,
            direction: 0,
            tag: s.tag,
            olddirection: 0,
        };
        match kind {
            CeilingKind::FastCrushAndRaise => {
                c.crush = true;
                c.topheight = s.ceiling_height;
                c.bottomheight = s.floor_height + Fixed(8 * FRACUNIT);
                c.direction = -1;
                c.speed = CEILSPEED * 2;
            }
            CeilingKind::SilentCrushAndRaise | CeilingKind::CrushAndRaise | CeilingKind::LowerAndCrush | CeilingKind::LowerToFloor => {
                if matches!(kind, CeilingKind::SilentCrushAndRaise | CeilingKind::CrushAndRaise) {
                    c.crush = true;
                    c.topheight = s.ceiling_height;
                }
                c.bottomheight = s.floor_height;
                if kind != CeilingKind::LowerToFloor {
                    c.bottomheight += Fixed(8 * FRACUNIT);
                }
                c.direction = -1;
                c.speed = CEILSPEED;
            }
            CeilingKind::RaiseToHighest => {
                c.topheight = find_highest_ceiling_surrounding(w, sec);
                c.direction = 1;
                c.speed = CEILSPEED;
            }
        }
        let id = add_special(w, SpecialThinker::Ceiling(c));
        w.level.sector_mut(sec).special_data = Some(id);
        add_active_ceiling(w, id);
    }
    rtn
}

/// `P_AddActiveCeiling`. Silently drops the ceiling if the table is full,
/// as vanilla does.
pub fn add_active_ceiling(w: &mut World, id: ThinkerId) {
    if let Some(slot) = w.activeceilings.iter_mut().find(|s| s.is_none()) {
        *slot = Some(id);
    }
}

/// `P_RemoveActiveCeiling`. Hexen-style ceilings (in `ceiling_data`) are
/// freed even when vanilla's 30-entry list was full.
pub fn remove_active_ceiling(w: &mut World, id: ThinkerId) {
    let sec = match super::special(w, id) {
        Some(SpecialThinker::Ceiling(c)) => Some(c.sector),
        _ => None,
    };
    if let Some(i) = w.activeceilings.iter().position(|s| *s == Some(id)) {
        if let Some(sec) = sec {
            super::finish(w, id, sec);
        } else {
            w.thinkers.remove(id);
        }
        w.activeceilings[i] = None;
    } else if let Some(sec) = sec.filter(|&s| w.level.sector(s).ceiling_data == Some(id)) {
        super::finish(w, id, sec);
    }
}

/// `P_ActivateInStasisCeiling`: restart a ceiling that's in stasis.
pub fn activate_in_stasis_ceiling(w: &mut World, line: LineId) {
    let tag = w.level.line(line).tag;
    for i in 0..MAXCEILINGS {
        let Some(id) = w.activeceilings[i] else { continue };
        let Some(c) = ceiling(w, id) else { continue };
        if c.tag == tag && c.direction == 0 {
            c.direction = c.olddirection;
            w.thinkers.set_func(id, ThinkFn::Active);
        }
    }
}

/// `EV_CeilingCrushStop`: stop a ceiling from crushing!
pub fn ev_ceiling_crush_stop(w: &mut World, line: LineId) -> bool {
    let tag = w.level.line(line).tag;
    let mut rtn = false;
    for i in 0..MAXCEILINGS {
        let Some(id) = w.activeceilings[i] else { continue };
        let Some(c) = ceiling(w, id) else { continue };
        if c.tag == tag && c.direction != 0 {
            c.olddirection = c.direction;
            c.direction = 0; // in-stasis
            w.thinkers.set_func(id, ThinkFn::Stasis);
            rtn = true;
        }
    }
    rtn
}
