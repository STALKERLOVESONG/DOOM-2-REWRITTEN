//! Finale (f_finale.c): the episode/story text crawl over a flat, the
//! end art screens, the episode 3 bunny scroll, and the Doom II cast call.

use std::sync::Arc;

use doom_content::game::{GameInfo, GameMission, GameMode, GameVersion};
use doom_content::picture::Picture;
use doom_content::ResourceStack;
use doom_render::RenderData;
use doom_sim::info::*;

use super::gfx::{draw_patch, draw_patch_flipped, flat_pixels, tile_flat, Font, Patches, SCREENHEIGHT, SCREENWIDTH};
use super::strings::*;

const TEXTSPEED: i32 = 3;
const TEXTWAIT: i32 = 250;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Text,
    ArtScreen,
    Cast,
}

/// What the session must do after a finale tic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinaleEvent {
    StartSound(SfxNum),
    ChangeMusic(MusicNum),
    /// Doom II text screens: continue to the next map (`ga_worlddone`).
    WorldDone,
}

const CAST: [(&str, MobjType); 17] = [
    (CC_ZOMBIE, MT_POSSESSED),
    (CC_SHOTGUN, MT_SHOTGUY),
    (CC_HEAVY, MT_CHAINGUY),
    (CC_IMP, MT_TROOP),
    (CC_DEMON, MT_SERGEANT),
    (CC_LOST, MT_SKULL),
    (CC_CACO, MT_HEAD),
    (CC_HELL, MT_KNIGHT),
    (CC_BARON, MT_BRUISER),
    (CC_ARACH, MT_BABY),
    (CC_PAIN, MT_PAIN),
    (CC_REVEN, MT_UNDEAD),
    (CC_MANCU, MT_FATSO),
    (CC_ARCH, MT_VILE),
    (CC_SPIDER, MT_SPIDER),
    (CC_CYBER, MT_CYBORG),
    (CC_HERO, MT_PLAYER),
];

pub struct Finale {
    stage: Stage,
    count: i32,
    text: &'static str,
    flat: Vec<u8>,
    commercial: bool,
    episode: i32,
    map: i32,
    version: GameVersion,
    art: Option<Arc<Picture>>,
    pfub: [Option<Arc<Picture>>; 2],
    end: Vec<Option<Arc<Picture>>>,
    bossback: Option<Arc<Picture>>,
    laststage: i32,
    // Cast call.
    castnum: usize,
    caststate: StateNum,
    casttics: i32,
    castdeath: bool,
    castframes: i32,
    castonmelee: bool,
    castattacking: bool,
}

impl Finale {
    /// `F_StartFinale`.
    pub fn new(stack: &ResourceStack, p: &mut Patches, game: &GameInfo, episode: i32, map: i32, out: &mut Vec<FinaleEvent>) -> Self {
        let commercial = game.mode == GameMode::Commercial;
        out.push(FinaleEvent::ChangeMusic(if game.mission == GameMission::Doom { MUS_VICTOR } else { MUS_READ_M }));
        let (text, flat) = match game.mission {
            GameMission::Doom => match episode {
                1 => (E1TEXT, "FLOOR4_8"),
                2 => (E2TEXT, "SFLR6_1"),
                3 => (E3TEXT, "MFLR8_4"),
                _ => (E4TEXT, "MFLR8_3"),
            },
            mission => {
                let texts = match mission {
                    GameMission::Tnt => [T1TEXT, T2TEXT, T3TEXT, T4TEXT, T5TEXT, T6TEXT],
                    GameMission::Plutonia => [P1TEXT, P2TEXT, P3TEXT, P4TEXT, P5TEXT, P6TEXT],
                    _ => [C1TEXT, C2TEXT, C3TEXT, C4TEXT, C5TEXT, C6TEXT],
                };
                match map {
                    6 => (texts[0], "SLIME16"),
                    11 => (texts[1], "RROCK14"),
                    20 => (texts[2], "RROCK07"),
                    30 => (texts[3], "RROCK17"),
                    15 => (texts[4], "RROCK13"),
                    _ => (texts[5], "RROCK19"),
                }
            }
        };
        let art_name = match episode {
            1 if game.version >= GameVersion::Ultimate => "CREDIT",
            1 => "HELP2",
            2 => "VICTORY2",
            4 => "ENDPIC",
            _ => "",
        };
        Finale {
            stage: Stage::Text,
            count: 0,
            text,
            flat: flat_pixels(stack, flat).map(<[u8]>::to_vec).unwrap_or_default(),
            commercial,
            episode,
            map,
            version: game.version,
            art: if art_name.is_empty() { None } else { p.get(stack, art_name) },
            pfub: [p.get(stack, "PFUB2"), p.get(stack, "PFUB1")],
            end: (0..7).map(|i| p.get(stack, &format!("END{i}"))).collect(),
            bossback: p.get(stack, "BOSSBACK"),
            laststage: 0,
            castnum: 0,
            caststate: StateNum(0),
            casttics: 0,
            castdeath: false,
            castframes: 0,
            castonmelee: false,
            castattacking: false,
        }
    }

    /// `F_Ticker`. `any_button`: the player pressed a game button this tic.
    pub fn ticker(&mut self, info: &Info, any_button: bool, out: &mut Vec<FinaleEvent>) {
        if self.commercial && self.count > 50 && any_button && self.stage == Stage::Text {
            if self.map == 30 {
                self.start_cast(info, out);
            } else {
                out.push(FinaleEvent::WorldDone);
            }
        }
        self.count += 1;
        if self.stage == Stage::Cast {
            self.cast_ticker(info, out);
            return;
        }
        if self.commercial {
            return;
        }
        if self.stage == Stage::Text && self.count > self.text.len() as i32 * TEXTSPEED + TEXTWAIT {
            self.count = 0;
            self.stage = Stage::ArtScreen;
            if self.episode == 3 {
                out.push(FinaleEvent::ChangeMusic(MUS_BUNNY));
            }
        }
        let _ = self.version;
    }

    fn start_cast(&mut self, info: &Info, out: &mut Vec<FinaleEvent>) {
        self.castnum = 0;
        self.caststate = info.mobjinfo[CAST[0].1.idx()].seestate;
        self.casttics = info.states[self.caststate.idx()].tics;
        self.castdeath = false;
        self.stage = Stage::Cast;
        self.castframes = 0;
        self.castonmelee = false;
        self.castattacking = false;
        out.push(FinaleEvent::ChangeMusic(MUS_EVIL));
    }

    fn cast_ticker(&mut self, info: &Info, out: &mut Vec<FinaleEvent>) {
        self.casttics -= 1;
        if self.casttics > 0 {
            return;
        }
        let mi = |n: usize| &info.mobjinfo[CAST[n].1.idx()];
        let st = info.states[self.caststate.idx()];
        let mut stop_attack = false;
        if st.tics == -1 || st.next == S_NULL {
            // Switch from deathstate to next monster.
            self.castnum = (self.castnum + 1) % CAST.len();
            self.castdeath = false;
            if mi(self.castnum).seesound.0 != 0 {
                out.push(FinaleEvent::StartSound(mi(self.castnum).seesound));
            }
            self.caststate = mi(self.castnum).seestate;
            self.castframes = 0;
        } else if self.caststate == S_PLAY_ATK1 {
            // Oh, gross hack!
            stop_attack = true;
        } else {
            let next = st.next;
            self.caststate = next;
            self.castframes += 1;
            let sfx = match next {
                S_PLAY_ATK1 => SFX_DSHTGN,
                S_POSS_ATK2 => SFX_PISTOL,
                S_SPOS_ATK2 => SFX_SHOTGN,
                S_VILE_ATK2 => SFX_VILATK,
                S_SKEL_FIST2 => SFX_SKESWG,
                S_SKEL_FIST4 => SFX_SKEPCH,
                S_SKEL_MISS2 => SFX_SKEATK,
                S_FATT_ATK8 | S_FATT_ATK5 | S_FATT_ATK2 => SFX_FIRSHT,
                S_CPOS_ATK2 | S_CPOS_ATK3 | S_CPOS_ATK4 => SFX_SHOTGN,
                S_TROO_ATK3 => SFX_CLAW,
                S_SARG_ATK2 => SFX_SGTATK,
                S_BOSS_ATK2 | S_BOS2_ATK2 | S_HEAD_ATK2 => SFX_FIRSHT,
                S_SKULL_ATK2 => SFX_SKLATK,
                S_SPID_ATK2 | S_SPID_ATK3 => SFX_SHOTGN,
                S_BSPI_ATK2 => SFX_PLASMA,
                S_CYBER_ATK2 | S_CYBER_ATK4 | S_CYBER_ATK6 => SFX_RLAUNC,
                S_PAIN_ATK3 => SFX_SKLATK,
                _ => SfxNum(0),
            };
            if sfx.0 != 0 {
                out.push(FinaleEvent::StartSound(sfx));
            }
        }
        if !stop_attack && self.castframes == 12 {
            // Go into attack frame.
            self.castattacking = true;
            let m = mi(self.castnum);
            self.caststate = if self.castonmelee { m.meleestate } else { m.missilestate };
            self.castonmelee = !self.castonmelee;
            if self.caststate == S_NULL {
                self.caststate = if self.castonmelee { m.meleestate } else { m.missilestate };
            }
        }
        if stop_attack || (self.castattacking && (self.castframes == 24 || self.caststate == mi(self.castnum).seestate)) {
            self.castattacking = false;
            self.castframes = 0;
            self.caststate = mi(self.castnum).seestate;
        }
        self.casttics = info.states[self.caststate.idx()].tics;
        if self.casttics == -1 {
            self.casttics = 15;
        }
    }

    /// `F_Responder`: a key during the cast call kills the current monster.
    pub fn responder(&mut self, info: &Info, out: &mut Vec<FinaleEvent>) -> bool {
        if self.stage != Stage::Cast {
            return false;
        }
        if self.castdeath {
            return true;
        }
        let m = &info.mobjinfo[CAST[self.castnum].1.idx()];
        self.castdeath = true;
        self.caststate = m.deathstate;
        self.casttics = info.states[self.caststate.idx()].tics;
        self.castframes = 0;
        self.castattacking = false;
        if m.deathsound.0 != 0 {
            out.push(FinaleEvent::StartSound(m.deathsound));
        }
        true
    }

    /// `F_Drawer`.
    pub fn draw(&mut self, screen: &mut [u8], font: &Font, rdata: &RenderData, info: &Info, out: &mut Vec<FinaleEvent>) {
        match self.stage {
            Stage::Text => {
                tile_flat(screen, &self.flat, 0, 0, SCREENWIDTH as i32, SCREENHEIGHT as i32);
                let count = ((self.count - 10) / TEXTSPEED).max(0) as usize;
                let shown: String = self.text.chars().take(count).collect();
                // F_TextWrite: 11-pixel line spacing from (10, 10).
                for (i, line) in shown.split('\n').enumerate() {
                    font.draw(screen, 10, 10 + i as i32 * 11, line);
                }
            }
            Stage::ArtScreen => {
                if self.episode == 3 {
                    self.bunny_scroll(screen, out);
                } else if let Some(a) = &self.art {
                    draw_patch(screen, 0, 0, a);
                }
            }
            Stage::Cast => {
                if let Some(b) = &self.bossback {
                    draw_patch(screen, 0, 0, b);
                }
                let name = CAST[self.castnum].0;
                let x = SCREENWIDTH as i32 / 2 - font.width(name) / 2;
                font.draw(screen, x, 180, name);
                let st = info.states[self.caststate.idx()];
                let Some(def) = rdata.sprites.get(st.sprite.idx()) else { return };
                let Some(frame) = def.frames.get((st.frame & FF_FRAMEMASK) as usize) else { return };
                if let Some(pic) = rdata.sprite_picture(frame.lump[0] as usize) {
                    if frame.flip[0] {
                        draw_patch_flipped(screen, SCREENWIDTH as i32 / 2, 170, &pic);
                    } else {
                        draw_patch(screen, SCREENWIDTH as i32 / 2, 170, &pic);
                    }
                }
            }
        }
    }

    fn bunny_scroll(&mut self, screen: &mut [u8], out: &mut Vec<FinaleEvent>) {
        let scrolled = (SCREENWIDTH as i32 - (self.count - 230) / 2).clamp(0, SCREENWIDTH as i32);
        let [Some(p1), Some(p2)] = &self.pfub else { return };
        for x in 0..SCREENWIDTH as i32 {
            let (pic, col) = if x + scrolled < SCREENWIDTH as i32 { (p1, x + scrolled) } else { (p2, x + scrolled - SCREENWIDTH as i32) };
            // F_DrawPatchCol
            if col < 0 || col >= pic.width {
                continue;
            }
            for post in pic.column(col as usize) {
                for (i, &c) in pic.post_pixels(post).iter().enumerate() {
                    let y = post.top + i as i32;
                    if (0..SCREENHEIGHT as i32).contains(&y) {
                        screen[y as usize * SCREENWIDTH + x as usize] = c;
                    }
                }
            }
        }
        if self.count < 1130 {
            return;
        }
        let pos = ((SCREENWIDTH as i32 - 13 * 8) / 2, (SCREENHEIGHT as i32 - 8 * 8) / 2);
        if self.count < 1180 {
            if let Some(Some(e)) = self.end.first() {
                draw_patch(screen, pos.0, pos.1, e);
            }
            self.laststage = 0;
            return;
        }
        let stage = ((self.count - 1180) / 5).min(6);
        if stage > self.laststage {
            out.push(FinaleEvent::StartSound(SFX_PISTOL));
            self.laststage = stage;
        }
        if let Some(Some(e)) = self.end.get(stage as usize) {
            draw_patch(screen, pos.0, pos.1, e);
        }
    }
}
