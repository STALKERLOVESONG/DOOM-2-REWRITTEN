//! Differential demo test: replay the IWAD's demos and compare every tic
//! against traces recorded from Chocolate Doom's unmodified game code
//! (`tools/refdoom`). Skipped when the IWAD or fixtures are absent or the
//! IWAD differs from the one the traces were made with.
//!
//! The playsim is incomplete (no monster AI, weapons, damage or specials
//! yet), so the demos desync at some point. The test reports how far each
//! demo stays in sync and fails if that regresses below the recorded floor.

use std::path::PathBuf;

use doom_content::game::identify;
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_sim::demo::Demo;
use doom_sim::info::Info;
use doom_sim::level::Level;
use doom_sim::{GameAction, GameParams, World};

/// Minimum number of tics each demo must stay in sync (raise these as the
/// playsim grows; never lower them without understanding why).
const SYNC_FLOOR: [(u32, usize); 4] = [(1, 1710), (2, 2347), (3, 3863), (4, 818)];

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn mix(h: u32, v: i32) -> u32 {
    (h ^ v as u32).wrapping_mul(16777619)
}

fn trace_line(w: &World, gametic: usize) -> String {
    let mut hash = 2166136261u32;
    let mut n = 0;
    for (_, mo) in w.thinkers.mobjs() {
        for v in [
            mo.mobj_type.0 as i32,
            mo.x.0,
            mo.y.0,
            mo.z.0,
            mo.angle.0 as i32,
            mo.state.0 as i32,
            mo.tics,
            mo.flags as i32,
            mo.health,
        ] {
            hash = mix(hash, v);
        }
        n += 1;
    }
    let p = w.console_player();
    let (x, y, z, a, mx, my, mz) = match p.mo.and_then(|id| w.thinkers.get_mobj(id)) {
        Some(m) => (m.x.0, m.y.0, m.z.0, m.angle.0, m.momx.0, m.momy.0, m.momz.0),
        None => (0, 0, 0, 0, 0, 0, 0),
    };
    format!(
        "tic {gametic} {} {} {x} {y} {z} {a} {mx} {my} {mz} {} {n} {hash:08x}",
        w.leveltime, w.rng.play_index, p.viewz.0
    )
}

const FIELDS: [&str; 14] =
    ["tic", "gametic", "leveltime", "prndindex", "x", "y", "z", "angle", "momx", "momy", "momz", "viewz", "nmobjs", "hash"];

fn first_difference(expected: &str, actual: &str) -> String {
    let (e, a): (Vec<_>, Vec<_>) = (expected.split(' ').collect(), actual.split(' ').collect());
    for (i, (x, y)) in e.iter().zip(&a).enumerate() {
        if x != y {
            return format!("{} expected {x} got {y}", FIELDS.get(i).unwrap_or(&"?"));
        }
    }
    "length".into()
}

struct Content {
    stack: ResourceStack,
    textures: TextureSet,
    gi: doom_content::game::GameInfo,
    iwad_hash: String,
}

fn content() -> Option<Content> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let iwad = std::env::var_os("DOOM_IWAD").map(PathBuf::from).unwrap_or_else(|| root.join("../../WADS/doom1.wad"));
    let Ok(bytes) = std::fs::read(&iwad) else {
        eprintln!("skipping: no IWAD at {}", iwad.display());
        return None;
    };
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let gi = identify(&stack, None).unwrap();
    Some(Content { stack, textures, gi, iwad_hash: format!("{:016x}", fnv1a64(&bytes)) })
}

/// Replay `demo` and compare against `trace` (without its header line).
/// Returns (tics in sync, total level tics in trace, report).
fn run_demo(c: &Content, demo: &Demo, trace: &str) -> (usize, usize, String, Vec<&'static str>) {
    let map = match c.gi.mode {
        doom_content::game::GameMode::Commercial => format!("MAP{:02}", demo.map),
        _ => format!("E{}M{}", demo.episode, demo.map),
    };
    let params = GameParams {
        mode: c.gi.mode,
        version: c.gi.version,
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
    let level = Level::load(&c.stack, &c.textures, &map).unwrap();
    let content = doom_sim::content::ContentInfo::from_content(&c.stack, &c.textures);
    let mut w = World::new_game(Info::doom(), content, params, demo.playeringame, level).unwrap();

    let lines: Vec<&str> = trace.lines().collect();
    let mut synced = 0;
    let mut report = String::from("in sync through the whole trace");
    for (gametic, (cmds, expected)) in demo.tics.iter().zip(&lines).enumerate() {
        match w.tick(cmds) {
            Some(GameAction::LoadLevel) => {
                w.setup_level(Level::load(&c.stack, &c.textures, &map).unwrap()).unwrap();
                w.tick(cmds);
            }
            Some(GameAction::Completed(_)) => {
                report = format!("level exited at tic {gametic} (intermission not implemented)");
                break;
            }
            None => {}
        }
        let mut actual = trace_line(&w, gametic);
        if expected.starts_with("c ") {
            // Compact line: gametic, prndindex, mobj hash.
            let f: Vec<&str> = actual.split(' ').collect();
            actual = format!("c {} {} {}", f[1], f[3], f[13]);
        }
        if actual != *expected {
            report = format!("desync at tic {gametic}: {}", first_difference(expected, &actual));
            break;
        }
        synced += 1;
    }
    (synced, lines.len(), report, w.unimplemented.keys().copied().collect())
}

#[test]
fn iwad_demos_stay_in_sync() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(c) = content() else { return };
    for (n, floor) in SYNC_FLOOR {
        let Ok(trace) = std::fs::read_to_string(root.join(format!("tests/fixtures/DEMO{n}.trace"))) else {
            eprintln!("skipping DEMO{n}: no trace fixture");
            continue;
        };
        let Some(body) = trace.strip_prefix(&format!("iwad {}\n", c.iwad_hash)) else {
            eprintln!("skipping DEMO{n}: trace was recorded with a different IWAD");
            continue;
        };
        let demo = Demo::parse(c.stack.lump(&format!("DEMO{n}")).unwrap()).unwrap();
        let (synced, total, report, unimpl) = run_demo(&c, &demo, body);
        eprintln!("DEMO{n}: {synced}/{total} tics in sync; {report}; unimplemented reached: {unimpl:?}");
        assert!(synced >= floor, "DEMO{n} regressed: {synced} < {floor}");
    }
}

/// Pseudo-random input demos on every map (tools/refdoom/gen_synthetic_*).
/// These must stay in sync for every recorded level tic.
#[test]
fn synthetic_demos_stay_in_sync() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic");
    let Some(c) = content() else { return };
    let Ok(sets) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no synthetic fixtures");
        return;
    };
    let mut names: Vec<PathBuf> = sets
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .flat_map(|d| std::fs::read_dir(d).unwrap().filter_map(|e| e.ok().map(|e| e.path())))
        .filter(|p| p.extension().is_some_and(|e| e == "lmp"))
        .collect();
    names.sort();
    let mut failures = Vec::new();
    for lmp in names {
        let name = format!(
            "{}/{}",
            lmp.parent().unwrap().file_name().unwrap().to_string_lossy(),
            lmp.file_stem().unwrap().to_string_lossy()
        );
        let trace = std::fs::read_to_string(lmp.with_extension("trace")).unwrap();
        let Some(body) = trace.strip_prefix(&format!("iwad {}\n", c.iwad_hash)) else {
            eprintln!("skipping {name}: trace was recorded with a different IWAD");
            continue;
        };
        let demo = Demo::parse(&std::fs::read(&lmp).unwrap()).unwrap();
        let (synced, total, report, unimpl) = run_demo(&c, &demo, body);
        eprintln!("{name}: {synced}/{total}; {report}; unimplemented: {unimpl:?}");
        if synced < total && !report.starts_with("level exited") {
            failures.push(format!("{name}: {report}"));
        }
    }
    assert!(failures.is_empty(), "desyncs:\n{}", failures.join("\n"));
}

/// Debug aid: `DUMP_DEMO=<path to .lmp> DUMP_TIC=<n> cargo test -p doom_sim
/// --test demo_sync dump_mobjs -- --nocapture` prints every mobj at tics
/// n-1 and n in refdoom's REFDOOM_DUMP_TIC format, for diffing.
#[test]
fn dump_mobjs() {
    let (Some(path), Some(tic)) = (std::env::var_os("DUMP_DEMO"), std::env::var("DUMP_TIC").ok()) else { return };
    let tic: usize = tic.parse().unwrap();
    let Some(c) = content() else { return };
    let demo = Demo::parse(&std::fs::read(path).unwrap()).unwrap();
    let map = format!("E{}M{}", demo.episode, demo.map);
    let params = GameParams {
        mode: c.gi.mode,
        version: c.gi.version,
        skill: demo.skill,
        episode: demo.episode,
        map: demo.map,
        deathmatch: demo.deathmatch,
        netgame: false,
        respawnmonsters: demo.respawnparm,
        nomonsters: demo.nomonsters,
        fastparm: demo.fastparm,
        consoleplayer: 0,
    };
    let level = Level::load(&c.stack, &c.textures, &map).unwrap();
    let content = doom_sim::content::ContentInfo::from_content(&c.stack, &c.textures);
    let mut w = World::new_game(Info::doom(), content, params, demo.playeringame, level).unwrap();
    for (gametic, cmds) in demo.tics.iter().enumerate().take(tic + 1) {
        if w.tick(cmds) == Some(GameAction::LoadLevel) {
            w.setup_level(Level::load(&c.stack, &c.textures, &map).unwrap()).unwrap();
            w.tick(cmds);
        }
        if gametic + 1 >= tic {
            for (i, (_, mo)) in w.thinkers.mobjs().enumerate() {
                println!(
                    "T{gametic} mobj {i} type {} xyz {} {} {} ang {} state {} tics {} flags {:x} health {} mom {} {} {}",
                    mo.mobj_type.0, mo.x.0, mo.y.0, mo.z.0, mo.angle.0, mo.state.0, mo.tics, mo.flags, mo.health, mo.momx.0, mo.momy.0, mo.momz.0
                );
            }
        }
    }
}
