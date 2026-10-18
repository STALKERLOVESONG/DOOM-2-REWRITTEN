//! Hexen-style specials and ACS scripts acting on a real level (E1M1).
//! Skipped without an IWAD.

use std::path::PathBuf;

use doom_content::game::{identify, GameVersion};
use doom_content::map::{spac, LineAction};
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_core::Fixed;
use doom_script::acs::{pcode::*, AcsGlobals, AcsRuntime, Module};
use doom_sim::content::ContentInfo;
use doom_sim::hexen::{self, SpecialCtx, Trigger};
use doom_sim::info::Info;
use doom_sim::level::{Level, HEXEN_LINE};
use doom_sim::player::MAXPLAYERS;
use doom_sim::ticcmd::TicCmd;
use doom_sim::{GameParams, LineId, Skill, World};

fn world() -> Option<World> {
    let iwad = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad");
    if !iwad.exists() {
        eprintln!("skipping: no IWAD");
        return None;
    }
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let game = identify(&stack, None).unwrap();
    let level = Level::load(&stack, &textures, "E1M1").unwrap();
    let params = GameParams {
        mode: game.mode,
        version: GameVersion::Ultimate,
        skill: Skill::Medium,
        episode: 1,
        map: 1,
        deathmatch: 0,
        netgame: false,
        respawnmonsters: false,
        nomonsters: true,
        fastparm: false,
        consoleplayer: 0,
    };
    let mut pig = [false; MAXPLAYERS];
    pig[0] = true;
    Some(World::new_game(Info::doom(), ContentInfo::from_content(&stack, &textures), params, pig, level).unwrap())
}

fn run(w: &mut World, tics: usize) {
    for _ in 0..tics {
        w.tick(&[TicCmd::default(); MAXPLAYERS]);
    }
}

/// A tagged sector of E1M1 that nothing else moves, and its tag.
fn tagged_sector(w: &World) -> (usize, i32) {
    let i = (0..w.level.num_map_sectors()).find(|&i| w.level.sectors[i].tag != 0 && w.level.sectors[i].special_data.is_none()).unwrap();
    (i, w.level.sectors[i].tag as i32)
}

#[test]
fn floors_ceilings_lights_and_plats_move_by_tag() {
    let Some(mut w) = world() else { return };
    let (s, tag) = tagged_sector(&w);
    let floor = w.level.sectors[s].floor_height;
    let ctx = SpecialCtx::default();
    // Floor_LowerByValue(tag, speed 16 = 2 units/tic, 32 units)
    assert!(hexen::execute(&mut w, 20, [tag, 16, 32, 0, 0], &ctx));
    assert!(!hexen::execute(&mut w, 20, [tag, 16, 32, 0, 0], &ctx), "busy sectors are skipped");
    run(&mut w, 30);
    assert_eq!(w.level.sectors[s].floor_height, floor - Fixed::from_int(32));
    assert!(w.level.sectors[s].special_data.is_none(), "mover finished");
    // Floor_RaiseInstant moves in one tic.
    hexen::execute(&mut w, 67, [tag, 0, 4, 0, 0], &ctx);
    run(&mut w, 1);
    assert_eq!(w.level.sectors[s].floor_height, floor);
    // A ceiling and a floor can move at once (ZDoom), unlike vanilla.
    let ceiling = w.level.sectors[s].ceiling_height;
    hexen::execute(&mut w, 41, [tag, 64, 16, 0, 0], &ctx); // Ceiling_RaiseByValue
    hexen::execute(&mut w, 23, [tag, 64, 8, 0, 0], &ctx); // Floor_RaiseByValue
    run(&mut w, 10);
    assert_eq!(w.level.sectors[s].ceiling_height, ceiling + Fixed::from_int(16));
    assert_eq!(w.level.sectors[s].floor_height, floor + Fixed::from_int(8));
    assert!(w.level.sectors[s].ceiling_data.is_none());
    // Light_ChangeToValue and Light_Fade.
    hexen::execute(&mut w, 112, [tag, 200, 0, 0, 0], &ctx);
    assert_eq!(w.level.sectors[s].light_level, 200);
    hexen::execute(&mut w, 113, [tag, 100, 10, 0, 0], &ctx);
    run(&mut w, 5);
    assert_eq!(w.level.sectors[s].light_level, 150, "halfway through the fade");
    run(&mut w, 10);
    assert_eq!(w.level.sectors[s].light_level, 100);
    // Plat_UpByValue(tag, speed, delay, height/8): up, wait, down, done.
    let floor = w.level.sectors[s].floor_height;
    hexen::execute(&mut w, 65, [tag, 64, 5, 2, 0], &ctx);
    run(&mut w, 3);
    assert!(w.level.sectors[s].floor_height > floor);
    run(&mut w, 30);
    assert_eq!(w.level.sectors[s].floor_height, floor);
    assert!(w.level.sectors[s].special_data.is_none());
    assert!(w.unimplemented.is_empty(), "{:?}", w.unimplemented);
}

#[test]
fn hexen_lines_trigger_by_activation_and_clear_when_used() {
    let Some(mut w) = world() else { return };
    let (s, tag) = tagged_sector(&w);
    let floor = w.level.sectors[s].floor_height;
    let player = w.players[0].mo.unwrap();
    // Turn line 0 into a one-shot "use: Floor_LowerByValue" line.
    let line = LineId(0);
    let l = &mut w.level.lines[0];
    l.special = HEXEN_LINE;
    l.action = Some(LineAction { special: 20, args: [tag, 64, 24, 0, 0], activation: spac::USE, repeat: false, lock: 0 });
    assert!(!hexen::activate_line(&mut w, line, 0, Some(player), Trigger::Cross), "a use line ignores crossing");
    assert!(hexen::activate_line(&mut w, line, 0, Some(player), Trigger::Use));
    assert!(w.level.lines[0].action.is_none() && w.level.lines[0].special == 0, "one-shot line cleared");
    run(&mut w, 10);
    assert_eq!(w.level.sectors[s].floor_height, floor - Fixed::from_int(24));
    // Locked line: needs the blue card.
    let l = &mut w.level.lines[0];
    l.special = HEXEN_LINE;
    l.action = Some(LineAction { special: 23, args: [tag, 64, 24, 0, 0], activation: spac::USE, repeat: true, lock: 2 });
    assert!(!hexen::activate_line(&mut w, line, 0, Some(player), Trigger::Use));
    assert!(!w.script_events.is_empty(), "the player is told about the key");
    w.players[0].cards[0] = true;
    assert!(hexen::activate_line(&mut w, line, 0, Some(player), Trigger::Use));
    assert!(w.level.lines[0].action.is_some(), "repeatable line stays");
}

#[test]
fn teleport_to_tid_and_thing_specials() {
    let Some(mut w) = world() else { return };
    let player = w.players[0].mo.unwrap();
    // A MapSpot (TID 7) next to the start.
    let (x, y) = (w.mo(player).x + Fixed::from_int(64), w.mo(player).y);
    let spot = w.info.type_by_name("mapspot").unwrap();
    let id = hexen::spawn_at(&mut w, spot, x, y, Fixed(0), doom_core::Angle(0), 7, false).unwrap();
    assert_eq!(hexen::mobjs_by_tid(&w, 7), [id]);
    let ctx = SpecialCtx { activator: Some(player), ..Default::default() };
    // Teleport_NoFog(tid)
    assert!(hexen::execute(&mut w, 71, [7, 0, 0, 0, 0], &ctx));
    assert_eq!((w.mo(player).x, w.mo(player).y), (x, y));
    // Thing_ChangeTID(0 = activator, 3), then DamageThing by TID.
    hexen::execute(&mut w, 176, [0, 3, 0, 0, 0], &ctx);
    assert_eq!(w.mo(player).tid, 3);
    let health = w.mo(player).health;
    hexen::execute(&mut w, 119, [3, 10, 0, 0, 0], &ctx);
    assert!(w.mo(player).health < health);
    // Exit_Secret
    hexen::execute(&mut w, 244, [0; 5], &ctx);
    assert_eq!(w.gameexit, Some(doom_sim::world::GameExit::Secret));
}

/// ACS0 module: one OPEN script.
fn module(code: &[(u32, &[i32])]) -> Module {
    let mut d = b"ACS\0\0\0\0\0".to_vec();
    let start = d.len() as i32;
    for (p, ops) in code {
        d.extend_from_slice(&(*p as i32).to_le_bytes());
        for o in *ops {
            d.extend_from_slice(&o.to_le_bytes());
        }
    }
    let dir = d.len() as u32;
    d[4..8].copy_from_slice(&dir.to_le_bytes());
    for v in [1, 1000 + 1, start, 0, 0] {
        d.extend_from_slice(&(v as i32).to_le_bytes());
    }
    Module::load("test", d).unwrap()
}

#[test]
fn acs_runs_inside_the_tic_loop() {
    let Some(mut w) = world() else { return };
    let (s, tag) = tagged_sector(&w);
    let floor = w.level.sectors[s].floor_height;
    // OPEN: Floor_LowerByValue(tag, 32, 16); TagWait(tag); Light_ChangeToValue(tag, 255); Print(d: 42)
    let m = module(&[
        (LSPEC3DIRECT, &[20, tag, 32, 16]),
        (TAGWAITDIRECT, &[tag]),
        (LSPEC2DIRECT, &[112, tag, 255]),
        (BEGINPRINT, &[]),
        (PUSHNUMBER, &[42]),
        (PRINTNUMBER, &[]),
        (ENDPRINT, &[]),
        (TERMINATE, &[]),
    ]);
    w.level.sectors[s].light_level = 100;
    let mut rt = AcsRuntime::new(Some(m), vec![], |_| None, AcsGlobals::default());
    rt.start_kind(doom_script::acs::ScriptKind::Open, Default::default(), &[]);
    w.acs = Some(Box::new(rt));
    run(&mut w, 1);
    assert!(w.level.sectors[s].special_data.is_some(), "the script started the floor");
    assert_ne!(w.level.sectors[s].light_level, 255, "and waits for it");
    run(&mut w, 12);
    assert_eq!(w.level.sectors[s].floor_height, floor - Fixed::from_int(16));
    assert_eq!(w.level.sectors[s].light_level, 255);
    assert!(w.script_events.iter().any(|e| matches!(e, doom_sim::world::ScriptEvent::Print { text, .. } if text == "42")));
    assert_eq!(w.acs.as_ref().unwrap().running_scripts(), 0);
}
