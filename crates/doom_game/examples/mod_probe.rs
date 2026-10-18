//! Load an IWAD plus mods (WADs/PK3s) the way the game does and report what
//! the engine makes of them: content inventory, maps and their formats,
//! load warnings, and screenshots of the first map from the player start.
//!
//! cargo run -p doom_game --release --example mod_probe -- <out dir> <iwad> [pwad/pk3 ...] [--map MAP01] [--walk TICS]
//!
//! Paste its text output when reporting a mod that does not load or play.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use doom_content::map::{detect_format, list_maps};
use doom_content::{LumpId, Namespace, ResourceStack};
use doom_game::{png, InputState, Session, SessionConfig};

fn kind_of(data: &[u8]) -> &'static str {
    match data {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xff, 0xd8, ..] => "jpeg",
        [b'O', b'g', b'g', b'S', ..] => "ogg",
        [b'R', b'I', b'F', b'F', ..] => "wav",
        [b'f', b'L', b'a', b'C', ..] => "flac",
        [b'I', b'D', b'3', ..] | [0xff, 0xfb, ..] => "mp3",
        [b'M', b'U', b'S', 0x1a, ..] => "mus",
        [b'M', b'T', b'h', b'd', ..] => "midi",
        [3, 0, ..] => "dmx-sound",
        [b'I', b'W', b'A', b'D', ..] | [b'P', b'W', b'A', b'D', ..] => "wad",
        [b'E', b'x', b't', b'e', b'n', b'd', b'e', b'd', ..] => "xm",
        [b'I', b'M', b'P', b'M', ..] => "it",
        _ => "other",
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut map = None;
    if let Some(i) = args.iter().position(|a| a == "--map") {
        map = args.get(i + 1).cloned();
        args.drain(i..(i + 2).min(args.len()));
    }
    let mut walk = 0usize;
    if let Some(i) = args.iter().position(|a| a == "--walk") {
        walk = args.get(i + 1).and_then(|w| w.parse().ok()).unwrap_or(0);
        args.drain(i..(i + 2).min(args.len()));
    }
    if args.len() < 2 {
        eprintln!("usage: mod_probe <out dir> <iwad> [pwad/pk3 ...] [--map MAP01]");
        std::process::exit(2);
    }
    let out = PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).unwrap();
    let iwad = PathBuf::from(&args[1]);
    let pwads: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();

    // --- content inventory ------------------------------------------------
    let t = Instant::now();
    let stack = match ResourceStack::load(&iwad, &pwads) {
        Ok(s) => s,
        Err(e) => {
            println!("LOAD FAILED: {e}");
            std::process::exit(1);
        }
    };
    println!("loaded {} lumps from {} sources in {:.2}s", stack.num_lumps(), stack.sources().len(), t.elapsed().as_secs_f64());
    for s in stack.sources() {
        println!("  source: {} ({:?})", s.path.display(), s.kind);
    }
    for ns in [Namespace::Sprites, Namespace::Flats, Namespace::Patches, Namespace::Textures, Namespace::Colormaps, Namespace::Acs, Namespace::Voxels, Namespace::Hires] {
        let n = stack.namespace(ns).len();
        if n > 0 {
            println!("  namespace {ns:?}: {n} entries");
        }
    }
    let special = [
        "DECORATE", "ZSCRIPT", "MAPINFO", "ZMAPINFO", "UMAPINFO", "TEXTURES", "SNDINFO", "SNDSEQ", "ANIMDEFS", "ANIMATED",
        "SWITCHES", "GLDEFS", "LANGUAGE", "KEYCONF", "SBARINFO", "MENUDEF", "DEHACKED", "LOCKDEFS", "DECALDEF", "TERRAIN",
        "MODELDEF", "VOXELDEF", "CVARINFO", "LOADACS", "GAMEINFO", "FONTDEFS", "TRNSLATE", "REVERBS",
    ];
    let present: Vec<String> = special
        .iter()
        .filter_map(|n| {
            let c = stack.all_named(n).len();
            (c > 0).then(|| if c > 1 { format!("{n} x{c}") } else { n.to_string() })
        })
        .collect();
    println!("  definition lumps: {}", if present.is_empty() { "none".into() } else { present.join(", ") });
    // Formats of archive files by top-level directory.
    let mut formats: BTreeMap<(String, &str), usize> = BTreeMap::new();
    for i in 0..stack.num_lumps() {
        let id = LumpId(i as u32);
        let info = stack.info(id);
        let Some(path) = info.path.as_deref() else { continue };
        let dir = path.split('/').next().filter(|_| path.contains('/')).unwrap_or("(root)").to_ascii_lowercase();
        // Only sniff the first bytes of big entries' kinds via data(): this
        // decompresses, which is fine for a report.
        *formats.entry((dir, kind_of(stack.data(id)))).or_default() += 1;
    }
    if !formats.is_empty() {
        println!("  archive contents (directory, format, files):");
        for ((dir, kind), n) in &formats {
            println!("    {dir:<14} {kind:<10} {n}");
        }
    }
    let maps = list_maps(&stack);
    println!("  maps: {}", maps.len());
    for m in &maps {
        let id = stack.check_num(*m).unwrap();
        println!("    {m}: {:?}", detect_format(&stack, id));
    }
    for w in stack.warnings.iter().take(20) {
        println!("  content warning: {w}");
    }
    for w in stack.take_late_warnings().iter().take(20) {
        println!("  decode warning: {w}");
    }

    // --- run it -------------------------------------------------------------
    let mut cfg = SessionConfig::new(&iwad);
    cfg.pwads = pwads.clone();
    // Default: the first map a mod defines, else the IWAD's first.
    let mod_map = maps.iter().find(|m| stack.info(stack.check_num(**m).unwrap()).source > 0);
    cfg.map = map.or_else(|| mod_map.or(maps.first()).map(|m| m.to_string()));
    let t = Instant::now();
    let mut s = match Session::new(&cfg) {
        Ok(s) => s,
        Err(e) => {
            println!("SESSION FAILED: {e}");
            std::process::exit(1);
        }
    };
    println!("\nsession on {} ready in {:.2}s; {} warnings", s.map_name(), t.elapsed().as_secs_f64(), s.warnings.len());
    let mut grouped: BTreeMap<String, usize> = BTreeMap::new();
    for w in &s.warnings {
        // Group repeated messages ("texture X not found" -> one line each).
        *grouped.entry(w.clone()).or_default() += 1;
    }
    // MOD_PROBE_ALL=1 prints every distinct warning.
    let limit = if std::env::var_os("MOD_PROBE_ALL").is_some() { usize::MAX } else { 60 };
    for (w, n) in grouped.iter().take(limit) {
        if *n > 1 {
            println!("  warning (x{n}): {w}");
        } else {
            println!("  warning: {w}");
        }
    }
    if grouped.len() > 60 {
        println!("  ... {} more distinct warnings", grouped.len() - 60);
    }
    let level = &s.world.level;
    println!(
        "  level: {} vertices, {} lines, {} sides, {} sectors, {} segs, {} subsectors, {} nodes, {} things",
        level.vertexes.len(),
        level.lines.len(),
        level.sides.len(),
        level.num_map_sectors(),
        level.segs.len(),
        level.subsectors.len(),
        level.nodes.len(),
        level.things.len()
    );
    let mobjs = s.world.thinkers.mobjs().count();
    println!("  spawned {mobjs} map objects");
    let turn = InputState { turn_right: true, ..Default::default() };
    let idle = InputState::default();
    for _ in 0..5 {
        s.tick(&idle);
    }
    let forward = InputState { forward: true, ..Default::default() };
    for _ in 0..walk {
        s.tick(&forward);
    }
    let mut rgba = vec![0u8; 320 * 200 * 4];
    for view in 0..4 {
        let t = Instant::now();
        s.render_rgba(&mut rgba);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let path = out.join(format!("view{view}.png"));
        std::fs::write(&path, png::encode_rgba(320, 200, &rgba)).unwrap();
        println!("  wrote {} ({ms:.1} ms)", path.display());
        // About 90 degrees of keyboard turning.
        for _ in 0..13 {
            s.tick(&turn);
        }
    }
    for w in s.render_data.warnings.iter().take(5) {
        println!("  render warning: {w}");
    }
    // Scripts and gaps met while playing.
    match &s.world.acs {
        Some(acs) => println!(
            "  ACS: {} modules, {} scripts running after {} tics",
            acs.modules.len(),
            acs.running_scripts(),
            s.world.leveltime
        ),
        None => println!("  ACS: none"),
    }
    for w in s.world.load_warnings.iter().filter(|w| w.starts_with("ACS")).take(10) {
        println!("  {w}");
    }
    if !s.world.unimplemented.is_empty() {
        println!("  not implemented yet (hits):");
        for (what, n) in &s.world.unimplemented {
            println!("    {what}: {n}");
        }
    }
}
