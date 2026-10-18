//! Bevy host for the doom_* engine crates.
//!
//! cargo run --release -- [--iwad WADS/doom1.wad] [--pwad mod.wad ...] [--map E1M1] [--skill 3]
//!
//! The host owns the window, input devices, audio output and presentation
//! only. Everything else — title loop, menus, levels, status bar, automap,
//! intermission, finale, sound logic — runs in `doom_game::Session` at a
//! fixed 35 Hz (`FixedUpdate`). Every frame the session composes its
//! 320x200 palette-indexed screen, which is uploaded to a texture and
//! scaled to 4:3 (Doom's pixels are 20% taller than wide). Audio is one
//! endless Bevy audio source pulling from a `doom_audio::Mixer` (OPL music
//! + 8 sfx channels) that the session's commands drive.
//!
//! Controls (vanilla defaults plus WASD and mouse look):
//! - Move: W/S or Up/Down, strafe A/D or , and ., turn Left/Right or mouse,
//!   Shift run, Alt + turn strafes.
//! - Fire: Ctrl or left mouse; secondary fire (mod weapons): right mouse
//!   or Z. Use: Space or E. Weapon slots: 1-9 and 0.
//! - Look: mouse (freelook), PageUp/PageDown, End/middle mouse centres.
//! - Esc menu, Tab automap, Pause pause, -/= view size, F1-F11 as vanilla
//!   (F11 = gamma).
//! - Alt+Enter fullscreen, F12 / PrintScreen screenshot, \ toggle
//!   freelook.
//! - ` (backtick / tilde) opens the developer console (`help` lists
//!   commands). `skate 1` rides a skateboard: W push, S brake, A/D carve,
//!   arrows/mouse steer, hold and release Space to ollie.
//!
//! Settings are console variables (`doom_game::cvar`), loaded from
//! `config.toml` at startup and saved on exit.
//! - Vanilla cheats can be typed (iddqd, idkfa, idfa, idclip, idclevXY, iddt).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::audio::{AddAudioSource, Decodable, Source};
use bevy::image::ImageSampler;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::ButtonState;
use bevy::input::InputSystem;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorGrabMode, MonitorSelection, PrimaryWindow, WindowMode, WindowResolution};
use clap::Parser;
use doom_audio::Mixer;
use doom_game::{png, InputState, Key, RenderMode, Session, SessionConfig, TICRATE};
use doom_render::{SCREENHEIGHT, SCREENWIDTH};
use doom_sim::Skill;

#[derive(Parser, Debug)]
#[command(about = "Doom engine (Rust + Bevy)")]
struct Args {
    /// IWAD path. Default: WADS/doom1.wad, doom1.wad, or $DOOMWADDIR/doom1.wad.
    #[arg(long)]
    iwad: Option<PathBuf>,
    /// PWADs loaded on top of the IWAD, in order.
    #[arg(long, num_args = 1..)]
    pwad: Vec<PathBuf>,
    /// Start a game on this map (E1M1, MAP01, "1 1" ...) instead of the
    /// title screen.
    #[arg(long)]
    map: Option<String>,
    /// 1 = I'm too young to die ... 5 = Nightmare! (with --map).
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=5))]
    skill: u8,
    #[arg(long)]
    nomonsters: bool,
    #[arg(long)]
    fast: bool,
    #[arg(long)]
    respawn: bool,
    /// Play back a demo (lump name like DEMO1, or a .lmp path).
    #[arg(long)]
    playdemo: Option<String>,
    /// Window size as a multiple of 320x240.
    #[arg(long, default_value_t = 3)]
    scale: u32,
    /// Start in borderless fullscreen.
    #[arg(long)]
    fullscreen: bool,
    /// Scale the 320x200 frame with plain nearest-neighbour filtering. The
    /// default is Chocolate Doom's method: integer upscale, then a smooth
    /// resize to 4:3, so every Doom pixel row has the same height.
    #[arg(long)]
    sharp: bool,
    /// Mouse sensitivity multiplier on top of the menu setting
    /// (1.0 = vanilla units per mouse count).
    #[arg(long, default_value_t = 1.0)]
    sensitivity: f32,
    /// Vertical mouse walks forward/back (vanilla) instead of looking.
    #[arg(long)]
    mouse_move: bool,
    /// Invert vertical mouse look.
    #[arg(long)]
    invert_mouse: bool,
    /// Start in the vanilla presentation: Classic render mode, freelook
    /// off (overrides the config file for this run).
    #[arg(long)]
    classic: bool,
    /// Turn freelook on for this run (`cl_freelook 1`).
    #[arg(long)]
    freelook: bool,
    /// Settings file (console variables).
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,
    /// View size 3..=11 as in the vanilla menu (10 = status bar, 11 = full
    /// screen). Default: the menu default.
    #[arg(long)]
    screen_size: Option<i32>,
    #[arg(long)]
    low_detail: bool,
    /// Disable sound and music.
    #[arg(long)]
    nosound: bool,
    /// Resource folder (skateboard model). Default: a `RESOURCES` folder
    /// near the working directory or the executable.
    #[arg(long)]
    resources: Option<PathBuf>,
}

fn find_iwad(arg: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = arg {
        return Some(p);
    }
    let names = ["doom1.wad", "doom.wad", "doom2.wad", "doomu.wad", "plutonia.wad", "tnt.wad"];
    let mut dirs = vec![PathBuf::from("WADS"), PathBuf::from(".")];
    if let Some(dir) = std::env::var_os("DOOMWADDIR") {
        dirs.push(PathBuf::from(dir));
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.exists())
}

const SAMPLE_RATE: u32 = 44_100;
/// Sfx channels (`snd_channels`).
/// Sound channels: as many as the game's sound system uses.
const SFX_CHANNELS: usize = doom_game::sound::NUM_CHANNELS;

fn main() {
    let args = Args::parse();
    let Some(iwad) = find_iwad(args.iwad.clone()) else {
        eprintln!("no IWAD found: pass --iwad <path> (or put doom1.wad in WADS/)");
        std::process::exit(1);
    };
    let cfg = SessionConfig {
        iwad,
        pwads: args.pwad.clone(),
        map: args.map.clone(),
        skill: [Skill::Baby, Skill::Easy, Skill::Medium, Skill::Hard, Skill::Nightmare][args.skill as usize - 1],
        nomonsters: args.nomonsters,
        fast: args.fast,
        respawn: args.respawn,
        demo: args.playdemo.clone(),
        render_mode: RenderMode::Classic,
        resources: args.resources.clone(),
    };
    let mut session = match Session::new(&cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    for w in session.warnings.iter().take(20) {
        eprintln!("warning: {w}");
    }
    for w in session.cvars.load_file(&args.config) {
        eprintln!("config: {w}");
    }
    // Command-line settings win over the config file.
    if args.classic {
        session.set_render_mode(RenderMode::Classic);
        session.set_freelook(false);
    }
    if args.freelook {
        session.set_freelook(true);
    }
    let cvars = session.cvars.clone();
    if args.screen_size.is_some() || args.low_detail {
        let size = args.screen_size.unwrap_or(session.settings.screenblocks);
        session.set_view(size, args.low_detail);
    }
    let mixer = (!args.nosound).then(|| Arc::new(Mutex::new(Mixer::new(SAMPLE_RATE, SFX_CHANNELS, session.genmidi()))));

    let scale = args.scale.max(1) as f32;
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "doom_bevy".into(),
                    resolution: WindowResolution::new(320.0 * scale, 240.0 * scale),
                    mode: if args.fullscreen {
                        WindowMode::BorderlessFullscreen(MonitorSelection::Current)
                    } else {
                        WindowMode::Windowed
                    },
                    ..default()
                }),
                ..default()
            })
            // The framebuffer is upscaled with nearest filtering.
            .set(ImagePlugin::default_nearest()),
    )
    .add_audio_source::<DoomAudio>()
    .insert_resource(ClearColor(Color::BLACK))
    .insert_resource(Time::<Fixed>::from_hz(TICRATE as f64))
    .insert_resource(Game(session))
    .insert_resource(HostSettings {
        sensitivity: args.sensitivity,
        mouse_move: args.mouse_move,
        invert_mouse: args.invert_mouse,
        smooth: !args.sharp,
    })
    .insert_resource(AudioOut(mixer))
    .init_resource::<PendingInput>()
    .add_systems(Startup, (setup, start_audio))
    .add_systems(PreUpdate, gather_input.after(InputSystem))
    .add_systems(FixedUpdate, run_tic)
    .add_systems(Update, (host_keys, mouse_grab, present, fit_to_window, update_title, quit_when_done))
    .run();
    if let Err(e) = cvars.save_file(&args.config) {
        eprintln!("saving {}: {e}", args.config.display());
    }
}

#[derive(Resource)]
struct Game(Session);

#[derive(Resource)]
struct HostSettings {
    sensitivity: f32,
    mouse_move: bool,
    invert_mouse: bool,
    /// Integer prescale + linear filtering (see `--sharp`).
    smooth: bool,
}

/// Input gathered every frame and consumed by the next tic, so short key
/// presses and mouse motion are not lost when a frame runs zero tics.
#[derive(Resource, Default)]
struct PendingInput {
    /// Mouse motion in vanilla units (sensitivity applied), not yet sent.
    mouse: Vec2,
    /// Weapon slot key (0..=9) pressed since the last tic.
    weapon_slot: Option<u8>,
}

/// The frame texture: the 320x200 frame replicated `scale` times in each
/// direction (see [`present`]).
#[derive(Resource)]
struct Framebuffer {
    image: Handle<Image>,
    scale: u32,
    rgba: Vec<u8>,
}

/// Largest integer prescale (1920x1200 texture).
const MAX_PRESCALE: u32 = 6;

#[derive(Component)]
struct Screen;

// ---- audio ------------------------------------------------------------------

#[derive(Resource)]
struct AudioOut(Option<Arc<Mutex<Mixer>>>);

/// The game's whole audio output as one endless Bevy audio source.
#[derive(Asset, TypePath)]
struct DoomAudio(Arc<Mutex<Mixer>>);

/// Pulls stereo samples from the mixer in small blocks (one lock per block).
struct DoomAudioDecoder {
    mixer: Arc<Mutex<Mixer>>,
    buf: Vec<f32>,
    pos: usize,
}

/// Frames per block: ~5.8 ms at 44.1 kHz, small enough to keep sfx latency
/// low and large enough to keep locking cheap.
const BLOCK_FRAMES: usize = 256;

impl Iterator for DoomAudioDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos == self.buf.len() {
            self.buf.resize(BLOCK_FRAMES * 2, 0.0);
            match self.mixer.lock() {
                Ok(mut m) => m.render(&mut self.buf),
                Err(_) => self.buf.fill(0.0),
            }
            self.pos = 0;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for DoomAudioDecoder {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Decodable for DoomAudio {
    type DecoderItem = f32;
    type Decoder = DoomAudioDecoder;

    fn decoder(&self) -> Self::Decoder {
        DoomAudioDecoder { mixer: self.0.clone(), buf: Vec::new(), pos: 0 }
    }
}

fn start_audio(mut commands: Commands, out: Res<AudioOut>, mut assets: ResMut<Assets<DoomAudio>>) {
    if let Some(mixer) = &out.0 {
        commands.spawn(AudioPlayer(assets.add(DoomAudio(mixer.clone()))));
    }
}

/// Hand the session's sound commands to the mixer.
fn flush_audio(game: &mut Session, out: &AudioOut) {
    let cmds = game.take_audio_commands();
    if let Some(mixer) = &out.0 {
        if let Ok(mut m) = mixer.lock() {
            for c in cmds {
                m.apply(c);
            }
        }
    }
}

// ---- setup / input ------------------------------------------------------------

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, settings: Res<HostSettings>) {
    commands.spawn(Camera2d);
    let mut image = Image::new_fill(
        Extent3d { width: SCREENWIDTH as u32, height: SCREENHEIGHT as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        // PLAYPAL values are sRGB-encoded.
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = if settings.smooth { ImageSampler::linear() } else { ImageSampler::nearest() };
    let handle = images.add(image);
    commands.spawn((Sprite::from_image(handle.clone()), Screen));
    commands.insert_resource(Framebuffer { image: handle, scale: 1, rgba: vec![0; SCREENWIDTH * SCREENHEIGHT * 4] });
}

/// The session's key for a Bevy key code (None: not a game key).
fn game_key(k: KeyCode) -> Option<Key> {
    use KeyCode::*;
    let letters = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR,
        KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    let fkeys = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11];
    if let Some(i) = letters.iter().position(|&l| l == k) {
        return Some(Key::Char((b'a' + i as u8) as char));
    }
    if let Some(i) = digits.iter().position(|&d| d == k) {
        return Some(Key::Char((b'0' + i as u8) as char));
    }
    if let Some(i) = fkeys.iter().position(|&f| f == k) {
        return Some(Key::F(i as u8 + 1));
    }
    Some(match k {
        ArrowUp => Key::Up,
        ArrowDown => Key::Down,
        ArrowLeft => Key::Left,
        ArrowRight => Key::Right,
        Enter | NumpadEnter => Key::Enter,
        Escape => Key::Escape,
        Backspace => Key::Backspace,
        Tab => Key::Tab,
        Pause => Key::Pause,
        Minus | NumpadSubtract => Key::Minus,
        Equal | NumpadAdd => Key::Equals,
        Backquote => Key::Grave,
        Delete => Key::Delete,
        Home => Key::Home,
        End => Key::End,
        PageUp => Key::PageUp,
        PageDown => Key::PageDown,
        Space => Key::Char(' '),
        _ => return None,
    })
}

/// Radians of turn per vanilla mouse unit: `angleturn -= mousex * 8`, i.e.
/// 8 << 16 BAM. Freelook pitch uses the same rate as yaw.
const RADIANS_PER_MOUSE_UNIT: f32 = 8.0 * 65536.0 / 4294967296.0 * std::f32::consts::TAU;
/// Keyboard look speed (PageUp/PageDown), radians per second.
const KEY_LOOK_SPEED: f32 = 1.6;

/// Runs after Bevy's input systems each frame: key events go to the session
/// right away (like vanilla's event queue), mouse motion is accumulated for
/// the next tic, and pitch (presentation only) is applied per frame.
#[allow(clippy::too_many_arguments)]
fn gather_input(
    mut pending: ResMut<PendingInput>,
    mut game: ResMut<Game>,
    mut keys_ev: EventReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    settings: Res<HostSettings>,
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let game = &mut game.0;
    for ev in keys_ev.read() {
        let Some(key) = game_key(ev.key_code).or_else(|| {
            // Keys the game ignores can still type into the console.
            (game.console_open() && ev.state == ButtonState::Pressed && ev.text.is_some()).then_some(Key::Char('\0'))
        }) else {
            continue;
        };
        // Alt+Enter belongs to the host (fullscreen).
        if key == Key::Enter && (keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight)) {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => {
                // Auto-repeat only navigates menus and the console.
                if ev.repeat && !game.wants_key_repeat() {
                    continue;
                }
                if !ev.repeat && !game.menu_open() && !game.console_open() {
                    // Keys 1-9 and 0 are weapon slots 1-9 and 0.
                    if let Key::Char(c @ '0'..='9') = key {
                        pending.weapon_slot = Some(c as u8 - b'0');
                    }
                }
                let was_open = game.console_open();
                game.key_down(key);
                // Typed characters go to the console as text (with shift,
                // layout and repeat applied by the OS).
                if was_open && game.console_open() && key != Key::Grave {
                    if let Some(text) = &ev.text {
                        game.text_input(text);
                    }
                }
            }
            ButtonState::Released => game.key_up(key),
        }
    }

    let grabbed = windows.single().is_ok_and(|w| w.cursor_options.grab_mode != CursorGrabMode::None);
    if grabbed && game.wants_mouse() {
        // Vanilla: ev->data2 * (mouseSensitivity + 5) / 10.
        let sens = (game.settings.mouse_sensitivity + 5) as f32 / 10.0 * settings.sensitivity;
        let d = motion.delta * sens;
        pending.mouse.x += d.x;
        // Without freelook the mouse walks, as in vanilla (not on the
        // skateboard: there forward is pushing).
        if game.controller_is_skate() {
        } else if settings.mouse_move || !game.freelook() {
            pending.mouse.y += d.y;
        } else {
            let dy = if settings.invert_mouse { d.y } else { -d.y };
            game.look(dy * RADIANS_PER_MOUSE_UNIT);
        }
    }
    if game.wants_mouse() {
        let dt = time.delta_secs();
        if keys.pressed(KeyCode::PageUp) {
            game.look(KEY_LOOK_SPEED * dt);
        }
        if keys.pressed(KeyCode::PageDown) {
            game.look(-KEY_LOOK_SPEED * dt);
        }
        if keys.just_pressed(KeyCode::End) || mouse.just_pressed(MouseButton::Middle) {
            game.center_view();
        }
    }
}

fn run_tic(
    mut game: ResMut<Game>,
    mut pending: ResMut<PendingInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    audio: Res<AudioOut>,
) {
    let session = &mut game.0;
    let k = |c: KeyCode| keys.pressed(c);
    // Arrows pan the automap when it isn't following the player.
    let arrows = !session.captures_arrows();
    let ka = |c: KeyCode| arrows && keys.pressed(c);
    let alt = k(KeyCode::AltLeft) || k(KeyCode::AltRight);
    // Whole vanilla units go out; fractions carry to the next tic.
    let m = pending.mouse;
    let (mouse_x, mouse_y) = (m.x.trunc() as i32, m.y.trunc() as i32);
    pending.mouse = m - Vec2::new(mouse_x as f32, mouse_y as f32);
    let grabbed = session.wants_mouse();
    // On the skateboard Space is the ollie; E (or right mouse) still uses.
    let skate = session.controller_is_skate();
    let input = InputState {
        forward: k(KeyCode::KeyW) || ka(KeyCode::ArrowUp),
        backward: k(KeyCode::KeyS) || ka(KeyCode::ArrowDown),
        turn_left: ka(KeyCode::ArrowLeft),
        turn_right: ka(KeyCode::ArrowRight),
        strafe_left: k(KeyCode::KeyA) || k(KeyCode::Comma),
        strafe_right: k(KeyCode::KeyD) || k(KeyCode::Period),
        strafe: alt,
        speed: k(KeyCode::ShiftLeft) || k(KeyCode::ShiftRight),
        fire: k(KeyCode::ControlLeft) || k(KeyCode::ControlRight) || (grabbed && mouse.pressed(MouseButton::Left)),
        use_: (!skate && k(KeyCode::Space)) || k(KeyCode::KeyE),
        jump: skate && k(KeyCode::Space),
        weapon_slot: pending.weapon_slot.take(),
        // Secondary fire: right mouse or the Z key (mods with AltFire).
        alt_fire: k(KeyCode::KeyZ) || (grabbed && mouse.pressed(MouseButton::Right)),
        mouse_x,
        // Screen y grows downwards; vanilla moves forward on mouse-up.
        mouse_y: -mouse_y,
        ..default()
    };
    session.tick(&input);
    flush_audio(session, &audio);
}

fn host_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut game: ResMut<Game>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let session = &mut game.0;
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    if alt && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter)) {
        if let Ok(mut window) = windows.single_mut() {
            window.mode = match window.mode {
                WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
                _ => WindowMode::Windowed,
            };
        }
    }
    if keys.just_pressed(KeyCode::F12) || keys.just_pressed(KeyCode::PrintScreen) {
        save_screenshot(session);
    }
    if keys.just_pressed(KeyCode::Backslash) {
        let on = !session.freelook();
        session.set_freelook(on);
        session.show_message(if on { "FREELOOK ON" } else { "FREELOOK OFF" });
    }
}

/// F12: the 320x200 frame as `screenshots/doomNNN.png`.
fn save_screenshot(session: &mut Session) {
    let mut rgba = vec![0u8; SCREENWIDTH * SCREENHEIGHT * 4];
    session.render_rgba(&mut rgba);
    let dir = PathBuf::from("screenshots");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("screenshot: {e}");
        return;
    }
    let Some(path) = (0..1000).map(|i| dir.join(format!("doom{i:03}.png"))).find(|p| !p.exists()) else { return };
    match std::fs::write(&path, png::encode_rgba(SCREENWIDTH, SCREENHEIGHT, &rgba)) {
        Ok(()) => session.show_message("SCREEN SHOT"),
        Err(e) => eprintln!("screenshot {}: {e}", path.display()),
    }
}

/// Capture the mouse while a level is being played; free it for menus,
/// pauses, demos and when the window loses focus.
fn mouse_grab(game: Res<Game>, mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let Ok(mut window) = windows.single_mut() else { return };
    let want = game.0.wants_mouse() && window.focused;
    let grabbed = window.cursor_options.grab_mode != CursorGrabMode::None;
    if want != grabbed {
        window.cursor_options.grab_mode = if want { CursorGrabMode::Locked } else { CursorGrabMode::None };
        window.cursor_options.visible = !want;
    }
}

// ---- presentation -------------------------------------------------------------

/// Render the current game state into the frame texture.
///
/// Doom pixels are shown 1.2 times taller than wide, so the 200 rows never
/// map to whole screen pixels: plain nearest filtering makes some rows one
/// pixel taller than others, which breaks up the evenly spaced light bands
/// on floors and walls. Like Chocolate Doom, the frame is first replicated
/// to the smallest integer multiple at least as large as the on-screen
/// rectangle, then drawn with linear filtering: every row stays the same
/// height and only the edges between rows are blended.
#[allow(clippy::too_many_arguments)]
fn present(
    mut game: ResMut<Game>,
    mut fb: ResMut<Framebuffer>,
    mut images: ResMut<Assets<Image>>,
    audio: Res<AudioOut>,
    settings: Res<HostSettings>,
    windows: Query<&Window, With<PrimaryWindow>>,
    fixed: Res<Time<Fixed>>,
    time: Res<Time>,
) {
    let fb = &mut *fb;
    // Where this frame falls between game tics, and how long it took: the
    // skate camera and rider are drawn smoothly at the display rate.
    game.0.set_frame_time(fixed.overstep_fraction(), time.delta_secs());
    game.0.render_rgba(&mut fb.rgba);
    // Drawing can start sounds (finale cast call).
    flush_audio(&mut game.0, &audio);

    let scale = match (settings.smooth, windows.single()) {
        (true, Ok(w)) => {
            let unit = (w.physical_width() as f32 / 4.0).min(w.physical_height() as f32 / 3.0);
            let rows = (unit * 3.0 / SCREENHEIGHT as f32).ceil() as u32;
            let cols = (unit * 4.0 / SCREENWIDTH as f32).ceil() as u32;
            rows.max(cols).clamp(1, MAX_PRESCALE)
        }
        _ => 1,
    };
    let Some(image) = images.get_mut(&fb.image) else { return };
    if scale != fb.scale {
        fb.scale = scale;
        image.resize(Extent3d { width: SCREENWIDTH as u32 * scale, height: SCREENHEIGHT as u32 * scale, depth_or_array_layers: 1 });
    }
    let Some(data) = image.data.as_mut() else { return };
    upscale(&fb.rgba, data, scale as usize);
}

/// Replicate each RGBA pixel of the 320x200 frame into an `n`x`n` block.
fn upscale(src: &[u8], dst: &mut [u8], n: usize) {
    if n == 1 {
        dst.copy_from_slice(src);
        return;
    }
    let row_bytes = SCREENWIDTH * n * 4;
    for (y, src_row) in src.chunks_exact(SCREENWIDTH * 4).enumerate() {
        let first = y * n * row_bytes;
        let out = &mut dst[first..first + row_bytes];
        for (px, block) in src_row.chunks_exact(4).zip(out.chunks_exact_mut(n * 4)) {
            for o in block.chunks_exact_mut(4) {
                o.copy_from_slice(px);
            }
        }
        for r in 1..n {
            dst.copy_within(first..first + row_bytes, first + r * row_bytes);
        }
    }
}

/// Scale the 320x200 framebuffer to the largest 4:3 rectangle that fits:
/// Doom's pixels are 20% taller than wide on the CRTs it was made for.
fn fit_to_window(windows: Query<&Window, With<PrimaryWindow>>, mut screen: Query<&mut Sprite, With<Screen>>) {
    let (Ok(window), Ok(mut sprite)) = (windows.single(), screen.single_mut()) else { return };
    let unit = (window.width() / 4.0).min(window.height() / 3.0);
    let size = Vec2::new(unit * 4.0, unit * 3.0);
    if sprite.custom_size != Some(size) {
        sprite.custom_size = Some(size);
    }
}

fn update_title(game: Res<Game>, mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let Ok(mut window) = windows.single_mut() else { return };
    let g = &game.0;
    let title = format!(
        "doom_bevy - {}{}{}",
        g.map_name(),
        if g.is_demo() { " (demo)" } else { "" },
        if g.render_mode == RenderMode::GZDoomClean { " [GZDoomClean]" } else { "" }
    );
    if window.title != title {
        window.title = title;
    }
}

fn quit_when_done(game: Res<Game>, mut exit: EventWriter<AppExit>) {
    if game.0.quit_requested() {
        exit.write(AppExit::Success);
    }
}
