//! Headless session tests: title loop, menus, level flow through the
//! intermission and finale, death, cheats, automap, demo playback, sound and
//! frame output. Skipped when the IWAD is absent.

use std::path::PathBuf;

use doom_audio::AudioCommand;
use doom_game::{GameState, InputState, Key, MapRef, RenderMode, Session, SessionConfig, ViewPath, MAX_PITCH};
use doom_sim::player::{PlayerState, Weapon};

fn iwad() -> Option<PathBuf> {
    let p = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    p.exists().then_some(p).or_else(|| {
        eprintln!("skipping: no IWAD");
        None
    })
}

fn session(map: Option<&str>) -> Option<Session> {
    let mut cfg = SessionConfig::new(iwad()?);
    cfg.map = map.map(str::to_string);
    Some(Session::new(&cfg).unwrap())
}

fn idle(s: &mut Session, tics: usize) {
    for _ in 0..tics {
        s.tick(&InputState::default());
    }
}

/// Press use until the intermission hands back to a level (or 2000 tics).
fn through_intermission(s: &mut Session) {
    let press = InputState { use_: true, ..Default::default() };
    let idle = InputState::default();
    for i in 0..2000 {
        if s.state() != GameState::Intermission {
            return;
        }
        s.tick(if i % 4 == 0 { &press } else { &idle });
    }
}

#[test]
fn title_screen_menu_new_game() {
    let Some(mut s) = session(None) else { return };
    assert_eq!(s.state(), GameState::DemoScreen);
    idle(&mut s, 5);
    // Escape opens the menu; New Game -> episode -> skill (default HMP).
    s.key_down(Key::Escape);
    assert!(s.menu_open());
    s.key_down(Key::Enter);
    s.key_down(Key::Enter);
    s.key_down(Key::Enter);
    assert!(!s.menu_open());
    assert_eq!(s.state(), GameState::Level);
    assert_eq!(s.map(), MapRef::new(1, 1));
    assert_eq!(s.world.params.skill, doom_sim::Skill::Medium);
    idle(&mut s, 5);
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_rgba(&mut rgba);
    assert!(rgba.chunks(4).all(|p| p[3] == 255));
    let lit = rgba.chunks(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count();
    assert!(lit > 320 * 200 / 2, "frame mostly dark: {lit} lit pixels");
    let hud = s.hud();
    assert_eq!((hud.health, hud.weapon, hud.ammo), (100, Weapon::Pistol, Some((50, 200))));
}

#[test]
fn walking_and_firing_runs() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    let start = s.hud().ammo.unwrap().0;
    let inp = InputState { forward: true, fire: true, turn_left: true, ..Default::default() };
    for _ in 0..200 {
        s.tick(&inp);
        s.render_indexed();
    }
    assert!(s.hud().ammo.unwrap().0 < start, "pistol fired");
    assert!(!s.take_events().is_empty(), "sound events were produced");
}

#[test]
fn exit_carries_inventory_to_next_map() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    s.give_all();
    idle(&mut s, 2);
    s.exit_level();
    assert_eq!(s.state(), GameState::Intermission);
    through_intermission(&mut s);
    assert_eq!(s.state(), GameState::Level);
    assert_eq!(s.map(), MapRef::new(1, 2));
    idle(&mut s, 2);
    let hud = s.hud();
    assert_eq!(hud.armor, 200);
    assert!(hud.weapons_owned.iter().all(|&o| o));
    assert_eq!(hud.keys, [false; 6], "keys are cleared between levels");
    assert_eq!(hud.health, 100);
}

#[test]
fn secret_exit_and_return() {
    let Some(mut s) = session(Some("E1M3")) else { return };
    // Secret exit goes to E1M9, which returns to E1M4.
    s.world.gameexit = Some(doom_sim::world::GameExit::Secret);
    idle(&mut s, 1);
    through_intermission(&mut s);
    assert_eq!(s.map(), MapRef::new(1, 9));
    s.exit_level();
    through_intermission(&mut s);
    assert_eq!(s.map(), MapRef::new(1, 4));
}

#[test]
fn death_then_use_reloads_level() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    s.give_all();
    idle(&mut s, 2);
    let mo = s.world.console_player().mo.unwrap();
    doom_sim::inter::damage_mobj(&mut s.world, mo, None, None, 10_000);
    idle(&mut s, 40);
    assert!(s.hud().dead);
    s.tick(&InputState { use_: true, ..Default::default() });
    idle(&mut s, 2);
    let p = s.world.console_player();
    assert_eq!(p.playerstate, PlayerState::Live);
    assert_eq!(p.health, 100);
    assert!(!p.weaponowned[Weapon::Shotgun as usize], "reborn with pistol start");
    assert_eq!(s.map(), MapRef::new(1, 1));
}

#[test]
fn god_mode_survives_damage_and_warp_resets() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    s.toggle_god();
    let mo = s.world.console_player().mo.unwrap();
    // Damage >= 1000 (telefrag) ignores god mode in vanilla; stay below it.
    doom_sim::inter::damage_mobj(&mut s.world, mo, None, None, 999);
    idle(&mut s, 2);
    assert!(!s.hud().dead);
    s.give_all();
    s.warp("E2M1").unwrap();
    assert_eq!(s.map(), MapRef::new(2, 1));
    assert!(!s.hud().god, "warp starts a new game");
    assert!(!s.hud().weapons_owned[Weapon::Shotgun as usize]);
    assert!(s.warp("E9M9").is_err());
}

#[test]
fn demo_playback_ends() {
    let Some(iwad) = iwad() else { return };
    let mut cfg = SessionConfig::new(iwad);
    cfg.demo = Some("DEMO1".into());
    let mut s = Session::new(&cfg).unwrap();
    assert!(s.is_demo());
    let mut tics = 0;
    while !s.demo_finished() && tics < 10_000 {
        s.tick(&InputState::default());
        tics += 1;
    }
    assert!(s.demo_finished());
    // DEMO1 exits E1M5 near its end; on the next tic the title loop starts.
    assert!(tics > 1700, "{tics}");
    idle(&mut s, 2);
    assert_eq!(s.state(), GameState::DemoScreen);
}

#[test]
fn menu_pauses_the_game() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    idle(&mut s, 3);
    let t = s.world.leveltime;
    s.key_down(Key::Escape);
    idle(&mut s, 10);
    assert_eq!(s.world.leveltime, t, "no playsim tics while the menu is open");
    s.key_down(Key::Escape);
    idle(&mut s, 10);
    assert_eq!(s.world.leveltime, t + 10);
}

#[test]
fn automap_and_reveal_cheat() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    s.key_down(Key::Tab);
    assert!(s.automap_active());
    for c in "iddt".chars() {
        s.key_down(Key::Char(c));
    }
    idle(&mut s, 2);
    s.render_indexed();
    s.key_down(Key::Tab);
    assert!(!s.automap_active());
}

#[test]
fn episode_end_goes_to_finale() {
    let Some(mut s) = session(Some("E1M8")) else { return };
    s.exit_level();
    assert_eq!(s.state(), GameState::Finale, "Doom 1 map 8 skips the tally");
    idle(&mut s, 3000);
    s.render_indexed();
}

#[test]
fn audio_commands_for_music_and_sounds() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    let cmds = s.take_audio_commands();
    assert!(cmds.iter().any(|c| matches!(c, AudioCommand::PlayMusic { .. })), "level music starts");
    let fire = InputState { fire: true, ..Default::default() };
    // The pistol takes 16 tics to raise before it can fire.
    for _ in 0..30 {
        s.tick(&fire);
    }
    let cmds = s.take_audio_commands();
    assert!(cmds.iter().any(|c| matches!(c, AudioCommand::StartSound { .. })), "pistol sound");
    assert!(s.genmidi().is_some());
}

#[test]
fn palette_effects() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    assert_eq!(s.palette_index(), 0);
    let cp = s.world.params.consoleplayer;
    s.world.players[cp].damagecount = 20;
    assert_eq!(s.palette_index(), 1 + 3);
    s.world.players[cp].damagecount = 0;
    s.world.players[cp].bonuscount = 6;
    assert_eq!(s.palette_index(), 10);
}

#[test]
fn session_is_send_and_sync() {
    // Hosts (Bevy) keep the session in a resource.
    fn check<T: Send + Sync + 'static>() {}
    check::<Session>();
}

#[test]
fn warp_cheat() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    let mut m = doom_game::CheatMatcher::default();
    let cheat = "idclev15".chars().filter_map(|c| m.feed(c)).next().unwrap();
    s.apply_cheat(&cheat);
    assert_eq!(s.map(), MapRef::new(1, 5));
}

#[test]
fn freelook_pitch_changes_the_view() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    idle(&mut s, 20);
    assert_eq!(s.render_mode, RenderMode::Classic, "classic is the default");
    assert_eq!(s.view_path(), ViewPath::Columns);
    s.set_freelook(true);
    assert_eq!(s.view_path(), ViewPath::Perspective, "classic freelook uses true pitch");
    let level = s.render_indexed().to_vec();
    s.look(0.4);
    assert!((s.pitch() - 0.4).abs() < 1e-6);
    let up = s.render_indexed().to_vec();
    // Only the view window changes; the status bar (rows 168..200) stays.
    let view_diff = level[..320 * 168].iter().zip(&up[..320 * 168]).filter(|(a, b)| a != b).count();
    assert!(view_diff > 320 * 168 / 4, "pitch barely changed the view ({view_diff} px)");
    assert_eq!(level[320 * 168..], up[320 * 168..]);
    // Clamped, and centred again.
    s.look(10.0);
    assert!(s.pitch() <= MAX_PITCH + 1e-6);
    s.look(-20.0);
    assert!(s.pitch() >= -MAX_PITCH - 1e-6);
    s.center_view();
    assert_eq!(s.pitch(), 0.0);
    assert_eq!(s.render_indexed(), &level[..]);
    // Fixed horizon without freelook.
    s.set_freelook(false);
    s.look(0.3);
    assert_eq!(s.pitch(), 0.0);
}

#[test]
fn perspective_view_reveals_automap_lines() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    idle(&mut s, 2);
    s.render_indexed();
    let seen = s.renderer.mapped_lines.iter().filter(|&&m| m).count();
    assert!(seen > 10, "only {seen} lines mapped");
    // Not the whole level: hidden lines stay unmapped.
    assert!(seen < s.world.level.lines.len() / 2, "{seen} of {} lines mapped", s.world.level.lines.len());
}

#[test]
fn screen_melts_into_a_new_game() {
    let Some(mut s) = session(None) else { return };
    idle(&mut s, 2);
    let title = s.render_indexed().to_vec();
    assert!(!s.is_wiping(), "the first frame never melts");
    for k in [Key::Escape, Key::Enter, Key::Enter, Key::Enter] {
        s.key_down(k);
    }
    assert_eq!(s.state(), GameState::Level);
    // D_Display: the melt starts from the frame still on screen.
    assert_eq!(s.render_indexed(), &title[..]);
    assert!(s.is_wiping());
    let t = s.world.leveltime;
    idle(&mut s, 12);
    assert_eq!(s.world.leveltime, t, "no game tics during the melt");
    let mid = s.render_indexed().to_vec();
    let changed = mid.iter().zip(&title).filter(|(a, b)| a != b).count();
    assert!(changed > 320 * 200 / 10 && changed < 320 * 200, "mid-melt frame mixes both screens ({changed} px changed)");
    let mut tics = 12;
    while s.is_wiping() {
        s.tick(&InputState::default());
        tics += 1;
        assert!(tics < 60, "melt never finished");
    }
    idle(&mut s, 3);
    assert_eq!(s.world.leveltime, t + 3, "the game resumes after the melt");
}

#[test]
fn level_loads_melt_and_r_wipe_turns_it_off() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    s.render_indexed();
    assert!(!s.is_wiping());
    // Level -> level (G_DoLoadLevel forces a wipe).
    s.warp("E1M2").unwrap();
    s.render_indexed();
    assert!(s.is_wiping());
    // Turning the melt off ends the running one.
    s.cvars.set(doom_game::cvar::R_WIPE, false).unwrap();
    s.tick(&InputState::default());
    assert!(!s.is_wiping());
    s.warp("E1M3").unwrap();
    s.render_indexed();
    assert!(!s.is_wiping());
}

#[test]
fn cl_freelook_locks_the_pitch() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    s.look(0.3);
    assert_eq!(s.pitch(), 0.0, "freelook is off by default");
    s.cvars.set(doom_game::cvar::CL_FREELOOK, true).unwrap();
    s.look(0.3);
    assert!(s.pitch() > 0.0);
    // GZDoomClean keeps the pitch: the host renders it.
    s.set_render_mode(RenderMode::GZDoomClean);
    assert!(s.pitch() > 0.0);
    assert!((s.host_view().unwrap().camera.pitch - s.pitch()).abs() < 1e-6);
    s.cvars.set(doom_game::cvar::CL_FREELOOK, false).unwrap();
    s.look(0.3);
    assert_eq!(s.pitch(), 0.0, "turning freelook off levels the view");
    s.cvars.set(doom_game::cvar::CL_FREELOOK, true).unwrap();
    s.look(0.3);
    assert!(s.pitch() > 0.0);
}

#[test]
fn engine_options_menu_edits_cvars() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    // Main -> Options -> "Engine options" (hotkey x) -> first entry.
    for k in [Key::Escape, Key::Down, Key::Enter, Key::Char('x'), Key::Enter] {
        s.key_down(k);
    }
    s.render_indexed();
    // First entry: Render mode.
    s.key_down(Key::Right);
    assert_eq!(s.cvars.get(doom_game::cvar::R_RENDER_MODE), "GZDoomClean");
    assert_eq!(s.view_path(), ViewPath::Host, "render mode switches without a restart");
    s.key_down(Key::Left);
    assert_eq!(s.render_mode, RenderMode::Classic);
    s.key_down(Key::Down);
    assert!(!s.cvars.get(doom_game::cvar::CL_FREELOOK));
    s.key_down(Key::Enter);
    assert!(s.freelook(), "Enter toggles Freelook");
    s.key_down(Key::Down);
    s.key_down(Key::Enter);
    assert!(!s.autoaim(), "then Autoaim");
    s.render_indexed();
}

#[test]
fn settings_and_config_stay_in_sync() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    // Vanilla keys write through to the cvars...
    s.key_down(Key::F(5));
    assert!(s.cvars.get(doom_game::cvar::R_LOWDETAIL));
    s.key_down(Key::Minus);
    assert_eq!(s.cvars.get(doom_game::cvar::R_SCREENBLOCKS), 9);
    s.set_render_mode(RenderMode::GZDoomClean);
    let saved = s.cvars.to_config_string();
    // ...and a loaded config drives the session.
    let Some(mut s2) = session(Some("E1M1")) else { return };
    assert!(s2.cvars.load_config_str(&saved).is_empty());
    s2.tick(&InputState::default());
    assert_eq!(s2.render_mode, RenderMode::GZDoomClean);
    assert_eq!((s2.settings.screenblocks, s2.settings.low_detail), (9, true));
    assert_eq!(s2.renderer.viewheight, s.renderer.viewheight);
}

#[test]
fn host_overlay_leaves_the_view_window_transparent() {
    let Some(mut s) = session(Some("E1M1")) else { return };
    idle(&mut s, 20);
    s.set_render_mode(RenderMode::GZDoomClean);
    let view = s.host_view().expect("a level is on screen");
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_overlay_rgba(&mut rgba);
    let r = view.rect;
    let alpha = |x: i32, y: i32| rgba[((y * 320 + x) * 4 + 3) as usize];
    let (mut clear, mut total) = (0, 0);
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            total += 1;
            clear += (alpha(x, y) == 0) as usize;
        }
    }
    // The weapon covers part of the window; the rest is the host's.
    assert!(clear > total * 3 / 4 && clear < total, "{clear} of {total} view pixels transparent");
    assert!((r.y + r.height..200).all(|y| (0..320).all(|x| alpha(x, y) == 255)), "status bar is opaque");
    // The menu draws over the view.
    s.key_down(Key::Escape);
    let mut with_menu = vec![0u8; 320 * 200 * 4];
    s.render_overlay_rgba(&mut with_menu);
    let opaque = |b: &[u8]| b.chunks(4).filter(|p| p[3] == 255).count();
    assert!(opaque(&with_menu) > opaque(&rgba));
    // Outside a level the overlay is the whole frame.
    s.key_down(Key::Escape);
    s.key_down(Key::Tab);
    assert!(s.host_view().is_none(), "automap");
    s.render_overlay_rgba(&mut rgba);
    assert!(rgba.chunks(4).all(|p| p[3] == 255));
}
