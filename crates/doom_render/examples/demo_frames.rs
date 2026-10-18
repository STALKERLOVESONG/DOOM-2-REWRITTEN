//! Replay a demo and write rendered frames as PNG for visual inspection.
//!
//! cargo run -p doom_render --release --example demo_frames -- \
//!     <iwad> <DEMOn | path.lmp> <out dir> <screenblocks 3..11> <low 0|1> <gametic>...
//!
//! For each requested gametic it writes:
//! - `<demo>_t<tic>_native.png`: the 320x200 framebuffer as Doom draws it
//!   (palette 0; only the view window is drawn, there is no border or
//!   status bar yet).
//! - `<demo>_t<tic>_4x3.png`: the same, scaled 3x to 960x720 so pixels are
//!   1:1.2 as on a 4:3 CRT.
//!
//! The PNG writer uses stored (uncompressed) deflate blocks, so it needs no
//! dependencies.

use std::path::{Path, PathBuf};

use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_render::{Limits, RenderData, Renderer, SCREENHEIGHT, SCREENWIDTH};
use doom_sim::demo::Demo;
use doom_sim::info::Info;
use doom_sim::level::Level;
use doom_sim::{GameAction, GameParams, World};

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn write_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    // Raw scanlines with filter byte 0.
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * w * 3..(y + 1) * w * 3]);
    }
    // zlib stream with stored blocks.
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, c) in chunks.iter().enumerate() {
        z.push((i + 1 == chunks.len()) as u8);
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        png.extend_from_slice(&body);
        png.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, png).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 7 {
        eprintln!("usage: demo_frames <iwad> <DEMOn | file.lmp> <out dir> <screenblocks> <low 0|1> <gametic>...");
        std::process::exit(2);
    }
    let iwad = PathBuf::from(&args[1]);
    let demo_arg = &args[2];
    let out = PathBuf::from(&args[3]);
    let blocks: i32 = args[4].parse().expect("screenblocks");
    let low = args[5] == "1";
    let mut tics: Vec<i32> = args[6..].iter().map(|t| t.parse().expect("gametic")).collect();
    tics.sort();
    std::fs::create_dir_all(&out).unwrap();

    let stack = std::sync::Arc::new(ResourceStack::load(&iwad, &[]).expect("load IWAD"));
    let textures = TextureSet::load(&stack).expect("textures");
    let gi = doom_content::game::identify(&stack, None).expect("identify");
    let rdata = RenderData::build(&stack, &textures, &Info::doom().sprnames).expect("render data");
    let (demo, name) = match stack.lump(demo_arg) {
        Some(l) => (Demo::parse(l).unwrap(), demo_arg.clone()),
        None => (
            Demo::parse(&std::fs::read(demo_arg).expect("demo file")).unwrap(),
            Path::new(demo_arg).file_stem().unwrap().to_string_lossy().into_owned(),
        ),
    };
    let map = match gi.mode {
        doom_content::game::GameMode::Commercial => format!("MAP{:02}", demo.map),
        _ => format!("E{}M{}", demo.episode, demo.map),
    };
    let params = GameParams {
        mode: gi.mode,
        version: gi.version,
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
    let level = Level::load(&stack, &textures, &map).expect("level");
    let content = doom_sim::content::ContentInfo::from_content(&stack, &textures);
    let mut w = World::new_game(Info::doom(), content, params, demo.playeringame, level).unwrap();
    let mut r = Renderer::new(blocks, low, Limits::VANILLA);
    let mut screen = vec![0u8; SCREENWIDTH * SCREENHEIGHT];
    let pal = rdata.palettes.base();

    let mut next = tics.iter().peekable();
    for cmds in &demo.tics {
        let Some(&&want) = next.peek() else { break };
        match w.tick(cmds) {
            Some(GameAction::LoadLevel) => {
                w.setup_level(Level::load(&stack, &textures, &map).unwrap()).unwrap();
                w.tick(cmds);
            }
            Some(GameAction::Completed(_)) => {
                eprintln!("level exited at gametic {}; later tics not rendered", w.gametic);
                break;
            }
            None => {}
        }
        if w.gametic != want {
            continue;
        }
        next.next();
        screen.fill(0);
        r.render_player_view(&rdata, &w, w.params.consoleplayer, &mut screen);
        let rgb: Vec<u8> = screen.iter().flat_map(|&i| pal[i as usize]).collect();
        write_png(&out.join(format!("{name}_t{want}_native.png")), SCREENWIDTH, SCREENHEIGHT, &rgb);
        let (sw, sh) = (SCREENWIDTH * 3, 720);
        let mut big = Vec::with_capacity(sw * sh * 3);
        for y in 0..sh {
            let row = &rgb[(y * SCREENHEIGHT / sh) * SCREENWIDTH * 3..][..SCREENWIDTH * 3];
            for x in 0..sw {
                big.extend_from_slice(&row[x / 3 * 3..x / 3 * 3 + 3]);
            }
        }
        write_png(&out.join(format!("{name}_t{want}_4x3.png")), sw, sh, &big);
        let mo = w.mo(w.console_player().mo.unwrap());
        println!(
            "{name} gametic {want}: player at ({:.1}, {:.1}) angle {:.1} deg, overflows {:?}",
            mo.x.0 as f64 / 65536.0,
            mo.y.0 as f64 / 65536.0,
            mo.angle.0 as f64 * 360.0 / 4294967296.0,
            r.overflows
        );
    }
}
