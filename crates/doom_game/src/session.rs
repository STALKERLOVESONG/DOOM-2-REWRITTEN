//! [`Session`]: the running game front end. Owns the loaded content, the
//! simulation [`World`], the renderer and the presentation layer, and runs
//! the d_main.c / g_game.c flow around the playsim: title pages and demo
//! loop, levels, intermission, finale, menus, automap, status bar and sound.
//!
//! Hosts drive it with [`Session::tick`] at 35 Hz, feed key presses with
//! [`Session::key_down`]/[`Session::key_up`], present
//! [`Session::render_rgba`], and play [`Session::take_audio_commands`]
//! through a `doom_audio::Mixer`.
//!
//! Settings live in [`Session::cvars`]; the session re-reads them whenever
//! the registry's generation moves, so hosts and the menu change behaviour
//! by setting variables.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use doom_audio::{AudioCommand, Genmidi};
use doom_content::game::{GameInfo, GameMode, GameVersion};
use doom_content::picture::Picture;
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_render::{Camera, DebugScene, Limits, PerspectiveRenderer, RenderData, Renderer, ViewRect, SCREENHEIGHT, SCREENWIDTH};
use doom_sim::content::ContentInfo;
use doom_sim::demo::Demo;
use doom_sim::info::{Info, SfxNum};
use doom_sim::player::{cheat, power, PlayerState, Weapon, MAXPLAYERS, NUMWEAPONS};
use doom_sim::ticcmd::{bt, InputState, TicCmd, TicCmdBuilder};
use doom_sim::world::{GameExit, SimEvent};
use doom_sim::{GameAction, GameParams, Level, Skill, World};

use crate::cheats::{Cheat, CheatMatcher};
use crate::console::{self as con, CommandRegistry, Console, ConsoleAction, LineKind};
use crate::cvar::{self, CVarRegistry};
use crate::maps::{next_map, MapRef, Next};
use crate::sound::SoundSystem;
use crate::ui::automap::Automap;
use crate::ui::finale::{Finale, FinaleEvent};
use crate::ui::gfx::{draw_patch, fill_rect, Font, Patches};
use crate::ui::hud::{map_title, Hud as HudText};
use crate::ui::intermission::{Intermission, WbStart, WiEvent};
use crate::ui::menu::{MenuAction, MenuSystem, Settings};
use crate::ui::status_bar::{StatusBar, ST_Y};
use crate::ui::{Key, ViewBorder};
use crate::wipe::Wipe;

mod commands;
mod skate_view;

/// Tics per second.
pub const TICRATE: u32 = 35;

/// The debug switches (`show_*`, `r_wireframe`, `skate_debug`).
#[derive(Clone, Copy, Debug, Default)]
struct DebugFlags {
    show_player: bool,
    show_collision: bool,
    show_velocity: bool,
    show_grindables: bool,
    show_skate_state: bool,
    skate_debug: bool,
    wireframe: bool,
}

impl DebugFlags {
    /// Anything that draws into the 3D view.
    fn draws_3d(self) -> bool {
        self.show_collision || self.show_velocity || self.show_grindables || self.skate_debug || self.wireframe
    }
}

/// `r_render_mode`: how the game is presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// 320x200 palettized Doom, scaled to 4:3. The 3D view is drawn by the
    /// vanilla column renderer, or with `cl_freelook` by the palettized
    /// perspective renderer (true camera pitch, same look).
    #[default]
    Classic,
    /// The host renders the world as truecolor 3D geometry at the display
    /// resolution; the session supplies the camera and the screen-space
    /// layers (status bar, HUD, menus) as an overlay.
    GZDoomClean,
}

impl RenderMode {
    /// The `r_render_mode` value.
    pub fn as_str(self) -> &'static str {
        match self {
            RenderMode::Classic => "Classic",
            RenderMode::GZDoomClean => "GZDoomClean",
        }
    }

    pub fn parse(s: &str) -> Option<RenderMode> {
        match s.to_ascii_lowercase().as_str() {
            "classic" => Some(RenderMode::Classic),
            "gzdoomclean" => Some(RenderMode::GZDoomClean),
            _ => None,
        }
    }
}

/// What draws the 3D view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewPath {
    /// The vanilla column/span renderer: fixed horizon.
    Columns,
    /// The palettized polygon renderer: true camera pitch at 320x200.
    Perspective,
    /// The host's truecolor renderer (see [`Session::host_view`]).
    Host,
}

/// Everything a host renderer needs to draw the 3D view.
#[derive(Clone, Copy, Debug)]
pub struct HostView {
    /// The player whose eyes the camera is.
    pub player: usize,
    pub camera: Camera,
    /// The view window in 320x200 screen coordinates (the rest is status
    /// bar and border, drawn by the overlay).
    pub rect: ViewRect,
}

/// Freelook limit (radians). GZDoom's default range is similar; the
/// projection itself stays valid a little further (see
/// [`Camera::pitch_limit`]).
pub const MAX_PITCH: f32 = 50.0 * std::f32::consts::PI / 180.0;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub iwad: PathBuf,
    pub pwads: Vec<PathBuf>,
    /// Starting map (`E1M1`, `MAP01`, `-warp` forms). `None` (and no demo)
    /// starts at the title screen with the demo loop, like vanilla.
    pub map: Option<String>,
    pub skill: Skill,
    pub nomonsters: bool,
    pub fast: bool,
    pub respawn: bool,
    /// Play this demo (lump name or file path) instead of taking input.
    pub demo: Option<String>,
    pub render_mode: RenderMode,
    /// The engine's resource folder (skateboard model). `None`: look for
    /// `RESOURCES` near the working directory and the executable.
    pub resources: Option<PathBuf>,
}

impl SessionConfig {
    pub fn new(iwad: impl Into<PathBuf>) -> Self {
        SessionConfig {
            iwad: iwad.into(),
            pwads: Vec::new(),
            map: None,
            skill: Skill::Medium,
            nomonsters: false,
            fast: false,
            respawn: false,
            demo: None,
            render_mode: RenderMode::default(),
            resources: None,
        }
    }
}

/// Snapshot of the player's status, for tools and tests.
#[derive(Clone, Debug, Default)]
pub struct Hud {
    pub map: String,
    pub health: i32,
    pub armor: i32,
    pub weapon: Weapon,
    /// (current, max) for the ready weapon's ammo type; `None` for fist/chainsaw.
    pub ammo: Option<(i32, i32)>,
    pub weapons_owned: [bool; NUMWEAPONS],
    /// Blue, yellow, red cards; blue, yellow, red skulls.
    pub keys: [bool; 6],
    pub kills: (i32, i32),
    pub items: (i32, i32),
    pub secrets: (i32, i32),
    pub dead: bool,
    pub god: bool,
    pub noclip: bool,
    pub message: Option<String>,
    pub level_seconds: u32,
}

/// `gamestate_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameState {
    Level,
    Intermission,
    Finale,
    /// A title/credit page of the demo loop.
    DemoScreen,
}

struct DemoPlayback {
    demo: Demo,
    pos: usize,
    /// Part of the title loop (vs. `--playdemo`).
    attract: bool,
}

pub struct Session {
    /// Shared with the renderers, which read graphics from it lazily.
    pub stack: Arc<ResourceStack>,
    pub textures: TextureSet,
    pub game: GameInfo,
    pub render_data: RenderData,
    /// Actor tables (Doom's plus DECORATE), copied into each new game.
    pub info: Info,
    /// MAPINFO/ZMAPINFO/UMAPINFO map definitions.
    pub mapinfo: doom_content::mapinfo::MapInfo,
    pub world: World,
    pub renderer: Renderer,
    /// The free-camera renderer ([`ViewPath::Perspective`]).
    pub perspective: PerspectiveRenderer,
    /// Mirrors `r_render_mode`; change it with [`Session::set_render_mode`].
    pub render_mode: RenderMode,
    /// Mirrors the vanilla settings variables; the menu edits it directly
    /// and the session copies changes back to `cvars`.
    pub settings: Settings,
    /// Console variables (shared with the host).
    pub cvars: Arc<CVarRegistry>,
    /// `cvars.generation()` when last applied.
    cvar_generation: u64,
    /// `settings` as last synced with `cvars` (to see which side changed).
    synced_settings: Settings,
    /// `cl_freelook`.
    freelook: bool,
    /// `sv_autoaim`.
    autoaim: bool,
    /// `r_wipe`.
    wipe_enabled: bool,
    /// The melt in progress; game tics are suspended while it runs.
    wipe: Option<Wipe>,
    /// `wipegamestate`: the state of the last displayed frame (`None` until
    /// the first frame is shown, so a session never melts from black).
    wipe_state: Option<GameState>,
    /// A level was (re)loaded: melt even if the state stays `Level`.
    force_wipe: bool,
    /// Reused buffer for [`Session::render_overlay_rgba`].
    overlay_scratch: Vec<u8>,
    /// Where a script sent the level exit (`Teleport_NewMap`, `ChangeLevel`).
    scripted_exit: Option<doom_sim::world::ExitTarget>,
    /// View pitch in radians (> 0 looks up). Presentation only: the playsim
    /// still aims vertically with vanilla autoaim.
    look_pitch: f32,
    state: GameState,
    current: MapRef,
    base_params: GameParams,
    screen: Vec<u8>,
    builder: TicCmdBuilder,
    demo: Option<DemoPlayback>,
    /// `usergame`: a game the player started is in progress.
    usergame: bool,
    paused: bool,
    secret_exit: bool,
    demosequence: i32,
    pagetic: i32,
    page: Option<Arc<Picture>>,
    quit_countdown: Option<i32>,
    quit: bool,
    // Presentation.
    patches: Patches,
    font: Font,
    status_bar: StatusBar,
    hud_text: HudText,
    menu: MenuSystem,
    automap: Automap,
    border: ViewBorder,
    intermission: Option<Intermission>,
    finale: Option<Finale>,
    sound: SoundSystem,
    cheats: CheatMatcher,
    events: Vec<SimEvent>,
    /// Content warnings collected while loading (PWAD problems etc.).
    pub warnings: Vec<String>,
    // Developer console.
    console: Console,
    /// Console commands; add your own with [`Session::commands_mut`].
    commands: CommandRegistry,
    con_palette: con::Palette,
    // Player controller (`player_controller`) and skateboarding.
    skate_mode: bool,
    skate_params: doom_sim::skate::SkateParams,
    /// Grind graphs by map and settings, so reloading a level (death,
    /// restart) does not analyse it again.
    grind_cache: HashMap<String, Arc<doom_sim::grind::GrindGraph>>,
    chase: skate_view::ChaseCam,
    model_colors: skate_view::ModelColors,
    /// The skateboard model (`RESOURCES/skateboard.model`); `None` falls
    /// back to a plain box board.
    board_model: Option<Arc<crate::skate_model::BoardModel>>,
    /// The skater's pose before the last game tic, for drawing between
    /// tics.
    skate_prev: Option<doom_sim::skate::Skater>,
    /// The host reports frame timing ([`Session::set_frame_time`]): draw
    /// interpolated and move the camera per frame. Otherwise per tic.
    frame_timed: bool,
    /// Where this frame is between the last two tics (0..=1).
    frame_alpha: f32,
    /// The last trick reported on the HUD.
    last_feat: Option<doom_sim::skate::Feat>,
    // Debugging.
    debug: DebugFlags,
    /// `timescale`, and the fraction of a tic carried between host tics.
    timescale: f32,
    time_carry: f32,
    /// Extra geometry for the 3D view (model, debug lines), rebuilt each
    /// frame with its allocations kept.
    scene: DebugScene,
    /// A weapon slot chosen from the console, sent with the next tic.
    pending_weapon_slot: Option<u8>,
}

impl Session {
    pub fn new(cfg: &SessionConfig) -> Result<Session, String> {
        let stack = Arc::new(ResourceStack::load(&cfg.iwad, &cfg.pwads).map_err(|e| format!("loading WADs: {e}"))?);
        let textures = TextureSet::load(&stack).map_err(|e| format!("textures: {e}"))?;
        let game = doom_content::game::identify(&stack, None).ok_or("not a Doom IWAD (no map or game lumps found)")?;
        // Actor tables: Doom's, extended by DECORATE.
        let mapinfo = doom_content::mapinfo::MapInfo::load(&stack);
        let mut info = Info::doom();
        let decorate_warnings = doom_sim::decorate::Decorate::apply_stack(&mut info, &stack);
        // DEHACKED after DECORATE, as in GZDoom: patches edit Doom's things
        // and frames by number. Sprite renames must precede RenderData.
        let mut deh_warnings = doom_sim::dehacked::load(&mut info, &stack);
        let unknown = info.deh.resolve_bex_strings(&crate::ui::strings::bex_original);
        if !unknown.is_empty() {
            deh_warnings.push(format!("DEHACKED: {} unknown BEX strings ignored: {}", unknown.len(), unknown.join(", ")));
        }
        let render_data = RenderData::build(&stack, &textures, &info.sprnames).map_err(|e| format!("graphics: {e}"))?;
        let mut warnings = stack.warnings.clone();
        warnings.extend(textures.warnings.iter().cloned());
        warnings.extend(decorate_warnings);
        warnings.extend(deh_warnings);
        warnings.extend(mapinfo.warnings.iter().cloned());
        warnings.extend(render_data.warnings.iter().cloned());

        let start = match &cfg.map {
            Some(m) => MapRef::parse(m, game.mode).ok_or_else(|| {
                let doom2 = game.mode == GameMode::Commercial;
                if m.trim().to_ascii_uppercase().starts_with("MAP") && !doom2 {
                    format!("{m} is a Doom II map name, but the IWAD is Doom 1; use ExMy names or a Doom II IWAD (doom2.wad)")
                } else if doom2 && m.trim().to_ascii_uppercase().starts_with('E') {
                    format!("{m} is a Doom 1 map name, but the IWAD is Doom II; use MAPxx names or a Doom 1 IWAD")
                } else {
                    format!("bad map name {m}")
                }
            })?,
            None => first_map(&stack, game.mode).ok_or("the WADs contain no playable map")?,
        };
        if !start.exists(&stack, game.mode) {
            return Err(format!("map {} not found", start.lump_name(game.mode)));
        }
        let base_params = GameParams {
            mode: game.mode,
            version: game.version,
            skill: cfg.skill,
            episode: start.episode,
            map: start.map,
            deathmatch: 0,
            netgame: false,
            respawnmonsters: cfg.respawn,
            nomonsters: cfg.nomonsters,
            fastparm: cfg.fast,
            consoleplayer: 0,
        };
        let level = Level::load(&stack, &textures, &start.lump_name(game.mode)).map_err(|e| e.to_string())?;
        warnings.extend(level.warnings.iter().cloned());
        let content = ContentInfo::from_content(&stack, &textures);
        let world = World::new_game(info.clone(), content, base_params, single_player(), level)?;
        warnings.extend(stack.take_late_warnings());

        let mut patches = Patches::default();
        let font = Font::load(&stack, &mut patches);
        let status_bar = StatusBar::new(&stack, &mut patches);
        let cheats = CheatMatcher::with_overrides(&info.deh.cheats);
        let mut menu = MenuSystem::new(&stack, &mut patches, game);
        menu.set_strings(info.deh.strings.clone());
        let automap = Automap::new(&stack, &mut patches, SCREENWIDTH as i32, ST_Y);
        let border = ViewBorder::new(&stack, &mut patches, game.mode);
        let mut sound = SoundSystem::new(&stack);
        sound.set_music_renames(info.deh.strings.clone());
        let settings = Settings::default();
        let playpal = *render_data.palettes.get(0);
        let con_palette = con::Palette::from_playpal(&playpal);
        let model_colors = skate_view::ModelColors::from_playpal(&playpal);
        // The skateboard model: optional (a box board stands in), loaded
        // once.
        let resources = cfg.resources.clone().or_else(crate::skate_model::find_resources);
        let board_model = match resources.as_ref().map(|r| r.join("skateboard.model")) {
            Some(path) if path.exists() => match crate::skate_model::load(&path, &playpal) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    warnings.push(format!("skateboard model: {e}"));
                    None
                }
            },
            _ => None,
        };
        let mut command_registry = CommandRegistry::default();
        commands::register(&mut command_registry);
        let cvars = Arc::new(CVarRegistry::with_engine_vars());
        cvars.set(cvar::R_RENDER_MODE, cfg.render_mode.as_str().to_string()).map_err(|e| e.to_string())?;

        let mut s = Session {
            stack,
            textures,
            game,
            render_data,
            info,
            mapinfo,
            world,
            // Limit-removing: mod maps overflow vanilla's drawseg/visplane/sprite tables.
            renderer: Renderer::new(settings.screenblocks, settings.low_detail, Limits::UNLIMITED),
            perspective: PerspectiveRenderer::new(),
            render_mode: cfg.render_mode,
            settings,
            cvars,
            // Forces the first sync.
            cvar_generation: u64::MAX,
            synced_settings: settings,
            freelook: false,
            autoaim: true,
            wipe_enabled: true,
            wipe: None,
            wipe_state: None,
            force_wipe: false,
            overlay_scratch: Vec::new(),
            scripted_exit: None,
            look_pitch: 0.0,
            state: GameState::Level,
            current: start,
            base_params,
            screen: vec![0; SCREENWIDTH * SCREENHEIGHT],
            builder: TicCmdBuilder::default(),
            demo: None,
            usergame: false,
            paused: false,
            secret_exit: false,
            demosequence: -1,
            pagetic: 0,
            page: None,
            quit_countdown: None,
            quit: false,
            patches,
            font,
            status_bar,
            hud_text: HudText::default(),
            menu,
            automap,
            border,
            intermission: None,
            finale: None,
            sound,
            cheats,
            events: Vec::new(),
            warnings,
            console: Console::default(),
            commands: command_registry,
            con_palette,
            skate_mode: false,
            skate_params: Default::default(),
            grind_cache: HashMap::new(),
            chase: Default::default(),
            model_colors,
            board_model,
            skate_prev: None,
            frame_timed: false,
            frame_alpha: 1.0,
            last_feat: None,
            debug: DebugFlags::default(),
            timescale: 1.0,
            time_carry: 0.0,
            scene: DebugScene::default(),
            pending_weapon_slot: None,
        };
        s.console.print(LineKind::Info, &format!("doom_bevy console: {:?} / {:?}. Type help for commands.", s.game.mode, s.game.version));
        let shown: Vec<String> = s.warnings.iter().take(40).cloned().collect();
        for w in &shown {
            s.console.print(LineKind::Info, &format!("warning: {w}"));
        }
        s.store_settings();
        s.sound.set_volumes(s.settings.sfx_volume, s.settings.music_volume);

        if let Some(d) = &cfg.demo {
            let data = match s.stack.lump(&d.to_ascii_uppercase()) {
                Some(l) => l.to_vec(),
                None => std::fs::read(d).map_err(|e| format!("demo {d}: {e}"))?,
            };
            let demo = Demo::parse(&data)?;
            s.play_demo(demo, false)?;
        } else if cfg.map.is_some() {
            s.usergame = true;
            s.start_level();
        } else {
            s.start_title();
        }
        Ok(s)
    }

    // ---- queries ------------------------------------------------------------

    pub fn map(&self) -> MapRef {
        self.current
    }

    pub fn map_name(&self) -> String {
        self.current.lump_name(self.game.mode)
    }

    pub fn state(&self) -> GameState {
        self.state
    }

    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    /// True once a `--playdemo` demo has played all its tics.
    pub fn demo_finished(&self) -> bool {
        match &self.demo {
            Some(d) => d.pos >= d.demo.tics.len(),
            None => !self.usergame && self.state != GameState::Level,
        }
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_open()
    }

    pub fn automap_active(&self) -> bool {
        self.automap.active
    }

    /// The host should stop: the player confirmed Quit.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// Mouse capture makes sense: a level is being played by the user.
    pub fn wants_mouse(&self) -> bool {
        self.state == GameState::Level && self.demo.is_none() && !self.menu.is_open() && !self.paused && !self.console.is_open()
    }

    /// The developer console is down (it takes all keys and text).
    pub fn console_open(&self) -> bool {
        self.console.is_open()
    }

    /// Keys should auto-repeat (menu or console navigation, typing).
    pub fn wants_key_repeat(&self) -> bool {
        self.menu.is_open() || self.console.is_open()
    }

    /// Open or close the console.
    pub fn set_console_open(&mut self, open: bool) {
        self.console.set_open(open);
    }

    /// The console's text state (scrollback, input line, history).
    pub fn console(&self) -> &Console {
        &self.console
    }

    /// Console commands, to register more.
    pub fn commands_mut(&mut self) -> &mut CommandRegistry {
        &mut self.commands
    }

    /// Print to the console.
    pub fn console_print(&mut self, text: &str) {
        self.console.print(LineKind::Output, text);
    }

    /// Skateboard mode is selected (`player_controller Skate`).
    pub fn controller_is_skate(&self) -> bool {
        self.skate_mode
    }

    /// The console player's skateboard state, when riding.
    pub fn skater(&self) -> Option<&doom_sim::skate::Skater> {
        self.world.console_player().skater.as_deref()
    }

    /// Frame timing from the host, once per displayed frame before
    /// rendering: `alpha` is how far this frame is from the last game tic
    /// toward the next (0..=1, e.g. Bevy's fixed-step overstep fraction),
    /// `dt` the frame's duration in seconds. The game still runs at 35 Hz;
    /// the skate camera, board and rider are then drawn between tics and
    /// the camera moves smoothly at the display rate.
    pub fn set_frame_time(&mut self, alpha: f32, dt: f32) {
        self.frame_timed = true;
        self.frame_alpha = alpha.clamp(0.0, 1.0);
        if self.state == GameState::Level && !self.paused {
            self.update_chase(dt.clamp(0.0, 0.25));
        }
    }

    /// The extra 3D geometry of the last frame (skateboard model, Doomguy
    /// billboard, debug lines), for a host renderer that draws the level
    /// itself ([`ViewPath::Host`]). Palette indices; the frame's palette is
    /// [`Session::display_palette`]. [`Session::render_overlay_rgba`]
    /// already includes it.
    pub fn scene(&self) -> &DebugScene {
        &self.scene
    }

    /// The skateboard model in use (`None`: the box fallback).
    pub fn board_model(&self) -> Option<&crate::skate_model::BoardModel> {
        self.board_model.as_deref()
    }

    /// Arrow keys pan the automap instead of moving the player.
    pub fn captures_arrows(&self) -> bool {
        self.automap.active && !self.automap.follow()
    }

    /// The GENMIDI instrument bank for the host's mixer.
    pub fn genmidi(&self) -> Option<Arc<Genmidi>> {
        self.sound.genmidi.clone()
    }

    /// Mixer commands produced since the last call.
    pub fn take_audio_commands(&mut self) -> Vec<AudioCommand> {
        self.sound.take_commands()
    }

    /// Simulation events (sounds) seen since the last call.
    pub fn take_events(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.events)
    }

    /// Show a HUD message for the standard 4 seconds.
    /// Show a HUD message, after DEHACKED's replacements for vanilla's
    /// texts (`STSTR_DQDON`...).
    pub fn show_message(&mut self, m: impl Into<String>) {
        let m = m.into();
        self.hud_text.show(self.info.deh.strings.get(&m));
    }

    /// View size as in the vanilla menu: 3..=11 (10 = full width above the
    /// status bar, 11 = full screen).
    pub fn set_view(&mut self, screenblocks: i32, low_detail: bool) {
        self.settings.screenblocks = screenblocks.clamp(3, 11);
        self.settings.low_detail = low_detail;
        self.apply_view();
        self.store_settings();
    }

    fn apply_view(&mut self) {
        self.renderer.set_view_size(self.settings.screenblocks, self.settings.low_detail);
        self.look_pitch = self.look_pitch.clamp(-self.max_pitch(), self.max_pitch());
    }

    // ---- settings (cvars) ---------------------------------------------------

    /// Copy settings the menu or a key changed into the cvars, then apply.
    fn store_settings(&mut self) {
        let (s, old, c) = (self.settings, self.synced_settings, &self.cvars);
        // Only fields that changed here are written, so a cvar changed
        // elsewhere since the last sync is not overwritten with a stale value.
        // The menu keeps values in range; the registry clamps regardless.
        if s.sfx_volume != old.sfx_volume {
            let _ = c.set(cvar::SND_SFXVOLUME, s.sfx_volume);
        }
        if s.music_volume != old.music_volume {
            let _ = c.set(cvar::SND_MUSICVOLUME, s.music_volume);
        }
        if s.mouse_sensitivity != old.mouse_sensitivity {
            let _ = c.set(cvar::M_SENSITIVITY, s.mouse_sensitivity);
        }
        if s.screenblocks != old.screenblocks {
            let _ = c.set(cvar::R_SCREENBLOCKS, s.screenblocks);
        }
        if s.low_detail != old.low_detail {
            let _ = c.set(cvar::R_LOWDETAIL, s.low_detail);
        }
        if s.show_messages != old.show_messages {
            let _ = c.set(cvar::HUD_MESSAGES, s.show_messages);
        }
        if s.gamma != old.gamma {
            let _ = c.set(cvar::VID_GAMMA, s.gamma as i32);
        }
        self.sync_cvars();
        // In sync even when every write was a no-op (the cvars already held
        // these values).
        self.synced_settings = self.settings;
    }

    /// Apply the cvars if any changed since the last call.
    fn sync_cvars(&mut self) {
        let generation = self.cvars.generation();
        if generation == self.cvar_generation {
            return;
        }
        self.cvar_generation = generation;
        let c = &self.cvars;
        let s = Settings {
            sfx_volume: c.get(cvar::SND_SFXVOLUME),
            music_volume: c.get(cvar::SND_MUSICVOLUME),
            mouse_sensitivity: c.get(cvar::M_SENSITIVITY),
            screenblocks: c.get(cvar::R_SCREENBLOCKS),
            low_detail: c.get(cvar::R_LOWDETAIL),
            show_messages: c.get(cvar::HUD_MESSAGES),
            gamma: c.get(cvar::VID_GAMMA) as usize,
        };
        let old = std::mem::replace(&mut self.settings, s);
        self.synced_settings = s;
        self.render_mode = RenderMode::parse(&c.get(cvar::R_RENDER_MODE)).unwrap_or_default();
        self.freelook = c.get(cvar::CL_FREELOOK);
        self.autoaim = c.get(cvar::SV_AUTOAIM);
        self.wipe_enabled = c.get(cvar::R_WIPE);
        self.skate_mode = c.get(cvar::PLAYER_CONTROLLER).eq_ignore_ascii_case("skate");
        self.skate_params = doom_sim::skate::SkateParams {
            acceleration: c.get(cvar::SKATE_ACCELERATION),
            max_speed: c.get(cvar::SKATE_MAX_SPEED),
            turn_rate: c.get(cvar::SKATE_TURN_RATE),
            carve_strength: c.get(cvar::SKATE_CARVE_STRENGTH),
            friction: c.get(cvar::SKATE_FRICTION),
            brake_strength: c.get(cvar::SKATE_BRAKE_STRENGTH),
            jump_strength: c.get(cvar::SKATE_JUMP_STRENGTH),
            air_control: c.get(cvar::SKATE_AIR_CONTROL),
            gravity: c.get(cvar::SKATE_GRAVITY),
            board_length: c.get(cvar::SKATE_BOARD_LENGTH),
            grind_snap_distance: c.get(cvar::SKATE_GRIND_SNAP_DISTANCE),
            grind_max_angle: c.get(cvar::SKATE_GRIND_MAX_ANGLE),
            grind_max_height: c.get(cvar::SKATE_GRIND_MAX_HEIGHT),
            grind_assist: c.get(cvar::SKATE_GRIND_ASSIST),
        };
        self.world.skate.params = self.skate_params;
        self.timescale = c.get(cvar::TIMESCALE);
        self.debug = DebugFlags {
            show_player: c.get(cvar::SHOW_PLAYER),
            show_collision: c.get(cvar::SHOW_COLLISION),
            show_velocity: c.get(cvar::SHOW_VELOCITY),
            show_grindables: c.get(cvar::SHOW_GRINDABLES),
            show_skate_state: c.get(cvar::SHOW_SKATE_STATE),
            skate_debug: c.get(cvar::SKATE_DEBUG),
            wireframe: c.get(cvar::R_WIREFRAME),
        };
        if (s.screenblocks, s.low_detail) != (old.screenblocks, old.low_detail) {
            self.apply_view();
        }
        if (s.sfx_volume, s.music_volume) != (old.sfx_volume, old.music_volume) {
            self.sound.set_volumes(s.sfx_volume, s.music_volume);
        }
        if !self.freelook {
            self.look_pitch = 0.0;
        }
        if !self.wipe_enabled {
            self.wipe = None;
        }
    }

    /// A screen melt is running (game tics are suspended).
    pub fn is_wiping(&self) -> bool {
        self.wipe.is_some()
    }

    // ---- camera -------------------------------------------------------------

    /// Current view pitch (radians, > 0 up).
    pub fn pitch(&self) -> f32 {
        self.look_pitch
    }

    /// The pitch range allowed for the current view window.
    pub fn max_pitch(&self) -> f32 {
        let r = &self.renderer;
        // Vertical focal length: half the full view width, in any detail.
        Camera::pitch_limit(r.viewheight as f32, r.scaledviewwidth as f32 / 2.0).min(MAX_PITCH)
    }

    /// Add to the view pitch (mouse/keyboard look). Ignored with
    /// `cl_freelook` off and during demos.
    pub fn look(&mut self, delta: f32) {
        self.sync_cvars();
        if !self.freelook || self.demo.is_some() {
            return;
        }
        let max = self.max_pitch();
        self.look_pitch = (self.look_pitch + delta).clamp(-max, max);
    }

    /// Level the view (`centerview`).
    pub fn center_view(&mut self) {
        self.look_pitch = 0.0;
    }

    /// Set `r_render_mode`.
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.cvars.set(cvar::R_RENDER_MODE, mode.as_str().to_string()).expect("r_render_mode accepts every RenderMode");
        self.sync_cvars();
    }

    /// Set `cl_freelook` (off levels the view).
    pub fn set_freelook(&mut self, on: bool) {
        let _ = self.cvars.set(cvar::CL_FREELOOK, on);
        self.sync_cvars();
    }

    pub fn freelook(&self) -> bool {
        self.freelook
    }

    /// `sv_autoaim`.
    pub fn autoaim(&self) -> bool {
        self.autoaim
    }

    /// What draws the 3D view with the current settings.
    pub fn view_path(&self) -> ViewPath {
        match (self.render_mode, self.freelook) {
            // The skateboard's chase camera needs the free camera.
            (RenderMode::Classic, _) if self.third_person() => ViewPath::Perspective,
            (RenderMode::GZDoomClean, _) => ViewPath::Host,
            (RenderMode::Classic, true) => ViewPath::Perspective,
            (RenderMode::Classic, false) => ViewPath::Columns,
        }
    }

    /// The camera and view window for a host renderer, when a 3D view is on
    /// screen (a level, not the automap). Available in every mode, so tools
    /// can drive a host renderer next to the classic ones.
    pub fn host_view(&self) -> Option<HostView> {
        if self.state != GameState::Level || self.automap.active {
            return None;
        }
        let player = self.world.params.consoleplayer;
        // A dead player's view stays level as the eye sinks.
        let pitch = if self.world.players[player].playerstate == PlayerState::Dead { 0.0 } else { self.look_pitch };
        let camera = match self.third_person().then(|| self.chase_camera()).flatten() {
            Some(c) => c,
            None => Camera::from_player(&self.world, player, pitch)?,
        };
        Some(HostView { player, camera, rect: self.view_rect() })
    }

    fn view_rect(&self) -> ViewRect {
        let r = &self.renderer;
        ViewRect { x: r.viewwindowx, y: r.viewwindowy, width: r.scaledviewwidth, height: r.viewheight, low_detail: self.settings.low_detail }
    }

    // ---- level / game flow --------------------------------------------------

    fn load_level(&self, m: MapRef) -> Result<Level, String> {
        Level::load(&self.stack, &self.textures, &m.lump_name(self.game.mode)).map_err(|e| e.to_string())
    }

    /// The per-level part of `G_DoLoadLevel`, `S_Start`, `ST_Start`,
    /// `AM_LevelInit`.
    fn start_level(&mut self) {
        // G_DoLoadLevel: `wipegamestate = -1` forces a melt even from a level.
        self.force_wipe = true;
        self.state = GameState::Level;
        self.intermission = None;
        self.finale = None;
        self.automap.stop();
        self.automap.level_init(&self.world);
        self.renderer.mapped_lines.clear();
        self.perspective.level_changed();
        self.look_pitch = 0.0;
        self.chase.valid = false;
        self.skate_prev = None;
        self.last_feat = None;
        self.world.skate.params = self.skate_params;
        self.status_bar.start(&self.world);
        self.hud_text.clear();
        self.sound.stop_all();
        self.sound.start_level(&self.stack, &self.world);
        self.sound.set_music_scale(1.0);
        let lump = self.current.lump_name(self.game.mode);
        let entry = self.mapinfo.get(&lump).cloned().unwrap_or_default();
        match &entry.music {
            Some(m) => self.sound.change_music_lump(&self.stack, &m.to_ascii_uppercase(), true),
            None => {
                let music = SoundSystem::level_music(self.game.mode, self.current.episode, self.current.map);
                self.sound.change_music(&self.stack, music, true);
            }
        }
        // MAPINFO sky.
        if let Some(t) = entry.sky.as_deref().and_then(|sky| self.textures.check_num(doom_content::LumpName::new(sky))) {
            self.world.sky_texture = doom_sim::ids::TexNum(t as u16);
        }
        // Problems met while spawning (unknown actors, ...).
        let new: Vec<String> = self.world.load_warnings.iter().map(|w| format!("{lump}: {w}")).collect();
        self.warnings.extend(new);
        self.warnings.extend(self.stack.take_late_warnings());
    }

    /// The map after this one: MAPINFO `next`/`secretnext` if given, else
    /// the game's fixed order.
    fn next_map(&self) -> Next {
        use doom_sim::world::ExitTarget;
        match &self.scripted_exit {
            Some(ExitTarget::Name(name)) => {
                if let Some(m) = MapRef::parse(name, self.game.mode) {
                    return Next::Map(m);
                }
            }
            Some(ExitTarget::Number(n)) => {
                let m = if self.game.mode == GameMode::Commercial { MapRef::new(1, *n) } else { MapRef::new(n / 10 + 1, n % 10) };
                return Next::Map(m);
            }
            Some(ExitTarget::EndGame) => return Next::EpisodeEnd,
            None => {}
        }
        let lump = self.current.lump_name(self.game.mode);
        let entry = self.mapinfo.get(&lump);
        let named = if self.secret_exit { entry.and_then(|e| e.secret_next.as_deref()) } else { entry.and_then(|e| e.next.as_deref()) };
        match named.and_then(|n| MapRef::parse(n, self.game.mode)) {
            Some(m) => Next::Map(m),
            None => next_map(self.game.mode, self.current, self.secret_exit),
        }
    }

    fn reload_level(&mut self) -> Result<(), String> {
        let level = self.load_level(self.current)?;
        self.world.setup_level(level)?;
        self.start_level();
        Ok(())
    }

    /// Start a fresh game on `m` (new inventory), like `G_InitNew`.
    pub fn new_game(&mut self, m: MapRef) -> Result<(), String> {
        self.new_game_with(m, self.base_params.skill)
    }

    fn new_game_with(&mut self, m: MapRef, skill: Skill) -> Result<(), String> {
        self.scripted_exit = None;
        if !m.exists(&self.stack, self.game.mode) {
            return Err(format!("map {} not found", m.lump_name(self.game.mode)));
        }
        let level = self.load_level(m)?;
        let mut params = self.base_params;
        params.episode = m.episode;
        params.map = m.map;
        params.skill = skill;
        self.base_params.skill = skill;
        let content = ContentInfo::from_content(&self.stack, &self.textures);
        self.world = World::new_game(self.info.clone(), content, params, single_player(), level)?;
        self.current = m;
        self.demo = None;
        self.usergame = true;
        self.paused = false;
        self.start_level();
        Ok(())
    }

    /// `G_DoCompleted`: leave the level for the intermission (or straight to
    /// the finale after Doom 1's map 8).
    fn level_completed(&mut self, exit: GameExit) {
        self.automap.stop();
        self.scripted_exit = self.world.exit_target.take();
        self.secret_exit = exit == GameExit::Secret;
        let mode = self.game.mode;
        let MapRef { episode, map } = self.current;
        if mode != GameMode::Commercial {
            if map == 8 {
                self.start_finale();
                return;
            }
            if map == 9 {
                for p in &mut self.world.players {
                    p.didsecret = true;
                }
            }
        }
        let next = match self.next_map() {
            Next::Map(m) => m.map - 1,
            Next::EpisodeEnd => map,
        };
        let p = self.world.console_player();
        let wbs = WbStart {
            epsd: episode - 1,
            last: map - 1,
            next,
            didsecret: p.didsecret,
            maxkills: self.world.totalkills,
            maxitems: self.world.totalitems,
            maxsecret: self.world.totalsecret,
            partime: WbStart::par_time(mode, episode, map, &self.info.deh.pars),
            kills: p.killcount,
            items: p.itemcount,
            secret: p.secretcount,
            time: self.world.leveltime,
        };
        self.intermission =
            Some(Intermission::new(&self.stack, &mut self.patches, mode, self.game.version, wbs, &mut self.world.rng));
        self.state = GameState::Intermission;
    }

    /// `G_WorldDone` + `G_DoWorldDone`.
    fn world_done(&mut self) {
        if self.secret_exit {
            for p in &mut self.world.players {
                p.didsecret = true;
            }
        }
        if self.game.mode == GameMode::Commercial && self.state != GameState::Finale {
            let map = self.current.map;
            let finale = match map {
                15 | 31 => self.secret_exit,
                6 | 11 | 20 | 30 => true,
                _ => false,
            };
            if finale {
                self.start_finale();
                return;
            }
        }
        let next = self.next_map();
        self.scripted_exit = None;
        match next {
            Next::Map(m) if m.exists(&self.stack, self.game.mode) => {
                let result = self.load_level(m).and_then(|level| self.world.next_level(m.episode, m.map, level));
                match result {
                    Ok(()) => {
                        self.current = m;
                        self.start_level();
                    }
                    Err(e) => {
                        self.start_title();
                        self.show_message(e);
                    }
                }
            }
            // The game is over (or the next map is missing): title screen.
            _ => self.start_title(),
        }
    }

    fn start_finale(&mut self) {
        let mut out = Vec::new();
        self.finale = Some(Finale::new(&self.stack, &mut self.patches, &self.game, &self.info.deh.strings, self.current.episode, self.current.map, &mut out));
        self.intermission = None;
        self.state = GameState::Finale;
        self.handle_finale_events(out);
    }

    fn handle_finale_events(&mut self, events: Vec<FinaleEvent>) {
        for ev in events {
            match ev {
                FinaleEvent::StartSound(s) => self.local_sound(s),
                FinaleEvent::ChangeMusic(m) => self.sound.change_music(&self.stack, m, true),
                FinaleEvent::WorldDone => self.world_done(),
            }
        }
    }

    fn local_sound(&mut self, sfx: SfxNum) {
        self.sound.start_local(&self.stack, &self.world.info, sfx, &mut self.world.rng);
    }

    // ---- title / demo loop (d_main.c) ----------------------------------------

    /// `D_StartTitle`.
    fn start_title(&mut self) {
        self.usergame = false;
        self.demo = None;
        self.paused = false;
        self.automap.stop();
        self.demosequence = -1;
        self.advance_demo();
    }

    /// `D_DoAdvanceDemo`.
    fn advance_demo(&mut self) {
        self.demo = None;
        self.paused = false;
        let commercial = self.game.mode == GameMode::Commercial;
        let ultimate = matches!(self.game.version, GameVersion::Ultimate | GameVersion::Final | GameVersion::Final2);
        self.demosequence = (self.demosequence + 1) % if ultimate { 7 } else { 6 };
        let page = |s: &mut Session, name: &str, tics: i32| {
            s.state = GameState::DemoScreen;
            s.pagetic = tics;
            s.page = s.patches.get(&s.stack, name);
        };
        match self.demosequence {
            0 => {
                page(self, "TITLEPIC", if commercial { 35 * 11 } else { 170 });
                let m = if commercial { doom_sim::info::MUS_DM2TTL } else { doom_sim::info::MUS_INTRO };
                self.sound.change_music(&self.stack, m, false);
            }
            2 => page(self, "CREDIT", 200),
            4 => {
                if commercial {
                    page(self, "TITLEPIC", 35 * 11);
                    self.sound.change_music(&self.stack, doom_sim::info::MUS_DM2TTL, false);
                } else {
                    page(self, if self.game.version >= GameVersion::Ultimate { "CREDIT" } else { "HELP2" }, 200);
                }
            }
            n => {
                let name = match n {
                    1 => "DEMO1",
                    3 => "DEMO2",
                    5 => "DEMO3",
                    _ => "DEMO4",
                };
                let demo = self.stack.lump(name).and_then(|d| Demo::parse(d).ok());
                if demo.and_then(|d| self.play_demo(d, true).ok()).is_none() {
                    // Missing or unplayable demo: skip it.
                    self.advance_demo();
                }
            }
        }
    }

    fn play_demo(&mut self, demo: Demo, attract: bool) -> Result<(), String> {
        let m = MapRef::new(demo.episode, demo.map);
        let level = self.load_level(m)?;
        let params = GameParams {
            mode: self.game.mode,
            version: self.game.version,
            skill: demo.skill,
            episode: demo.episode,
            map: demo.map,
            deathmatch: demo.deathmatch,
            netgame: demo.playeringame[1..].iter().any(|&p| p),
            respawnmonsters: demo.respawnparm,
            nomonsters: demo.nomonsters,
            fastparm: demo.fastparm,
            consoleplayer: demo.consoleplayer,
        };
        let content = ContentInfo::from_content(&self.stack, &self.textures);
        self.world = World::new_game(self.info.clone(), content, params, demo.playeringame, level)?;
        self.current = m;
        self.demo = Some(DemoPlayback { demo, pos: 0, attract });
        self.usergame = false;
        self.start_level();
        Ok(())
    }

    // ---- input --------------------------------------------------------------

    /// A key was pressed (including auto-repeat for menu navigation).
    pub fn key_down(&mut self, key: Key) {
        self.sync_cvars();
        if key == Key::Grave {
            self.console.toggle();
            return;
        }
        if self.console.is_open() {
            self.console_key(key);
            return;
        }
        self.respond(key);
        // The menu and some keys edit `settings` (or cvars) directly.
        self.store_settings();
    }

    fn respond(&mut self, key: Key) {
        if self.quit_countdown.is_some() {
            return;
        }
        // M_Responder gets first chance.
        let mut actions = Vec::new();
        let in_game = self.usergame && self.state == GameState::Level;
        let eaten = self.menu.responder(key, &mut self.settings, &self.cvars, self.world.gametic, in_game, &mut actions);
        self.handle_menu_actions(actions);
        if eaten {
            return;
        }
        // Screen size keys outside the automap (also M_Responder).
        if !self.automap.active && matches!(key, Key::Minus | Key::Equals) && self.state == GameState::Level {
            let b = self.settings.screenblocks + if key == Key::Minus { -1 } else { 1 };
            self.settings.screenblocks = b.clamp(3, 11);
            self.apply_view();
            self.local_sound(doom_sim::info::SFX_STNMOV);
            return;
        }
        // G_Responder: during the title loop or demo playback any key opens
        // the menu.
        if !self.usergame && (self.demo.is_some() || self.state == GameState::DemoScreen) {
            self.menu.open();
            self.local_sound(doom_sim::info::SFX_SWTCHN);
            return;
        }
        match self.state {
            GameState::Finale => {
                let mut out = Vec::new();
                if let Some(f) = &mut self.finale {
                    f.responder(&self.world.info, &mut out);
                }
                self.handle_finale_events(out);
            }
            GameState::Level => {
                if key == Key::Pause {
                    self.paused = !self.paused;
                    self.sound.pause_music(self.paused);
                    return;
                }
                if key == Key::Tab {
                    if self.automap.active {
                        self.automap.stop();
                    } else {
                        self.automap.start(&self.world);
                    }
                    return;
                }
                if let Some(msg) = self.automap.responder(key, true, &self.world) {
                    self.show_message(msg);
                }
                if let Key::Char(c) = key {
                    if let Some(cheat) = self.cheats.feed(c) {
                        self.cheat(cheat);
                    }
                }
            }
            _ => {}
        }
    }

    /// Typed text (for the console; ignored while it is closed).
    pub fn text_input(&mut self, text: &str) {
        if self.console.is_open() {
            self.console.type_text(text);
            self.update_suggestions();
        }
    }

    fn console_key(&mut self, key: Key) {
        match self.console.key(key) {
            Some(ConsoleAction::Close) => self.console.set_open(false),
            Some(ConsoleAction::Submit(line)) => {
                self.console.print(LineKind::Input, &format!("] {line}"));
                match self.exec(&line) {
                    Ok(out) if !out.is_empty() => self.console.print(LineKind::Output, &out),
                    Ok(_) => {}
                    Err(e) => self.console.print(LineKind::Error, &e),
                }
            }
            Some(ConsoleAction::Complete) => {
                let candidates = self.completions(&self.console.input.clone());
                if let Some(list) = self.console.complete_with(candidates) {
                    self.console.print(LineKind::Info, &list);
                }
            }
            None => {}
        }
        self.update_suggestions();
    }

    /// Words that can complete the last word of `line`.
    pub fn completions(&self, line: &str) -> Vec<String> {
        let (before, _) = con::split_last_word(line);
        let cmd_line = con::split_commands(before).last().copied().unwrap_or("");
        let words = con::tokenize(cmd_line);
        match words.first() {
            None => self.commands.names().map(str::to_string).chain(self.cvars.list().into_iter().map(|v| v.name.to_string())).collect(),
            Some(name) => {
                if let Some(c) = self.commands.get(name) {
                    return c.complete.as_ref().map(|f| f(self, words.len() - 1, &words[1..])).unwrap_or_default();
                }
                match self.cvars.info(name) {
                    Some(v) if words.len() == 1 => {
                        let mut out: Vec<String> = v.choices.iter().map(|c| c.to_string()).collect();
                        if matches!(v.value, cvar::CVarValue::Bool(_)) {
                            out.extend(["0".to_string(), "1".to_string()]);
                        }
                        out
                    }
                    _ => Vec::new(),
                }
            }
        }
    }

    /// Live hints under the input line: matching commands and variables
    /// (first word), or the command's usage.
    fn update_suggestions(&mut self) {
        let line = self.console.input.clone();
        let mut out = Vec::new();
        let last = con::split_commands(&line).last().copied().unwrap_or("");
        let words = con::tokenize(last);
        if let Some(first) = words.first() {
            let typing_name = words.len() == 1 && !last.ends_with(' ');
            if typing_name {
                let p = first.to_ascii_lowercase();
                for c in self.commands.iter().filter(|c| c.name.starts_with(&p)).take(6) {
                    out.push(format!("{} {} - {}", c.name, c.usage, c.help));
                }
                if out.len() < 6 {
                    for v in self.cvars.list().into_iter().filter(|v| v.name.starts_with(&p)).take(6 - out.len()) {
                        out.push(format!("{} = {} - {}", v.name, v.value, v.help));
                    }
                }
            } else if let Some(c) = self.commands.get(first) {
                out.push(format!("{} {}", c.name, c.usage));
            } else if let Some(v) = self.cvars.info(first) {
                out.push(commands::describe_cvar(&v).replace('\n', " -"));
            }
        }
        self.console.suggestions = out;
    }

    /// Run console commands (`;`-separated) as if typed, returning their
    /// output. Words that are not commands read or set console variables.
    pub fn exec(&mut self, line: &str) -> Result<String, String> {
        let mut out = Vec::new();
        for part in con::split_commands(line) {
            let words = con::tokenize(part);
            if words.is_empty() {
                continue;
            }
            let text = self.exec_words(&words)?;
            if !text.is_empty() {
                out.push(text);
            }
        }
        Ok(out.join("\n"))
    }

    fn exec_words(&mut self, words: &[String]) -> Result<String, String> {
        let name = &words[0];
        if let Some(c) = self.commands.get(name) {
            let run = c.run.clone();
            let r = run(self, &words[1..]);
            self.sync_cvars();
            return r;
        }
        let Some(info) = self.cvars.info(&name.to_ascii_lowercase()) else {
            return Err(commands::unknown(self, name));
        };
        let r = if words.len() == 1 {
            match info.value {
                // Debug switches flip when named alone.
                cvar::CVarValue::Bool(b) if !info.archive => {
                    self.cvars.set_value(info.name, cvar::CVarValue::Bool(!b)).map_err(|e| e.to_string())?;
                    Ok(format!("{} = {}", info.name, self.cvars.value(info.name).unwrap()))
                }
                _ => Ok(commands::describe_cvar(&info)),
            }
        } else {
            let text = words[1..].join(" ");
            self.cvars.set_str(info.name, &text).map_err(|e| e.to_string())?;
            Ok(format!("{} = {}", info.name, self.cvars.value(info.name).unwrap()))
        };
        self.sync_cvars();
        r
    }

    pub fn key_up(&mut self, key: Key) {
        let _ = self.automap.responder(key, false, &self.world);
    }

    fn cheat(&mut self, cheat: Cheat) {
        // ST_Responder: no cheats on Nightmare (except map reveal/warp).
        let nightmare = self.world.params.skill == Skill::Nightmare;
        match cheat {
            Cheat::MapReveal => {
                if self.automap.active {
                    self.automap.cheating = (self.automap.cheating + 1) % 3;
                }
            }
            Cheat::Warp(_) => self.apply_cheat(&cheat),
            _ if nightmare => {}
            _ => self.apply_cheat(&cheat),
        }
    }

    fn handle_menu_actions(&mut self, actions: Vec<MenuAction>) {
        for a in actions {
            match a {
                MenuAction::StartSound(s) => self.local_sound(s),
                MenuAction::NewGame { episode, skill } => {
                    let m = if self.game.mode == GameMode::Commercial { MapRef::new(1, 1) } else { MapRef::new(episode, 1) };
                    if let Err(e) = self.new_game_with(m, skill) {
                        self.show_message(e);
                    }
                }
                MenuAction::EndGame => self.start_title(),
                MenuAction::Quit => self.quit_countdown = Some(53),
                MenuAction::ViewChanged => self.apply_view(),
                MenuAction::VolumeChanged => self.sound.set_volumes(self.settings.sfx_volume, self.settings.music_volume),
                MenuAction::Message(m) => self.show_message(m),
            }
        }
    }

    // ---- ticking --------------------------------------------------------------

    /// Run one 35 Hz tic with this input (ignored during demo playback).
    pub fn tick(&mut self, input: &InputState) {
        self.sync_cvars();
        self.menu.ticker();
        if let Some(n) = &mut self.quit_countdown {
            *n -= 1;
            if *n <= 0 {
                self.quit = true;
            }
            self.sound.update(&self.stack, None, false);
            return;
        }
        // D_Display's wipe loop runs no game tics: the melt advances one
        // step per tic and the game resumes where it was afterwards.
        if let Some(w) = &mut self.wipe {
            if w.tick() {
                self.wipe = None;
            }
            return;
        }
        // P_Ticker does nothing while the menu is up in a user game.
        let menu_pause = self.menu.is_open() && self.demo.is_none() && self.usergame;
        let mut cmds = [TicCmd::default(); MAXPLAYERS];
        let neutral = InputState::default();
        let input = if self.menu.is_open() || self.console.is_open() { &neutral } else { input };
        // A slot picked at the console goes out as if its key was pressed.
        let with_slot;
        let input = match self.pending_weapon_slot.take() {
            Some(slot) => {
                with_slot = InputState { weapon_slot: Some(slot), ..*input };
                &with_slot
            }
            None => input,
        };
        // G_Ticker reads demo commands in every game state.
        match &mut self.demo {
            Some(d) => match d.demo.tics.get(d.pos) {
                Some(c) => {
                    cmds = *c;
                    d.pos += 1;
                }
                None => {
                    // G_CheckDemoStatus: demo over.
                    let attract = d.attract;
                    self.demo = None;
                    if attract {
                        self.advance_demo();
                    } else {
                        self.start_title();
                    }
                    return;
                }
            },
            None => cmds[self.world.params.consoleplayer] = self.builder.build(input),
        }

        match self.state {
            GameState::Level => {
                if !(self.paused || menu_pause) {
                    // `timescale`: run more or fewer game tics per host tic
                    // (never during demos, whose tics are fixed).
                    let runs = if self.demo.is_some() || self.timescale == 1.0 {
                        1
                    } else {
                        self.time_carry += self.timescale;
                        let n = self.time_carry.floor();
                        self.time_carry -= n;
                        (n as usize).min(10)
                    };
                    for _ in 0..runs {
                        self.sync_controller();
                        self.snapshot_skater();
                        self.level_tic(&cmds);
                        if self.state != GameState::Level {
                            break;
                        }
                    }
                    // Without frame timing from the host the camera moves
                    // once per tic; with it, per frame (set_frame_time).
                    if !self.frame_timed {
                        self.update_chase(runs as f32 / TICRATE as f32);
                    }
                    self.report_feats();
                }
                if self.state == GameState::Level {
                    self.status_bar.ticker(&mut self.world);
                    self.automap.ticker(&self.world);
                    self.hud_text.ticker();
                }
            }
            GameState::Intermission => {
                let b = cmds[self.world.params.consoleplayer].buttons;
                let mut out = Vec::new();
                if let Some(wi) = &mut self.intermission {
                    wi.ticker(b & bt::ATTACK != 0, b & bt::USE != 0, &mut self.world.rng, &mut out);
                }
                for ev in out {
                    match ev {
                        WiEvent::StartSound(s) => self.local_sound(s),
                        WiEvent::ChangeMusic(m) => self.sound.change_music(&self.stack, m, true),
                        WiEvent::Done => self.world_done(),
                    }
                }
            }
            GameState::Finale => {
                let any_button = cmds[self.world.params.consoleplayer].buttons != 0;
                let mut out = Vec::new();
                if let Some(f) = &mut self.finale {
                    f.ticker(&self.world.info, any_button, &mut out);
                }
                self.handle_finale_events(out);
            }
            GameState::DemoScreen => {
                self.pagetic -= 1;
                if self.pagetic < 0 {
                    self.advance_demo();
                }
            }
        }
        let boss_map = self.current.map == 8;
        let w = (self.state == GameState::Level).then_some(&mut self.world);
        self.sound.update(&self.stack, w, boss_map);
        // Sounds that failed to decode are reported once each.
        for msg in self.sound.take_warnings() {
            self.console.print(LineKind::Info, &format!("warning: {msg}"));
            self.warnings.push(msg);
        }
    }

    fn level_tic(&mut self, cmds: &[TicCmd; MAXPLAYERS]) {
        match self.world.tick(cmds) {
            None => {}
            Some(GameAction::LoadLevel) => {
                // Single-player death: `G_DoReborn` reloads the level; the
                // tic then runs on the fresh level as in G_Ticker.
                if let Err(e) = self.reload_level() {
                    self.show_message(e);
                    return;
                }
                self.world.tick(cmds);
            }
            Some(GameAction::Completed(exit)) => {
                self.level_completed(exit);
                return;
            }
        }
        // HU_Ticker: pick up the player's message.
        let cp = self.world.params.consoleplayer;
        if let Some(m) = self.world.players[cp].message.take() {
            let m = self.info.deh.strings.get(m);
            self.console.print(LineKind::Info, m);
            if self.settings.show_messages {
                self.hud_text.show(m);
            }
        }
        self.handle_script_events();
        let events = std::mem::take(&mut self.world.events);
        let boss_map = self.current.map == 8;
        self.sound.handle_events(&self.stack, &mut self.world, &events, boss_map);
        self.events.extend(events);
    }

    /// Messages and music changes from map scripts.
    fn handle_script_events(&mut self) {
        use doom_sim::world::ScriptEvent;
        let cp = self.world.params.consoleplayer;
        for ev in std::mem::take(&mut self.world.script_events) {
            match ev {
                ScriptEvent::Print { player, text } => {
                    if player.is_none_or(|p| p == cp) {
                        self.hud_text.show(text);
                    }
                }
                ScriptEvent::Music { name } => {
                    if name.is_empty() || name == "-" {
                        self.sound.stop_music();
                    } else {
                        self.sound.change_music_lump(&self.stack, &name.to_ascii_uppercase(), true);
                    }
                }
                ScriptEvent::MusicVolume(v) => self.sound.set_music_scale(v),
            }
        }
    }

    // ---- developer cheats (st_stuff.c equivalents) ------------------------

    fn player_mut(&mut self) -> &mut doom_sim::player::Player {
        let cp = self.world.params.consoleplayer;
        &mut self.world.players[cp]
    }

    /// `iddqd`.
    pub fn toggle_god(&mut self) {
        // DEHACKED `God Mode Health` (100 in vanilla).
        let health = self.info.misc.god_mode_health;
        let p = self.player_mut();
        p.cheats ^= cheat::GODMODE;
        let on = p.cheats & cheat::GODMODE != 0;
        if on {
            p.health = health;
            if let Some(mo) = p.mo {
                self.world.mo_mut(mo).health = health;
            }
        }
        self.show_message(if on { "Degreelessness Mode On" } else { "Degreelessness Mode Off" });
    }

    /// `idclip`.
    pub fn toggle_noclip(&mut self) {
        let p = self.player_mut();
        p.cheats ^= cheat::NOCLIP;
        let on = p.cheats & cheat::NOCLIP != 0;
        self.show_message(if on { "No Clipping Mode ON" } else { "No Clipping Mode OFF" });
    }

    /// `idkfa`.
    pub fn give_all(&mut self) {
        let (armor, class) = (self.info.misc.idkfa_armor, self.info.misc.idkfa_armor_class);
        let p = self.player_mut();
        p.armorpoints = armor;
        p.armortype = class;
        p.weaponowned = [true; NUMWEAPONS];
        p.ammo = p.maxammo;
        p.cards = [true; 6];
        // Mod weapons with a slot too, as GZDoom's IDKFA does.
        let cp = self.world.params.consoleplayer;
        doom_sim::weapons::give_all(&mut self.world, cp);
        self.show_message("Very Happy Ammo Added");
    }

    /// `idclev`: new game on another map.
    pub fn warp(&mut self, map: &str) -> Result<(), String> {
        let m = MapRef::parse(map, self.game.mode).ok_or_else(|| format!("bad map name {map}"))?;
        self.new_game(m)
    }

    /// Exit the level as if the exit switch was used (developer shortcut).
    pub fn exit_level(&mut self) {
        if self.state == GameState::Level {
            self.level_completed(GameExit::Normal);
        }
    }

    // ---- presentation -------------------------------------------------------

    /// `ST_doPaletteStuff`: PLAYPAL index for damage/pickup/radiation tints.
    pub fn palette_index(&self) -> usize {
        if self.state != GameState::Level {
            return 0;
        }
        let p = self.world.console_player();
        let mut cnt = p.damagecount;
        if p.powers[power::STRENGTH] != 0 {
            // Slowly fade the berserk out.
            let bzc = 12 - (p.powers[power::STRENGTH] >> 6);
            cnt = cnt.max(bzc);
        }
        if cnt > 0 {
            1 + (((cnt + 7) >> 3) as usize).min(7)
        } else if p.bonuscount > 0 {
            9 + (((p.bonuscount + 7) >> 3) as usize).min(3)
        } else if p.powers[power::IRONFEET] > 4 * 32 || p.powers[power::IRONFEET] & 8 != 0 {
            13
        } else {
            0
        }
    }

    /// `D_Display`: compose the whole 320x200 frame (palette indices).
    ///
    /// When the game state changed since the last displayed frame (or a
    /// level was loaded), this starts a melt from that frame to the new one;
    /// while it runs, frames show the melt with the menu on top.
    pub fn render_indexed(&mut self) -> &[u8] {
        self.sync_cvars();
        if let Some(w) = &self.wipe {
            w.draw(&mut self.screen, SCREENWIDTH, SCREENHEIGHT);
        } else {
            let melt = self.wipe_enabled
                && self.render_mode == RenderMode::Classic
                && self.wipe_state.is_some_and(|last| last != self.state || self.force_wipe);
            // wipe_StartScreen: the frame still on screen.
            let start = melt.then(|| self.screen.clone());
            self.draw_frame(None);
            if let Some(start) = start {
                // wipe_EndScreen: the new state's first frame, before the menu
                // (drawn live over the melt instead).
                let w = Wipe::new(start, self.screen.clone(), &mut self.world.rng);
                w.draw(&mut self.screen, SCREENWIDTH, SCREENHEIGHT);
                self.wipe = Some(w);
            }
            self.wipe_state = Some(self.state);
            self.force_wipe = false;
        }
        self.draw_overlays();
        &self.screen
    }

    /// Draw the current game state (everything but the menu) into `screen`.
    ///
    /// `host_fill`: the host draws the 3D view, so fill the view window with
    /// this index instead (see [`Session::render_overlay_rgba`]).
    fn draw_frame(&mut self, host_fill: Option<u8>) {
        let path = self.view_path();
        let rect = self.view_rect();
        let third = self.third_person();
        // Extra 3D geometry: the skater (third person) and debug views.
        let mut debug_cam = None;
        if self.state == GameState::Level && !self.automap.active && (third || self.debug.draws_3d()) {
            let player = self.world.params.consoleplayer;
            let pitch = if path == ViewPath::Columns { 0.0 } else { self.look_pitch };
            let cam = if third { self.chase_camera() } else { None }.or_else(|| Camera::from_player(&self.world, player, pitch));
            if let Some(cam) = cam {
                self.build_scene(&cam);
                debug_cam = Some(cam);
            }
        } else {
            self.scene.clear();
        }
        let screen = &mut self.screen;
        match self.state {
            GameState::Level => {
                let blocks = self.settings.screenblocks;
                if self.automap.active {
                    self.automap.draw(screen, &self.world, &self.renderer.mapped_lines);
                } else if self.world.console_player().mo.is_some() {
                    let player = self.world.params.consoleplayer;
                    match (host_fill, path) {
                        (Some(fill), _) => {
                            fill_rect(screen, rect.x, rect.y, rect.width, rect.height, fill);
                            // The host draws the level; the skateboard and
                            // rider (and debug views) still come from here.
                            if let Some(cam) = debug_cam {
                                self.perspective.draw_scene_overlay(&self.render_data, &self.world, player, &cam, rect, screen, &self.scene);
                            }
                            if !third {
                                self.renderer.render_psprites(&self.render_data, &self.world, player, screen);
                            }
                        }
                        (None, ViewPath::Columns) => {
                            self.renderer.render_player_view(&self.render_data, &self.world, player, screen);
                            if let Some(cam) = debug_cam {
                                self.perspective.draw_scene_overlay(&self.render_data, &self.world, player, &cam, rect, screen, &self.scene);
                            }
                        }
                        // Without a host renderer (tools, screenshots) the
                        // perspective renderer stands in for it.
                        (None, ViewPath::Perspective | ViewPath::Host) => {
                            // A dead player's view stays level as the eye sinks.
                            let pitch = if self.world.players[player].playerstate == PlayerState::Dead { 0.0 } else { self.look_pitch };
                            let cam = if third { debug_cam } else { None };
                            if let Some(cam) = cam.or_else(|| Camera::from_player(&self.world, player, pitch)) {
                                self.perspective.render_scene(&self.render_data, &self.world, player, &cam, rect, screen, &self.scene);
                            }
                            // The weapon is screen space, drawn as in vanilla
                            // (not in third person).
                            if !third {
                                self.renderer.render_psprites(&self.render_data, &self.world, player, screen);
                            }
                            // One automap memory whichever renderer drew.
                            let seen = &self.perspective.mapped_lines;
                            let mapped = &mut self.renderer.mapped_lines;
                            if mapped.len() < seen.len() {
                                mapped.resize(seen.len(), false);
                            }
                            for (m, &s) in mapped.iter_mut().zip(seen) {
                                *m |= s;
                            }
                        }
                    }
                    let r = &self.renderer;
                    self.border.draw(screen, r.viewwindowx, r.viewwindowy, r.scaledviewwidth, r.viewheight, ST_Y);
                }
                if self.automap.active || blocks <= 10 {
                    self.status_bar.draw(screen, &self.world);
                } else {
                    self.status_bar.draw_fullscreen(screen, &self.world);
                }
                if self.automap.active {
                    let lump = self.current.lump_name(self.game.mode);
                    let title = match self.mapinfo.get(&lump).and_then(|e| e.name.clone()) {
                        Some(name) => format!("{lump}: {name}"),
                        None => map_title(&self.game, &self.info.deh.strings, self.current.episode, self.current.map),
                    };
                    self.font.draw(screen, 0, ST_Y - 1 - self.font.height(), &title);
                }
                self.hud_text.draw(screen, &self.font);
                self.draw_debug_text();
                let screen = &mut self.screen;
                if self.paused {
                    let top = if self.automap.active { 4 } else { self.renderer.viewwindowy };
                    self.menu.draw_pause(screen, top);
                }
            }
            GameState::Intermission => {
                if let Some(wi) = &self.intermission {
                    wi.draw(screen);
                }
            }
            GameState::Finale => {
                let mut out = Vec::new();
                if let Some(f) = &mut self.finale {
                    f.draw(screen, &self.font, &self.render_data, &self.world.info, &mut out);
                }
                self.handle_finale_events(out);
            }
            GameState::DemoScreen => {
                if let Some(p) = &self.page {
                    draw_patch(&mut self.screen, 0, 0, p);
                }
            }
        }
    }

    /// Render and convert to RGBA8 (`out.len() == 320 * 200 * 4`), applying
    /// the current palette effect and gamma level.
    pub fn render_rgba(&mut self, out: &mut [u8]) {
        assert_eq!(out.len(), SCREENWIDTH * SCREENHEIGHT * 4);
        let pal = self.display_palette();
        let screen = self.render_indexed();
        for (px, &i) in out.chunks_exact_mut(4).zip(screen) {
            let [r, g, b] = pal[i as usize];
            px.copy_from_slice(&[r, g, b, 255]);
        }
    }

    /// `I_SetPalette`: the current PLAYPAL entry (damage/pickup tint)
    /// through the gamma table.
    pub fn display_palette(&self) -> [[u8; 3]; 256] {
        let gamma = &crate::gamma::GAMMATABLE[self.settings.gamma.min(4)];
        let pal = *self.render_data.palettes.get(self.palette_index());
        pal.map(|c| c.map(|v| gamma[v as usize]))
    }

    /// The screen-space layers for a host that draws the 3D view itself
    /// ([`ViewPath::Host`]): 320x200 RGBA where the view window is
    /// transparent (alpha 0) except for the weapon, HUD text, full-screen
    /// HUD, pause graphic and menu drawn over it. Without a 3D view on
    /// screen (title, intermission, automap...) this is the whole opaque
    /// frame, as [`Session::render_rgba`].
    pub fn render_overlay_rgba(&mut self, out: &mut [u8]) {
        assert_eq!(out.len(), SCREENWIDTH * SCREENHEIGHT * 4);
        self.sync_cvars();
        if self.host_view().is_none() || self.world.console_player().mo.is_none() {
            return self.render_rgba(out);
        }
        // Draw twice over different fills: a view pixel that kept the fill
        // both times was not drawn by any layer.
        let pal = self.display_palette();
        let rect = self.view_rect();
        self.draw_frame(Some(0));
        self.draw_overlays();
        let mut first = std::mem::take(&mut self.overlay_scratch);
        first.clear();
        first.extend_from_slice(&self.screen);
        self.draw_frame(Some(255));
        self.draw_overlays();
        for (i, (px, (&lo, &hi))) in out.chunks_exact_mut(4).zip(first.iter().zip(&self.screen)).enumerate() {
            let (x, y) = ((i % SCREENWIDTH) as i32, (i / SCREENWIDTH) as i32);
            let in_view = x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
            let [r, g, b] = pal[hi as usize];
            let alpha = if in_view && lo == 0 && hi == 255 { 0 } else { 255 };
            px.copy_from_slice(&[r, g, b, alpha]);
        }
        self.overlay_scratch = first;
        // The melt is not composited with a host view (yet).
        self.wipe = None;
        self.wipe_state = Some(self.state);
        self.force_wipe = false;
    }

    /// Layers drawn over the frame: the menu, then the console.
    fn draw_overlays(&mut self) {
        self.menu.draw(&mut self.screen, &self.font, &self.settings, &self.cvars);
        if self.console.is_open() {
            let darken = &self.render_data.colormaps[22 * 256..23 * 256];
            con::draw(&mut self.console, &mut self.screen, SCREENWIDTH, SCREENHEIGHT as i32 / 2, &self.con_palette, darken, None);
        }
    }

    // ---- player controller ------------------------------------------------

    /// Put the console player on or off the board to match
    /// `player_controller`, and make sure the level's grind graph exists
    /// while it is needed. Never during demos (they must play back as
    /// recorded).
    fn sync_controller(&mut self) {
        let cp = self.world.params.consoleplayer;
        let p = &self.world.players[cp];
        let alive = p.playerstate == PlayerState::Live && p.mo.is_some();
        let want = self.skate_mode && self.demo.is_none() && self.state == GameState::Level;
        match (want && alive, p.skater.is_some()) {
            (true, false) => doom_sim::skate::enable(&mut self.world, cp),
            (false, true) if alive || !want => doom_sim::skate::disable(&mut self.world, cp),
            _ => {}
        }
        if (want || self.debug.show_grindables) && self.world.skate.grind.is_none() && self.state == GameState::Level {
            self.ensure_grind_graph();
        }
    }

    /// The current level's grind graph, from the cache or freshly built.
    fn ensure_grind_graph(&mut self) {
        let params = doom_sim::skate::grind_params(&self.skate_params);
        let key = format!("{}:{:?}", self.map_name(), params);
        if let Some(g) = self.grind_cache.get(&key) {
            if g.stats.lines_checked == self.world.level.lines.len() {
                self.world.skate.grind = Some(g.clone());
                return;
            }
        }
        doom_sim::skate::ensure_grind_graph(&mut self.world);
        if let Some(g) = &self.world.skate.grind {
            self.grind_cache.insert(key, g.clone());
        }
    }

    /// Show completed tricks on the HUD.
    fn report_feats(&mut self) {
        use doom_sim::skate::Feat;
        let feat = self.world.console_player().skater.as_ref().and_then(|s| s.last_feat);
        if feat == self.last_feat {
            return;
        }
        self.last_feat = feat;
        let Some(f) = feat else { return };
        let text = match f {
            Feat::Air { spin_halves, airtime, fakie } => {
                let spin = spin_halves.unsigned_abs() * 180;
                if spin == 0 && airtime < 18 {
                    return;
                }
                let name = if spin > 0 { format!("{spin} OLLIE") } else { "OLLIE".to_string() };
                format!("{}{name} {:.1}S AIR", if fakie { "FAKIE " } else { "" }, airtime as f32 / 35.0)
            }
            Feat::Grind { style, kind, tics, edges } => {
                let mut t = format!("{} {} {:.1}S", style.name().to_ascii_uppercase(), kind.name().to_ascii_uppercase(), tics as f32 / 35.0);
                if edges > 1 {
                    t.push_str(&format!(" X{edges}"));
                }
                t
            }
            Feat::Bail => "BAIL!".to_string(),
        };
        self.hud_text.show(text);
    }

    pub fn hud(&self) -> Hud {
        let w = &self.world;
        let p = w.console_player();
        Hud {
            map: self.map_name(),
            health: p.health,
            armor: p.armorpoints,
            weapon: p.readyweapon,
            ammo: doom_sim::weapons::ready_ammo(w, w.params.consoleplayer),
            weapons_owned: p.weaponowned,
            keys: p.cards,
            kills: (p.killcount, w.totalkills),
            items: (p.itemcount, w.totalitems),
            secrets: (p.secretcount, w.totalsecret),
            dead: p.playerstate == PlayerState::Dead,
            god: p.cheats & cheat::GODMODE != 0,
            noclip: p.cheats & cheat::NOCLIP != 0,
            message: self.hud_text.message().map(str::to_string),
            level_seconds: (w.leveltime.max(0) as u32) / TICRATE,
        }
    }
}

fn single_player() -> [bool; MAXPLAYERS] {
    let mut pig = [false; MAXPLAYERS];
    pig[0] = true;
    pig
}

/// The first map present: E1M1 / MAP01, else the first map lump found.
fn first_map(stack: &ResourceStack, mode: GameMode) -> Option<MapRef> {
    let preferred = MapRef::new(1, 1);
    if preferred.exists(stack, mode) {
        return Some(preferred);
    }
    doom_content::map::list_maps(stack).iter().find_map(|n| MapRef::parse(&n.to_string(), mode))
}
