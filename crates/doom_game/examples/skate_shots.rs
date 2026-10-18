//! Headless check of the skateboard mode: rides the start of a map and
//! writes PNGs of the third-person view (rolling, ollie, grind overlay,
//! a grind, debug views, a bail, the console), then prints timings.
//!
//! cargo run -p doom_game --release --example skate_shots -- <iwad> <out dir> [map]

use std::path::{Path, PathBuf};
use std::time::Instant;

use doom_game::{png, InputState, Session, SessionConfig};
use doom_sim::grind::GrindKind;
use doom_sim::skate::SkateState;

fn shot(s: &mut Session, dir: &Path, name: &str) {
    let mut rgba = vec![0u8; 320 * 200 * 4];
    let t = Instant::now();
    s.render_rgba(&mut rgba);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    std::fs::write(dir.join(format!("{name}.png")), png::encode_rgba(320, 200, &rgba)).unwrap();
    let st = s.skater().map_or("-".to_string(), |k| format!("{} {:.0} u/s", k.state.name(), k.speed() * 35.0));
    println!("{name}: {ms:.2} ms  [{st}]");
}

fn ticks(s: &mut Session, n: usize, inp: &InputState) {
    for _ in 0..n {
        s.tick(inp);
    }
}

/// Put the skater just above a long ledge, riding along it, so the next
/// tics lock into a grind. Returns the edge.
pub fn setup_grind(s: &mut Session, min_len: f32) -> Option<u32> {
    let graph = s.world.skate.grind.clone()?;
    let mo = s.world.console_player().mo?;
    let (px, py) = (s.world.mo(mo).x.0 as f32 / 65536.0, s.world.mo(mo).y.0 as f32 / 65536.0);
    let mut cands: Vec<u32> = (0..graph.edges.len() as u32)
        .filter(|&e| {
            let g = &graph.edges[e as usize];
            g.kind != GrindKind::Rail && g.length >= min_len
        })
        .collect();
    cands.sort_by(|&a, &b| {
        let d = |e: u32| {
            let p = graph.edges[e as usize].point(graph.edges[e as usize].length * 0.5);
            (p[0] - px).powi(2) + (p[1] - py).powi(2)
        };
        d(a).total_cmp(&d(b))
    });
    for e in cands.into_iter().take(60) {
        let edge = &graph.edges[e as usize];
        let top = edge.top_z(&s.world.level);
        let p = edge.point(edge.length * 0.25);
        let (x, y) = (p[0] - edge.normal[0] * 6.0, p[1] - edge.normal[1] * 6.0);
        if s.exec(&format!("warp {x} {y}")).is_err() {
            continue;
        }
        if (s.world.mo(mo).floorz.0 as f32 / 65536.0 - top).abs() > 0.5 {
            continue;
        }
        ticks(s, 1, &InputState::default());
        {
            let m = s.world.mo_mut(mo);
            m.z = doom_core::Fixed(((top + 6.0) * 65536.0) as i32);
        }
        let sk = s.world.players[0].skater.as_deref_mut()?;
        sk.state = SkateState::Airborne;
        sk.heading = edge.tangent[1].atan2(edge.tangent[0]);
        sk.vel = [edge.tangent[0] * 9.0, edge.tangent[1] * 9.0, -0.5];
        ticks(s, 2, &InputState::default());
        if s.skater().is_some_and(|k| k.state == SkateState::Grinding) {
            return Some(e);
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&dir).unwrap();
    let map = args.get(3).cloned().unwrap_or_else(|| "E1M1".into());
    let mut cfg = SessionConfig::new(&args[1]);
    cfg.map = Some(map.clone());
    cfg.nomonsters = true;
    let mut s = Session::new(&cfg).unwrap();
    s.exec("r_wipe 0; skate 1; show_skate_state 1").unwrap();
    let idle = InputState::default();
    ticks(&mut s, 2, &idle);
    shot(&mut s, &dir, "00_idle");
    match s.board_model() {
        Some(m) => println!("board model: {} tris, length {:.1}, deck top {:.1}, from {}", m.tris.len(), m.length, m.deck_top, m.source.display()),
        None => println!("board model: none (box fallback)"),
    }
    println!("scene: {} textured tris, {} sprites", s.scene().tex_tris.len(), s.scene().sprites.len());

    // Close-ups of the board model: a low camera close behind, then the
    // board turned across the view (the camera turns slowly).
    s.exec("skate_camera_distance 48; skate_camera_min_distance 32; skate_camera_height 10; skate_camera_look_height 6").unwrap();
    ticks(&mut s, 30, &idle);
    shot(&mut s, &dir, "00a_board_behind");
    s.exec("skate_camera_rotation 0.5").unwrap();
    if let Some(sk) = s.world.players[0].skater.as_deref_mut() {
        sk.heading += std::f32::consts::FRAC_PI_2;
    }
    ticks(&mut s, 2, &idle);
    shot(&mut s, &dir, "00b_board_side");
    s.exec("skate_debug 1").unwrap();
    shot(&mut s, &dir, "00c_board_side_debug");
    s.exec("skate_debug 0").unwrap();
    if let Some(cam) = s.host_view().map(|v| v.camera) {
        let sk = s.skater().unwrap();
        println!("camera {:.1} {:.1} {:.1} pitch {:.2}; board {:.1} {:.1} {:.1}", cam.x, cam.y, cam.z, cam.pitch, sk.board.pos[0], sk.board.pos[1], sk.board.pos[2]);
    }
    for sp in &s.scene().sprites {
        println!("sprite at {:?} frame {} scale {:?}", sp.pos, sp.frame, sp.scale);
    }
    s.exec("reset skate_camera_distance; reset skate_camera_min_distance; reset skate_camera_height; reset skate_camera_look_height; reset skate_camera_rotation").unwrap();
    ticks(&mut s, 60, &idle);

    let push = InputState { forward: true, ..Default::default() };
    ticks(&mut s, 50, &push);
    shot(&mut s, &dir, "01_rolling");

    let crouch = InputState { jump: true, ..Default::default() };
    ticks(&mut s, 10, &crouch);
    shot(&mut s, &dir, "02_crouch");
    ticks(&mut s, 7, &idle);
    shot(&mut s, &dir, "03_ollie");
    ticks(&mut s, 40, &idle);

    s.exec("show_grindables 1").unwrap();
    shot(&mut s, &dir, "04_grindables");
    match setup_grind(&mut s, 96.0) {
        Some(e) => {
            println!("grinding edge {e}");
            ticks(&mut s, 3, &idle);
            shot(&mut s, &dir, "05_grind");
            s.exec("skate_debug 1; show_velocity 1").unwrap();
            ticks(&mut s, 2, &idle);
            shot(&mut s, &dir, "06_grind_debug");
            s.exec("skate_debug 0; show_velocity 0").unwrap();
            for i in 0..80 {
                s.tick(&idle);
                if s.skater().is_some_and(|k| k.state != SkateState::Grinding) {
                    println!("left the grind after {} more tics: {:?}", i + 1, s.skater().unwrap().last_feat);
                    break;
                }
            }
        }
        None => println!("no grind set up"),
    }
    s.exec("show_grindables 0").unwrap();

    // A bail: full speed into the nearest wall.
    {
        let sk = s.world.players[0].skater.as_deref_mut().unwrap();
        sk.state = SkateState::Cruising;
        sk.vel = [sk.heading.cos() * 26.0, sk.heading.sin() * 26.0, 0.0];
    }
    for _ in 0..60 {
        s.tick(&push);
        if s.skater().is_some_and(|k| k.state == SkateState::Bail) {
            break;
        }
    }
    ticks(&mut s, 8, &idle);
    shot(&mut s, &dir, "07_bail");
    ticks(&mut s, 60, &idle);

    s.exec("r_wireframe 1").unwrap();
    shot(&mut s, &dir, "08_wireframe");
    s.exec("r_wireframe 0; show_collision 1; show_player 1").unwrap();
    shot(&mut s, &dir, "09_collision");
    s.exec("skate 0").unwrap();
    ticks(&mut s, 1, &idle);
    shot(&mut s, &dir, "10_classic_collision");
    s.exec("show_collision 0; show_player 0; show_skate_state 0").unwrap();
    s.set_console_open(true);
    s.text_input("give a");
    shot(&mut s, &dir, "11_console");
    s.set_console_open(false);

    // Timings.
    s.exec("skate 1").unwrap();
    let t = Instant::now();
    s.exec("rebuild_grindables").map(|m| println!("{m}")).unwrap();
    println!("rebuild: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let n = 2000;
    let lt = s.world.leveltime;
    let wiggle = InputState { forward: true, turn_left: true, ..Default::default() };
    for i in 0..n {
        s.tick(if i % 70 < 35 { &push } else { &wiggle });
    }
    println!(
        "{n} tics skating: {:.3} ms/tic ({} game tics ran, state {:?}, skater {:?})",
        t.elapsed().as_secs_f64() * 1000.0 / n as f64,
        s.world.leveltime - lt,
        s.state(),
        s.skater().map(|k| k.state)
    );
}
