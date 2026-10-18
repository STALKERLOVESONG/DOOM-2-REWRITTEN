//! Heads-up text (hu_stuff.c): the one-line message at the top of the
//! screen and the level title shown on the automap.

use doom_content::game::{GameInfo, GameMission, GameMode};

use super::gfx::Font;
use super::strings;

/// `HU_MSGTIMEOUT`.
const MSG_TICS: i32 = 4 * 35;

#[derive(Default)]
pub struct Hud {
    message: Option<(String, i32)>,
}

impl Hud {
    pub fn show(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), MSG_TICS));
    }

    pub fn clear(&mut self) {
        self.message = None;
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(m, _)| m.as_str())
    }

    /// `HU_Ticker`.
    pub fn ticker(&mut self) {
        if let Some((_, t)) = &mut self.message {
            *t -= 1;
            if *t <= 0 {
                self.message = None;
            }
        }
    }

    /// `HU_Drawer`: message at (0, 0).
    pub fn draw(&self, screen: &mut [u8], font: &Font) {
        if let Some((m, _)) = &self.message {
            font.draw(screen, 0, 0, m);
        }
    }
}

/// The automap title (`HU_TITLE` etc.): "E1M1: Hangar", "level 1: entryway".
/// `strings`: DEHACKED replacements (BEX `HUSTR_*`).
pub fn map_title(game: &GameInfo, strings: &doom_sim::dehacked::StringTable, episode: i32, map: i32) -> String {
    let title = match (game.mode, game.mission) {
        (GameMode::Commercial, GameMission::Plutonia) => strings::MAP_TITLES_PLUTONIA.get((map - 1) as usize),
        (GameMode::Commercial, GameMission::Tnt) => strings::MAP_TITLES_TNT.get((map - 1) as usize),
        (GameMode::Commercial, _) => strings::MAP_TITLES_DOOM2.get((map - 1) as usize),
        _ => {
            if (1..=4).contains(&episode) && (1..=9).contains(&map) {
                strings::MAP_TITLES_DOOM1.get(((episode - 1) * 9 + map - 1) as usize)
            } else {
                None
            }
        }
    };
    match title {
        Some(t) => strings.get(t).to_string(),
        None if game.mode == GameMode::Commercial => format!("MAP{map:02}"),
        None => format!("E{episode}M{map}"),
    }
}
