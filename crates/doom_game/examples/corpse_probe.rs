//! Diagnostic: list corpse-like things in a map — authored corpse decorations
//! from the THINGS lump, and mobjs in a death/dead state after spawning.
use doom_game::{Session, SessionConfig};
use doom_sim::mobj::flags::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut cfg = SessionConfig::new(&args[1]);
    cfg.map = Some(args.get(2).cloned().unwrap_or("E1M1".into()));
    let skill: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    cfg.skill = [doom_sim::Skill::Baby, doom_sim::Skill::Easy, doom_sim::Skill::Medium, doom_sim::Skill::Hard, doom_sim::Skill::Nightmare][skill - 1];
    let mut s = Session::new(&cfg).unwrap();
    // Authored decorations: doomednums of the dead-thing decorations.
    let names = [(10, "bloody mess"), (12, "bloody mess 2"), (15, "dead player"), (18, "dead former human"),
        (19, "dead former sergeant"), (20, "dead imp"), (21, "dead demon"), (22, "dead cacodemon"),
        (23, "dead lost soul (invisible)"), (24, "pool of blood and flesh"), (79, "pool of blood"), (80, "pool of blood 2"), (81, "pool of brains")];
    let mut authored = std::collections::BTreeMap::new();
    for t in &s.world.level.things {
        if let Some((_, n)) = names.iter().find(|(k, _)| *k == t.kind as i32) {
            *authored.entry(*n).or_insert(0) += 1;
        }
    }
    println!("{}: authored corpse decorations in THINGS: {:?}", s.map_name(), authored);
    let report = |s: &Session, when: &str| {
        let (mut monsters, mut dead_monsters, mut corpse_flag) = (0, 0, 0);
        for (_, mo) in s.world.thinkers.mobjs() {
            let info = &s.world.info.mobjinfo[mo.mobj_type.0 as usize];
            if mo.flags & MF_CORPSE != 0 { corpse_flag += 1; }
            if info.flags & MF_COUNTKILL != 0 {
                monsters += 1;
                if mo.health <= 0 || mo.flags & MF_CORPSE != 0 { dead_monsters += 1; }
            }
        }
        println!("{when}: {monsters} monsters spawned, {dead_monsters} of them dead, {corpse_flag} mobjs with MF_CORPSE");
    };
    report(&s, "after spawn");
    for _ in 0..350 { s.tick(&Default::default()); }
    report(&s, "after 10 s idle");
}
