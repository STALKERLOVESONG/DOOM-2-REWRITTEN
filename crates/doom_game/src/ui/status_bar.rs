//! Status bar (st_stuff.c, st_lib.c): STBAR with ammo, health, arms, face,
//! armour, keys and ammo tallies, plus the face animation state machine.
//!
//! Also provides a compact full-screen HUD (an extension, not vanilla) for
//! `screenblocks 11`, built from the same WAD graphics.

use std::sync::Arc;

use doom_content::picture::Picture;
use doom_content::ResourceStack;
use doom_core::angle::{ANG180, ANG45};
use doom_core::geom::point_to_angle;
use doom_sim::player::{cheat, power, NUMWEAPONS};
use doom_sim::World;

use super::gfx::{draw_patch, Patches, SCREENHEIGHT};

pub const ST_HEIGHT: i32 = 32;
pub const ST_Y: i32 = SCREENHEIGHT as i32 - ST_HEIGHT;

const ST_NUMPAINFACES: usize = 5;
const ST_NUMSTRAIGHTFACES: usize = 3;
const ST_FACESTRIDE: usize = 8;
const ST_TURNOFFSET: usize = ST_NUMSTRAIGHTFACES;
const ST_OUCHOFFSET: usize = ST_TURNOFFSET + 2;
const ST_EVILGRINOFFSET: usize = ST_OUCHOFFSET + 1;
const ST_RAMPAGEOFFSET: usize = ST_EVILGRINOFFSET + 1;
const ST_GODFACE: usize = ST_NUMPAINFACES * ST_FACESTRIDE;
const ST_DEADFACE: usize = ST_GODFACE + 1;
const TICRATE: i32 = 35;
const ST_EVILGRINCOUNT: i32 = 2 * TICRATE;
const ST_STRAIGHTFACECOUNT: i32 = TICRATE / 2;
const ST_TURNCOUNT: i32 = TICRATE;
const ST_RAMPAGEDELAY: i32 = 2 * TICRATE;
const ST_MUCHPAIN: i32 = 20;

struct Graphics {
    sbar: Option<Arc<Picture>>,
    sbarr: Option<Arc<Picture>>,
    tallnum: Vec<Arc<Picture>>,
    shortnum: Vec<Arc<Picture>>,
    tallpercent: Arc<Picture>,
    minus: Option<Arc<Picture>>,
    keys: Vec<Arc<Picture>>,
    armsbg: Arc<Picture>,
    /// Grey (not owned) weapon numbers 2..7.
    arms_grey: Vec<Arc<Picture>>,
    faces: Vec<Arc<Picture>>,
}

pub struct StatusBar {
    g: Graphics,
    // Face state (ST_updateFaceWidget's statics included).
    faceindex: usize,
    facecount: i32,
    priority: i32,
    lastattackdown: i32,
    oldhealth: i32,
    oldweaponsowned: [bool; NUMWEAPONS],
    paincalc_health: i32,
    paincalc: usize,
    randomnumber: i32,
}

impl StatusBar {
    pub fn new(stack: &ResourceStack, p: &mut Patches) -> Self {
        let mut faces = Vec::new();
        for i in 0..ST_NUMPAINFACES {
            for j in 0..ST_NUMSTRAIGHTFACES {
                faces.push(p.need(stack, &format!("STFST{i}{j}")));
            }
            faces.push(p.need(stack, &format!("STFTR{i}0")));
            faces.push(p.need(stack, &format!("STFTL{i}0")));
            faces.push(p.need(stack, &format!("STFOUCH{i}")));
            faces.push(p.need(stack, &format!("STFEVL{i}")));
            faces.push(p.need(stack, &format!("STFKILL{i}")));
        }
        faces.push(p.need(stack, "STFGOD0"));
        faces.push(p.need(stack, "STFDEAD0"));
        let (sbar, sbarr) = match p.get(stack, "STBAR") {
            Some(b) => (Some(b), None),
            None => (p.get(stack, "STMBARL"), p.get(stack, "STMBARR")),
        };
        let g = Graphics {
            sbar,
            sbarr,
            tallnum: (0..10).map(|i| p.need(stack, &format!("STTNUM{i}"))).collect(),
            shortnum: (0..10).map(|i| p.need(stack, &format!("STYSNUM{i}"))).collect(),
            tallpercent: p.need(stack, "STTPRCNT"),
            minus: p.get(stack, "STTMINUS"),
            keys: (0..6).map(|i| p.need(stack, &format!("STKEYS{i}"))).collect(),
            armsbg: p.need(stack, "STARMS"),
            arms_grey: (2..8).map(|i| p.need(stack, &format!("STGNUM{i}"))).collect(),
            faces,
        };
        StatusBar {
            g,
            faceindex: 0,
            facecount: 0,
            priority: 0,
            lastattackdown: -1,
            oldhealth: -1,
            oldweaponsowned: [false; NUMWEAPONS],
            paincalc_health: -1,
            paincalc: 0,
            randomnumber: 0,
        }
    }

    /// `ST_Start`: reset per-level face state.
    pub fn start(&mut self, w: &World) {
        let p = w.console_player();
        self.faceindex = 0;
        self.facecount = 0;
        self.priority = 0;
        self.lastattackdown = -1;
        self.oldhealth = -1;
        self.oldweaponsowned = p.weaponowned;
    }

    fn calc_pain_offset(&mut self, health: i32) -> usize {
        let health = health.min(100);
        if health != self.paincalc_health {
            self.paincalc = ST_FACESTRIDE * (((100 - health) * ST_NUMPAINFACES as i32) / 101) as usize;
            self.paincalc_health = health;
        }
        self.paincalc
    }

    /// `ST_Ticker`: once per game tic.
    pub fn ticker(&mut self, w: &mut World) {
        self.randomnumber = w.rng.m_random();
        self.update_face(w);
        self.oldhealth = w.console_player().health;
    }

    fn update_face(&mut self, w: &World) {
        let p = w.console_player();
        let mo = p.mo.and_then(|id| w.thinkers.get_mobj(id));
        if self.priority < 10 && p.health == 0 {
            self.priority = 9;
            self.faceindex = ST_DEADFACE;
            self.facecount = 1;
        }
        if self.priority < 9 && p.bonuscount != 0 {
            let mut evilgrin = false;
            for i in 0..NUMWEAPONS {
                if self.oldweaponsowned[i] != p.weaponowned[i] {
                    evilgrin = true;
                    self.oldweaponsowned[i] = p.weaponowned[i];
                }
            }
            if evilgrin {
                self.priority = 8;
                self.facecount = ST_EVILGRINCOUNT;
                self.faceindex = self.calc_pain_offset(p.health) + ST_EVILGRINOFFSET;
            }
        }
        if self.priority < 8 && p.damagecount != 0 {
            let attacker = p.attacker.filter(|&a| Some(a) != p.mo).and_then(|a| w.thinkers.get_mobj(a));
            if let (Some(att), Some(me)) = (attacker, mo) {
                self.priority = 7;
                if p.health - self.oldhealth > ST_MUCHPAIN {
                    self.facecount = ST_TURNCOUNT;
                    self.faceindex = self.calc_pain_offset(p.health) + ST_OUCHOFFSET;
                } else {
                    let badguyangle = point_to_angle(att.x - me.x, att.y - me.y).0;
                    let (diffang, right) = if badguyangle > me.angle.0 {
                        let d = badguyangle.wrapping_sub(me.angle.0);
                        (d, d > ANG180)
                    } else {
                        let d = me.angle.0.wrapping_sub(badguyangle);
                        (d, d <= ANG180)
                    };
                    self.facecount = ST_TURNCOUNT;
                    self.faceindex = self.calc_pain_offset(p.health)
                        + if diffang < ANG45 {
                            ST_RAMPAGEOFFSET
                        } else if right {
                            ST_TURNOFFSET
                        } else {
                            ST_TURNOFFSET + 1
                        };
                }
            }
        }
        if self.priority < 7 && p.damagecount != 0 {
            // Hurt by your own damn stupidity.
            if p.health - self.oldhealth > ST_MUCHPAIN {
                self.priority = 7;
                self.facecount = ST_TURNCOUNT;
                self.faceindex = self.calc_pain_offset(p.health) + ST_OUCHOFFSET;
            } else {
                self.priority = 6;
                self.facecount = ST_TURNCOUNT;
                self.faceindex = self.calc_pain_offset(p.health) + ST_RAMPAGEOFFSET;
            }
        }
        if self.priority < 6 {
            // Rapid firing.
            if p.attackdown {
                if self.lastattackdown == -1 {
                    self.lastattackdown = ST_RAMPAGEDELAY;
                } else {
                    self.lastattackdown -= 1;
                    if self.lastattackdown == 0 {
                        self.priority = 5;
                        self.faceindex = self.calc_pain_offset(p.health) + ST_RAMPAGEOFFSET;
                        self.facecount = 1;
                        self.lastattackdown = 1;
                    }
                }
            } else {
                self.lastattackdown = -1;
            }
        }
        if self.priority < 5 && (p.cheats & cheat::GODMODE != 0 || p.powers[power::INVULNERABILITY] != 0) {
            self.priority = 4;
            self.faceindex = ST_GODFACE;
            self.facecount = 1;
        }
        // Look left or right if the facecount has timed out.
        if self.facecount == 0 {
            self.faceindex = self.calc_pain_offset(p.health) + (self.randomnumber % 3) as usize;
            self.facecount = ST_STRAIGHTFACECOUNT;
            self.priority = 0;
        }
        self.facecount -= 1;
    }

    /// `STlib_drawNum`: right-aligned at x.
    fn draw_num(screen: &mut [u8], digits: &[Arc<Picture>], minus: Option<&Picture>, x: i32, y: i32, width: i32, num: i32) {
        let w = digits[0].width;
        let mut num = num;
        let neg = num < 0;
        if neg {
            num = match width {
                2 if num < -9 => -9,
                3 if num < -99 => -99,
                _ => num,
            };
            num = -num;
        }
        // 1994 means "not applicable" (fist/chainsaw ammo).
        if num == 1994 {
            return;
        }
        let mut x = x;
        if num == 0 {
            draw_patch(screen, x - w, y, &digits[0]);
        }
        let mut n = width;
        while num != 0 && n > 0 {
            x -= w;
            draw_patch(screen, x, y, &digits[(num % 10) as usize]);
            num /= 10;
            n -= 1;
        }
        if neg {
            if let Some(m) = minus {
                draw_patch(screen, x - 8, y, m);
            }
        }
    }

    /// `ST_Drawer` with the status bar on.
    pub fn draw(&self, screen: &mut [u8], w: &World) {
        let p = w.console_player();
        let g = &self.g;
        if let Some(b) = &g.sbar {
            draw_patch(screen, 0, ST_Y, b);
        }
        if let Some(b) = &g.sbarr {
            draw_patch(screen, 104, ST_Y, b);
        }
        let ready = doom_sim::weapons::ready_ammo(w, w.params.consoleplayer).map_or(1994, |(n, _)| n);
        let minus = g.minus.as_deref();
        Self::draw_num(screen, &g.tallnum, minus, 44, 171, 3, ready);
        Self::draw_num(screen, &g.tallnum, minus, 90, 171, 3, p.health);
        draw_patch(screen, 90, 171, &g.tallpercent);
        Self::draw_num(screen, &g.tallnum, minus, 221, 171, 3, p.armorpoints);
        draw_patch(screen, 221, 171, &g.tallpercent);
        if w.params.deathmatch == 0 {
            draw_patch(screen, 104, 168, &g.armsbg);
            for i in 0..6 {
                let (x, y) = (111 + (i % 3) as i32 * 12, 172 + (i / 3) as i32 * 10);
                // Owned weapons use the yellow short numbers.
                let pic = if p.weaponowned[i + 1] { &g.shortnum[i + 2] } else { &g.arms_grey[i] };
                draw_patch(screen, x, y, pic);
            }
        } else {
            let mut frags = 0;
            for (i, f) in p.frags.iter().enumerate() {
                if i == w.params.consoleplayer {
                    frags -= f;
                } else {
                    frags += f;
                }
            }
            Self::draw_num(screen, &g.tallnum, minus, 138, 171, 2, frags);
        }
        draw_patch(screen, 143, 168, &g.faces[self.faceindex.min(g.faces.len() - 1)]);
        for i in 0..3 {
            let k = if p.cards[i + 3] { Some(i + 3) } else if p.cards[i] { Some(i) } else { None };
            if let Some(k) = k {
                draw_patch(screen, 239, 171 + i as i32 * 10, &g.keys[k]);
            }
        }
        // Ammo tallies: bullets, shells, rockets, cells laid out as vanilla.
        let ys = [173, 179, 191, 185];
        for (i, y) in ys.into_iter().enumerate() {
            Self::draw_num(screen, &g.shortnum, None, 288, y, 3, p.ammo[i]);
            Self::draw_num(screen, &g.shortnum, None, 314, y, 3, p.maxammo[i]);
        }
    }

    /// Full-screen HUD (extension): health, armour, ammo and keys in the
    /// corners using the status bar's tall numbers.
    pub fn draw_fullscreen(&self, screen: &mut [u8], w: &World) {
        let p = w.console_player();
        let g = &self.g;
        let minus = g.minus.as_deref();
        let y = SCREENHEIGHT as i32 - 2 - g.tallnum[0].height;
        Self::draw_num(screen, &g.tallnum, minus, 2 + 3 * g.tallnum[0].width, y, 3, p.health);
        draw_patch(screen, 2 + 3 * g.tallnum[0].width, y, &g.tallpercent);
        let ax = 2 + 4 * g.tallnum[0].width + g.tallpercent.width + 12;
        if p.armorpoints > 0 {
            Self::draw_num(screen, &g.tallnum, minus, ax + 3 * g.tallnum[0].width, y, 3, p.armorpoints);
            draw_patch(screen, ax + 3 * g.tallnum[0].width, y, &g.tallpercent);
        }
        if let Some((n, _)) = doom_sim::weapons::ready_ammo(w, w.params.consoleplayer) {
            Self::draw_num(screen, &g.tallnum, minus, 318, y, 3, n);
        }
        for i in 0..3 {
            let k = if p.cards[i + 3] { Some(i + 3) } else if p.cards[i] { Some(i) } else { None };
            if let Some(k) = k {
                draw_patch(screen, 318 - 3 * g.tallnum[0].width - 14, y + i as i32 * 6, &g.keys[k]);
            }
        }
    }
}
