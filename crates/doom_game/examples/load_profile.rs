//! Time each stage of content loading for an IWAD + mods, to find what makes
//! big mods slow to start.
//!
//! cargo run -p doom_game --release --example load_profile -- <iwad> [mods...] [--map MAP01]

use std::path::PathBuf;
use std::time::Instant;

use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_render::RenderData;
use doom_sim::info::Info;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let map = args.iter().position(|a| a == "--map").map(|i| {
        let m = args[i + 1].clone();
        args.drain(i..i + 2);
        m
    });
    let args: Vec<PathBuf> = args.into_iter().map(PathBuf::from).collect();
    let mut t = Instant::now();
    let mut lap = |what: &str| {
        println!("{what:<24} {:>8.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        t = Instant::now();
    };
    let stack = std::sync::Arc::new(ResourceStack::load(&args[0], &args[1..]).unwrap());
    lap("resource stack");
    let textures = TextureSet::load(&stack).unwrap();
    lap("textures");
    let _mapinfo = doom_content::mapinfo::MapInfo::load(&stack);
    lap("mapinfo");
    let mut info = Info::doom();
    let _ = doom_sim::decorate::Decorate::apply_stack(&mut info, &stack);
    lap("decorate");
    let rd = RenderData::build(&stack, &textures, &info.sprnames).unwrap();
    lap("render data");
    println!("{} textures, {} sprite lumps", textures.len(), rd.sprite_lumps.len());
    let Some(map) = map else { return };
    let raw = doom_content::map::RawMap::load(&stack, &map).unwrap();
    lap("raw map (parse + nodes)");
    drop(raw);
    let level = doom_sim::Level::load(&stack, &textures, &map).unwrap();
    lap("level (raw + setup)");
    let content = doom_sim::content::ContentInfo::from_content(&stack, &textures);
    lap("content info");
    let game = doom_content::game::identify(&stack, None).unwrap();
    let params = doom_sim::GameParams {
        mode: game.mode,
        version: game.version,
        skill: doom_sim::Skill::Medium,
        episode: 1,
        map: 1,
        deathmatch: 0,
        netgame: false,
        respawnmonsters: false,
        nomonsters: false,
        fastparm: false,
        consoleplayer: 0,
    };
    let mut pig = [false; doom_sim::player::MAXPLAYERS];
    pig[0] = true;
    let _world = doom_sim::World::new_game(info, content, params, pig, level).unwrap();
    lap("world (spawn, specials)");
}
