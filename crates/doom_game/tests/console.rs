//! Developer console: command registry, execution from code, keyboard
//! editing, completion and errors. Skipped when the IWAD is absent.

use std::path::PathBuf;

use doom_game::console::LineKind;
use doom_game::{cvar, InputState, Key, Session, SessionConfig};
use doom_sim::player::{PlayerState, NUMWEAPONS};

fn iwad() -> Option<PathBuf> {
    let p = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    p.exists().then_some(p).or_else(|| {
        eprintln!("skipping: no IWAD");
        None
    })
}

fn session(map: &str) -> Option<Session> {
    let mut cfg = SessionConfig::new(iwad()?);
    cfg.map = Some(map.to_string());
    Some(Session::new(&cfg).unwrap())
}

fn idle(s: &mut Session, tics: usize) {
    for _ in 0..tics {
        s.tick(&InputState::default());
    }
}

#[test]
fn give_and_cheat_commands() {
    let Some(mut s) = session("E1M1") else { return };
    idle(&mut s, 2);
    assert_eq!(s.exec("give all").unwrap(), "gave everything");
    let h = s.hud();
    assert_eq!(h.weapons_owned, [true; NUMWEAPONS]);
    assert_eq!(h.keys, [true; 6]);
    assert_eq!(h.armor, 200);
    assert!(s.exec("god").unwrap().contains("ON"));
    assert!(s.hud().god);
    s.exec("god 0").unwrap();
    assert!(!s.hud().god);
    s.exec("noclip 1; notarget 1").unwrap();
    assert!(s.hud().noclip);
    assert_ne!(s.world.console_player().cheats & doom_sim::player::cheat::NOTARGET, 0);

    // Item variants.
    s.world.players[0].ammo[1] = 0;
    assert_eq!(s.exec("give shells 7").unwrap(), "shells: 7");
    s.world.players[0].cards = [false; 6];
    assert!(s.exec("give key blue").unwrap().contains("bluecard"));
    assert!(s.hud().keys[0] && s.hud().keys[3], "blue card and skull");
    s.exec("give health 1").unwrap();
    // A pickup class handed over as if touched.
    let p = &mut s.world.players[0];
    p.health = 40;
    let mo = p.mo.unwrap();
    s.world.mo_mut(mo).health = 40;
    assert_eq!(s.exec("give Medikit").unwrap().to_ascii_lowercase(), "gave medikit");
    assert_eq!(s.hud().health, 65);
    assert!(s.exec("give bogusthing").unwrap_err().starts_with("no item called"));
}

#[test]
fn errors_name_the_problem() {
    let Some(mut s) = session("E1M1") else { return };
    let e = s.exec("gvie all").unwrap_err();
    assert!(e.contains("Did you mean") && e.contains("give"), "{e}");
    assert!(s.exec("god 7").unwrap_err().contains("expected 0 or 1"));
    assert!(s.exec("map nowhere").unwrap_err().contains("bad map name"));
    assert!(s.exec("map E9M1").is_err(), "no such map");
    assert!(s.exec("skate_max_speed fast").unwrap_err().contains("bad value"));
    assert!(s.exec("summon NotAMonster").unwrap_err().contains("no actor class"));
    assert!(s.exec("warp").unwrap_err().contains("usage"));
}

#[test]
fn map_flow_commands() {
    let Some(mut s) = session("E1M1") else { return };
    s.exec("map e1m2").unwrap();
    assert_eq!(s.map_name(), "E1M2");
    s.exec("give all").unwrap();
    s.exec("nextmap").unwrap();
    assert_eq!(s.map_name(), "E1M3");
    assert!(s.hud().weapons_owned[5], "nextmap keeps the inventory");
    s.exec("restart").unwrap();
    assert_eq!(s.map_name(), "E1M3");
    assert!(!s.hud().weapons_owned[5], "restart is a pistol start");
    s.exec("warp e1m5").unwrap();
    assert_eq!(s.map_name(), "E1M5");
    // Coordinates: the E1M1 start is at (1056, -3616).
    s.exec("map e1m1").unwrap();
    s.exec("warp 1056 -3520").unwrap();
    let mo = s.world.console_player().mo.unwrap();
    assert_eq!(s.world.mo(mo).y.0 >> 16, -3520);
}

#[test]
fn player_state_commands() {
    let Some(mut s) = session("E1M1") else { return };
    idle(&mut s, 2);
    s.exec("suicide").unwrap();
    assert!(s.hud().dead);
    assert!(s.exec("kill").unwrap_err().contains("already dead"));
    idle(&mut s, 10);
    s.exec("respawn").unwrap();
    assert_eq!(s.world.console_player().playerstate, PlayerState::Live);
    assert_eq!(s.hud().health, 100);
    idle(&mut s, 5);
    assert!(!s.hud().dead);
}

#[test]
fn summon_and_killall() {
    let Some(mut s) = session("E1M1") else { return };
    idle(&mut s, 2);
    let imps = |s: &Session| s.world.thinkers.mobjs().filter(|(_, m)| m.mobj_type == doom_sim::info::MT_TROOP && m.health > 0).count();
    let before = imps(&s);
    assert_eq!(s.exec("summon DoomImp 2").unwrap().to_ascii_lowercase(), "spawned 2 doomimp");
    assert_eq!(imps(&s), before + 2);
    s.exec("spawn 3001").unwrap();
    assert_eq!(imps(&s), before + 3, "editor numbers work too");
    let out = s.exec("killall").unwrap();
    let n: usize = out.trim_start_matches("killed ").parse().unwrap();
    assert!(n >= before + 3, "{out}");
    assert_eq!(imps(&s), 0);
    assert_eq!(s.exec("killall").unwrap(), "killed 0");
}

#[test]
fn cvars_through_the_console() {
    let Some(mut s) = session("E1M1") else { return };
    assert_eq!(s.exec("skate_max_speed 500").unwrap(), "skate_max_speed = 500.00");
    assert_eq!(s.cvars.get(cvar::SKATE_MAX_SPEED), 500.0);
    // Debug switches toggle when named alone; saved settings only show.
    assert_eq!(s.exec("show_collision").unwrap(), "show_collision = on");
    assert_eq!(s.exec("show_collision").unwrap(), "show_collision = off");
    assert!(s.exec("cl_freelook").unwrap().starts_with("cl_freelook = off"));
    assert!(!s.freelook());
    assert!(s.exec("help give").unwrap().contains("Give items"));
    assert!(s.exec("cvarlist skate_grind").unwrap().contains("skate_grind_assist"));
    // Timescale: two game tics per host tic.
    s.exec("timescale 2").unwrap();
    let t0 = s.world.leveltime;
    idle(&mut s, 10);
    assert_eq!(s.world.leveltime - t0, 20);
    s.exec("timescale 0.5").unwrap();
    let t0 = s.world.leveltime;
    idle(&mut s, 10);
    assert_eq!(s.world.leveltime - t0, 5);
    s.exec("pause").unwrap();
    let t0 = s.world.leveltime;
    idle(&mut s, 5);
    assert_eq!(s.world.leveltime, t0);
    s.exec("unpause; timescale 1").unwrap();
    idle(&mut s, 5);
    assert_eq!(s.world.leveltime, t0 + 5);
}

#[test]
fn console_keyboard() {
    let Some(mut s) = session("E1M1") else { return };
    assert!(!s.console_open());
    s.key_down(Key::Grave);
    assert!(s.console_open());
    assert!(!s.wants_mouse());
    s.text_input("giv");
    s.key_down(Key::Tab);
    assert_eq!(s.console().input, "give ");
    s.text_input("al");
    s.key_down(Key::Tab);
    // "all" and "allmap" share the prefix: extended and listed.
    assert_eq!(s.console().input, "give all");
    assert!(s.console().lines().back().unwrap().text.contains("allmap"));
    s.key_down(Key::Enter);
    let lines: Vec<_> = s.console().lines().iter().map(|l| (l.kind, l.text.clone())).collect();
    assert!(lines.contains(&(LineKind::Input, "] give all".into())), "{lines:?}");
    assert!(lines.contains(&(LineKind::Output, "gave everything".into())), "{lines:?}");
    // Errors go to the scrollback as errors.
    s.text_input("frobnicate");
    s.key_down(Key::Enter);
    assert_eq!(s.console().lines().back().unwrap().kind, LineKind::Error);
    s.key_down(Key::Up);
    assert_eq!(s.console().input, "frobnicate");
    s.key_down(Key::Up);
    assert_eq!(s.console().input, "give all");
    // Map name completion.
    s.key_down(Key::End);
    for _ in 0..8 {
        s.key_down(Key::Backspace);
    }
    s.text_input("map e1m");
    s.key_down(Key::Tab);
    assert!(s.console().lines().back().unwrap().text.contains("e1m9"), "lists the candidates");
    // The game does not see keys while the console is open.
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_rgba(&mut rgba);
    s.key_down(Key::Escape);
    assert!(!s.console_open());
    assert!(!s.menu_open(), "escape closed the console, not opening the menu");
}

#[test]
fn custom_commands_from_code() {
    let Some(mut s) = session("E1M1") else { return };
    s.commands_mut().add("double", "<n>", "Twice n.", |_, a| {
        let n: i32 = a.first().ok_or("usage: double <n>")?.parse().map_err(|_| "not a number".to_string())?;
        Ok((n * 2).to_string())
    });
    assert_eq!(s.exec("double 21").unwrap(), "42");
    assert_eq!(s.exec("double x").unwrap_err(), "not a number");
    assert_eq!(s.exec("echo a; echo b").unwrap(), "a\nb");
}
