//! Light effects (p_lights.c).

use super::{add_special, special_mut, SpecialThinker};
use crate::ids::{LineId, SectorId, ThinkerId};
use crate::spec::{find_min_surrounding_light, find_sector_from_line_tag, next_sector};
use crate::world::World;

pub const GLOWSPEED: i32 = 8;
pub const STROBEBRIGHT: i32 = 5;
pub const FASTDARK: i32 = 15;
pub const SLOWDARK: i32 = 35;

#[derive(Clone, Debug)]
pub struct FireFlicker {
    pub sector: SectorId,
    pub count: i32,
    pub maxlight: i32,
    pub minlight: i32,
}

#[derive(Clone, Debug)]
pub struct LightFlash {
    pub sector: SectorId,
    pub count: i32,
    pub maxlight: i32,
    pub minlight: i32,
    pub maxtime: i32,
    pub mintime: i32,
}

#[derive(Clone, Debug)]
pub struct Strobe {
    pub sector: SectorId,
    pub count: i32,
    pub minlight: i32,
    pub maxlight: i32,
    pub darktime: i32,
    pub brighttime: i32,
}

#[derive(Clone, Debug)]
pub struct Glow {
    pub sector: SectorId,
    pub minlight: i32,
    pub maxlight: i32,
    pub direction: i32,
}

fn light(w: &World, s: SectorId) -> i32 {
    w.level.sector(s).light_level as i32
}

fn set_light(w: &mut World, s: SectorId, v: i32) {
    // `lightlevel` is a short.
    w.level.sector_mut(s).light_level = v as i16;
}

/// `T_FireFlicker`.
pub fn t_fire_flicker(w: &mut World, id: ThinkerId) {
    let Some(SpecialThinker::FireFlicker(f)) = special_mut(w, id) else { return };
    f.count -= 1;
    if f.count != 0 {
        return;
    }
    let (sector, minlight, maxlight) = (f.sector, f.minlight, f.maxlight);
    let amount = (w.p_random() & 3) * 16;
    if light(w, sector) - amount < minlight {
        set_light(w, sector, minlight);
    } else {
        set_light(w, sector, maxlight - amount);
    }
    if let Some(SpecialThinker::FireFlicker(f)) = special_mut(w, id) {
        f.count = 4;
    }
}

/// `P_SpawnFireFlicker`.
pub fn spawn_fire_flicker(w: &mut World, sector: SectorId) {
    // Note that we are resetting sector attributes. Nothing special about
    // it during gameplay.
    w.level.sector_mut(sector).special = 0;
    let maxlight = light(w, sector);
    let minlight = find_min_surrounding_light(w, sector, maxlight) + 16;
    add_special(w, SpecialThinker::FireFlicker(FireFlicker { sector, count: 4, maxlight, minlight }));
}

/// `T_LightFlash`: do flashing lights.
pub fn t_light_flash(w: &mut World, id: ThinkerId) {
    let Some(SpecialThinker::LightFlash(f)) = special_mut(w, id) else { return };
    f.count -= 1;
    if f.count != 0 {
        return;
    }
    let f = f.clone();
    let count = if light(w, f.sector) == f.maxlight {
        set_light(w, f.sector, f.minlight);
        (w.p_random() & f.mintime) + 1
    } else {
        set_light(w, f.sector, f.maxlight);
        (w.p_random() & f.maxtime) + 1
    };
    if let Some(SpecialThinker::LightFlash(f)) = special_mut(w, id) {
        f.count = count;
    }
}

/// `P_SpawnLightFlash`: after the map has been loaded, scan each sector for
/// specials that spawn thinkers.
pub fn spawn_light_flash(w: &mut World, sector: SectorId) {
    // Nothing special about it during gameplay.
    w.level.sector_mut(sector).special = 0;
    let maxlight = light(w, sector);
    let minlight = find_min_surrounding_light(w, sector, maxlight);
    let (maxtime, mintime) = (64, 7);
    // The thinker is added before the random count is drawn, as in vanilla
    // (order of P_AddThinker and P_Random is not observable either way).
    let id = add_special(
        w,
        SpecialThinker::LightFlash(LightFlash { sector, count: 0, maxlight, minlight, maxtime, mintime }),
    );
    let count = (w.p_random() & maxtime) + 1;
    if let Some(SpecialThinker::LightFlash(f)) = special_mut(w, id) {
        f.count = count;
    }
}

/// `T_StrobeFlash`.
pub fn t_strobe_flash(w: &mut World, id: ThinkerId) {
    let Some(SpecialThinker::Strobe(f)) = special_mut(w, id) else { return };
    f.count -= 1;
    if f.count != 0 {
        return;
    }
    let f = f.clone();
    let count = if light(w, f.sector) == f.minlight {
        set_light(w, f.sector, f.maxlight);
        f.brighttime
    } else {
        set_light(w, f.sector, f.minlight);
        f.darktime
    };
    if let Some(SpecialThinker::Strobe(f)) = special_mut(w, id) {
        f.count = count;
    }
}

/// `P_SpawnStrobeFlash`.
pub fn spawn_strobe_flash(w: &mut World, sector: SectorId, fast_or_slow: i32, in_sync: bool) {
    let maxlight = light(w, sector);
    let mut minlight = find_min_surrounding_light(w, sector, maxlight);
    if minlight == maxlight {
        minlight = 0;
    }
    // Nothing special about it during gameplay.
    w.level.sector_mut(sector).special = 0;
    let id = add_special(
        w,
        SpecialThinker::Strobe(Strobe {
            sector,
            count: 0,
            minlight,
            maxlight,
            darktime: fast_or_slow,
            brighttime: STROBEBRIGHT,
        }),
    );
    let count = if !in_sync { (w.p_random() & 7) + 1 } else { 1 };
    if let Some(SpecialThinker::Strobe(f)) = special_mut(w, id) {
        f.count = count;
    }
}

/// `EV_StartLightStrobing`: start strobing lights (usually from a trigger).
pub fn ev_start_light_strobing(w: &mut World, line: LineId) {
    let mut secnum = -1;
    while let Some(s) = find_sector_from_line_tag(w, line, secnum) {
        secnum = s.0 as i32;
        if w.level.sector(s).special_data.is_some() {
            continue;
        }
        spawn_strobe_flash(w, s, SLOWDARK, false);
    }
}

/// `EV_TurnTagLightsOff`: turn lights off to the lowest neighbour.
pub fn ev_turn_tag_lights_off(w: &mut World, line: LineId) {
    let tag = w.level.line(line).tag;
    for j in 0..w.level.num_map_sectors() {
        let sec = SectorId(j as u32);
        if w.level.sector(sec).tag != tag {
            continue;
        }
        let mut min = light(w, sec);
        for &l in w.level.lines_of(sec) {
            if let Some(t) = next_sector(&w.level, l, sec) {
                min = min.min(light(w, t));
            }
        }
        set_light(w, sec, min);
    }
}

/// `EV_LightTurnOn`: turn lights on to `bright` (0 = brightest neighbour).
///
/// Vanilla quirk preserved: `bright` is updated in place, so once one tagged
/// sector picks a neighbour's level, later tagged sectors reuse it.
pub fn ev_light_turn_on(w: &mut World, line: LineId, mut bright: i32) {
    let tag = w.level.line(line).tag;
    for i in 0..w.level.num_map_sectors() {
        let sec = SectorId(i as u32);
        if w.level.sector(sec).tag != tag {
            continue;
        }
        // bright = 0 means to search for highest light level surrounding
        // sector.
        if bright == 0 {
            for &l in w.level.lines_of(sec) {
                if let Some(t) = next_sector(&w.level, l, sec) {
                    if light(w, t) > bright {
                        bright = light(w, t);
                    }
                }
            }
        }
        set_light(w, sec, bright);
    }
}

/// `T_Glow`: spawn glowing light.
pub fn t_glow(w: &mut World, id: ThinkerId) {
    let Some(SpecialThinker::Glow(g)) = special_mut(w, id) else { return };
    let g = g.clone();
    let mut dir = g.direction;
    let mut l = light(w, g.sector);
    match g.direction {
        -1 => {
            // DOWN
            l -= GLOWSPEED;
            if l <= g.minlight {
                l += GLOWSPEED;
                dir = 1;
            }
        }
        1 => {
            // UP
            l += GLOWSPEED;
            if l >= g.maxlight {
                l -= GLOWSPEED;
                dir = -1;
            }
        }
        _ => {}
    }
    set_light(w, g.sector, l);
    if let Some(SpecialThinker::Glow(g)) = special_mut(w, id) {
        g.direction = dir;
    }
}

/// `P_SpawnGlowingLight`.
pub fn spawn_glowing_light(w: &mut World, sector: SectorId) {
    let maxlight = light(w, sector);
    let minlight = find_min_surrounding_light(w, sector, maxlight);
    add_special(w, SpecialThinker::Glow(Glow { sector, minlight, maxlight, direction: -1 }));
    w.level.sector_mut(sector).special = 0;
}
