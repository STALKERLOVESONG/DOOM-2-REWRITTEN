//! Line and sector specials (p_spec.c, p_switch.c, p_telept.c).

use doom_content::game::{GameMode, GameVersion};
use doom_core::{Fixed, FRACUNIT};

use crate::ids::*;
use crate::info::*;
use crate::level::Level;
use crate::map::teleport_move;
use crate::mobj::flags::*;
use crate::mobj::spawn_mobj;
use crate::player::power;
use crate::specials::ceilings::{ev_ceiling_crush_stop, ev_do_ceiling, CeilingKind};
use crate::specials::doors::{ev_do_door, ev_do_locked_door, ev_vertical_door, spawn_door_close_in_30, spawn_door_raise_in_5_mins, DoorKind};
use crate::specials::floors::{ev_build_stairs, ev_do_donut, ev_do_floor, FloorKind, StairKind};
use crate::specials::lights::*;
use crate::specials::plats::{ev_do_plat, ev_stop_plat, PlatKind};
use crate::thinker::{ThinkFn, ThinkerData};
use crate::world::{GameExit, SoundSource, World};
use doom_content::map::ml;
use doom_core::tables::{finecosine, finesine};

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// `getNextSector`: return the sector on the other side of a two-sided line,
/// or `None` (also for one-sided lines, even with a back sidedef).
pub fn next_sector(level: &Level, line: LineId, sec: SectorId) -> Option<SectorId> {
    let l = level.line(line);
    if l.flags & ml::TWOSIDED == 0 {
        return None;
    }
    if l.front_sector == Some(sec) {
        l.back_sector
    } else {
        l.front_sector
    }
}

fn surrounding(w: &World, sec: SectorId) -> impl Iterator<Item = SectorId> + '_ {
    w.level.lines_of(sec).iter().filter_map(move |&l| next_sector(&w.level, l, sec))
}

/// `P_FindLowestFloorSurrounding`.
pub fn find_lowest_floor_surrounding(w: &World, sec: SectorId) -> Fixed {
    let mut floor = w.level.sector(sec).floor_height;
    for o in surrounding(w, sec) {
        floor = floor.min(w.level.sector(o).floor_height);
    }
    floor
}

/// `P_FindHighestFloorSurrounding`.
pub fn find_highest_floor_surrounding(w: &World, sec: SectorId) -> Fixed {
    let mut floor = Fixed(-500 * FRACUNIT);
    for o in surrounding(w, sec) {
        floor = floor.max(w.level.sector(o).floor_height);
    }
    floor
}

const MAX_ADJOINING_SECTORS: usize = 20;

/// `P_FindNextHighestFloor`, with Chocolate Doom's emulation of vanilla's
/// `heightlist[]` overflow (entry 21 overwrites `height`; 22 crashes).
pub fn find_next_highest_floor(w: &mut World, sec: SectorId, currentheight: Fixed) -> Fixed {
    let mut height = currentheight;
    let mut heightlist: Vec<Fixed> = Vec::new();
    let others: Vec<SectorId> = surrounding(w, sec).collect();
    for o in others {
        let fh = w.level.sector(o).floor_height;
        if fh > height {
            // Emulation of memory (stack) overflow.
            if heightlist.len() == MAX_ADJOINING_SECTORS + 1 {
                height = fh;
            } else if heightlist.len() == MAX_ADJOINING_SECTORS + 2 {
                // Vanilla crashes here.
                w.note_overflow("P_FindNextHighestFloor: more than 22 adjoining sectors (vanilla crashes)");
            }
            heightlist.push(fh);
        }
    }
    // Find lowest height in list.
    if heightlist.is_empty() {
        return if w.params.version < GameVersion::Doom1_5 { Fixed(0) } else { currentheight };
    }
    *heightlist.iter().min().unwrap()
}

/// `P_FindLowestCeilingSurrounding`.
pub fn find_lowest_ceiling_surrounding(w: &World, sec: SectorId) -> Fixed {
    let mut height = Fixed::MAX;
    for o in surrounding(w, sec) {
        height = height.min(w.level.sector(o).ceiling_height);
    }
    height
}

/// `P_FindHighestCeilingSurrounding`.
pub fn find_highest_ceiling_surrounding(w: &World, sec: SectorId) -> Fixed {
    let mut height = Fixed(0);
    for o in surrounding(w, sec) {
        height = height.max(w.level.sector(o).ceiling_height);
    }
    height
}

/// `P_FindSectorFromLineTag`: next sector after `start` with the line's tag.
pub fn find_sector_from_line_tag(w: &World, line: LineId, start: i32) -> Option<SectorId> {
    find_sector_from_tag(w, w.level.line(line).tag, start)
}

/// `P_FindSectorFromLineTag` for a tag alone (the C code builds fake
/// "junk" lines with just a tag for boss deaths).
pub fn find_sector_from_tag(w: &World, tag: i16, start: i32) -> Option<SectorId> {
    ((start + 1).max(0) as usize..w.level.num_map_sectors())
        .find(|&i| w.level.sectors[i].tag == tag)
        .map(|i| SectorId(i as u32))
}

/// `P_FindMinSurroundingLight`.
pub fn find_min_surrounding_light(w: &World, sec: SectorId, max: i32) -> i32 {
    let mut min = max;
    for o in surrounding(w, sec) {
        min = min.min(w.level.sector(o).light_level as i32);
    }
    min
}

// ---------------------------------------------------------------------------
// Animations (P_InitPicAnims) and switches (P_InitSwitchList)
// ---------------------------------------------------------------------------

/// `anim_t`.
#[derive(Clone, Copy, Debug)]
pub struct Anim {
    pub istexture: bool,
    pub picnum: usize,
    pub basepic: usize,
    pub numpics: usize,
    pub speed: i32,
}

/// `animdefs[]`: (istexture, endname, startname, speed).
const ANIMDEFS: [(bool, &str, &str, i32); 22] = [
    (false, "NUKAGE3", "NUKAGE1", 8),
    (false, "FWATER4", "FWATER1", 8),
    (false, "SWATER4", "SWATER1", 8),
    (false, "LAVA4", "LAVA1", 8),
    (false, "BLOOD3", "BLOOD1", 8),
    // DOOM II flat animations.
    (false, "RROCK08", "RROCK05", 8),
    (false, "SLIME04", "SLIME01", 8),
    (false, "SLIME08", "SLIME05", 8),
    (false, "SLIME12", "SLIME09", 8),
    (true, "BLODGR4", "BLODGR1", 8),
    (true, "SLADRIP3", "SLADRIP1", 8),
    (true, "BLODRIP4", "BLODRIP1", 8),
    (true, "FIREWALL", "FIREWALA", 8),
    (true, "GSTFONT3", "GSTFONT1", 8),
    (true, "FIRELAVA", "FIRELAV3", 8),
    (true, "FIREMAG3", "FIREMAG1", 8),
    (true, "FIREBLU2", "FIREBLU1", 8),
    (true, "ROCKRED3", "ROCKRED1", 8),
    (true, "BFALL4", "BFALL1", 8),
    (true, "SFALL4", "SFALL1", 8),
    (true, "WFALL4", "WFALL1", 8),
    (true, "DBRAIN4", "DBRAIN1", 8),
];

/// `alphSwitchList[]`: (off, on, minimum episode set).
const SWITCHES: [(&str, &str, i32); 40] = [
    // Doom shareware episode 1 switches
    ("SW1BRCOM", "SW2BRCOM", 1),
    ("SW1BRN1", "SW2BRN1", 1),
    ("SW1BRN2", "SW2BRN2", 1),
    ("SW1BRNGN", "SW2BRNGN", 1),
    ("SW1BROWN", "SW2BROWN", 1),
    ("SW1COMM", "SW2COMM", 1),
    ("SW1COMP", "SW2COMP", 1),
    ("SW1DIRT", "SW2DIRT", 1),
    ("SW1EXIT", "SW2EXIT", 1),
    ("SW1GRAY", "SW2GRAY", 1),
    ("SW1GRAY1", "SW2GRAY1", 1),
    ("SW1METAL", "SW2METAL", 1),
    ("SW1PIPE", "SW2PIPE", 1),
    ("SW1SLAD", "SW2SLAD", 1),
    ("SW1STARG", "SW2STARG", 1),
    ("SW1STON1", "SW2STON1", 1),
    ("SW1STON2", "SW2STON2", 1),
    ("SW1STONE", "SW2STONE", 1),
    ("SW1STRTN", "SW2STRTN", 1),
    // Doom registered episodes 2&3 switches
    ("SW1BLUE", "SW2BLUE", 2),
    ("SW1CMT", "SW2CMT", 2),
    ("SW1GARG", "SW2GARG", 2),
    ("SW1GSTON", "SW2GSTON", 2),
    ("SW1HOT", "SW2HOT", 2),
    ("SW1LION", "SW2LION", 2),
    ("SW1SATYR", "SW2SATYR", 2),
    ("SW1SKIN", "SW2SKIN", 2),
    ("SW1VINE", "SW2VINE", 2),
    ("SW1WOOD", "SW2WOOD", 2),
    // Doom II switches
    ("SW1PANEL", "SW2PANEL", 3),
    ("SW1ROCK", "SW2ROCK", 3),
    ("SW1MET2", "SW2MET2", 3),
    ("SW1WDMET", "SW2WDMET", 3),
    ("SW1BRIK", "SW2BRIK", 3),
    ("SW1MOD1", "SW2MOD1", 3),
    ("SW1ZIM", "SW2ZIM", 3),
    ("SW1STON6", "SW2STON6", 3),
    ("SW1TEK", "SW2TEK", 3),
    ("SW1MARB", "SW2MARB", 3),
    ("SW1SKULL", "SW2SKULL", 3),
];

/// Startup tables derived from content (`P_InitPicAnims`, `P_InitSwitchList`).
#[derive(Clone, Debug, Default)]
pub struct SpecTables {
    pub anims: Vec<Anim>,
    /// Texture numbers in pairs (off, on).
    pub switchlist: Vec<usize>,
    pub warnings: Vec<String>,
}

impl SpecTables {
    pub fn build(content: &crate::content::ContentInfo, mode: GameMode) -> Self {
        let mut t = SpecTables::default();
        for &(istexture, endname, startname, speed) in &ANIMDEFS {
            let (base, pic) = if istexture {
                // Different episode?
                let Some(base) = content.texture_num(startname) else { continue };
                let Some(pic) = content.texture_num(endname) else {
                    t.warnings.push(format!("animation end texture {endname} missing"));
                    continue;
                };
                (base, pic)
            } else {
                let Some(base) = content.flat_num(startname) else { continue };
                let Some(pic) = content.flat_num(endname) else {
                    t.warnings.push(format!("animation end flat {endname} missing"));
                    continue;
                };
                (base, pic)
            };
            if pic + 1 < base + 2 {
                // Vanilla: "P_InitPicAnims: bad cycle from %s to %s".
                t.warnings.push(format!("bad animation cycle from {startname} to {endname}"));
                continue;
            }
            t.anims.push(Anim { istexture, picnum: pic, basepic: base, numpics: pic - base + 1, speed });
        }
        let episode = match mode {
            GameMode::Registered | GameMode::Retail => 2,
            GameMode::Commercial => 3,
            GameMode::Shareware => 1,
        };
        for &(n1, n2, ep) in &SWITCHES {
            if ep <= episode {
                // Vanilla aborts if a switch texture is missing.
                match (content.texture_num(n1), content.texture_num(n2)) {
                    (Some(a), Some(b)) => {
                        t.switchlist.push(a);
                        t.switchlist.push(b);
                    }
                    _ => t.warnings.push(format!("switch texture {n1}/{n2} missing")),
                }
            }
        }
        t
    }
}

pub const MAXBUTTONS: usize = 16;
/// 1 second, in tics.
pub const BUTTONTIME: i32 = 35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonWhere {
    Top,
    Middle,
    Bottom,
}

/// `button_t`.
#[derive(Clone, Copy, Debug)]
pub struct Button {
    pub line: Option<LineId>,
    pub where_: ButtonWhere,
    pub btexture: u16,
    pub btimer: i32,
    /// `&line->frontsector->soundorg`.
    pub soundorg: Option<SectorId>,
}

impl Default for Button {
    fn default() -> Self {
        Self { line: None, where_: ButtonWhere::Top, btexture: 0, btimer: 0, soundorg: None }
    }
}

/// `P_StartButton`: start a button counting down till it turns off.
fn start_button(w: &mut World, line: LineId, where_: ButtonWhere, texture: u16, time: i32) {
    // See if button is already pressed.
    if w.buttons.iter().any(|b| b.btimer != 0 && b.line == Some(line)) {
        return;
    }
    let soundorg = w.level.line(line).front_sector;
    match w.buttons.iter_mut().find(|b| b.btimer == 0) {
        Some(b) => *b = Button { line: Some(line), where_, btexture: texture, btimer: time, soundorg },
        None => panic!("P_StartButton: no button slots left!"),
    }
}

/// `P_ChangeSwitchTexture`: swap the texture on a switch and maybe start a
/// timer to swap it back.
pub fn change_switch_texture(w: &mut World, line: LineId, use_again: bool) {
    if !use_again {
        w.level.lines[line.idx()].special = 0;
    }
    let side = w.level.line(line).sidenum[0].unwrap();
    let (top, mid, bot) = {
        let s = w.level.side(side);
        (s.top_texture.0 as usize, s.mid_texture.0 as usize, s.bottom_texture.0 as usize)
    };
    // EXIT SWITCH?
    let sound = if w.level.line(line).special == 11 { SFX_SWTCHX } else { SFX_SWTCHN };
    // Vanilla bug: the sound always plays from `buttonlist[0]`'s origin.
    let origin = w.buttons[0].soundorg.map_or(SoundSource::None, SoundSource::Sector);
    let list = w.spec.switchlist.clone();
    for i in 0..list.len() {
        let partner = TexNum(list[i ^ 1] as u16);
        let (where_, matched) = if list[i] == top {
            (ButtonWhere::Top, true)
        } else if list[i] == mid {
            (ButtonWhere::Middle, true)
        } else if list[i] == bot {
            (ButtonWhere::Bottom, true)
        } else {
            (ButtonWhere::Top, false)
        };
        if !matched {
            continue;
        }
        w.start_sound(origin, sound);
        let s = &mut w.level.sides[side.idx()];
        match where_ {
            ButtonWhere::Top => s.top_texture = partner,
            ButtonWhere::Middle => s.mid_texture = partner,
            ButtonWhere::Bottom => s.bottom_texture = partner,
        }
        if use_again {
            start_button(w, line, where_, list[i] as u16, BUTTONTIME);
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// `G_ExitLevel` / `G_SecretExitLevel`. In Doom II, the secret exit only
/// works if MAP31 exists — that check belongs to the host (content).
fn exit_level(w: &mut World, secret: bool) {
    w.gameexit = Some(if secret { GameExit::Secret } else { GameExit::Normal });
}

/// `P_CrossSpecialLine`: called every time a thing origin is about to cross
/// a line with a non-zero special.
pub fn cross_special_line(w: &mut World, linenum: LineId, side: usize, thing: MobjId) {
    if w.level.line(linenum).action.is_some() {
        crate::hexen::activate_line(w, linenum, side, Some(thing), crate::hexen::Trigger::Cross);
        return;
    }
    let special = w.level.line(linenum).special;
    let (is_player, mtype) = {
        let m = w.mo(thing);
        (m.player.is_some(), m.mobj_type)
    };

    if w.params.version <= GameVersion::Doom1_2 {
        if special > 98 && special != 104 {
            return;
        }
    } else if !is_player {
        // Things that should NOT trigger specials...
        if matches!(mtype, MT_ROCKET | MT_PLASMA | MT_BFG | MT_TROOPSHOT | MT_HEADSHOT | MT_BRUISERSHOT) {
            return;
        }
    }

    if !is_player {
        // Monsters can only trigger a few specials.
        if !matches!(special, 39 | 97 | 125 | 126 | 4 | 10 | 88) {
            return;
        }
    }

    let line = linenum;
    let clear = |w: &mut World| w.level.lines[line.idx()].special = 0;
    // Note: could use some const's here.
    match special {
        // TRIGGERS. All from here to RETRIGGERS.
        2 => {
            ev_do_door(w, line, DoorKind::Open);
            clear(w);
        }
        3 => {
            ev_do_door(w, line, DoorKind::Close);
            clear(w);
        }
        4 => {
            ev_do_door(w, line, DoorKind::Normal);
            clear(w);
        }
        5 => {
            ev_do_floor(w, line, FloorKind::RaiseFloor);
            clear(w);
        }
        6 => {
            ev_do_ceiling(w, line, CeilingKind::FastCrushAndRaise);
            clear(w);
        }
        8 => {
            ev_build_stairs(w, line, StairKind::Build8);
            clear(w);
        }
        10 => {
            ev_do_plat(w, line, PlatKind::DownWaitUpStay, 0);
            clear(w);
        }
        12 => {
            ev_light_turn_on(w, line, 0);
            clear(w);
        }
        13 => {
            ev_light_turn_on(w, line, 255);
            clear(w);
        }
        16 => {
            ev_do_door(w, line, DoorKind::Close30ThenOpen);
            clear(w);
        }
        17 => {
            ev_start_light_strobing(w, line);
            clear(w);
        }
        19 => {
            ev_do_floor(w, line, FloorKind::LowerFloor);
            clear(w);
        }
        22 => {
            ev_do_plat(w, line, PlatKind::RaiseToNearestAndChange, 0);
            clear(w);
        }
        25 => {
            ev_do_ceiling(w, line, CeilingKind::CrushAndRaise);
            clear(w);
        }
        30 => {
            ev_do_floor(w, line, FloorKind::RaiseToTexture);
            clear(w);
        }
        35 => {
            ev_light_turn_on(w, line, 35);
            clear(w);
        }
        36 => {
            ev_do_floor(w, line, FloorKind::TurboLower);
            clear(w);
        }
        37 => {
            ev_do_floor(w, line, FloorKind::LowerAndChange);
            clear(w);
        }
        38 => {
            ev_do_floor(w, line, FloorKind::LowerFloorToLowest);
            clear(w);
        }
        39 => {
            ev_teleport(w, line, side, thing);
            clear(w);
        }
        40 => {
            ev_do_ceiling(w, line, CeilingKind::RaiseToHighest);
            ev_do_floor(w, line, FloorKind::LowerFloorToLowest);
            clear(w);
        }
        44 => {
            ev_do_ceiling(w, line, CeilingKind::LowerAndCrush);
            clear(w);
        }
        52 => exit_level(w, false),
        53 => {
            ev_do_plat(w, line, PlatKind::PerpetualRaise, 0);
            clear(w);
        }
        54 => {
            ev_stop_plat(w, line);
            clear(w);
        }
        56 => {
            ev_do_floor(w, line, FloorKind::RaiseFloorCrush);
            clear(w);
        }
        57 => {
            ev_ceiling_crush_stop(w, line);
            clear(w);
        }
        58 => {
            ev_do_floor(w, line, FloorKind::RaiseFloor24);
            clear(w);
        }
        59 => {
            ev_do_floor(w, line, FloorKind::RaiseFloor24AndChange);
            clear(w);
        }
        104 => {
            ev_turn_tag_lights_off(w, line);
            clear(w);
        }
        108 => {
            ev_do_door(w, line, DoorKind::BlazeRaise);
            clear(w);
        }
        109 => {
            ev_do_door(w, line, DoorKind::BlazeOpen);
            clear(w);
        }
        100 => {
            ev_build_stairs(w, line, StairKind::Turbo16);
            clear(w);
        }
        110 => {
            ev_do_door(w, line, DoorKind::BlazeClose);
            clear(w);
        }
        119 => {
            ev_do_floor(w, line, FloorKind::RaiseFloorToNearest);
            clear(w);
        }
        121 => {
            ev_do_plat(w, line, PlatKind::BlazeDwus, 0);
            clear(w);
        }
        124 => exit_level(w, true),
        125 => {
            if !is_player {
                ev_teleport(w, line, side, thing);
                clear(w);
            }
        }
        130 => {
            ev_do_floor(w, line, FloorKind::RaiseFloorTurbo);
            clear(w);
        }
        141 => {
            ev_do_ceiling(w, line, CeilingKind::SilentCrushAndRaise);
            clear(w);
        }
        // RETRIGGERS. All from here till end.
        72 => {
            ev_do_ceiling(w, line, CeilingKind::LowerAndCrush);
        }
        73 => {
            ev_do_ceiling(w, line, CeilingKind::CrushAndRaise);
        }
        74 => {
            ev_ceiling_crush_stop(w, line);
        }
        75 => {
            ev_do_door(w, line, DoorKind::Close);
        }
        76 => {
            ev_do_door(w, line, DoorKind::Close30ThenOpen);
        }
        77 => {
            ev_do_ceiling(w, line, CeilingKind::FastCrushAndRaise);
        }
        79 => ev_light_turn_on(w, line, 35),
        80 => ev_light_turn_on(w, line, 0),
        81 => ev_light_turn_on(w, line, 255),
        82 => {
            ev_do_floor(w, line, FloorKind::LowerFloorToLowest);
        }
        83 => {
            ev_do_floor(w, line, FloorKind::LowerFloor);
        }
        84 => {
            ev_do_floor(w, line, FloorKind::LowerAndChange);
        }
        86 => {
            ev_do_door(w, line, DoorKind::Open);
        }
        87 => {
            ev_do_plat(w, line, PlatKind::PerpetualRaise, 0);
        }
        88 => {
            ev_do_plat(w, line, PlatKind::DownWaitUpStay, 0);
        }
        89 => ev_stop_plat(w, line),
        90 => {
            ev_do_door(w, line, DoorKind::Normal);
        }
        91 => {
            ev_do_floor(w, line, FloorKind::RaiseFloor);
        }
        92 => {
            ev_do_floor(w, line, FloorKind::RaiseFloor24);
        }
        93 => {
            ev_do_floor(w, line, FloorKind::RaiseFloor24AndChange);
        }
        94 => {
            ev_do_floor(w, line, FloorKind::RaiseFloorCrush);
        }
        95 => {
            ev_do_plat(w, line, PlatKind::RaiseToNearestAndChange, 0);
        }
        96 => {
            ev_do_floor(w, line, FloorKind::RaiseToTexture);
        }
        97 => {
            ev_teleport(w, line, side, thing);
        }
        98 => {
            ev_do_floor(w, line, FloorKind::TurboLower);
        }
        105 => {
            ev_do_door(w, line, DoorKind::BlazeRaise);
        }
        106 => {
            ev_do_door(w, line, DoorKind::BlazeOpen);
        }
        107 => {
            ev_do_door(w, line, DoorKind::BlazeClose);
        }
        120 => {
            ev_do_plat(w, line, PlatKind::BlazeDwus, 0);
        }
        126 => {
            if !is_player {
                ev_teleport(w, line, side, thing);
            }
        }
        128 => {
            ev_do_floor(w, line, FloorKind::RaiseFloorToNearest);
        }
        129 => {
            ev_do_floor(w, line, FloorKind::RaiseFloorTurbo);
        }
        _ => {}
    }
}

/// `P_ShootSpecialLine`: called when a thing shoots a special line.
pub fn shoot_special_line(w: &mut World, thing: Option<MobjId>, line: LineId) {
    if w.level.line(line).action.is_some() {
        crate::hexen::activate_line(w, line, 0, thing, crate::hexen::Trigger::Impact);
        return;
    }
    let special = w.level.line(line).special;
    // Impacts that other things can activate.
    let is_player = thing.and_then(|t| w.try_mo(Some(t))).is_some_and(|m| m.player.is_some());
    if !is_player && special != 46 {
        return;
    }
    match special {
        24 => {
            // RAISE FLOOR
            ev_do_floor(w, line, FloorKind::RaiseFloor);
            change_switch_texture(w, line, false);
        }
        46 => {
            // OPEN DOOR
            ev_do_door(w, line, DoorKind::Open);
            change_switch_texture(w, line, true);
        }
        47 => {
            // RAISE FLOOR NEAR AND CHANGE
            ev_do_plat(w, line, PlatKind::RaiseToNearestAndChange, 0);
            change_switch_texture(w, line, false);
        }
        _ => {}
    }
}

/// `P_UseSpecialLine`: called when a thing uses a special line. Only the
/// front sides of lines are usable.
pub fn use_special_line(w: &mut World, thing: MobjId, line: LineId, side: usize) -> bool {
    if w.level.line(line).action.is_some() {
        return crate::hexen::activate_line(w, line, side, Some(thing), crate::hexen::Trigger::Use);
    }
    let special = w.level.line(line).special;
    // Err...
    // Use the back sides of VERY SPECIAL lines...
    if side != 0 && special != 124 {
        return false;
    }
    // Switches that other things can activate.
    if w.mo(thing).player.is_none() {
        // Never open secret doors.
        if w.level.line(line).flags & ml::SECRET != 0 {
            return false;
        }
        if !matches!(special, 1 | 32 | 33 | 34) {
            return false;
        }
    }

    let sw = |w: &mut World, ok: bool, again: bool| {
        if ok {
            change_switch_texture(w, line, again);
        }
    };
    // Do something.
    match special {
        // MANUALS
        1 | 26 | 27 | 28 | 31 | 32 | 33 | 34 | 117 | 118 => ev_vertical_door(w, line, thing),
        // SWITCHES
        7 => {
            let ok = ev_build_stairs(w, line, StairKind::Build8);
            sw(w, ok, false)
        }
        9 => {
            let ok = ev_do_donut(w, line);
            sw(w, ok, false)
        }
        11 => {
            change_switch_texture(w, line, false);
            exit_level(w, false);
        }
        14 => {
            let ok = ev_do_plat(w, line, PlatKind::RaiseAndChange, 32);
            sw(w, ok, false)
        }
        15 => {
            let ok = ev_do_plat(w, line, PlatKind::RaiseAndChange, 24);
            sw(w, ok, false)
        }
        18 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloorToNearest);
            sw(w, ok, false)
        }
        20 => {
            let ok = ev_do_plat(w, line, PlatKind::RaiseToNearestAndChange, 0);
            sw(w, ok, false)
        }
        21 => {
            let ok = ev_do_plat(w, line, PlatKind::DownWaitUpStay, 0);
            sw(w, ok, false)
        }
        23 => {
            let ok = ev_do_floor(w, line, FloorKind::LowerFloorToLowest);
            sw(w, ok, false)
        }
        29 => {
            let ok = ev_do_door(w, line, DoorKind::Normal);
            sw(w, ok, false)
        }
        41 => {
            let ok = ev_do_ceiling(w, line, CeilingKind::LowerToFloor);
            sw(w, ok, false)
        }
        71 => {
            let ok = ev_do_floor(w, line, FloorKind::TurboLower);
            sw(w, ok, false)
        }
        49 => {
            let ok = ev_do_ceiling(w, line, CeilingKind::CrushAndRaise);
            sw(w, ok, false)
        }
        50 => {
            let ok = ev_do_door(w, line, DoorKind::Close);
            sw(w, ok, false)
        }
        51 => {
            change_switch_texture(w, line, false);
            exit_level(w, true);
        }
        55 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloorCrush);
            sw(w, ok, false)
        }
        101 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloor);
            sw(w, ok, false)
        }
        102 => {
            let ok = ev_do_floor(w, line, FloorKind::LowerFloor);
            sw(w, ok, false)
        }
        103 => {
            let ok = ev_do_door(w, line, DoorKind::Open);
            sw(w, ok, false)
        }
        111 => {
            let ok = ev_do_door(w, line, DoorKind::BlazeRaise);
            sw(w, ok, false)
        }
        112 => {
            let ok = ev_do_door(w, line, DoorKind::BlazeOpen);
            sw(w, ok, false)
        }
        113 => {
            let ok = ev_do_door(w, line, DoorKind::BlazeClose);
            sw(w, ok, false)
        }
        122 => {
            let ok = ev_do_plat(w, line, PlatKind::BlazeDwus, 0);
            sw(w, ok, false)
        }
        127 => {
            let ok = ev_build_stairs(w, line, StairKind::Turbo16);
            sw(w, ok, false)
        }
        131 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloorTurbo);
            sw(w, ok, false)
        }
        133 | 135 | 137 => {
            let ok = ev_do_locked_door(w, line, DoorKind::BlazeOpen, thing);
            sw(w, ok, false)
        }
        140 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloor512);
            sw(w, ok, false)
        }
        // BUTTONS
        42 => {
            let ok = ev_do_door(w, line, DoorKind::Close);
            sw(w, ok, true)
        }
        43 => {
            let ok = ev_do_ceiling(w, line, CeilingKind::LowerToFloor);
            sw(w, ok, true)
        }
        45 => {
            let ok = ev_do_floor(w, line, FloorKind::LowerFloor);
            sw(w, ok, true)
        }
        60 => {
            let ok = ev_do_floor(w, line, FloorKind::LowerFloorToLowest);
            sw(w, ok, true)
        }
        61 => {
            let ok = ev_do_door(w, line, DoorKind::Open);
            sw(w, ok, true)
        }
        62 => {
            let ok = ev_do_plat(w, line, PlatKind::DownWaitUpStay, 1);
            sw(w, ok, true)
        }
        63 => {
            let ok = ev_do_door(w, line, DoorKind::Normal);
            sw(w, ok, true)
        }
        64 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloor);
            sw(w, ok, true)
        }
        66 => {
            let ok = ev_do_plat(w, line, PlatKind::RaiseAndChange, 24);
            sw(w, ok, true)
        }
        67 => {
            let ok = ev_do_plat(w, line, PlatKind::RaiseAndChange, 32);
            sw(w, ok, true)
        }
        65 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloorCrush);
            sw(w, ok, true)
        }
        68 => {
            let ok = ev_do_plat(w, line, PlatKind::RaiseToNearestAndChange, 0);
            sw(w, ok, true)
        }
        69 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloorToNearest);
            sw(w, ok, true)
        }
        70 => {
            let ok = ev_do_floor(w, line, FloorKind::TurboLower);
            sw(w, ok, true)
        }
        114 => {
            let ok = ev_do_door(w, line, DoorKind::BlazeRaise);
            sw(w, ok, true)
        }
        115 => {
            let ok = ev_do_door(w, line, DoorKind::BlazeOpen);
            sw(w, ok, true)
        }
        116 => {
            let ok = ev_do_door(w, line, DoorKind::BlazeClose);
            sw(w, ok, true)
        }
        123 => {
            let ok = ev_do_plat(w, line, PlatKind::BlazeDwus, 0);
            sw(w, ok, true)
        }
        132 => {
            let ok = ev_do_floor(w, line, FloorKind::RaiseFloorTurbo);
            sw(w, ok, true)
        }
        99 | 134 | 136 => {
            let ok = ev_do_locked_door(w, line, DoorKind::BlazeOpen, thing);
            sw(w, ok, true)
        }
        138 => {
            ev_light_turn_on(w, line, 255);
            change_switch_texture(w, line, true);
        }
        139 => {
            ev_light_turn_on(w, line, 35);
            change_switch_texture(w, line, true);
        }
        _ => {}
    }
    true
}

/// `EV_Teleport` (p_telept.c).
pub fn ev_teleport(w: &mut World, line: LineId, side: usize, thing: MobjId) -> bool {
    // Don't teleport missiles.
    if w.mo(thing).flags & MF_MISSILE != 0 {
        return false;
    }
    // Don't teleport if hit back of line, so you can get out of teleporter.
    if side == 1 {
        return false;
    }
    let tag = w.level.line(line).tag;
    for i in 0..w.level.num_map_sectors() {
        if w.level.sectors[i].tag != tag {
            continue;
        }
        let mut cur = w.thinkers.first();
        while let Some(t) = cur {
            cur = w.thinkers.next(t);
            // Not a mobj (or removed).
            let is_mobj = matches!(w.thinkers.data(t), Some(ThinkerData::Mobj(_))) && w.thinkers.func(t) == Some(ThinkFn::Active);
            if !is_mobj {
                continue;
            }
            let m = w.mo(t);
            // Not a teleportman.
            if m.mobj_type != MT_TELEPORTMAN {
                continue;
            }
            // Wrong sector.
            if w.level.subsectors[m.subsector.idx()].sector.idx() != i {
                continue;
            }
            let (dx, dy, dangle) = (m.x, m.y, m.angle);
            let (oldx, oldy, oldz) = {
                let th = w.mo(thing);
                (th.x, th.y, th.z)
            };
            if !teleport_move(w, thing, dx, dy) {
                return false;
            }
            // The first Final Doom executable does not set thing->z when
            // teleporting.
            if w.params.version != GameVersion::Final {
                let fz = w.mo(thing).floorz;
                w.mo_mut(thing).z = fz;
            }
            if let Some(p) = w.mo(thing).player {
                let z = w.mo(thing).z;
                w.players[p].viewz = z + w.players[p].viewheight;
            }
            // Spawn teleport fog at source and destination.
            let fog = spawn_mobj(w, oldx, oldy, oldz, MT_TFOG);
            w.start_sound(SoundSource::Mobj(fog), SFX_TELEPT);
            let an = dangle.fine();
            let tz = w.mo(thing).z;
            let fog = spawn_mobj(w, dx + finecosine(an) * 20, dy + finesine(an) * 20, tz, MT_TFOG);
            // Emit sound, where?
            w.start_sound(SoundSource::Mobj(fog), SFX_TELEPT);
            // Don't move for a bit.
            let is_player = w.mo(thing).player.is_some();
            let th = w.mo_mut(thing);
            if is_player {
                th.reactiontime = 18;
            }
            th.angle = dangle;
            th.momx = Fixed(0);
            th.momy = Fixed(0);
            th.momz = Fixed(0);
            return true;
        }
    }
    false
}

/// `P_PlayerInSpecialSector`: called every tic frame that the player origin
/// is in a special sector.
pub fn player_in_special_sector(w: &mut World, player: usize) {
    let mo = w.players[player].mo.unwrap();
    let sec = w.level.subsectors[w.mo(mo).subsector.idx()].sector;
    // Falling, not all the way down yet?
    if w.mo(mo).z != w.level.sector(sec).floor_height {
        return;
    }
    let ironfeet = w.players[player].powers[power::IRONFEET] != 0;
    let leveltime = w.leveltime;
    // Has hitten ground.
    match w.level.sector(sec).special {
        5 => {
            // HELLSLIME DAMAGE
            if !ironfeet && leveltime & 0x1f == 0 {
                crate::inter::damage_mobj(w, mo, None, None, 10);
            }
        }
        7 => {
            // NUKAGE DAMAGE
            if !ironfeet && leveltime & 0x1f == 0 {
                crate::inter::damage_mobj(w, mo, None, None, 5);
            }
        }
        16 | 4 => {
            // SUPER HELLSLIME DAMAGE / STROBE HURT
            if (!ironfeet || w.p_random() < 5) && leveltime & 0x1f == 0 {
                crate::inter::damage_mobj(w, mo, None, None, 20);
            }
        }
        9 => {
            // SECRET SECTOR
            w.players[player].secretcount += 1;
            w.level.sector_mut(sec).special = 0;
        }
        11 => {
            // EXIT SUPER DAMAGE! (for E1M8 finale)
            w.players[player].cheats &= !crate::player::cheat::GODMODE;
            if leveltime & 0x1f == 0 {
                crate::inter::damage_mobj(w, mo, None, None, 20);
            }
            if w.players[player].health <= 10 {
                exit_level(w, false);
            }
        }
        _ => {
            // Vanilla: I_Error("P_PlayerInSpecialSector: unknown special %i").
            w.note_overflow("P_PlayerInSpecialSector: unknown sector special (vanilla I_Error)");
        }
    }
}

/// `P_UpdateSpecials`: animate planes, scroll walls, etc.
pub fn update_specials(w: &mut World) {
    // LEVEL TIMER
    if w.level_timer {
        w.level_time_count -= 1;
        if w.level_time_count == 0 {
            exit_level(w, false);
        }
    }

    // ANIMATE FLATS AND TEXTURES GLOBALLY
    let leveltime = w.leveltime;
    for a in &w.spec.anims {
        for i in a.basepic..a.basepic + a.numpics {
            let pic = a.basepic + ((leveltime / a.speed) as usize + i) % a.numpics;
            if a.istexture {
                w.texture_translation[i] = pic as u16;
            } else {
                w.flat_translation[i] = pic as u16;
            }
        }
    }

    // ANIMATE LINE SPECIALS
    for i in 0..w.line_specials.len() {
        let l = w.line_specials[i];
        if w.level.line(l).special == 48 {
            // EFFECT FIRSTCOL SCROLL +
            let side = w.level.line(l).sidenum[0].unwrap();
            w.level.sides[side.idx()].texture_offset += Fixed::ONE;
        }
    }

    // DO BUTTONS
    for i in 0..MAXBUTTONS {
        let b = w.buttons[i];
        if b.btimer == 0 {
            continue;
        }
        w.buttons[i].btimer -= 1;
        if w.buttons[i].btimer == 0 {
            let line = b.line.unwrap();
            let side = w.level.line(line).sidenum[0].unwrap();
            let s = &mut w.level.sides[side.idx()];
            match b.where_ {
                ButtonWhere::Top => s.top_texture = TexNum(b.btexture),
                ButtonWhere::Middle => s.mid_texture = TexNum(b.btexture),
                ButtonWhere::Bottom => s.bottom_texture = TexNum(b.btexture),
            }
            w.start_sound(b.soundorg.map_or(SoundSource::None, SoundSource::Sector), SFX_SWTCHN);
            w.buttons[i] = Button::default();
        }
    }
}

/// Vanilla's limit on scrolling walls (`MAXLINEANIMS`; 1.2 allowed 66).
const MAXLINEANIMS: usize = 64;
const MAXLINEANIMS1_2: usize = 66;

/// `P_SpawnSpecials`: after the map has been loaded, scan for specials that
/// spawn thinkers.
pub fn spawn_specials(w: &mut World) {
    // See if -TIMER was specified.
    if w.timelimit > 0 && w.params.deathmatch != 0 {
        w.level_timer = true;
        w.level_time_count = w.timelimit * 60 * 35;
    } else {
        w.level_timer = false;
    }

    // Init special SECTORs.
    for i in 0..w.level.num_map_sectors() {
        let sec = SectorId(i as u32);
        match w.level.sector(sec).special {
            0 => {}
            // FLICKERING LIGHTS
            1 => spawn_light_flash(w, sec),
            // STROBE FAST
            2 => spawn_strobe_flash(w, sec, FASTDARK, false),
            // STROBE SLOW
            3 => spawn_strobe_flash(w, sec, SLOWDARK, false),
            4 => {
                // STROBE FAST/DEATH SLIME
                spawn_strobe_flash(w, sec, FASTDARK, false);
                w.level.sector_mut(sec).special = 4;
            }
            // GLOWING LIGHT
            8 => spawn_glowing_light(w, sec),
            // SECRET SECTOR
            9 => w.totalsecret += 1,
            // DOOR CLOSE IN 30 SECONDS
            10 => spawn_door_close_in_30(w, sec),
            // SYNC STROBE SLOW
            12 => spawn_strobe_flash(w, sec, SLOWDARK, true),
            // SYNC STROBE FAST
            13 => spawn_strobe_flash(w, sec, FASTDARK, true),
            // DOOR RAISE IN 5 MINUTES
            14 => spawn_door_raise_in_5_mins(w, sec),
            17 => {
                // First introduced in official v1.4 beta.
                if w.params.version > GameVersion::Doom1_2 {
                    spawn_fire_flicker(w, sec);
                }
            }
            _ => {}
        }
    }

    // Init line EFFECTs.
    let max = if w.params.version <= GameVersion::Doom1_2 { MAXLINEANIMS1_2 } else { MAXLINEANIMS };
    w.line_specials.clear();
    for i in 0..w.level.lines.len() {
        if w.level.lines[i].special == 48 {
            if w.line_specials.len() >= max {
                // Vanilla: I_Error "Too many scrolling wall linedefs".
                w.note_overflow("too many scrolling wall linedefs (vanilla I_Error)");
                continue;
            }
            // EFFECT FIRSTCOL SCROLL+
            w.line_specials.push(LineId(i as u32));
        }
    }

    // Init other misc stuff.
    w.activeceilings = [None; crate::specials::ceilings::MAXCEILINGS];
    w.activeplats = [None; crate::specials::plats::MAXPLATS];
    w.buttons = [Button::default(); MAXBUTTONS];
}
