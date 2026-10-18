//! Compare the classic and perspective renderers, and show true pitch:
//! renders the start of a map with both renderers at pitch 0, then the
//! perspective renderer at several pitches, as PNGs.
//!
//! cargo run -p doom_game --release --example pitch_shots -- <iwad> <out dir> [map] [walk tics] [turn-right tics]

use std::path::PathBuf;
use std::time::Instant;

use doom_game::{png, InputState, Session, SessionConfig};
use doom_render::{Camera, ViewRect};

fn shot(s: &mut Session, dir: &PathBuf, name: &str) {
    let mut rgba = vec![0u8; 320 * 200 * 4];
    let t = Instant::now();
    s.render_rgba(&mut rgba);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    std::fs::write(dir.join(format!("{name}.png")), png::encode_rgba(320, 200, &rgba)).unwrap();
    println!("{name}: {ms:.2} ms, pitch {:.1} deg", s.pitch().to_degrees());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&dir).unwrap();
    let map = args.get(3).cloned().unwrap_or_else(|| "E1M1".into());
    let walk_tics: usize = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(0);
    let turn_tics: usize = args.get(5).and_then(|a| a.parse().ok()).unwrap_or(0);
    let mut cfg = SessionConfig::new(&args[1]);
    cfg.map = Some(map.clone());
    let mut s = Session::new(&cfg).unwrap();
    let walk = InputState { forward: true, ..Default::default() };
    let turn = InputState { turn_right: true, ..Default::default() };
    for _ in 0..turn_tics {
        s.tick(&turn);
    }
    for _ in 0..walk_tics {
        s.tick(&walk);
    }
    s.tick(&InputState::default());

    s.set_freelook(false);
    shot(&mut s, &dir, &format!("{map}_classic"));
    s.set_freelook(true);
    shot(&mut s, &dir, &format!("{map}_persp_p0"));
    for deg in [30.0f32, -30.0, 50.0, -50.0] {
        s.center_view();
        s.look(deg.to_radians());
        shot(&mut s, &dir, &format!("{map}_persp_p{}", deg as i32));
    }
    s.center_view();
    sky_shots(&mut s, &dir, &map);
    if std::env::var_os("DEBUG_SECTORS").is_some() {
        debug_sectors(&s);
    }
    // Timing over a few frames.
    let t = Instant::now();
    let mut rgba = vec![0u8; 320 * 200 * 4];
    for _ in 0..20 {
        s.render_rgba(&mut rgba);
    }
    println!("avg perspective frame: {:.2} ms", t.elapsed().as_secs_f64() * 50.0);
}

/// Free cameras inside the largest outdoor (sky-ceiling) sector, looking
/// around and up, to check the sky model.
fn sky_shots(s: &mut Session, dir: &PathBuf, map: &str) {
    let w = &s.world;
    let level = &w.level;
    let mut count = vec![0usize; level.sectors.len()];
    for ss in &level.subsectors {
        if level.sector(ss.sector).ceiling_pic == w.sky_flat {
            count[ss.sector.idx()] += 1;
        }
    }
    let Some((best, _)) = count.iter().enumerate().filter(|(_, &c)| c > 0).max_by_key(|(_, &c)| c) else {
        println!("{map}: no sky sector");
        return;
    };
    // A point inside one of its subsectors: the mean of its seg vertices.
    let Some(ss) = level.subsectors.iter().find(|ss| ss.sector.idx() == best) else { return };
    let (mut x, mut y, mut n) = (0.0f32, 0.0f32, 0.0f32);
    for seg in &level.segs[ss.first_line as usize..(ss.first_line + ss.num_lines) as usize] {
        let v = level.vertex(seg.v1);
        x += v.x.0 as f32 / 65536.0;
        y += v.y.0 as f32 / 65536.0;
        n += 1.0;
    }
    let floor = level.sector(ss.sector).floor_height.0 as f32 / 65536.0;
    if std::env::var_os("DEBUG_SECTORS").is_some() {
        let (cx, cy) = (x / n, y / n);
        println!("camera sector {} at ({cx}, {cy}) z {}", ss.sector.idx(), floor + 41.0);
        // Probe along yaw 270 (south) at the depression of pixel row 73.
        let depress = ((73.0 + 0.5 - 84.0) * 1.2 / 160.0f32).atan();
        let _ = depress;
        for dist in (32..1200).step_by(32) {
            let (px, py) = (cx, cy - dist as f32);
            let sub = doom_sim::geometry::point_in_subsector(level, doom_core::Fixed((px * 65536.0) as i32), doom_core::Fixed((py * 65536.0) as i32));
            let ssec = level.subsectors[sub.idx()].sector;
            let sec = level.sector(ssec);
            println!("  d {dist}: subsector {} sector {} floor {} ceil {}", sub.idx(), ssec.idx(), sec.floor_height.0 >> 16, sec.ceiling_height.0 >> 16);
        }
    }
    let r = &s.renderer;
    let rect = ViewRect { x: r.viewwindowx, y: r.viewwindowy, width: r.scaledviewwidth, height: r.viewheight, low_detail: false };
    for (i, (yaw, pitch)) in [(0.0f32, 0.0f32), (90.0, 20.0), (180.0, 45.0), (270.0, -20.0), (270.0, 0.0)].into_iter().enumerate() {
        let cam = Camera { x: x / n, y: y / n, z: floor + 41.0, yaw: yaw.to_radians(), pitch: pitch.to_radians() };
        let mut screen = vec![0u8; 320 * 200];
        s.perspective.render(&s.render_data, &s.world, 0, &cam, rect, &mut screen);
        let pal = *s.render_data.palettes.get(0);
        let rgba: Vec<u8> = screen.iter().flat_map(|&c| { let [r, g, b] = pal[c as usize]; [r, g, b, 255] }).collect();
        std::fs::write(dir.join(format!("{map}_sky{i}.png")), png::encode_rgba(320, 200, &rgba)).unwrap();
        // Same viewpoint through the classic renderer (pitch 0 only), by
        // moving the player's body for the frame (not relinked: render only).
        if pitch == 0.0 || std::env::var_os("CLASSIC_ALL").is_some() {
            let id = s.world.players[0].mo.unwrap();
            let mo = s.world.mo_mut(id);
            mo.x = doom_core::Fixed((cam.x * 65536.0) as i32);
            mo.y = doom_core::Fixed((cam.y * 65536.0) as i32);
            mo.angle = doom_core::Angle((yaw / 360.0 * 4294967296.0) as i64 as u32);
            s.world.players[0].viewz = doom_core::Fixed((cam.z * 65536.0) as i32);
            s.renderer.render_player_view(&s.render_data, &s.world, 0, &mut screen);
            let rgba: Vec<u8> = screen.iter().flat_map(|&c| { let [r, g, b] = pal[c as usize]; [r, g, b, 255] }).collect();
            std::fs::write(dir.join(format!("{map}_sky{i}_classic.png")), png::encode_rgba(320, 200, &rgba)).unwrap();
        }
    }
}

#[allow(dead_code)]
pub fn debug_sectors(s: &Session) {
    let level = &s.world.level;
    for (i, sec) in level.sectors.iter().enumerate() {
        println!(
            "sector {i}: floor {} ceil {} light {} floorpic {} ceilpic {} sky {}",
            sec.floor_height.0 >> 16,
            sec.ceiling_height.0 >> 16,
            sec.light_level,
            sec.floor_pic.0,
            sec.ceiling_pic.0,
            sec.ceiling_pic == s.world.sky_flat
        );
    }
}
