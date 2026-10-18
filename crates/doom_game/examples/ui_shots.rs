//! Drive a session through the presentation states and save a PNG of each:
//! title, menus, level with status bar, small view with border, full-screen
//! HUD, automap, intermission, finale.
//!
//! cargo run -p doom_game --release --example ui_shots -- <iwad> <out dir>

use std::path::PathBuf;

use doom_game::{png, InputState, Key, Session, SessionConfig};

fn shot(s: &mut Session, dir: &PathBuf, name: &str) {
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_rgba(&mut rgba);
    std::fs::write(dir.join(format!("{name}.png")), png::encode_rgba(320, 200, &rgba)).unwrap();
    println!("{name}: state {:?}, map {}", s.state(), s.map_name());
}

fn tics(s: &mut Session, n: usize, input: &InputState) {
    for _ in 0..n {
        s.tick(input);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&dir).unwrap();
    let idle = InputState::default();
    let mut s = Session::new(&SessionConfig::new(&args[1])).unwrap();

    tics(&mut s, 5, &idle);
    shot(&mut s, &dir, "01_title");
    s.key_down(Key::Escape);
    tics(&mut s, 1, &idle);
    shot(&mut s, &dir, "02_main_menu");
    s.key_down(Key::Enter);
    shot(&mut s, &dir, "03_episode_menu");
    s.key_down(Key::Enter);
    shot(&mut s, &dir, "04_skill_menu");
    s.key_down(Key::Enter);
    let walk = InputState { forward: true, ..Default::default() };
    tics(&mut s, 30, &walk);
    shot(&mut s, &dir, "05_level_statusbar");

    s.key_down(Key::Escape);
    s.key_down(Key::Down);
    s.key_down(Key::Enter);
    shot(&mut s, &dir, "06_options_menu");
    s.key_down(Key::Escape);

    s.set_view(7, false);
    tics(&mut s, 2, &idle);
    shot(&mut s, &dir, "07_small_view_border");
    s.set_view(11, false);
    shot(&mut s, &dir, "08_fullscreen_hud");
    s.set_view(10, false);

    s.key_down(Key::Tab);
    tics(&mut s, 2, &idle);
    shot(&mut s, &dir, "09_automap");
    for c in "iddt".chars() {
        s.key_down(Key::Char(c));
    }
    tics(&mut s, 1, &idle);
    shot(&mut s, &dir, "10_automap_iddt");
    s.key_down(Key::Tab);

    // Pick something up for the pickup message and palette flash.
    s.give_all();
    tics(&mut s, 3, &idle);
    s.exit_level();
    tics(&mut s, 150, &idle);
    shot(&mut s, &dir, "11_intermission_stats");
    let use_ = InputState { use_: true, ..Default::default() };
    // Accelerate through the tally, then to the "entering" map.
    for _ in 0..3 {
        tics(&mut s, 1, &use_);
        tics(&mut s, 2, &idle);
    }
    tics(&mut s, 30, &idle);
    shot(&mut s, &dir, "12_intermission_map");
    tics(&mut s, 200, &idle);
    shot(&mut s, &dir, "13_next_level");

    s.warp("E1M8").unwrap();
    s.exit_level();
    tics(&mut s, 400, &idle);
    shot(&mut s, &dir, "14_finale_text");
    tics(&mut s, 2000, &idle);
    shot(&mut s, &dir, "15_finale_art");

    // Quit prompt.
    s.key_down(Key::Escape);
    s.key_down(Key::Char('q'));
    s.key_down(Key::Enter);
    shot(&mut s, &dir, "16_quit_prompt");

    let cmds = s.take_audio_commands();
    println!("audio commands produced: {}", cmds.len());
}
