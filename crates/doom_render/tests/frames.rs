//! Differential renderer test: replay demos through the playsim, render the
//! console player's view each tic and compare the view window against frames
//! captured from Chocolate Doom's unmodified renderer (`tools/refdoom`,
//! `REFDOOM_SHOT_DIR`).
//!
//! Cases are (view configuration x demo set). Each configuration has a
//! fixture `tests/fixtures/frames/<config>.txt`: an `iwad <hash>` header,
//! then `<demo> <gametic> <fuzzpos> <hash>` lines (FNV-1a over the view
//! window). `fuzzpos` is the reference's value at frame start: vanilla never
//! resets it, so the fuzz phase depends on how many frames were drawn, not
//! on the simulation.
//!
//! Pixels whose vanilla value came from memory it never defined (texel reads
//! past a lump's end, uninitialised composite bytes) are zeroed on our side
//! before hashing and excluded from raw comparisons; the count is reported.
//!
//! Raw mode, used by `tools/refdoom/gen_frame_fixtures.sh`:
//! `REFDOOM_SHOTS=<dir>` with `<dir>/shot_<tic>.raw` dumps, `FRAME_CONFIG`
//! and `FRAME_DEMO` select the case; `WRITE_FRAME_FIXTURES=1` merges the
//! verified hashes into the fixture; `FRAME_DIFF_OUT=<dir>` writes PPMs of
//! the first mismatching frame.
//!
//! Skipped when the IWAD or fixtures are absent or the IWAD differs from the
//! one the fixtures were made with.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use doom_content::game::identify;
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_render::{Limits, RenderData, Renderer, SCREENHEIGHT, SCREENWIDTH};
use doom_sim::demo::Demo;
use doom_sim::info::Info;
use doom_sim::level::Level;
use doom_sim::{GameAction, GameParams, World};

/// View configurations: (name, screenblocks, low detail). "default" is
/// Chocolate Doom's default (m_menu.c: screenblocks 9, detailLevel 0).
const CONFIGS: [(&str, i32, bool); 4] =
    [("default", 9, false), ("full_low", 11, true), ("full_high", 11, false), ("small_low", 5, true)];

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

struct Content {
    stack: std::sync::Arc<ResourceStack>,
    textures: TextureSet,
    gi: doom_content::game::GameInfo,
    rdata: RenderData,
    iwad_hash: String,
}

fn content() -> Option<Content> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let iwad = std::env::var_os("DOOM_IWAD").map(PathBuf::from).unwrap_or_else(|| root.join("../../WADS/doom1.wad"));
    let Ok(bytes) = std::fs::read(&iwad) else {
        eprintln!("skipping: no IWAD at {}", iwad.display());
        return None;
    };
    let stack = std::sync::Arc::new(ResourceStack::load(&iwad, &[]).unwrap());
    let textures = TextureSet::load(&stack).unwrap();
    let gi = identify(&stack, None).unwrap();
    let rdata = RenderData::build(&stack, &textures, &Info::doom().sprnames).unwrap();
    Some(Content { stack, textures, gi, rdata, iwad_hash: format!("{:016x}", fnv1a64(&bytes)) })
}

/// Demo ids: `DEMO<n>` (IWAD lumps) or `<set>/<stem>` (synthetic demos in
/// doom_sim's fixtures).
fn load_demo(c: &Content, id: &str) -> Option<Demo> {
    if let Some(lump) = c.stack.lump(id) {
        return Some(Demo::parse(lump).unwrap());
    }
    let path = synthetic_dir().join(format!("{id}.lmp"));
    std::fs::read(path).ok().map(|d| Demo::parse(&d).unwrap())
}

fn synthetic_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../doom_sim/tests/fixtures/synthetic")
}

/// `FRAME_FIXTURE_DIR` overrides the fixture directory (the generator sets
/// it to an absolute path).
fn fixture_path(config: &str) -> PathBuf {
    std::env::var_os("FRAME_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frames"))
        .join(format!("{config}.txt"))
}

/// The view window rows, concatenated.
fn view_window<T: Copy>(r: &Renderer, screen: &[T]) -> Vec<T> {
    let (x0, y0) = (r.viewwindowx as usize, r.viewwindowy as usize);
    let (w, h) = (r.scaledviewwidth as usize, r.viewheight as usize);
    let mut out = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        out.extend_from_slice(&screen[y * SCREENWIDTH + x0..y * SCREENWIDTH + x0 + w]);
    }
    out
}

fn write_ppm(path: &Path, w: usize, h: usize, pixels: impl Iterator<Item = [u8; 3]>) {
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in pixels {
        out.extend_from_slice(&p);
    }
    std::fs::write(path, out).unwrap();
}

enum Reference<'a> {
    /// gametic -> (fuzzpos at frame start, view window hash).
    Hashes(&'a HashMap<usize, (usize, u64)>),
    /// Directory of `shot_<tic>.raw` dumps.
    Raw(&'a Path),
}

struct Outcome {
    compared: usize,
    excluded: usize,
    failure: Option<String>,
    /// (tic, fuzzpos, hash) of every compared frame, for fixture writing.
    frames: Vec<(usize, usize, u64)>,
}

/// Replays `demo`, rendering after every tic, and compares every tic present
/// in the reference.
fn run(c: &Content, demo: &Demo, blocks: i32, low: bool, reference: Reference) -> Outcome {
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
    let mut r = Renderer::new(blocks, low, Limits::VANILLA);
    r.track_undefined();
    let mut screen = vec![0u8; SCREENWIDTH * SCREENHEIGHT];
    let mut out = Outcome { compared: 0, excluded: 0, failure: None, frames: Vec::new() };
    for cmds in &demo.tics {
        match w.tick(cmds) {
            Some(GameAction::LoadLevel) => {
                w.setup_level(Level::load(&c.stack, &c.textures, &map).unwrap()).unwrap();
                w.tick(cmds);
            }
            Some(GameAction::Completed(_)) => break,
            None => {}
        }
        let tic = w.gametic as usize;
        let raw = match reference {
            Reference::Raw(dir) => match std::fs::read(dir.join(format!("shot_{tic}.raw"))) {
                Ok(raw) if raw.len() >= SCREENWIDTH * SCREENHEIGHT + 4 => Some(raw),
                _ => continue,
            },
            Reference::Hashes(h) if !h.contains_key(&tic) => continue,
            Reference::Hashes(_) => None,
        };
        let fuzz = match (&raw, &reference) {
            (Some(raw), _) => {
                let t = &raw[SCREENWIDTH * SCREENHEIGHT..];
                u32::from_le_bytes([t[0], t[1], t[2], t[3]]) as usize
            }
            (None, Reference::Hashes(h)) => h[&tic].0,
            (None, Reference::Raw(_)) => unreachable!(),
        };
        r.fuzzpos = fuzz;
        r.render_player_view(&c.rdata, &w, w.params.consoleplayer, &mut screen);
        let mut actual = view_window(&r, &screen);
        let undefined = view_window(&r, r.undefined.as_ref().unwrap());
        for (a, &u) in actual.iter_mut().zip(&undefined) {
            if u {
                *a = 0;
            }
        }
        out.excluded += undefined.iter().filter(|&&u| u).count();
        let actual_hash = fnv1a64(&actual);

        match (&raw, &reference) {
            (None, Reference::Hashes(h)) => {
                if h[&tic].1 != actual_hash {
                    out.failure = Some(format!("frame mismatch at gametic {tic}"));
                    return out;
                }
            }
            (Some(raw), _) => {
                let mut expected = view_window(&r, &raw[..SCREENWIDTH * SCREENHEIGHT]);
                for (e, &u) in expected.iter_mut().zip(&undefined) {
                    if u {
                        *e = 0;
                    }
                }
                if expected != actual {
                    out.failure = Some(describe_mismatch(c, &r, tic, &expected, &actual, &screen));
                    return out;
                }
            }
            (None, Reference::Raw(_)) => unreachable!(),
        }
        out.frames.push((tic, fuzz, actual_hash));
        out.compared += 1;
    }
    out
}

fn describe_mismatch(c: &Content, r: &Renderer, tic: usize, expected: &[u8], actual: &[u8], screen: &[u8]) -> String {
    let bad = expected.iter().zip(actual).filter(|(a, b)| a != b).count();
    let first = expected.iter().zip(actual).position(|(a, b)| a != b).unwrap();
    let (vw, vh) = (r.scaledviewwidth as usize, r.viewheight as usize);
    if let Some(out) = std::env::var_os("FRAME_DIFF_OUT").map(PathBuf::from) {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(format!("{tic}_actual.raw")), screen).unwrap();
        let pal = c.rdata.palettes.base();
        write_ppm(&out.join(format!("{tic}_expected.ppm")), vw, vh, expected.iter().map(|&i| pal[i as usize]));
        write_ppm(&out.join(format!("{tic}_actual.ppm")), vw, vh, actual.iter().map(|&i| pal[i as usize]));
        let diff = expected.iter().zip(actual).map(|(&a, &b)| {
            if a == b {
                let g = pal[a as usize];
                [g[0] / 4, g[1] / 4, g[2] / 4]
            } else {
                [255, 0, 255]
            }
        });
        write_ppm(&out.join(format!("{tic}_diff.ppm")), vw, vh, diff);
    }
    format!(
        "frame mismatch at gametic {tic}: {bad} pixels differ, first at ({}, {}) expected {} got {}",
        first % vw,
        first / vw,
        expected[first],
        actual[first]
    )
}

/// Parsed fixture: demo id -> tic -> (fuzzpos, hash).
type Fixture = BTreeMap<String, HashMap<usize, (usize, u64)>>;

fn read_fixture(c: &Content, config: &str) -> Option<Fixture> {
    let text = std::fs::read_to_string(fixture_path(config)).ok()?;
    let Some(body) = text.strip_prefix(&format!("iwad {}\n", c.iwad_hash)) else {
        eprintln!("skipping {config}: fixture recorded with a different IWAD");
        return None;
    };
    let mut fx = Fixture::new();
    for l in body.lines() {
        let f: Vec<&str> = l.split(' ').collect();
        fx.entry(f[0].to_string())
            .or_default()
            .insert(f[1].parse().unwrap(), (f[2].parse().unwrap(), u64::from_str_radix(f[3], 16).unwrap()));
    }
    Some(fx)
}

fn write_fixture(c: &Content, config: &str, fx: &Fixture) {
    let mut s = format!("iwad {}\n", c.iwad_hash);
    for (demo, frames) in fx {
        let mut tics: Vec<_> = frames.iter().collect();
        tics.sort();
        for (tic, (fuzz, hash)) in tics {
            s += &format!("{demo} {tic} {fuzz} {hash:016x}\n");
        }
    }
    let p = fixture_path(config);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn config(name: &str) -> (i32, bool) {
    let &(_, blocks, low) = CONFIGS.iter().find(|c| c.0 == name).unwrap_or_else(|| panic!("unknown config {name}"));
    (blocks, low)
}

#[test]
fn frames_match_reference() {
    let Some(c) = content() else { return };

    // Raw mode: one (config, demo) from a dump directory.
    if let Some(dir) = std::env::var_os("REFDOOM_SHOTS").map(PathBuf::from) {
        let cfg = std::env::var("FRAME_CONFIG").unwrap_or_else(|_| "default".into());
        let id = std::env::var("FRAME_DEMO").expect("FRAME_DEMO");
        let (blocks, low) = config(&cfg);
        let demo = load_demo(&c, &id).unwrap_or_else(|| panic!("no demo {id}"));
        let out = run(&c, &demo, blocks, low, Reference::Raw(&dir));
        eprintln!("{cfg} {id}: {} frames match ({} undefined pixels excluded)", out.compared, out.excluded);
        if let Some(f) = out.failure {
            panic!("{cfg} {id}: {f}");
        }
        assert!(out.compared > 0, "{cfg} {id}: no reference frames in {}", dir.display());
        if std::env::var_os("WRITE_FRAME_FIXTURES").is_some() {
            let mut fx = read_fixture(&c, &cfg).unwrap_or_default();
            fx.insert(id, out.frames.into_iter().map(|(t, f, h)| (t, (f, h))).collect());
            write_fixture(&c, &cfg, &fx);
        }
        return;
    }

    let mut failures = Vec::new();
    for &(cfg, blocks, low) in &CONFIGS {
        let Some(fx) = read_fixture(&c, cfg) else {
            eprintln!("skipping {cfg}: no fixture");
            continue;
        };
        let (mut frames, mut excluded) = (0, 0);
        for (id, hashes) in &fx {
            let Some(demo) = load_demo(&c, id) else {
                eprintln!("skipping {cfg} {id}: demo not found");
                continue;
            };
            let out = run(&c, &demo, blocks, low, Reference::Hashes(hashes));
            frames += out.compared;
            excluded += out.excluded;
            if let Some(f) = out.failure {
                failures.push(format!("{cfg} {id}: {f}"));
            } else if out.compared != hashes.len() {
                failures.push(format!("{cfg} {id}: only {}/{} frames reached", out.compared, hashes.len()));
            }
        }
        eprintln!("{cfg}: {} demos, {frames} frames bit-exact ({excluded} undefined pixels excluded)", fx.len());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
