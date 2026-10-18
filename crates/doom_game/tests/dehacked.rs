//! DEHACKED through a whole session: a loose `.deh` patch on the shareware
//! IWAD, and Freedoom's own DEHACKED lump. Skipped without the IWADs
//! (`DOOM_IWAD` / `WADS/doom1.wad`, `FREEDOOM2_IWAD` / `WADS/freedoom2.wad`).

use std::path::PathBuf;

use doom_game::ui::hud::map_title;
use doom_game::ui::intermission::WbStart;
use doom_game::{InputState, Key, Session, SessionConfig};
use doom_sim::info::*;
use doom_sim::player::cheat;

fn wad(file: &str, var: &str) -> Option<PathBuf> {
    let p = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS").join(file));
    p.exists().then_some(p)
}

const PATCH: &str = "Patch File for DeHackEd v3.0\r
Doom version = 19\r
Patch format = 6\r
\r
Thing 12 (Imp)\r
Hit points = 300\r
\r
Frame 1\r
Duration = 9\r
\r
Ammo 0 (Bullets)\r
Max ammo = 123\r
\r
Misc 0\r
Initial Health = 150\r
God Mode Health = 999\r
\r
Cheat 0\r
God mode = idgod\r
\r
Text 17 13\r
Picked up a clip.Ammo, hooray!\r
\r
[PARS]\r
par 1 1 77\r
\r
[STRINGS]\r
HUSTR_E1M1 = E1M1: Test Hangar\r
";

#[test]
fn loose_patch_file() {
    let Some(iwad) = wad("doom1.wad", "DOOM_IWAD") else { return };
    let dir = std::env::temp_dir().join(format!("doom_deh_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let deh = dir.join("test.deh");
    std::fs::write(&deh, PATCH).unwrap();
    let mut cfg = SessionConfig::new(iwad);
    cfg.pwads.push(deh);
    cfg.map = Some("E1M1".into());
    let mut s = Session::new(&cfg).unwrap();
    std::fs::remove_dir_all(&dir).ok();

    let deh_warnings: Vec<_> = s.warnings.iter().filter(|w| w.contains("DEHACKED")).collect();
    assert!(deh_warnings.is_empty(), "{deh_warnings:?}");
    assert_eq!(s.world.info.mobjinfo[MT_TROOP.idx()].spawnhealth, 300);
    assert_eq!(s.world.info.states[1].tics, 9);
    let p = s.world.console_player();
    assert_eq!((p.health, p.maxammo[0]), (150, 123));
    assert_eq!(s.world.info.deh.strings.get("Picked up a clip."), "Ammo, hooray!");
    assert_eq!(WbStart::par_time(s.world.params.mode, 1, 1, &s.world.info.deh.pars), 77 * 35);
    let game = doom_content::game::identify(&doom_content::ResourceStack::load(&cfg.iwad, &[]).unwrap(), None).unwrap();
    assert_eq!(map_title(&game, &s.world.info.deh.strings, 1, 1), "E1M1: Test Hangar");

    // The renamed god mode cheat, with DEHACKED's health.
    for c in "idgod".chars() {
        s.key_down(Key::Char(c));
    }
    s.tick(&InputState::default());
    let p = s.world.console_player();
    assert!(p.cheats & cheat::GODMODE != 0, "idgod should turn on god mode");
    assert_eq!(p.health, 999);
    // The old sequence no longer works.
    for c in "iddqd".chars() {
        s.key_down(Key::Char(c));
    }
    assert!(s.world.console_player().cheats & cheat::GODMODE != 0);
}

#[test]
fn freedoom_dehacked_lump() {
    let Some(iwad) = wad("freedoom2.wad", "FREEDOOM2_IWAD") else { return };
    let mut cfg = SessionConfig::new(iwad);
    cfg.map = Some("MAP01".into());
    let s = Session::new(&cfg).unwrap();
    let deh_warnings: Vec<_> = s.warnings.iter().filter(|w| w.contains("DEHACKED")).collect();
    assert!(deh_warnings.is_empty(), "{deh_warnings:?}");
    let info = &s.world.info;
    // Frame edits: the super shotgun flash and bright zombie firing frames.
    assert_eq!(info.states[47].tics, 4);
    assert_eq!(info.states[48].tics, 3);
    assert_eq!(info.states[185].frame, 32773);
    // [PARS] in the two-number Doom II form.
    assert_eq!(WbStart::par_time(s.world.params.mode, 1, 1, &info.deh.pars), 30 * 35);
    assert_eq!(WbStart::par_time(s.world.params.mode, 1, 32, &info.deh.pars), 210 * 35);
    // [STRINGS]: pickups (playsim) and level names (game layer).
    assert_eq!(info.deh.strings.get("Picked up the armor."), "Put on a force field vest.");
    let game = doom_content::game::identify(&doom_content::ResourceStack::load(&cfg.iwad, &[]).unwrap(), None).unwrap();
    assert_eq!(map_title(&game, &info.deh.strings, 1, 1), "MAP01: Hydroelectric Plant");
}
