//! Vertical doors (p_doors.c).

use doom_core::{Fixed, FRACUNIT};

use super::plane::{move_plane, MoveResult, Plane};
use super::{add_special, finish, special_mut, SpecialThinker};
use crate::ids::{LineId, MobjId, SectorId, ThinkerId};
use crate::info::*;
use crate::spec::{find_lowest_ceiling_surrounding, find_sector_from_tag};
use crate::world::{SoundSource, World};

pub const VDOORSPEED: Fixed = Fixed(FRACUNIT * 2);
pub const VDOORWAIT: i32 = 150;
const TICRATE: i32 = 35;

/// `vldoor_e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorKind {
    Normal,
    Close30ThenOpen,
    Close,
    Open,
    RaiseIn5Mins,
    BlazeRaise,
    BlazeOpen,
    BlazeClose,
}

/// `vldoor_t`.
#[derive(Clone, Debug)]
pub struct VerticalDoor {
    pub kind: DoorKind,
    pub sector: SectorId,
    pub topheight: Fixed,
    pub speed: Fixed,
    /// 1 = up, 0 = waiting at top, -1 = down, 2 = initial wait (5 min).
    pub direction: i32,
    /// Tics to wait at the top.
    pub topwait: i32,
    /// (Keep in case a door going down is reset.) When it reaches 0, start
    /// going down.
    pub topcountdown: i32,
}

fn door(w: &mut World, id: ThinkerId) -> Option<&mut VerticalDoor> {
    match special_mut(w, id)? {
        SpecialThinker::Door(d) => Some(d),
        _ => None,
    }
}

fn snd(w: &mut World, sector: SectorId, sfx: SfxNum) {
    w.start_sound(SoundSource::Sector(sector), sfx);
}

/// `T_VerticalDoor`.
pub fn t_vertical_door(w: &mut World, id: ThinkerId) {
    let Some(d) = door(w, id) else { return };
    let d0 = d.clone();
    let sec = d0.sector;
    match d0.direction {
        0 => {
            // WAITING
            let d = door(w, id).unwrap();
            d.topcountdown -= 1;
            if d.topcountdown == 0 {
                match d.kind {
                    DoorKind::BlazeRaise => {
                        d.direction = -1; // time to go back down
                        snd(w, sec, SFX_BDCLS);
                    }
                    DoorKind::Normal => {
                        d.direction = -1; // time to go back down
                        snd(w, sec, SFX_DORCLS);
                    }
                    DoorKind::Close30ThenOpen => {
                        d.direction = 1;
                        snd(w, sec, SFX_DOROPN);
                    }
                    _ => {}
                }
            }
        }
        2 => {
            // INITIAL WAIT
            let d = door(w, id).unwrap();
            d.topcountdown -= 1;
            if d.topcountdown == 0 && d.kind == DoorKind::RaiseIn5Mins {
                d.direction = 1;
                d.kind = DoorKind::Normal;
                snd(w, sec, SFX_DOROPN);
            }
        }
        -1 => {
            // DOWN
            let floor = w.level.sector(sec).floor_height;
            let res = move_plane(w, sec, d0.speed, floor, 0, Plane::Ceiling, -1);
            if res == MoveResult::PastDest {
                match d0.kind {
                    DoorKind::BlazeRaise | DoorKind::BlazeClose => {
                        finish(w, id, sec); // unlink and free
                        snd(w, sec, SFX_BDCLS);
                    }
                    DoorKind::Normal | DoorKind::Close => finish(w, id, sec),
                    DoorKind::Close30ThenOpen => {
                        let d = door(w, id).unwrap();
                        d.direction = 0;
                        d.topcountdown = TICRATE * 30;
                    }
                    _ => {}
                }
            } else if res == MoveResult::Crushed {
                match d0.kind {
                    DoorKind::BlazeClose | DoorKind::Close => {} // DO NOT GO BACK UP!
                    _ => {
                        door(w, id).unwrap().direction = 1;
                        snd(w, sec, SFX_DOROPN);
                    }
                }
            }
        }
        1 => {
            // UP
            let res = move_plane(w, sec, d0.speed, d0.topheight, 0, Plane::Ceiling, 1);
            if res == MoveResult::PastDest {
                match d0.kind {
                    DoorKind::BlazeRaise | DoorKind::Normal => {
                        let d = door(w, id).unwrap();
                        d.direction = 0; // wait at top
                        d.topcountdown = d.topwait;
                    }
                    DoorKind::Close30ThenOpen | DoorKind::BlazeOpen | DoorKind::Open => finish(w, id, sec),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// `EV_DoLockedDoor`: move a locked door up/down.
pub fn ev_do_locked_door(w: &mut World, line: LineId, kind: DoorKind, thing: MobjId) -> bool {
    let Some(p) = w.mo(thing).player else { return false };
    use crate::player::NUMCARDS;
    let cards: [bool; NUMCARDS] = w.players[p].cards;
    // it_bluecard, it_yellowcard, it_redcard, it_blueskull, it_yellowskull, it_redskull
    let (have, msg) = match w.level.line(line).special {
        99 | 133 => (cards[0] || cards[3], PD_BLUEO),
        134 | 135 => (cards[2] || cards[5], PD_REDO),
        136 | 137 => (cards[1] || cards[4], PD_YELLOWO),
        _ => (true, ""),
    };
    if !have {
        w.players[p].message = Some(msg);
        w.start_sound(SoundSource::None, SFX_OOF);
        return false;
    }
    ev_do_door(w, line, kind)
}

/// `EV_DoDoor`.
pub fn ev_do_door(w: &mut World, line: LineId, kind: DoorKind) -> bool {
    let tag = w.level.line(line).tag;
    ev_do_door_tag(w, tag, kind)
}

/// `EV_DoDoor` on a tag (only the line's tag is used).
pub fn ev_do_door_tag(w: &mut World, tag: i16, kind: DoorKind) -> bool {
    let mut rtn = false;
    let mut secnum = -1;
    while let Some(sec) = find_sector_from_tag(w, tag, secnum) {
        secnum = sec.0 as i32;
        if w.level.sector(sec).special_data.is_some() {
            continue;
        }
        // New door thinker.
        rtn = true;
        let ceiling = w.level.sector(sec).ceiling_height;
        let mut d = VerticalDoor {
            kind,
            sector: sec,
            topheight: Fixed(0),
            speed: VDOORSPEED,
            direction: 0,
            topwait: VDOORWAIT,
            topcountdown: 0,
        };
        let mut sound = None;
        match kind {
            DoorKind::BlazeClose => {
                d.topheight = find_lowest_ceiling_surrounding(w, sec) - Fixed(4 * FRACUNIT);
                d.direction = -1;
                d.speed = VDOORSPEED * 4;
                sound = Some(SFX_BDCLS);
            }
            DoorKind::Close => {
                d.topheight = find_lowest_ceiling_surrounding(w, sec) - Fixed(4 * FRACUNIT);
                d.direction = -1;
                sound = Some(SFX_DORCLS);
            }
            DoorKind::Close30ThenOpen => {
                d.topheight = ceiling;
                d.direction = -1;
                sound = Some(SFX_DORCLS);
            }
            DoorKind::BlazeRaise | DoorKind::BlazeOpen => {
                d.direction = 1;
                d.topheight = find_lowest_ceiling_surrounding(w, sec) - Fixed(4 * FRACUNIT);
                d.speed = VDOORSPEED * 4;
                if d.topheight != ceiling {
                    sound = Some(SFX_BDOPN);
                }
            }
            DoorKind::Normal | DoorKind::Open => {
                d.direction = 1;
                d.topheight = find_lowest_ceiling_surrounding(w, sec) - Fixed(4 * FRACUNIT);
                if d.topheight != ceiling {
                    sound = Some(SFX_DOROPN);
                }
            }
            DoorKind::RaiseIn5Mins => {}
        }
        let id = add_special(w, SpecialThinker::Door(d));
        w.level.sector_mut(sec).special_data = Some(id);
        if let Some(s) = sound {
            snd(w, sec, s);
        }
    }
    rtn
}

/// Messages (dstrings / d_englsh.h).
pub const PD_BLUEO: &str = "You need a blue key to activate this object";
pub const PD_REDO: &str = "You need a red key to activate this object";
pub const PD_YELLOWO: &str = "You need a yellow key to activate this object";
pub const PD_BLUEK: &str = "You need a blue key to open this door";
pub const PD_REDK: &str = "You need a red key to open this door";
pub const PD_YELLOWK: &str = "You need a yellow key to open this door";

/// `EV_VerticalDoor`: open a door manually, no tag value.
pub fn ev_vertical_door(w: &mut World, line: LineId, thing: MobjId) {
    let special = w.level.line(line).special;
    let player = w.mo(thing).player;
    // Check for locks.
    let lock = match special {
        26 | 32 => Some(([0usize, 3], PD_BLUEK)),
        27 | 34 => Some(([1, 4], PD_YELLOWK)),
        28 | 33 => Some(([2, 5], PD_REDK)),
        _ => None,
    };
    if let Some((cards, msg)) = lock {
        let Some(p) = player else { return };
        if !w.players[p].cards[cards[0]] && !w.players[p].cards[cards[1]] {
            w.players[p].message = Some(msg);
            w.start_sound(SoundSource::None, SFX_OOF);
            return;
        }
    }

    // If the sector has an active thinker, use it. Only front sides can be
    // used, so the door sector is the back side's.
    let Some(back) = w.level.line(line).sidenum[1] else {
        // Chocolate: "EV_VerticalDoor: DR special type on 1-sided linedef".
        w.note_unimplemented("EV_VerticalDoor on a one-sided line (vanilla crashes)");
        return;
    };
    let sec = w.level.side(back).sector;

    if let Some(sd) = w.level.sector(sec).special_data {
        if matches!(special, 1 | 26 | 27 | 28 | 117) {
            // ONLY FOR "RAISE" DOORS, NOT "OPEN"s. The C code treats any
            // mover as a door (type punning); see `punned_direction`.
            let Some(s) = super::special(w, sd) else { return };
            if s.punned_direction() == -1 {
                special_mut(w, sd).unwrap().set_punned_direction(1); // go back up
            } else {
                if player.is_none() {
                    return; // JDC: bad guys never close doors
                }
                // Chocolate distinguishes the door and plat cases by the
                // thinker function; both end up writing -1 at the same offset.
                match special_mut(w, sd).unwrap() {
                    SpecialThinker::Plat(p) => p.wait = -1,
                    s => s.set_punned_direction(-1), // start going down immediately
                }
            }
            return;
        }
    }

    // For proper sound.
    match special {
        117 | 118 => w.start_sound(SoundSource::Sector(sec), SFX_BDOPN), // blazing door
        _ => w.start_sound(SoundSource::Sector(sec), SFX_DOROPN),          // normal / locked door sound
    }

    // New door thinker.
    let mut d = VerticalDoor {
        kind: DoorKind::Normal,
        sector: sec,
        topheight: Fixed(0),
        speed: VDOORSPEED,
        direction: 1,
        topwait: VDOORWAIT,
        topcountdown: 0,
    };
    match special {
        1 | 26 | 27 | 28 => d.kind = DoorKind::Normal,
        31 | 32 | 33 | 34 => {
            d.kind = DoorKind::Open;
            w.level.lines[line.idx()].special = 0;
        }
        117 => {
            d.kind = DoorKind::BlazeRaise;
            d.speed = VDOORSPEED * 4;
        }
        118 => {
            d.kind = DoorKind::BlazeOpen;
            w.level.lines[line.idx()].special = 0;
            d.speed = VDOORSPEED * 4;
        }
        _ => {}
    }
    // Find the top and bottom of the movement range.
    d.topheight = find_lowest_ceiling_surrounding(w, sec) - Fixed(4 * FRACUNIT);
    let id = add_special(w, SpecialThinker::Door(d));
    w.level.sector_mut(sec).special_data = Some(id);
}

/// `P_SpawnDoorCloseIn30`: spawn a door that closes after 30 seconds.
pub fn spawn_door_close_in_30(w: &mut World, sec: SectorId) {
    let id = add_special(
        w,
        SpecialThinker::Door(VerticalDoor {
            kind: DoorKind::Normal,
            sector: sec,
            topheight: Fixed(0),
            speed: VDOORSPEED,
            direction: 0,
            topwait: 0,
            topcountdown: 30 * TICRATE,
        }),
    );
    let s = w.level.sector_mut(sec);
    s.special_data = Some(id);
    s.special = 0;
}

/// `P_SpawnDoorRaiseIn5Mins`: spawn a door that opens after 5 minutes.
pub fn spawn_door_raise_in_5_mins(w: &mut World, sec: SectorId) {
    let topheight = find_lowest_ceiling_surrounding(w, sec) - Fixed(4 * FRACUNIT);
    let id = add_special(
        w,
        SpecialThinker::Door(VerticalDoor {
            kind: DoorKind::RaiseIn5Mins,
            sector: sec,
            topheight,
            speed: VDOORSPEED,
            direction: 2,
            topwait: VDOORWAIT,
            topcountdown: 5 * 60 * TICRATE,
        }),
    );
    let s = w.level.sector_mut(sec);
    s.special_data = Some(id);
    s.special = 0;
}
