//! The skateboard controller on real IWAD maps: grind graph extraction,
//! pushing, braking, ollies, landings, grinds (with connections), bails,
//! teleports, and staying out of demos. Skipped when the IWAD is absent.

use std::path::PathBuf;

use doom_game::{cvar, InputState, Session, SessionConfig, ViewPath};
use doom_sim::grind::{ConnKind, GrindGraph, GrindKind};
use doom_sim::mobj::flags::MF_NOGRAVITY;
use doom_sim::skate::{Feat, SkateState};

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
    cfg.nomonsters = true;
    Some(Session::new(&cfg).unwrap())
}

fn skating(map: &str) -> Option<Session> {
    let mut s = session(map)?;
    s.exec("skate 1").unwrap();
    s.tick(&InputState::default());
    assert!(s.skater().is_some());
    Some(s)
}

fn ticks(s: &mut Session, n: usize, inp: &InputState) {
    for _ in 0..n {
        s.tick(inp);
    }
}

fn graph(s: &Session) -> std::sync::Arc<GrindGraph> {
    s.world.skate.grind.clone().expect("grind graph built while skating")
}

fn fx(v: doom_core::Fixed) -> f32 {
    v.0 as f32 / 65536.0
}

/// Move to a spot with at least `headroom` between floor and ceiling.
/// Returns the direction into the room (away from the nearby wall).
fn warp_to_open_space(s: &mut Session, headroom: f32) -> f32 {
    let lv = &s.world.level;
    let mut spots = Vec::new();
    for l in &lv.lines {
        let Some(front) = l.front_sector else { continue };
        let sec = lv.sector(front);
        if fx(sec.ceiling_height) - fx(sec.floor_height) < headroom {
            continue;
        }
        let (a, b) = (lv.vertex(l.v1), lv.vertex(l.v2));
        let (ax, ay, bx, by) = (fx(a.x), fx(a.y), fx(b.x), fx(b.y));
        let len = (bx - ax).hypot(by - ay);
        if len < 128.0 {
            continue;
        }
        // The front side is on the right of v1 -> v2.
        let (nx, ny) = ((by - ay) / len, -(bx - ax) / len);
        spots.push((((ax + bx) / 2.0 + nx * 48.0), ((ay + by) / 2.0 + ny * 48.0), front, ny.atan2(nx)));
    }
    for (x, y, sec, inward) in spots {
        if s.exec(&format!("warp {x} {y}")).is_ok() {
            let mo = s.world.console_player().mo.unwrap();
            let m = s.world.mo(mo);
            if s.world.level.subsectors[m.subsector.idx()].sector == sec && fx(m.ceilingz) - fx(m.floorz) >= headroom {
                s.tick(&InputState::default());
                return inward;
            }
        }
    }
    panic!("no open space");
}

/// Put the skater just above `edge` at `s_pos` (distance from its `a`
/// end) on the raised side, falling slowly and moving along it in
/// direction `dir` at `speed`. True if that locked into a grind.
fn drop_onto(s: &mut Session, edge: u32, s_pos: f32, dir: f32, speed: f32) -> bool {
    let g = graph(s);
    let e = &g.edges[edge as usize];
    let top = e.top_z(&s.world.level);
    let p = e.point(s_pos);
    let (x, y) = (p[0] - e.normal[0] * 6.0, p[1] - e.normal[1] * 6.0);
    if s.exec(&format!("warp {x} {y}")).is_err() {
        return false;
    }
    let mo = s.world.console_player().mo.unwrap();
    if (fx(s.world.mo(mo).floorz) - top).abs() > 0.5 {
        return false;
    }
    s.tick(&InputState::default());
    s.world.mo_mut(mo).z = doom_core::Fixed(((top + 6.0) * 65536.0) as i32);
    let sk = s.world.players[0].skater.as_deref_mut().unwrap();
    sk.state = SkateState::Airborne;
    let t = [e.tangent[0] * dir, e.tangent[1] * dir];
    sk.heading = t[1].atan2(t[0]);
    sk.vel = [t[0] * speed, t[1] * speed, -0.5];
    ticks(s, 2, &InputState::default());
    s.skater().is_some_and(|k| k.state == SkateState::Grinding)
}

#[test]
fn grind_graphs_of_the_shareware_episode() {
    let Some(mut s) = session("E1M1") else { return };
    let mut total = 0;
    let mut corners = 0;
    for m in 1..=9 {
        if s.exec(&format!("map e1m{m}")).is_err() {
            continue;
        }
        let out = s.exec("rebuild_grindables").unwrap();
        let g = graph(&s);
        assert!(!g.edges.is_empty(), "E1M{m}: {out}");
        total += g.edges.len();
        corners += g.stats.corners;
        let max_h = s.cvars.get(cvar::SKATE_GRIND_MAX_HEIGHT);
        for (i, e) in g.edges.iter().enumerate() {
            assert!(e.length >= g.params.min_length);
            assert!(e.height <= max_h + 0.01 && e.height >= 4.0, "E1M{m} edge {i}: height {}", e.height);
            assert!(e.clearance >= g.params.min_clearance);
            assert!((e.tangent[0].hypot(e.tangent[1]) - 1.0).abs() < 1e-4);
            assert!((e.tangent[0] * e.normal[0] + e.tangent[1] * e.normal[1]).abs() < 1e-4, "normal is across the edge");
            // The lookup finds every edge from its own midpoint.
            let mid = e.point(e.length * 0.5);
            assert!(g.near(mid[0], mid[1]).any(|k| k as usize == i), "E1M{m} edge {i} missing from its grid cell");
            // A ledge's normal points down the step.
            if e.kind != GrindKind::Rail {
                let low = s.world.level.sectors[e.low_sector.idx()].floor_height;
                assert!(fx(low) < e.top_z(&s.world.level));
            }
            // Connections meet at the shared node.
            for end in 0..2 {
                for c in g.connections_at(i as u32, end) {
                    let other = &g.edges[c.edge as usize];
                    assert_eq!(other.nodes[c.enter_end as usize], e.nodes[end]);
                    assert!(c.turn.abs() <= g.params.max_corner_angle.to_radians() + 1e-4);
                    assert_eq!(c.kind == ConnKind::Continuous, c.turn.abs() <= g.params.continuous_angle.to_radians());
                }
            }
        }
    }
    assert!(total > 200, "{total} edges over the episode");
    assert!(corners > 20, "{corners} corner connections");
}

#[test]
fn graph_is_cached_and_follows_settings() {
    let Some(mut s) = skating("E1M1") else { return };
    let g1 = graph(&s);
    s.exec("restart").unwrap();
    s.tick(&InputState::default());
    assert!(std::sync::Arc::ptr_eq(&g1, &graph(&s)), "reloading the map reuses the cached analysis");
    s.exec("skate_grind_max_height 16").unwrap();
    s.world.skate.grind = None;
    s.tick(&InputState::default());
    let g2 = graph(&s);
    assert!(g2.edges.len() < g1.edges.len());
    assert!(g2.edges.iter().all(|e| e.height <= 16.0));
}

#[test]
fn push_brake_and_carve() {
    let Some(mut s) = skating("E1M1") else { return };
    let max = s.cvars.get(cvar::SKATE_MAX_SPEED) / 35.0;
    let push = InputState { forward: true, ..Default::default() };
    let mut best = 0.0f32;
    for _ in 0..60 {
        s.tick(&push);
        let k = s.skater().unwrap();
        best = best.max(k.speed());
        assert!(k.speed() <= max * 1.02, "pushing never passes skate_max_speed");
    }
    assert!(best > max * 0.6, "pushing reaches speed: {best} of {max}");
    let st = s.skater().unwrap().state;
    assert!(matches!(st, SkateState::Pushing | SkateState::Cruising), "{st:?}");
    // Coasting slows down slowly; braking quickly.
    let v0 = s.skater().unwrap().speed();
    ticks(&mut s, 5, &InputState::default());
    let v1 = s.skater().unwrap().speed();
    assert!(v1 < v0 && v1 > v0 * 0.85, "coast {v0} -> {v1}");
    let brake = InputState { backward: true, ..Default::default() };
    s.tick(&brake);
    assert_eq!(s.skater().unwrap().state, SkateState::Braking);
    ticks(&mut s, 30, &brake);
    let k = s.skater().unwrap();
    assert!(k.speed() < 0.3, "stopped: {}", k.speed());
    // Carving turns the board while moving.
    let inward = warp_to_open_space(&mut s, 128.0);
    {
        let sk = s.world.players[0].skater.as_deref_mut().unwrap();
        sk.heading = inward;
        sk.vel = [sk.heading.cos() * 6.0, sk.heading.sin() * 6.0, 0.0];
    }
    let h0 = s.skater().unwrap().heading;
    let carve = InputState { strafe_left: true, ..Default::default() };
    ticks(&mut s, 8, &carve);
    let k = s.skater().unwrap();
    assert_eq!(k.state, SkateState::Carving);
    assert!((k.heading - h0).abs() > 0.1, "carving turned {} rad", k.heading - h0);
}

#[test]
fn ollie_and_land() {
    let Some(mut s) = skating("E1M1") else { return };
    // The start room's ceiling is too low for a full ollie.
    warp_to_open_space(&mut s, 160.0);
    let mo = s.world.console_player().mo.unwrap();
    let floor = fx(s.world.mo(mo).floorz);
    let hold = InputState { jump: true, ..Default::default() };
    ticks(&mut s, 12, &hold);
    let k = s.skater().unwrap();
    assert!(k.crouch > 0.5 && k.charge > 0.5, "crouched and charged");
    s.tick(&InputState::default());
    assert_eq!(s.skater().unwrap().state, SkateState::Ollie);
    let mut peak = 0.0f32;
    let mut landed = None;
    for i in 0..80 {
        s.tick(&InputState::default());
        peak = peak.max(fx(s.world.mo(mo).z) - floor);
        if s.skater().unwrap().grounded() {
            landed = Some(i);
            break;
        }
    }
    assert!(peak > 30.0, "ollie height {peak}");
    assert!(landed.is_some(), "came back down");
    assert!(matches!(s.skater().unwrap().last_feat, Some(Feat::Air { .. })));
    // The mobj never falls under Doom gravity while skating.
    assert_ne!(s.world.mo(mo).flags & MF_NOGRAVITY, 0);
}

#[test]
fn grind_a_ledge_and_ride_off_the_end() {
    let Some(mut s) = skating("E1M1") else { return };
    let g = graph(&s);
    let mut edges: Vec<u32> = (0..g.edges.len() as u32).filter(|&e| g.edges[e as usize].length >= 96.0 && g.edges[e as usize].kind != GrindKind::Rail).collect();
    edges.sort_by(|a, b| g.edges[*b as usize].length.total_cmp(&g.edges[*a as usize].length));
    let edge = edges.into_iter().find(|&e| drop_onto(&mut s, e, 16.0, 1.0, 9.0)).expect("some long ledge in E1M1 is reachable");
    let k = s.skater().unwrap();
    let gc = k.grind.unwrap();
    assert_eq!(gc.edge, edge);
    assert_eq!(gc.style, doom_sim::skate::GrindStyle::FiftyFifty);
    // On the edge: board height is the edge height, momentum carried.
    let top = g.edges[edge as usize].top_z(&s.world.level);
    assert!((k.pos[2] - top).abs() < 0.01);
    assert!(gc.speed > 7.0);
    let mut s_prev = gc.s;
    let mut exited = false;
    for _ in 0..200 {
        s.tick(&InputState::default());
        let k = s.skater().unwrap();
        match k.grind {
            Some(gc) if gc.edge == edge => {
                assert!(gc.s >= s_prev, "moves along the edge");
                s_prev = gc.s;
            }
            Some(_) => {}
            None => {
                exited = true;
                break;
            }
        }
    }
    assert!(exited, "the grind ends");
    match s.skater().unwrap().last_feat {
        Some(Feat::Grind { tics, .. }) => assert!(tics > 3),
        f => panic!("expected a grind feat, got {f:?}"),
    }
}

#[test]
fn grinds_follow_connected_edges() {
    let Some(mut s) = skating("E1M1") else { return };
    let g = graph(&s);
    // An edge whose far end connects onward.
    let mut tried = 0;
    for e in 0..g.edges.len() as u32 {
        let edge = &g.edges[e as usize];
        if edge.length < 48.0 || g.connections_at(e, 1).is_empty() {
            continue;
        }
        tried += 1;
        if !drop_onto(&mut s, e, edge.length - 30.0, 1.0, 6.0) {
            continue;
        }
        for _ in 0..30 {
            s.tick(&InputState::default());
            let k = s.skater().unwrap();
            if let Some(gc) = k.grind {
                if gc.edge != e {
                    assert!(gc.edges >= 2);
                    let c = g.connections_at(e, 1).iter().find(|c| c.edge == gc.edge).expect("moved through a listed connection");
                    let _ = c.kind;
                    return;
                }
            } else {
                break;
            }
        }
    }
    panic!("no grind carried over a connection ({tried} edges tried)");
}

#[test]
fn pop_off_a_grind_and_balance_out() {
    let Some(mut s) = skating("E1M1") else { return };
    let g = graph(&s);
    let edge = (0..g.edges.len() as u32).find(|&e| g.edges[e as usize].length >= 96.0 && drop_onto(&mut s, e, 16.0, 1.0, 7.0)).expect("a grindable ledge");
    let _ = edge;
    // Jump: leave the edge upwards.
    s.tick(&InputState { jump: true, ..Default::default() });
    let k = s.skater().unwrap();
    assert_eq!(k.state, SkateState::Ollie);
    assert!(k.vel[2] > 5.0);
    assert!(k.grind.is_none());

    // Leaning hard one way without assist loses the balance.
    s.exec("skate_grind_assist 0").unwrap();
    ticks(&mut s, 60, &InputState::default());
    let edge = (0..g.edges.len() as u32).find(|&e| g.edges[e as usize].length >= 128.0 && drop_onto(&mut s, e, 8.0, 1.0, 8.0)).expect("a long ledge");
    let _ = edge;
    let lean = InputState { strafe_right: true, ..Default::default() };
    let mut fell = false;
    for _ in 0..60 {
        s.tick(&lean);
        if s.skater().unwrap().grind.is_none() {
            fell = true;
            break;
        }
    }
    assert!(fell, "leaning off balance ends the grind");
}

#[test]
fn bail_into_a_wall_and_get_up() {
    let Some(mut s) = skating("E1M1") else { return };
    // Aim at the nearest solid wall.
    let mo = s.world.console_player().mo.unwrap();
    let (px, py) = (fx(s.world.mo(mo).x), fx(s.world.mo(mo).y));
    let lv = &s.world.level;
    let mut best = (f32::MAX, 0.0f32);
    for l in lv.lines.iter().filter(|l| l.back_sector.is_none()) {
        let (a, b) = (lv.vertex(l.v1), lv.vertex(l.v2));
        let (ax, ay, bx, by) = (fx(a.x), fx(a.y), fx(b.x), fx(b.y));
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.1, 0.9);
        let (qx, qy) = (ax + dx * t, ay + dy * t);
        let d = (qx - px).hypot(qy - py);
        if d < best.0 && d > 40.0 {
            best = (d, (qy - py).atan2(qx - px));
        }
    }
    let sk = s.world.players[0].skater.as_deref_mut().unwrap();
    sk.heading = best.1;
    sk.vel = [best.1.cos() * 24.0, best.1.sin() * 24.0, 0.0];
    sk.state = SkateState::Cruising;
    let mut bailed = false;
    for _ in 0..80 {
        s.tick(&InputState::default());
        if s.skater().unwrap().state == SkateState::Bail {
            bailed = true;
            break;
        }
    }
    assert!(bailed, "hitting a wall at full speed bails");
    assert_eq!(s.skater().unwrap().last_feat, Some(Feat::Bail));
    ticks(&mut s, 60, &InputState::default());
    let k = s.skater().unwrap();
    assert_eq!(k.state, SkateState::Idle, "back on the board");
    assert!(k.bail.is_none());
}

#[test]
fn modes_switch_cleanly() {
    let Some(mut s) = skating("E1M1") else { return };
    let mo = s.world.console_player().mo.unwrap();
    // Third person needs the free camera; the camera sits behind the board.
    assert_eq!(s.view_path(), ViewPath::Perspective);
    ticks(&mut s, 3, &InputState::default());
    let v = s.host_view().unwrap();
    let d = (v.camera.x - fx(s.world.mo(mo).x)).hypot(v.camera.y - fx(s.world.mo(mo).y));
    assert!(d > 40.0, "chase camera {d} units back");
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_rgba(&mut rgba);

    // Teleports (warp) arrive standing still.
    s.world.players[0].skater.as_deref_mut().unwrap().vel = [10.0, 0.0, 0.0];
    s.exec("warp 1056 -3400").unwrap();
    s.tick(&InputState::default());
    let k = s.skater().unwrap();
    assert_eq!(k.speed(), 0.0);
    assert_eq!(k.state, SkateState::Idle);

    // Walking again: gravity back, first person.
    s.exec("skate 0").unwrap();
    assert!(s.skater().is_none());
    assert_eq!(s.world.mo(mo).flags & MF_NOGRAVITY, 0);
    assert_eq!(s.view_path(), ViewPath::Columns);

    // Dying takes the player off the board; respawning puts them back on.
    s.exec("skate 1").unwrap();
    s.tick(&InputState::default());
    s.exec("suicide").unwrap();
    s.tick(&InputState::default());
    assert!(s.skater().is_none());
    assert_eq!(s.world.mo(mo).flags & MF_NOGRAVITY, 0, "the corpse falls");
    s.exec("respawn").unwrap();
    s.tick(&InputState::default());
    assert!(s.skater().is_some());
}

#[test]
fn demos_never_skate() {
    let Some(iwad) = iwad() else { return };
    let mut cfg = SessionConfig::new(iwad);
    cfg.demo = Some("DEMO1".into());
    let mut s = Session::new(&cfg).unwrap();
    s.exec("player_controller Skate").unwrap();
    for _ in 0..300 {
        s.tick(&InputState::default());
        assert!(s.world.players.iter().all(|p| p.skater.is_none()));
    }
}

// ---- visuals ---------------------------------------------------------------------------

/// The scene's skateboard triangles and rider sprite, after a frame.
fn scene_counts(s: &mut Session) -> (usize, usize) {
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_rgba(&mut rgba);
    (s.scene().tex_tris.len(), s.scene().sprites.len())
}

#[test]
fn real_board_model_and_doomguy_sprite() {
    let Some(mut s) = skating("E1M1") else { return };
    let Some(model) = s.board_model() else {
        eprintln!("skipping: no RESOURCES/skateboard.model");
        return;
    };
    assert!(model.tris.len() > 5000, "the asset's mesh: {} triangles", model.tris.len());
    assert!((model.length - 36.0).abs() < 0.5);
    assert!(model.deck_top > 3.0 && model.deck_top < 8.0, "deck top {}", model.deck_top);
    let deck_top = model.deck_top;
    let (tris, sprites) = scene_counts(&mut s);
    assert!(tris > 1000, "board drawn: {tris} triangles");
    assert_eq!(sprites, 1, "Doomguy drawn");
    let play = s.world.info.state(doom_sim::info::S_PLAY).sprite.0;
    assert_eq!(s.scene().sprites[0].sprite, play, "the PLAY sprite, not a model");
    assert!(s.scene().tris.is_empty(), "no box rider or box board");

    // The rider stays on the deck through pushing, an ollie and a grind.
    let check = |s: &mut Session, what: &str| {
        scene_counts(s);
        let sk = s.skater().unwrap().clone();
        let sp = s.scene().sprites[0];
        let dz = sp.pos[2] - sk.board.pos[2];
        let dxy = (sp.pos[0] - sk.board.pos[0]).hypot(sp.pos[1] - sk.board.pos[1]);
        assert!(dxy < 2.0 && (dz - deck_top).abs() < 1.5, "{what}: rider {:?} vs board {:?}", sp.pos, sk.board.pos);
        sp.frame
    };
    warp_to_open_space(&mut s, 160.0);
    check(&mut s, "idle");
    s.exec("push").unwrap();
    ticks(&mut s, 6, &InputState::default());
    assert_eq!(s.skater().unwrap().state, SkateState::Pushing);
    let mut push_frames = std::collections::BTreeSet::new();
    for _ in 0..18 {
        s.tick(&InputState::default());
        push_frames.insert(check(&mut s, "pushing"));
    }
    assert!(push_frames.len() >= 3, "the push stroke animates: frames {push_frames:?}");
    ticks(&mut s, 12, &InputState { jump: true, ..Default::default() });
    let crouched = {
        scene_counts(&mut s);
        s.scene().sprites[0].scale[1]
    };
    assert!(crouched < 0.85, "charging squashes the rider: {crouched}");
    s.tick(&InputState::default());
    ticks(&mut s, 4, &InputState::default());
    assert!(!s.skater().unwrap().grounded());
    check(&mut s, "airborne");
    // The board pitches with the ollie.
    assert!(s.skater().unwrap().board.pitch.abs() > 0.01);
    ticks(&mut s, 60, &InputState::default());
    s.exec("bail").unwrap();
    ticks(&mut s, 15, &InputState::default());
    scene_counts(&mut s);
    assert!(s.scene().sprites[0].frame >= 7, "knocked down: death frames");
}

#[test]
fn skater_draws_in_both_render_modes() {
    let Some(mut s) = skating("E1M1") else { return };
    if s.board_model().is_none() {
        return;
    }
    warp_to_open_space(&mut s, 128.0);
    ticks(&mut s, 3, &InputState::default());
    let opaque_in_view = |s: &mut Session| {
        let mut rgba = vec![0u8; 320 * 200 * 4];
        s.render_overlay_rgba(&mut rgba);
        let r = s.host_view().unwrap().rect;
        let mut n = 0;
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                n += (rgba[((y * 320 + x) * 4 + 3) as usize] == 255) as usize;
            }
        }
        n
    };
    for mode in [doom_game::RenderMode::Classic, doom_game::RenderMode::GZDoomClean] {
        s.set_render_mode(mode);
        let (tris, sprites) = scene_counts(&mut s);
        assert!(tris > 1000 && sprites == 1, "{mode:?}: board {tris} tris, {sprites} rider");
    }
    // Host-drawn mode: the overlay a truecolor host composites carries the
    // board and rider as opaque pixels over the transparent 3D view.
    s.set_render_mode(doom_game::RenderMode::GZDoomClean);
    assert_eq!(s.view_path(), ViewPath::Host);
    let with = opaque_in_view(&mut s);
    s.exec("skate 0").unwrap();
    s.tick(&InputState::default());
    let without = opaque_in_view(&mut s);
    assert!(with > without + 200, "skater pixels in the overlay: {with} vs {without} (weapon only)");
}

#[test]
fn camera_is_smooth_between_tics() {
    let Some(mut s) = skating("E1M1") else { return };
    let inward = warp_to_open_space(&mut s, 128.0);
    {
        let sk = s.world.players[0].skater.as_deref_mut().unwrap();
        sk.heading = inward;
    }
    s.exec("set_speed 500").unwrap();
    // A 140 Hz display: four frames per tic.
    let mut positions = Vec::new();
    for _ in 0..12 {
        s.tick(&InputState::default());
        for k in 1..=4 {
            s.set_frame_time(k as f32 / 4.0, 1.0 / 140.0);
            let mut rgba = vec![0u8; 320 * 200 * 4];
            s.render_rgba(&mut rgba);
            let c = s.host_view().unwrap().camera;
            positions.push([c.x, c.y, c.z]);
        }
    }
    let steps: Vec<f32> = positions.windows(2).map(|p| ((p[1][0] - p[0][0]).powi(2) + (p[1][1] - p[0][1]).powi(2) + (p[1][2] - p[0][2]).powi(2)).sqrt()).collect();
    let moving: Vec<f32> = steps[8..].to_vec();
    let still = moving.iter().filter(|&&d| d < 1e-4).count();
    assert_eq!(still, 0, "the camera moves on every frame, not only on tics: {moving:?}");
    let mean = moving.iter().sum::<f32>() / moving.len() as f32;
    let max = moving.iter().cloned().fold(0.0f32, f32::max);
    assert!(max < mean * 2.5, "no 35 Hz steps: max {max} vs mean {mean} ({moving:?})");
    // The rider is drawn between tics too (not stepping with the sim).
    s.set_frame_time(0.0, 0.0);
    let mut rgba = vec![0u8; 320 * 200 * 4];
    s.render_rgba(&mut rgba);
    let a = s.scene().sprites[0].pos;
    s.set_frame_time(0.5, 0.0);
    s.render_rgba(&mut rgba);
    let b = s.scene().sprites[0].pos;
    assert!((a[0] - b[0]).hypot(a[1] - b[1]) > 1.0, "interpolated rider: {a:?} -> {b:?}");
}

#[test]
fn skate_action_commands() {
    let Some(mut s) = skating("E1M1") else { return };
    let inward = warp_to_open_space(&mut s, 160.0);
    s.world.players[0].skater.as_deref_mut().unwrap().heading = inward;
    let idle = InputState::default();
    let state = |s: &Session| s.skater().unwrap().state;

    // Errors name the problem.
    assert!(s.exec("spin 180").unwrap_err().contains("in the air"));
    assert!(s.exec("drop").unwrap_err().contains("not grinding"));
    assert!(s.exec("set_velocity 1 2").unwrap_err().contains("usage"));
    assert!(s.exec("spin fast").unwrap_err().contains("bad degrees"));

    s.exec("push").unwrap();
    s.tick(&idle);
    assert_eq!(state(&s), SkateState::Pushing);
    assert!(s.skater().unwrap().speed() > 0.0);
    ticks(&mut s, 25, &idle);
    s.exec("brake").unwrap();
    s.tick(&idle);
    assert_eq!(state(&s), SkateState::Braking);
    ticks(&mut s, 25, &idle);

    s.exec("set_speed 350").unwrap();
    s.tick(&idle);
    let v = s.skater().unwrap().speed() * 35.0;
    assert!((v - 350.0).abs() < 20.0, "set_speed: {v}");
    s.exec("jump").unwrap();
    s.tick(&idle);
    assert_eq!(state(&s), SkateState::Ollie, "jump is the ollie");
    assert!(s.exec("jump").unwrap_err().contains("can't jump"));
    s.exec("spin 180").unwrap();
    ticks(&mut s, 16, &idle);
    let spun = s.skater().unwrap().air_spin.to_degrees();
    assert!((spun - 180.0).abs() < 1.0, "spin 180 turned {spun}");
    ticks(&mut s, 60, &idle);
    assert!(s.skater().unwrap().grounded());

    s.exec("set_velocity 0 0 300").unwrap();
    s.tick(&idle);
    assert!(!s.skater().unwrap().grounded(), "upward velocity launches");
    ticks(&mut s, 60, &idle);
    s.exec("ollie").unwrap();
    s.tick(&idle);
    assert_eq!(state(&s), SkateState::Ollie);
    ticks(&mut s, 60, &idle);

    s.exec("bail").unwrap();
    s.tick(&idle);
    assert_eq!(state(&s), SkateState::Bail);
    ticks(&mut s, 60, &idle);
    assert_eq!(state(&s), SkateState::Idle);

    // grind / drop: next to a ledge.
    let g = graph(&s);
    let e = (0..g.edges.len() as u32).find(|&e| g.edges[e as usize].length >= 96.0 && g.edges[e as usize].kind != GrindKind::Rail).unwrap();
    let edge = &g.edges[e as usize];
    let p = edge.point(edge.length * 0.5);
    let (x, y) = (p[0] - edge.normal[0] * 20.0, p[1] - edge.normal[1] * 20.0);
    s.exec(&format!("warp {x} {y}")).unwrap();
    s.tick(&idle);
    s.exec("grind").unwrap();
    s.tick(&idle);
    assert_eq!(state(&s), SkateState::Grinding, "grind locked onto an edge");
    s.exec("drop").unwrap();
    s.tick(&idle);
    assert!(s.skater().unwrap().grind.is_none(), "drop left the grind");
    assert!(matches!(s.skater().unwrap().last_feat, Some(Feat::Grind { .. })));
}
