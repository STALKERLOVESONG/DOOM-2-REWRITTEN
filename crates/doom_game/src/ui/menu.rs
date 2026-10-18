//! Menus (m_menu.c): main, episode, skill, options, sound volume, read
//! this, and the y/n message prompts, with the vanilla layout, skull
//! cursor, keyboard behaviour and sounds.
//!
//! The menu edits [`Settings`] directly and reports everything else the
//! session must act on as [`MenuAction`]s. Save/load slots are shown but
//! saving is not implemented yet.
//!
//! Beyond vanilla, Options has an "Engine options" page listing every
//! console variable that declares a menu label ([`CVarSpec::menu`]), drawn
//! with the HUD font; left/right/enter step the value.
//!
//! [`CVarSpec::menu`]: crate::cvar::CVarSpec::menu

use std::sync::Arc;

use doom_content::game::{GameInfo, GameMission, GameMode, GameVersion};
use doom_content::picture::Picture;
use doom_content::ResourceStack;
use doom_sim::info::*;
use doom_sim::Skill;

use crate::cvar::CVarRegistry;

use super::gfx::{draw_patch, Font, Patches, SCREENHEIGHT, SCREENWIDTH};
use super::strings;
use super::Key;

const LINEHEIGHT: i32 = 16;
const SKULLXOFF: i32 = -32;

/// Player-adjustable settings the menu edits (vanilla units).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// 0..=15
    pub sfx_volume: i32,
    /// 0..=15
    pub music_volume: i32,
    /// 0..=9
    pub mouse_sensitivity: i32,
    /// 3..=11
    pub screenblocks: i32,
    pub low_detail: bool,
    pub show_messages: bool,
    /// `usegamma`: 0..=4 (F11).
    pub gamma: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { sfx_volume: 8, music_volume: 8, mouse_sensitivity: 5, screenblocks: 10, low_detail: false, show_messages: true, gamma: 0 }
    }
}

/// Something the session must do in response to menu input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewGame { episode: i32, skill: Skill },
    /// "End game": back to the title screen.
    EndGame,
    Quit,
    /// Screen size or detail changed.
    ViewChanged,
    VolumeChanged,
    /// A player-facing message (e.g. detail level changed).
    Message(String),
    StartSound(SfxNum),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuId {
    Main,
    Episode,
    NewGame,
    Options,
    Sound,
    Read1,
    Read2,
    Load,
    Save,
    /// Console variables with a menu label.
    Engine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemAct {
    NewGameMenu,
    Options,
    LoadGame,
    SaveGame,
    ReadThis,
    ReadThis2,
    FinishReadThis,
    Quit,
    Episode(i32),
    Skill(Skill),
    EndGame,
    Messages,
    Detail,
    ScreenSize,
    MouseSens,
    SoundMenu,
    SfxVol,
    MusicVol,
    Slot(usize),
    EngineOptions,
    /// Step the n-th variable of the engine page.
    CVar(usize),
}

#[derive(Clone, Debug)]
struct Item {
    /// 1 = selectable, 2 = slider (left/right), -1 = spacer.
    status: i8,
    patch: Option<Arc<Picture>>,
    /// Drawn with the HUD font when the WAD lacks the item's graphic (e.g.
    /// IWADs with E4 maps but no M_EPI4).
    label: &'static str,
    act: Option<ItemAct>,
    alpha: u8,
}

struct Menu {
    items: Vec<Item>,
    prev: Option<MenuId>,
    x: i32,
    y: i32,
    last_on: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageKind {
    /// Any key dismisses.
    Info,
    Nightmare,
    Quit,
    EndGame,
}

pub struct MenuSystem {
    menus: Vec<(MenuId, Menu)>,
    current: MenuId,
    item_on: usize,
    pub active: bool,
    which_skull: usize,
    skull_counter: i32,
    skulls: [Arc<Picture>; 2],
    message: Option<(String, MessageKind)>,
    message_last_active: bool,
    epi: i32,
    game: GameInfo,
    p: MenuPatches,
    /// Variables on the engine page, in item order.
    engine_vars: Vec<&'static str>,
}

struct MenuPatches {
    doom: Arc<Picture>,
    newg: Arc<Picture>,
    skill: Arc<Picture>,
    episod: Arc<Picture>,
    optttl: Arc<Picture>,
    svol: Arc<Picture>,
    detail: [Arc<Picture>; 2],
    msg: [Arc<Picture>; 2],
    therm: [Arc<Picture>; 4],
    help1: Option<Arc<Picture>>,
    help2: Option<Arc<Picture>>,
    help: Option<Arc<Picture>>,
    credit: Option<Arc<Picture>>,
    loadg: Arc<Picture>,
    saveg: Arc<Picture>,
    lsl: Arc<Picture>,
    lsc: Arc<Picture>,
    lsr: Arc<Picture>,
    pause: Arc<Picture>,
}

impl MenuSystem {
    pub fn new(stack: &ResourceStack, p: &mut Patches, game: GameInfo) -> Self {
        let item = |status: i8, patch: Option<Arc<Picture>>, act: Option<ItemAct>, alpha: u8| Item { status, patch, label: "", act, alpha };
        let mut pg = |name: &str| p.get(stack, name);
        let commercial = game.mode == GameMode::Commercial;
        let ultimate = game.version >= GameVersion::Ultimate;

        let mut main = vec![
            item(1, pg("M_NGAME"), Some(ItemAct::NewGameMenu), b'n'),
            item(1, pg("M_OPTION"), Some(ItemAct::Options), b'o'),
            item(1, pg("M_LOADG"), Some(ItemAct::LoadGame), b'l'),
            item(1, pg("M_SAVEG"), Some(ItemAct::SaveGame), b's'),
            item(1, pg("M_RDTHIS"), Some(if ultimate { ItemAct::ReadThis2 } else { ItemAct::ReadThis }), b'r'),
            item(1, pg("M_QUITG"), Some(ItemAct::Quit), b'q'),
        ];
        let mut main_y = 64;
        if commercial {
            // Doom II: "Read This" replaced by "Quit".
            main.remove(4);
            main_y += 8;
        }
        let mut episodes: Vec<Item> = ["M_EPI1", "M_EPI2", "M_EPI3", "M_EPI4"]
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut it = item(1, pg(n), Some(ItemAct::Episode(i as i32)), b"ktit"[i]);
                it.label = ["Knee-Deep in the Dead", "The Shores of Hell", "Inferno", "Thy Flesh Consumed"][i];
                it
            })
            .collect();
        if !ultimate || game.version == GameVersion::Chex {
            episodes.truncate(if game.version == GameVersion::Chex { 1 } else { 3 });
        }
        let skills = [
            ("M_JKILL", Skill::Baby, b'i'),
            ("M_ROUGH", Skill::Easy, b'h'),
            ("M_HURT", Skill::Medium, b'h'),
            ("M_ULTRA", Skill::Hard, b'u'),
            ("M_NMARE", Skill::Nightmare, b'n'),
        ]
        .iter()
        .map(|(n, s, a)| item(1, pg(n), Some(ItemAct::Skill(*s)), *a))
        .collect();
        let options = vec![
            item(1, pg("M_ENDGAM"), Some(ItemAct::EndGame), b'e'),
            item(1, pg("M_MESSG"), Some(ItemAct::Messages), b'm'),
            item(1, pg("M_DETAIL"), Some(ItemAct::Detail), b'g'),
            item(2, pg("M_SCRNSZ"), Some(ItemAct::ScreenSize), b's'),
            item(-1, None, None, 0),
            item(2, pg("M_MSENS"), Some(ItemAct::MouseSens), b'm'),
            item(-1, None, None, 0),
            item(1, pg("M_SVOL"), Some(ItemAct::SoundMenu), b's'),
            Item { label: "Engine options", ..item(1, None, Some(ItemAct::EngineOptions), b'x') },
        ];
        let sound = vec![
            item(2, pg("M_SFXVOL"), Some(ItemAct::SfxVol), b's'),
            item(-1, None, None, 0),
            item(2, pg("M_MUSVOL"), Some(ItemAct::MusicVol), b'm'),
            item(-1, None, None, 0),
        ];
        let slots = |_: ()| (0..6).map(|i| item(1, None, Some(ItemAct::Slot(i)), b'1' + i as u8)).collect::<Vec<_>>();
        let menus = vec![
            (MenuId::Main, Menu { items: main, prev: None, x: 97, y: main_y, last_on: 0 }),
            (MenuId::Episode, Menu { items: episodes, prev: Some(MenuId::Main), x: 48, y: 63, last_on: 0 }),
            (
                MenuId::NewGame,
                Menu {
                    items: skills,
                    prev: Some(if commercial { MenuId::Main } else { MenuId::Episode }),
                    x: 48,
                    y: 63,
                    last_on: 2,
                },
            ),
            (MenuId::Options, Menu { items: options, prev: Some(MenuId::Main), x: 60, y: 37, last_on: 0 }),
            (MenuId::Sound, Menu { items: sound, prev: Some(MenuId::Options), x: 80, y: 64, last_on: 0 }),
            (
                MenuId::Read1,
                Menu {
                    items: vec![item(
                        1,
                        None,
                        Some(if commercial { ItemAct::FinishReadThis } else { ItemAct::ReadThis2 }),
                        0,
                    )],
                    prev: Some(MenuId::Main),
                    x: if commercial { 330 } else { 280 },
                    y: if commercial { 165 } else { 185 },
                    last_on: 0,
                },
            ),
            (
                MenuId::Read2,
                Menu {
                    items: vec![item(1, None, Some(ItemAct::FinishReadThis), 0)],
                    prev: if ultimate { None } else { Some(MenuId::Read1) },
                    x: 330,
                    y: 175,
                    last_on: 0,
                },
            ),
            (MenuId::Load, Menu { items: slots(()), prev: Some(MenuId::Main), x: 80, y: 54, last_on: 0 }),
            (MenuId::Save, Menu { items: slots(()), prev: Some(MenuId::Main), x: 80, y: 54, last_on: 0 }),
            (MenuId::Engine, Menu { items: Vec::new(), prev: Some(MenuId::Options), x: 48, y: 36, last_on: 0 }),
        ];
        let skulls = [p.need(stack, "M_SKULL1"), p.need(stack, "M_SKULL2")];
        let p = MenuPatches {
            doom: p.need(stack, "M_DOOM"),
            newg: p.need(stack, "M_NEWG"),
            skill: p.need(stack, "M_SKILL"),
            episod: p.need(stack, "M_EPISOD"),
            optttl: p.need(stack, "M_OPTTTL"),
            svol: p.need(stack, "M_SVOL"),
            detail: [p.need(stack, "M_GDHIGH"), p.need(stack, "M_GDLOW")],
            msg: [p.need(stack, "M_MSGOFF"), p.need(stack, "M_MSGON")],
            therm: [p.need(stack, "M_THERML"), p.need(stack, "M_THERMM"), p.need(stack, "M_THERMR"), p.need(stack, "M_THERMO")],
            help1: p.get(stack, "HELP1"),
            help2: p.get(stack, "HELP2"),
            help: p.get(stack, "HELP"),
            credit: p.get(stack, "CREDIT"),
            loadg: p.need(stack, "M_LOADG"),
            saveg: p.need(stack, "M_SAVEG"),
            lsl: p.need(stack, "M_LSLEFT"),
            lsc: p.need(stack, "M_LSCNTR"),
            lsr: p.need(stack, "M_LSRGHT"),
            pause: p.need(stack, "M_PAUSE"),
        };
        MenuSystem {
            menus,
            current: MenuId::Main,
            item_on: 0,
            active: false,
            which_skull: 0,
            skull_counter: 10,
            skulls,
            message: None,
            message_last_active: false,
            epi: 0,
            game,
            p,
            engine_vars: Vec::new(),
        }
    }

    fn menu(&self, id: MenuId) -> &Menu {
        &self.menus.iter().find(|(m, _)| *m == id).unwrap().1
    }

    fn menu_mut(&mut self, id: MenuId) -> &mut Menu {
        &mut self.menus.iter_mut().find(|(m, _)| *m == id).unwrap().1
    }

    /// Is the menu (or a prompt) on screen? In single player this pauses
    /// the game, as `P_Ticker` does.
    pub fn is_open(&self) -> bool {
        self.active || self.message.is_some()
    }

    /// `M_StartControlPanel`.
    pub fn open(&mut self) {
        if self.active {
            return;
        }
        self.active = true;
        self.current = MenuId::Main;
        self.item_on = self.menu(MenuId::Main).last_on;
    }

    /// `M_ClearMenus`.
    pub fn close(&mut self) {
        self.active = false;
    }

    fn setup_next(&mut self, id: MenuId) {
        self.current = id;
        self.item_on = self.menu(id).last_on;
    }

    fn start_message(&mut self, text: String, kind: MessageKind) {
        self.message_last_active = self.active;
        self.message = Some((text, kind));
        self.active = true;
    }

    /// `M_Ticker`: once per tic.
    pub fn ticker(&mut self) {
        self.skull_counter -= 1;
        if self.skull_counter <= 0 {
            self.which_skull ^= 1;
            self.skull_counter = 8;
        }
    }

    /// `M_Responder`. Returns true if the key was eaten. `gametic` picks
    /// the quit message/sound like vanilla; `in_game` gates End Game/Save.
    #[allow(clippy::too_many_arguments)]
    pub fn responder(
        &mut self,
        key: Key,
        s: &mut Settings,
        cvars: &CVarRegistry,
        gametic: i32,
        in_game: bool,
        out: &mut Vec<MenuAction>,
    ) -> bool {
        // Prompts first.
        if let Some((_, kind)) = self.message.clone() {
            let needs_input = kind != MessageKind::Info;
            let yes = key == Key::Char('y');
            if needs_input && !matches!(key, Key::Char(' ') | Key::Escape | Key::Char('y') | Key::Char('n')) {
                return false;
            }
            self.message = None;
            self.active = self.message_last_active;
            match kind {
                MessageKind::Nightmare if yes => {
                    out.push(MenuAction::NewGame { episode: self.epi + 1, skill: Skill::Nightmare });
                }
                MessageKind::Quit if yes => {
                    let sounds2 = [SFX_VILACT, SFX_GETPOW, SFX_BOSCUB, SFX_SLOP, SFX_SKESWG, SFX_KNTDTH, SFX_BSPACT, SFX_SGTATK];
                    let sounds = [SFX_PLDETH, SFX_DMPAIN, SFX_POPAIN, SFX_SLOP, SFX_TELEPT, SFX_POSIT1, SFX_POSIT3, SFX_SGTATK];
                    let i = ((gametic >> 2) & 7) as usize;
                    out.push(MenuAction::StartSound(if self.game.mode == GameMode::Commercial { sounds2[i] } else { sounds[i] }));
                    out.push(MenuAction::Quit);
                }
                MessageKind::EndGame if yes => {
                    out.push(MenuAction::EndGame);
                }
                _ => {}
            }
            // Vanilla closes the menu after any prompt.
            self.active = false;
            out.push(MenuAction::StartSound(SFX_SWTCHX));
            return true;
        }

        if !self.active {
            match key {
                Key::Escape => {
                    self.open();
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    return true;
                }
                Key::F(1) => {
                    self.open();
                    let ultimate = self.game.version >= GameVersion::Ultimate;
                    self.current = if self.game.mode == GameMode::Commercial || !ultimate { MenuId::Read1 } else { MenuId::Read2 };
                    self.item_on = 0;
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    return true;
                }
                Key::F(4) => {
                    self.open();
                    self.setup_next(MenuId::Sound);
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    return true;
                }
                Key::F(5) => {
                    self.change_detail(s, out);
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    return true;
                }
                Key::F(7) => {
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    self.end_game(in_game, out);
                    return true;
                }
                Key::F(8) => {
                    self.change_messages(s, out);
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    return true;
                }
                Key::F(10) => {
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    self.quit(gametic);
                    return true;
                }
                Key::F(11) => {
                    // Gamma toggle (no sound in vanilla).
                    s.gamma = (s.gamma + 1) % 5;
                    out.push(MenuAction::Message(strings::GAMMA_MESSAGES[s.gamma].to_string()));
                    return true;
                }
                Key::F(2) | Key::F(3) | Key::F(6) | Key::F(9) => {
                    self.start_message("Saving and loading are not implemented yet.\n\n".to_string() + strings::PRESSKEY, MessageKind::Info);
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                    return true;
                }
                _ => return false,
            }
        }

        let n = self.menu(self.current).items.len();
        match key {
            Key::Down => {
                loop {
                    self.item_on = if self.item_on + 1 > n - 1 { 0 } else { self.item_on + 1 };
                    if self.menu(self.current).items[self.item_on].status != -1 {
                        break;
                    }
                }
                out.push(MenuAction::StartSound(SFX_PSTOP));
            }
            Key::Up => {
                loop {
                    self.item_on = if self.item_on == 0 { n - 1 } else { self.item_on - 1 };
                    if self.menu(self.current).items[self.item_on].status != -1 {
                        break;
                    }
                }
                out.push(MenuAction::StartSound(SFX_PSTOP));
            }
            Key::Left | Key::Right => {
                let item = self.menu(self.current).items[self.item_on].clone();
                if item.status == 2 {
                    if let Some(act) = item.act {
                        out.push(MenuAction::StartSound(SFX_STNMOV));
                        self.activate(act, (key == Key::Right) as i32, s, cvars, gametic, in_game, out);
                    }
                }
            }
            Key::Enter => {
                let item = self.menu(self.current).items[self.item_on].clone();
                if let (Some(act), true) = (item.act, item.status != 0) {
                    let on = self.item_on;
                    self.menu_mut(self.current).last_on = on;
                    if item.status == 2 {
                        self.activate(act, 1, s, cvars, gametic, in_game, out);
                        out.push(MenuAction::StartSound(SFX_STNMOV));
                    } else {
                        self.activate(act, on as i32, s, cvars, gametic, in_game, out);
                        out.push(MenuAction::StartSound(SFX_PISTOL));
                    }
                }
            }
            Key::Escape => {
                let on = self.item_on;
                self.menu_mut(self.current).last_on = on;
                self.close();
                out.push(MenuAction::StartSound(SFX_SWTCHX));
            }
            Key::Backspace => {
                let on = self.item_on;
                self.menu_mut(self.current).last_on = on;
                if let Some(prev) = self.menu(self.current).prev {
                    self.setup_next(prev);
                    out.push(MenuAction::StartSound(SFX_SWTCHN));
                }
            }
            Key::Char(c) => {
                // Jump to the next item with this hotkey.
                let c = c.to_ascii_lowercase() as u8;
                let items = &self.menu(self.current).items;
                let order = (self.item_on + 1..n).chain(0..=self.item_on);
                if let Some(i) = order.into_iter().find(|&i| items[i].alpha == c) {
                    self.item_on = i;
                    out.push(MenuAction::StartSound(SFX_PSTOP));
                }
            }
            _ => return false,
        }
        true
    }

    fn change_detail(&mut self, s: &mut Settings, out: &mut Vec<MenuAction>) {
        s.low_detail = !s.low_detail;
        out.push(MenuAction::ViewChanged);
        out.push(MenuAction::Message(if s.low_detail { strings::DETAILLO } else { strings::DETAILHI }.to_string()));
    }

    fn change_messages(&mut self, s: &mut Settings, out: &mut Vec<MenuAction>) {
        s.show_messages = !s.show_messages;
        // The "off" message is shown even though messages are now off.
        out.push(MenuAction::Message(if s.show_messages { strings::MSGON } else { strings::MSGOFF }.to_string()));
    }

    fn quit(&mut self, gametic: i32) {
        let table = if self.game.mission == GameMission::Doom { &strings::DOOM1_ENDMSG } else { &strings::DOOM2_ENDMSG };
        let msg = table[(gametic.rem_euclid(8)) as usize];
        self.start_message(format!("{msg}\n\n{}", strings::DOSY), MessageKind::Quit);
    }

    fn end_game(&mut self, in_game: bool, _out: &mut Vec<MenuAction>) {
        if !in_game {
            // `M_EndGame` does nothing outside a game.
            return;
        }
        self.start_message(strings::ENDGAME.to_string(), MessageKind::EndGame);
    }

    #[allow(clippy::too_many_arguments)]
    fn activate(
        &mut self,
        act: ItemAct,
        choice: i32,
        s: &mut Settings,
        cvars: &CVarRegistry,
        gametic: i32,
        in_game: bool,
        out: &mut Vec<MenuAction>,
    ) {
        match act {
            ItemAct::NewGameMenu => {
                if self.game.mode == GameMode::Commercial || self.game.version == GameVersion::Chex {
                    self.setup_next(MenuId::NewGame);
                } else {
                    self.setup_next(MenuId::Episode);
                }
            }
            ItemAct::Options => self.setup_next(MenuId::Options),
            ItemAct::LoadGame => self.setup_next(MenuId::Load),
            ItemAct::SaveGame => {
                if !in_game {
                    self.start_message(strings::SAVEDEAD.to_string(), MessageKind::Info);
                } else {
                    self.setup_next(MenuId::Save);
                }
            }
            ItemAct::Slot(_) => {
                self.start_message("Saving and loading are not implemented yet.\n\n".to_string() + strings::PRESSKEY, MessageKind::Info);
            }
            ItemAct::ReadThis => self.setup_next(MenuId::Read1),
            ItemAct::ReadThis2 => self.setup_next(MenuId::Read2),
            ItemAct::FinishReadThis => self.setup_next(MenuId::Main),
            ItemAct::Quit => self.quit(gametic),
            ItemAct::Episode(e) => {
                if self.game.mode == GameMode::Shareware && e != 0 {
                    self.start_message(strings::SWSTRING.to_string(), MessageKind::Info);
                    self.setup_next(MenuId::Read1);
                    return;
                }
                self.epi = e;
                self.setup_next(MenuId::NewGame);
            }
            ItemAct::Skill(skill) => {
                if skill == Skill::Nightmare {
                    self.start_message(strings::NIGHTMARE.to_string(), MessageKind::Nightmare);
                    return;
                }
                out.push(MenuAction::NewGame { episode: self.epi + 1, skill });
                self.close();
            }
            ItemAct::EndGame => self.end_game(in_game, out),
            ItemAct::Messages => self.change_messages(s, out),
            ItemAct::Detail => self.change_detail(s, out),
            ItemAct::ScreenSize => {
                if choice == 0 && s.screenblocks > 3 {
                    s.screenblocks -= 1;
                } else if choice == 1 && s.screenblocks < 11 {
                    s.screenblocks += 1;
                }
                out.push(MenuAction::ViewChanged);
            }
            ItemAct::MouseSens => {
                if choice == 0 && s.mouse_sensitivity > 0 {
                    s.mouse_sensitivity -= 1;
                } else if choice == 1 && s.mouse_sensitivity < 9 {
                    s.mouse_sensitivity += 1;
                }
            }
            ItemAct::SoundMenu => self.setup_next(MenuId::Sound),
            ItemAct::EngineOptions => {
                self.build_engine_page(cvars);
                if self.engine_vars.is_empty() {
                    self.start_message(format!("No engine options.

{}", strings::PRESSKEY), MessageKind::Info);
                } else {
                    self.setup_next(MenuId::Engine);
                }
            }
            ItemAct::CVar(i) => {
                if let Some(name) = self.engine_vars.get(i) {
                    // Registered names, stepping never fails.
                    let _ = cvars.step(name, if choice == 0 { -1 } else { 1 });
                }
            }
            ItemAct::SfxVol => {
                if choice == 0 && s.sfx_volume > 0 {
                    s.sfx_volume -= 1;
                } else if choice == 1 && s.sfx_volume < 15 {
                    s.sfx_volume += 1;
                }
                out.push(MenuAction::VolumeChanged);
            }
            ItemAct::MusicVol => {
                if choice == 0 && s.music_volume > 0 {
                    s.music_volume -= 1;
                } else if choice == 1 && s.music_volume < 15 {
                    s.music_volume += 1;
                }
                out.push(MenuAction::VolumeChanged);
            }
        }
    }

    /// One slider-style item per variable with a menu label. Rebuilt on
    /// entry, so variables registered later (host, mods) appear too.
    fn build_engine_page(&mut self, cvars: &CVarRegistry) {
        let rows = ((SCREENHEIGHT as i32 - 36) / LINEHEIGHT) as usize;
        let vars: Vec<_> = cvars.list().into_iter().filter_map(|v| Some((v.name, v.menu?))).take(rows).collect();
        self.engine_vars = vars.iter().map(|(name, _)| *name).collect();
        let items = vars
            .iter()
            .enumerate()
            .map(|(i, (_, label))| Item { status: 2, patch: None, label, act: Some(ItemAct::CVar(i)), alpha: 0 })
            .collect();
        let m = self.menu_mut(MenuId::Engine);
        m.items = items;
        m.last_on = m.last_on.min(m.items.len().saturating_sub(1));
    }

    fn draw_thermo(&self, screen: &mut [u8], x: i32, y: i32, width: i32, dot: i32) {
        let t = &self.p.therm;
        let mut xx = x;
        draw_patch(screen, xx, y, &t[0]);
        xx += 8;
        for _ in 0..width {
            draw_patch(screen, xx, y, &t[1]);
            xx += 8;
        }
        draw_patch(screen, xx, y, &t[2]);
        draw_patch(screen, (x + 8) + dot * 8, y, &t[3]);
    }

    /// `M_Drawer`. `paused` draws the PAUSE graphic (D_Display).
    pub fn draw(&self, screen: &mut [u8], font: &Font, s: &Settings, cvars: &CVarRegistry) {
        if let Some((text, _)) = &self.message {
            let lines: Vec<&str> = text.split('\n').collect();
            let mut y = SCREENHEIGHT as i32 / 2 - (lines.len() as i32 * font.height()) / 2;
            for line in lines {
                let x = SCREENWIDTH as i32 / 2 - font.width(line) / 2;
                font.draw(screen, x, y, line);
                y += font.height();
            }
            return;
        }
        if !self.active {
            return;
        }
        let m = self.menu(self.current);
        let p = &self.p;
        match self.current {
            MenuId::Main => draw_patch(screen, 94, 2, &p.doom),
            MenuId::NewGame => {
                draw_patch(screen, 96, 14, &p.newg);
                draw_patch(screen, 54, 38, &p.skill);
            }
            MenuId::Episode => draw_patch(screen, 54, 38, &p.episod),
            MenuId::Options => {
                draw_patch(screen, 108, 15, &p.optttl);
                draw_patch(screen, m.x + 175, m.y + LINEHEIGHT * 2, &p.detail[s.low_detail as usize]);
                draw_patch(screen, m.x + 120, m.y + LINEHEIGHT, &p.msg[s.show_messages as usize]);
                self.draw_thermo(screen, m.x, m.y + LINEHEIGHT * 6, 10, s.mouse_sensitivity);
                self.draw_thermo(screen, m.x, m.y + LINEHEIGHT * 4, 9, s.screenblocks - 3);
            }
            MenuId::Sound => {
                draw_patch(screen, 60, 38, &p.svol);
                self.draw_thermo(screen, m.x, m.y + LINEHEIGHT, 16, s.sfx_volume);
                self.draw_thermo(screen, m.x, m.y + LINEHEIGHT * 3, 16, s.music_volume);
            }
            MenuId::Read1 => {
                let pic = if self.game.mode == GameMode::Commercial { &p.help } else { &p.help2 };
                if let Some(pic) = pic.as_ref().or(p.credit.as_ref()) {
                    draw_patch(screen, 0, 0, pic);
                }
            }
            MenuId::Read2 => {
                if let Some(pic) = p.help1.as_ref().or(p.help.as_ref()) {
                    draw_patch(screen, 0, 0, pic);
                }
            }
            MenuId::Engine => {
                font.draw_centered(screen, 18, "ENGINE OPTIONS");
                for (i, name) in self.engine_vars.iter().enumerate() {
                    let value = cvars.value(name).map_or_else(String::new, |v| v.to_string());
                    let y = m.y + LINEHEIGHT * i as i32 + 4;
                    font.draw(screen, SCREENWIDTH as i32 - 40 - font.width(&value), y, &value);
                }
            }
            MenuId::Load | MenuId::Save => {
                draw_patch(screen, 72, 28, if self.current == MenuId::Load { &p.loadg } else { &p.saveg });
                for i in 0..6 {
                    let y = m.y + LINEHEIGHT * i;
                    // M_DrawSaveLoadBorder
                    draw_patch(screen, m.x - 8, y + 7, &p.lsl);
                    for j in 0..24 {
                        draw_patch(screen, m.x + j * 8, y + 7, &p.lsc);
                    }
                    draw_patch(screen, m.x + 24 * 8, y + 7, &p.lsr);
                    font.draw(screen, m.x, y, strings::EMPTYSTRING);
                }
            }
        }
        let mut y = m.y;
        for item in &m.items {
            if let Some(pic) = &item.patch {
                draw_patch(screen, m.x, y, pic);
            } else if !item.label.is_empty() {
                font.draw(screen, m.x, y + 4, item.label);
            }
            y += LINEHEIGHT;
        }
        // Read This screens have no visible cursor (it is off-screen).
        draw_patch(screen, m.x + SKULLXOFF, m.y - 5 + self.item_on as i32 * LINEHEIGHT, &self.skulls[self.which_skull]);
    }

    /// The PAUSE graphic, drawn by D_Display when the game is paused.
    pub fn draw_pause(&self, screen: &mut [u8], view_top: i32) {
        let x = (SCREENWIDTH as i32 - self.p.pause.width) / 2;
        draw_patch(screen, x, view_top + 4, &self.p.pause);
    }
}
