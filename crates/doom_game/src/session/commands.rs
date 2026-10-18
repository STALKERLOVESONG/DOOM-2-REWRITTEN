//! The built-in console commands.
//!
//! Every handler is an ordinary function of the session and its arguments,
//! so tests call them through [`Session::exec`] exactly as typed.

use doom_core::{Angle, Fixed, FRACUNIT};
use doom_sim::ids::MobjId;
use doom_sim::info::MobjType;
use doom_sim::mobj::flags::{MF_COUNTKILL, MF_SHOOTABLE, MF_SPAWNCEILING, MF_SPECIAL};
use doom_sim::player::{cheat, PlayerState, Weapon, NUMWEAPONS, VIEWHEIGHT};

use super::{GameState, Session};
use crate::console::{suggest, CommandRegistry};
use crate::cvar;
use crate::maps::MapRef;

type R = Result<String, String>;

/// Register the engine's commands.
pub(super) fn register(r: &mut CommandRegistry) {
    // ---- console ----
    r.add("help", "[command|cvar]", "List commands, or describe one.", help);
    r.add("cmdlist", "[prefix]", "List commands.", |s, a| {
        let p = a.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let lines: Vec<String> =
            s.commands.iter().filter(|c| c.name.starts_with(&p)).map(|c| format!("{} {} - {}", c.name, c.usage, c.help)).collect();
        Ok(format!("{}\n{} commands", lines.join("\n"), lines.len()))
    });
    r.add("cvarlist", "[prefix]", "List console variables and their values.", |s, a| {
        let p = a.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let lines: Vec<String> = s.cvars.list().into_iter().filter(|v| v.name.starts_with(&p)).map(|v| format!("{} = {}", v.name, v.value)).collect();
        Ok(format!("{}\n{} variables", lines.join("\n"), lines.len()))
    });
    r.add("echo", "<text>", "Print text.", |_, a| Ok(a.join(" ")));
    r.add("clear", "", "Clear the console.", |s, _| {
        s.console.clear();
        Ok(String::new())
    });
    r.add("toggle", "<cvar>", "Flip a boolean variable.", |s, a| {
        let name = one(a, "toggle <cvar>")?;
        match s.cvars.value(name) {
            Some(crate::cvar::CVarValue::Bool(b)) => {
                s.cvars.set_str(name, if b { "0" } else { "1" }).map_err(|e| e.to_string())?;
                Ok(format!("{name} = {}", s.cvars.value(name).unwrap()))
            }
            Some(_) => Err(format!("{name} is not a boolean")),
            None => Err(format!("unknown variable {name}")),
        }
    });
    r.add("reset", "<cvar>", "Set a variable back to its default.", |s, a| {
        let name = one(a, "reset <cvar>")?;
        s.cvars.reset(name).map_err(|e| e.to_string())?;
        Ok(format!("{name} = {}", s.cvars.value(name).unwrap()))
    });
    r.add("quit", "", "Exit the game.", |s, _| {
        s.quit = true;
        Ok("bye".into())
    });

    // ---- items ----
    r.add("give", "<all|weapons|ammo|health|armor|backpack|keys|key <color>|item> [amount]", "Give items to the player.", give);
    r.set_completer("give", |s, i, _| if i == 0 { give_names(s) } else { Vec::new() });

    // ---- player state ----
    r.add("god", "[0|1]", "Toggle invulnerability.", |s, a| cheat_cmd(s, a, cheat::GODMODE, "god mode"));
    r.add("noclip", "[0|1]", "Toggle walking through walls.", |s, a| cheat_cmd(s, a, cheat::NOCLIP, "noclip"));
    r.add("notarget", "[0|1]", "Toggle being ignored by monsters.", |s, a| cheat_cmd(s, a, cheat::NOTARGET, "notarget"));
    r.add("kill", "[monsters|<class>]", "Kill yourself, or every monster (of a class).", |s, a| match a.first() {
        Some(what) if what.eq_ignore_ascii_case("monsters") => killall(s, &[]),
        Some(_) => killall(s, a),
        None => suicide(s),
    });
    r.set_completer("kill", |s, i, _| if i == 0 { std::iter::once("monsters".to_string()).chain(monster_names(s)).collect() } else { Vec::new() });
    r.add("suicide", "", "Kill yourself.", |s, _| suicide(s));
    r.add("respawn", "", "Come back to life (if dead) at the level's start, keeping your inventory.", respawn);

    // ---- world ----
    r.add("map", "<map>", "Start a new game on a map (E1M1, MAP01).", |s, a| {
        let name = one(a, "map <map>")?;
        let m = s.parse_map(name)?;
        s.new_game(m)?;
        Ok(format!("map {}", m.lump_name(s.game.mode)))
    });
    r.set_completer("map", |s, i, _| if i == 0 { map_names(s) } else { Vec::new() });
    r.add("warp", "<x> <y> [z] | <map>", "Move to map coordinates, or go to a map keeping your inventory.", warp);
    r.set_completer("warp", |s, i, _| if i == 0 { map_names(s) } else { Vec::new() });
    r.add("nextmap", "", "Go to the next map now (no intermission), keeping your inventory.", |s, _| {
        need_level(s)?;
        let m = match s.next_map() {
            super::Next::Map(m) => m,
            super::Next::EpisodeEnd => return Err("this is the last map of the episode".into()),
        };
        s.change_map(m)?;
        Ok(format!("map {}", m.lump_name(s.game.mode)))
    });
    r.add("restart", "", "Restart the current map from a pistol start.", |s, _| {
        if s.state != GameState::Level {
            return Err("no level is running".into());
        }
        let (m, skill) = (s.current, s.base_params.skill);
        s.new_game_with(m, skill)?;
        Ok(format!("restarted {}", m.lump_name(s.game.mode)))
    });
    r.add("killall", "[class]", "Kill every monster (or every actor of a class).", killall);
    r.set_completer("killall", |s, i, _| if i == 0 { monster_names(s) } else { Vec::new() });
    r.add("spawn", "<actor|doomednum> [count]", "Spawn an actor in front of you.", summon);
    r.add("summon", "<actor|doomednum> [count]", "Spawn an actor in front of you.", summon);
    r.set_completer("spawn", |s, i, _| if i == 0 { class_names(s) } else { Vec::new() });
    r.set_completer("summon", |s, i, _| if i == 0 { class_names(s) } else { Vec::new() });

    // ---- debug ----
    r.add("pause", "", "Pause the game.", |s, _| {
        s.set_paused(true);
        Ok("paused".into())
    });
    r.add("unpause", "", "Resume the game.", |s, _| {
        s.set_paused(false);
        Ok("unpaused".into())
    });
    r.add("timescale", "[speed]", "Game speed (1 = normal, 0.25 = slow motion).", |s, a| {
        if let Some(v) = a.first() {
            s.cvars.set_str(cvar::TIMESCALE.name, v).map_err(|e| e.to_string())?;
        }
        Ok(format!("timescale = {}", s.cvars.get(cvar::TIMESCALE)))
    });
    r.add("where", "", "Print the player's position.", |s, _| {
        let mo = player_mo(s)?;
        let m = s.world.mo(mo);
        let ang = m.angle.0 as f64 / 4294967296.0 * 360.0;
        Ok(format!("x {:.1}  y {:.1}  z {:.1}  angle {ang:.0}", fx(m.x), fx(m.y), fx(m.z)))
    });

    // ---- skateboard ----
    r.add("skate", "[0|1]", "Ride a skateboard (player_controller Skate) or walk.", |s, a| {
        let on = flag(a, s.controller_is_skate())?;
        s.cvars.set_str(cvar::PLAYER_CONTROLLER.name, if on { "Skate" } else { "Classic" }).map_err(|e| e.to_string())?;
        s.sync_cvars();
        s.sync_controller();
        Ok(if on { "skateboard on (W push, S brake, A/D carve, Space ollie)" } else { "skateboard off" }.into())
    });
    // Skate actions: queued for the controller's next tic, the same
    // actions the keys produce (see doom_sim::skate::SkateAction).
    r.add("jump", "", "Skate: ollie (or pop off a grind).", |s, a| skate_action(s, a, SkateAction::Jump));
    r.add("ollie", "", "Skate: ollie (same as jump).", |s, a| skate_action(s, a, SkateAction::Jump));
    r.add("push", "", "Skate: one push stroke.", |s, a| skate_action(s, a, SkateAction::Push { power: 1.0 }));
    r.add("brake", "[tics]", "Skate: brake (default 20 tics).", |s, a| {
        let tics = num_arg(a, 0, "tics")?.map_or(20, |v| v.max(1.0) as u32);
        skate_action(s, &[], SkateAction::Brake { tics })
    });
    r.add("bail", "", "Skate: fall off the board.", |s, a| skate_action(s, a, SkateAction::Bail));
    r.add("grind", "", "Skate: lock onto the nearest grindable edge in reach.", |s, a| skate_action(s, a, SkateAction::Grind));
    r.add("drop", "", "Skate: step off the grind.", |s, a| skate_action(s, a, SkateAction::Drop));
    r.add("spin", "<degrees>", "Skate: spin in the air (+ = left).", |s, a| {
        let deg = num_arg(a, 0, "degrees")?.ok_or("usage: spin <degrees>")?;
        skate_action(s, &[], SkateAction::Spin { degrees: deg })
    });
    r.add("set_speed", "<units/s>", "Skate: set the speed along the board.", |s, a| {
        let v = num_arg(a, 0, "speed")?.ok_or("usage: set_speed <units/s>")?;
        skate_action(s, &[], SkateAction::SetSpeed(v))
    });
    r.add("set_velocity", "<x> <y> <z>", "Skate: set the velocity (units/s; upward launches).", |s, a| {
        let usage = "usage: set_velocity <x> <y> <z>";
        if a.len() != 3 {
            return Err(usage.into());
        }
        let v = [num_arg(a, 0, "x")?.ok_or(usage)?, num_arg(a, 1, "y")?.ok_or(usage)?, num_arg(a, 2, "z")?.ok_or(usage)?];
        skate_action(s, &[], SkateAction::SetVelocity(v))
    });
    r.add("slot", "<0-9>", "Select a weapon slot, as pressing its key.", |s, a| {
        need_level(s)?;
        let n = one(a, "slot <0-9>")?;
        let slot: u8 = n.parse().ok().filter(|&v: &u8| v <= 9).ok_or_else(|| format!("slot must be 0-9, got {n}"))?;
        s.pending_weapon_slot = Some(slot);
        Ok(format!("slot {slot}"))
    });
    r.add("skate_grind", "[radius]", "Snap onto the nearest grindable edge.", |s, a| {
        need_level(s)?;
        let radius = match a.first() {
            Some(v) => v.parse::<f32>().map_err(|_| format!("bad radius {v}"))?,
            None => 96.0,
        };
        let cp = s.world.params.consoleplayer;
        if s.world.players[cp].skater.is_none() {
            return Err("not skating: use `skate 1` first".into());
        }
        s.ensure_grind_graph();
        let e = doom_sim::skate::force_grind(&mut s.world, cp, radius)?;
        let g = s.world.skate.grind.as_ref().expect("built");
        let edge = &g.edges[e as usize];
        Ok(format!("grinding {} #{e} ({:.0} long, line {})", edge.kind.name(), edge.length, edge.line.0))
    });
    r.add("rebuild_grindables", "", "Re-analyse the map's grindable edges.", |s, _| {
        need_level(s)?;
        s.grind_cache.clear();
        s.world.skate.grind = None;
        s.ensure_grind_graph();
        let g = s.world.skate.grind.as_ref().expect("built");
        let k = g.stats.by_kind;
        Ok(format!(
            "{} edges (ledge {}, curb {}, bar {}, rail {}), {} nodes, {} connections ({} corners) from {} lines in {:.2} ms",
            g.edges.len(),
            k[0],
            k[1],
            k[2],
            k[3],
            g.nodes.len(),
            g.stats.connections,
            g.stats.corners,
            g.stats.lines_checked,
            g.stats.build_micros as f64 / 1000.0
        ))
    });
}

// ---- helpers ---------------------------------------------------------------------

use doom_sim::skate::SkateAction;

/// Queue a skate action (no arguments expected in `a`).
fn skate_action(s: &mut Session, a: &[String], action: SkateAction) -> R {
    need_level(s)?;
    if !a.is_empty() {
        return Err(format!("{} takes no arguments", action.name()));
    }
    let cp = s.world.params.consoleplayer;
    if action == SkateAction::Grind {
        s.ensure_grind_graph();
    }
    doom_sim::skate::queue_action(&mut s.world, cp, action)?;
    Ok(format!("{}", action.name()))
}

/// Argument `i` as a number (`None` if absent).
fn num_arg(a: &[String], i: usize, what: &str) -> Result<Option<f32>, String> {
    match a.get(i) {
        None => Ok(None),
        Some(v) => v.parse::<f32>().map(Some).map_err(|_| format!("bad {what} '{v}'")),
    }
}

#[inline]
fn fx(v: Fixed) -> f32 {
    v.0 as f32 / FRACUNIT as f32
}

fn one<'a>(a: &'a [String], usage: &str) -> Result<&'a str, String> {
    match a {
        [x] => Ok(x),
        _ => Err(format!("usage: {usage}")),
    }
}

/// An optional on/off argument; none toggles `current`.
fn flag(a: &[String], current: bool) -> Result<bool, String> {
    match a.first().map(|s| s.to_ascii_lowercase()) {
        None => Ok(!current),
        Some(v) => match v.as_str() {
            "1" | "on" | "true" | "yes" => Ok(true),
            "0" | "off" | "false" | "no" => Ok(false),
            _ => Err(format!("expected 0 or 1, got {v}")),
        },
    }
}

fn need_level(s: &Session) -> Result<(), String> {
    if s.state != GameState::Level {
        return Err("no level is running".into());
    }
    if s.demo.is_some() {
        return Err("not during demo playback".into());
    }
    Ok(())
}

fn player_mo(s: &Session) -> Result<MobjId, String> {
    need_level(s)?;
    s.world.console_player().mo.ok_or_else(|| "the player has no body".into())
}

fn cheat_cmd(s: &mut Session, a: &[String], bit: i32, name: &str) -> R {
    need_level(s)?;
    let cp = s.world.params.consoleplayer;
    let on = flag(a, s.world.players[cp].cheats & bit != 0)?;
    let p = &mut s.world.players[cp];
    if on {
        p.cheats |= bit;
    } else {
        p.cheats &= !bit;
    }
    if bit == cheat::GODMODE && on {
        p.health = p.health.max(100);
        let h = p.health;
        if let Some(mo) = p.mo {
            s.world.mo_mut(mo).health = h;
        }
    }
    Ok(format!("{name} {}", if on { "ON" } else { "OFF" }))
}

fn help(s: &mut Session, a: &[String]) -> R {
    let Some(name) = a.first() else {
        let names: Vec<&str> = s.commands.names().collect();
        return Ok(format!(
            "Commands: {}\nAny console variable can be typed to show it, or with a value to set it (cvarlist lists them).\nTab completes; Up/Down recall history; PgUp/PgDn scroll.",
            names.join(" ")
        ));
    };
    if let Some(c) = s.commands.get(name) {
        return Ok(format!("{} {}\n  {}", c.name, c.usage, c.help));
    }
    if let Some(v) = s.cvars.info(name) {
        return Ok(describe_cvar(&v));
    }
    Err(unknown(s, name))
}

pub(super) fn describe_cvar(v: &crate::cvar::CVarInfo) -> String {
    let mut out = format!("{} = {}  (default {})", v.name, v.value, v.default);
    if !v.choices.is_empty() {
        out.push_str(&format!("  [{}]", v.choices.join("|")));
    } else if let Some((lo, hi)) = v.range {
        out.push_str(&format!("  [{lo}..{hi}]"));
    }
    if !v.help.is_empty() {
        out.push_str(&format!("\n  {}", v.help));
    }
    out
}

/// "unknown command" with suggestions from commands and variables.
pub(super) fn unknown(s: &Session, word: &str) -> String {
    let cvars = s.cvars.list();
    let names = s.commands.names().chain(cvars.iter().map(|v| v.name));
    let close = suggest(word, names, 4);
    if close.is_empty() {
        format!("unknown command or variable '{word}' (try help)")
    } else {
        format!("unknown command or variable '{word}'. Did you mean: {}?", close.join(", "))
    }
}

fn map_names(s: &Session) -> Vec<String> {
    doom_content::map::list_maps(&s.stack).iter().map(|n| n.to_string().to_ascii_lowercase()).collect()
}

fn class_names(s: &Session) -> Vec<String> {
    s.world.info.class_names.iter().filter(|n| !n.is_empty()).cloned().collect()
}

fn is_monster(info: &doom_sim::info::Info, t: MobjType) -> bool {
    let m = info.mobj(t);
    m.flags & MF_COUNTKILL != 0 || (m.flags & MF_SHOOTABLE != 0 && m.seestate.0 != 0)
}

fn monster_names(s: &Session) -> Vec<String> {
    let info = &s.world.info;
    (0..info.mobjinfo.len())
        .map(|i| MobjType(i as u16))
        .filter(|&t| is_monster(info, t) && t != doom_sim::info::MT_PLAYER)
        .map(|t| info.class_name(t).to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

const GIVE_WORDS: &[&str] = &["all", "weapons", "ammo", "health", "armor", "backpack", "keys", "key"];
const KEY_NAMES: &[&str] = &["bluecard", "yellowcard", "redcard", "blueskull", "yellowskull", "redskull"];
const AMMO_ALIASES: &[(&str, usize)] = &[
    ("bullets", 0),
    ("clip", 0),
    ("shells", 1),
    ("shell", 1),
    ("cells", 2),
    ("cell", 2),
    ("rockets", 3),
    ("rocketammo", 3),
];

fn give_names(s: &Session) -> Vec<String> {
    let info = &s.world.info;
    let pickups = (0..info.mobjinfo.len())
        .map(|i| MobjType(i as u16))
        .filter(|&t| info.mobj(t).flags & MF_SPECIAL != 0 || doom_sim::weapons::def(&s.world, t).is_some())
        .map(|t| info.class_name(t).to_string());
    GIVE_WORDS
        .iter()
        .map(|w| w.to_string())
        .chain(AMMO_ALIASES.iter().map(|(n, _)| n.to_string()))
        .chain(doom_sim::inventory::WEAPONS.iter().map(|(n, _)| n.to_string()))
        .chain(pickups)
        .filter(|n| !n.is_empty())
        .collect()
}

fn amount(a: &[String], i: usize) -> Result<Option<i32>, String> {
    match a.get(i) {
        None => Ok(None),
        Some(v) => v.parse::<i32>().map(Some).map_err(|_| format!("bad amount {v}")),
    }
}

// ---- give ------------------------------------------------------------------------

fn give(s: &mut Session, a: &[String]) -> R {
    let mo = player_mo(s)?;
    let Some(what) = a.first().map(|w| w.to_ascii_lowercase()) else {
        return Err("usage: give <all|weapons|ammo|health|armor|backpack|keys|key <color>|item> [amount]".into());
    };
    let cp = s.world.params.consoleplayer;
    match what.as_str() {
        "all" | "everything" => {
            let p = &mut s.world.players[cp];
            if !p.backpack {
                p.backpack = true;
                p.maxammo = p.maxammo.map(|m| m * 2);
            }
            p.health = p.health.max(100);
            let h = p.health;
            s.world.mo_mut(mo).health = h;
            s.give_all();
            Ok("gave everything".into())
        }
        "weapons" => {
            let p = &mut s.world.players[cp];
            p.weaponowned = [true; NUMWEAPONS];
            let classes: Vec<MobjType> = (0..s.world.info.mobjinfo.len())
                .map(|i| MobjType(i as u16))
                .filter(|&t| doom_sim::weapons::def(&s.world, t).is_some_and(|d| d.slot > 0))
                .collect();
            for t in classes {
                doom_sim::weapons::give_weapon(&mut s.world, cp, t, false);
            }
            Ok("gave all weapons".into())
        }
        "ammo" => {
            let p = &mut s.world.players[cp];
            match amount(a, 1)? {
                Some(n) => {
                    for (am, max) in p.ammo.iter_mut().zip(p.maxammo) {
                        *am = (*am + n).clamp(0, max);
                    }
                }
                None => p.ammo = p.maxammo,
            }
            Ok("gave ammo".into())
        }
        "health" => {
            let p = &mut s.world.players[cp];
            p.health = match amount(a, 1)? {
                Some(n) => (p.health + n).clamp(1, 999),
                None => p.health.max(100),
            };
            let h = p.health;
            s.world.mo_mut(mo).health = h;
            Ok(format!("health {h}"))
        }
        "armor" | "armour" => {
            let p = &mut s.world.players[cp];
            match amount(a, 1)? {
                Some(n) => {
                    p.armorpoints = n.clamp(0, 999);
                    p.armortype = if n > 100 { 2 } else { 1 };
                }
                None => {
                    p.armorpoints = p.armorpoints.max(200);
                    p.armortype = 2;
                }
            }
            Ok(format!("armor {}", p.armorpoints))
        }
        "backpack" => {
            let p = &mut s.world.players[cp];
            if !p.backpack {
                p.backpack = true;
                p.maxammo = p.maxammo.map(|m| m * 2);
            }
            Ok("gave backpack".into())
        }
        "keys" => {
            s.world.players[cp].cards = [true; 6];
            Ok("gave all keys".into())
        }
        "key" => {
            let Some(color) = a.get(1).map(|c| c.to_ascii_lowercase()) else {
                return Err("usage: give key <blue|yellow|red>[card|skull]".into());
            };
            let found: Vec<usize> = KEY_NAMES.iter().enumerate().filter(|(_, k)| k.starts_with(color.as_str())).map(|(i, _)| i).collect();
            if found.is_empty() {
                return Err(format!("no key called {color} (blue, yellow, red; card or skull)"));
            }
            for &k in &found {
                s.world.players[cp].cards[k] = true;
            }
            Ok(format!("gave {}", found.iter().map(|&k| KEY_NAMES[k]).collect::<Vec<_>>().join(", ")))
        }
        _ => give_item(s, cp, mo, &what, amount(a, 1)?),
    }
}

fn give_item(s: &mut Session, cp: usize, mo: MobjId, what: &str, n: Option<i32>) -> R {
    // Raw ammo counts.
    if let Some(&(_, am)) = AMMO_ALIASES.iter().find(|(name, _)| *name == what) {
        let p = &mut s.world.players[cp];
        let add = n.unwrap_or(p.maxammo[am]);
        p.ammo[am] = (p.ammo[am] + add).clamp(0, p.maxammo[am]);
        return Ok(format!("{what}: {}", p.ammo[am]));
    }
    if let Some(k) = KEY_NAMES.iter().position(|k| *k == what) {
        s.world.players[cp].cards[k] = true;
        return Ok(format!("gave {what}"));
    }
    if let Some(&(_, wpn)) = doom_sim::inventory::WEAPONS.iter().find(|(name, _)| *name == what) {
        doom_sim::inter::give_weapon(&mut s.world, cp, wpn, false);
        if wpn == Weapon::Fist {
            s.world.players[cp].weaponowned[Weapon::Fist as usize] = true;
        }
        return Ok(format!("gave {what}"));
    }
    let Some(t) = s.world.info.type_by_name(what) else {
        let close = suggest(what, give_names(s).iter().map(String::as_str), 4);
        return Err(if close.is_empty() {
            format!("no item called '{what}'")
        } else {
            format!("no item called '{what}'. Did you mean: {}?", close.join(", "))
        });
    };
    if doom_sim::weapons::def(&s.world, t).is_some() {
        doom_sim::weapons::give_weapon(&mut s.world, cp, t, true);
        return Ok(format!("gave {}", s.world.info.class_name(t)));
    }
    if s.world.info.mobj(t).flags & MF_SPECIAL == 0 {
        return Err(format!("{} is not an item (use summon to spawn actors)", s.world.info.class_name(t)));
    }
    // Hand over a pickup the way touching it would.
    let count = n.unwrap_or(1).clamp(1, 100);
    let mut got = 0;
    for _ in 0..count {
        let (x, y, z) = {
            let m = s.world.mo(mo);
            (m.x, m.y, m.z)
        };
        let item = doom_sim::mobj::spawn_mobj(&mut s.world, x, y, z, t);
        doom_sim::inter::touch_special_thing(&mut s.world, item, mo);
        if s.world.thinkers.is_removed(item) {
            got += 1;
        } else {
            doom_sim::mobj::remove_mobj(&mut s.world, item);
        }
    }
    let name = s.world.info.class_name(t).to_string();
    if got == 0 {
        return Err(format!("could not pick up {name} (already full?)"));
    }
    Ok(if got == 1 { format!("gave {name}") } else { format!("gave {name} x{got}") })
}

// ---- player ----------------------------------------------------------------------

fn suicide(s: &mut Session) -> R {
    let mo = player_mo(s)?;
    if s.world.mo(mo).health <= 0 {
        return Err("already dead".into());
    }
    doom_sim::inter::damage_mobj(&mut s.world, mo, None, None, 10000);
    Ok("you died".into())
}

fn respawn(s: &mut Session, _: &[String]) -> R {
    let mo = player_mo(s)?;
    let cp = s.world.params.consoleplayer;
    if s.world.players[cp].playerstate == PlayerState::Dead {
        // Resurrect: GZDoom's `resurrect`.
        let t = s.world.mo(mo).mobj_type;
        let (flags, height, spawn) = {
            let i = s.world.info.mobj(t);
            (i.flags, i.height, i.spawnstate)
        };
        let health = s.world.deh.initial_health;
        {
            let m = s.world.mo_mut(mo);
            m.flags = flags;
            m.height = height;
            m.health = health;
        }
        doom_sim::mobj::set_mobj_state(&mut s.world, mo, spawn);
        let p = &mut s.world.players[cp];
        p.playerstate = PlayerState::Live;
        p.health = health;
        p.viewheight = VIEWHEIGHT;
        p.deltaviewheight = Fixed(0);
        p.damagecount = 0;
        p.attacker = None;
        p.pendingweapon = p.readyweapon;
        doom_sim::pspr::setup_psprites(&mut s.world, cp);
    }
    // Back to the start.
    let Some(start) = s.world.playerstarts[cp] else { return Ok("resurrected".into()) };
    let (x, y) = (Fixed::from_int(start.x as i32), Fixed::from_int(start.y as i32));
    if doom_sim::map::teleport_move(&mut s.world, mo, x, y) {
        let m = s.world.mo_mut(mo);
        m.z = m.floorz;
        m.angle = Angle::from_mapthing_degrees(start.angle);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
        m.momz = Fixed(0);
        s.chase.valid = false;
        Ok("respawned at the start".into())
    } else {
        Err("the start spot is blocked".into())
    }
}

// ---- world -----------------------------------------------------------------------

fn warp(s: &mut Session, a: &[String]) -> R {
    let nums: Vec<f32> = a.iter().filter_map(|v| v.parse::<f32>().ok()).collect();
    if a.len() >= 2 && nums.len() == a.len() {
        let mo = player_mo(s)?;
        let (x, y) = (Fixed((nums[0] * FRACUNIT as f32) as i32), Fixed((nums[1] * FRACUNIT as f32) as i32));
        if !doom_sim::map::teleport_move(&mut s.world, mo, x, y) {
            return Err(format!("can't stand at {} {}", nums[0], nums[1]));
        }
        let m = s.world.mo_mut(mo);
        m.z = match nums.get(2) {
            Some(&z) => Fixed((z * FRACUNIT as f32) as i32).clamp(m.floorz, m.ceilingz - m.height),
            None => m.floorz,
        };
        s.chase.valid = false;
        return Ok(format!("warped to {} {}", nums[0], nums[1]));
    }
    let name = one(a, "warp <x> <y> [z] | warp <map>")?;
    let m = s.parse_map(name)?;
    if s.state == GameState::Level && s.usergame && s.demo.is_none() {
        s.change_map(m)?;
    } else {
        s.new_game(m)?;
    }
    Ok(format!("map {}", m.lump_name(s.game.mode)))
}

fn killall(s: &mut Session, a: &[String]) -> R {
    need_level(s)?;
    let class = match a.first() {
        Some(name) => Some(s.world.info.type_by_name(name).ok_or_else(|| {
            let close = suggest(name, monster_names(s).iter().map(String::as_str), 4);
            format!("no actor class '{name}'{}", if close.is_empty() { String::new() } else { format!(". Did you mean: {}?", close.join(", ")) })
        })?),
        None => None,
    };
    let info = &s.world.info;
    let victims: Vec<MobjId> = s
        .world
        .thinkers
        .mobjs()
        .filter(|(_, m)| m.player.is_none() && m.health > 0)
        .filter(|(_, m)| match class {
            Some(c) => info.is_a(m.mobj_type, c),
            None => is_monster(info, m.mobj_type) && m.flags & MF_SHOOTABLE != 0,
        })
        .map(|(id, _)| id)
        .collect();
    let mut n = 0;
    for id in victims {
        if s.world.thinkers.get_mobj(id).is_some_and(|m| m.health > 0) {
            doom_sim::inter::damage_mobj(&mut s.world, id, None, None, 1_000_000);
            n += 1;
        }
    }
    Ok(format!("killed {n}"))
}

fn summon(s: &mut Session, a: &[String]) -> R {
    let mo = player_mo(s)?;
    let Some(name) = a.first() else { return Err("usage: summon <actor|doomednum> [count]".into()) };
    let t = match name.parse::<i32>() {
        Ok(ed) => s.world.info.type_for_doomednum(ed).ok_or_else(|| format!("no actor with editor number {ed}"))?,
        Err(_) => s.world.info.type_by_name(name).ok_or_else(|| {
            let close = suggest(name, class_names(s).iter().map(String::as_str), 4);
            format!("no actor class '{name}'{}", if close.is_empty() { String::new() } else { format!(". Did you mean: {}?", close.join(", ")) })
        })?,
    };
    let count = amount(a, 1)?.unwrap_or(1).clamp(1, 64);
    let (px, py, pangle, pradius) = {
        let m = s.world.mo(mo);
        (fx(m.x), fx(m.y), m.angle, fx(m.radius))
    };
    let (radius, ceiling) = {
        let i = s.world.info.mobj(t);
        (fx(i.radius), i.flags & MF_SPAWNCEILING != 0)
    };
    let yaw = pangle.0 as f32 / 4294967296.0 * std::f32::consts::TAU;
    let mut spawned = 0;
    for k in 0..count {
        // In a row in front of the player.
        let dist = pradius + radius + 16.0 + k as f32 * (radius * 2.0 + 8.0);
        let (x, y) = (px + yaw.cos() * dist, py + yaw.sin() * dist);
        let z = if ceiling { doom_sim::mobj::ONCEILINGZ } else { doom_sim::mobj::ONFLOORZ };
        let id = doom_sim::mobj::spawn_mobj(&mut s.world, Fixed((x * FRACUNIT as f32) as i32), Fixed((y * FRACUNIT as f32) as i32), z, t);
        // Face the player.
        s.world.mo_mut(id).angle = pangle + Angle::A180;
        spawned += 1;
    }
    let cname = s.world.info.class_name(t).to_string();
    Ok(if spawned == 1 { format!("spawned {cname}") } else { format!("spawned {spawned} {cname}") })
}

impl Session {
    pub(super) fn parse_map(&self, name: &str) -> Result<MapRef, String> {
        let m = MapRef::parse(name, self.game.mode).ok_or_else(|| {
            let close = suggest(name, map_names(self).iter().map(String::as_str), 4);
            format!("bad map name '{name}'{}", if close.is_empty() { String::new() } else { format!(". Did you mean: {}?", close.join(", ")) })
        })?;
        if !m.exists(&self.stack, self.game.mode) {
            return Err(format!("map {} not found", m.lump_name(self.game.mode)));
        }
        Ok(m)
    }

    /// Go to `m` keeping the players' inventory (no intermission).
    pub(super) fn change_map(&mut self, m: MapRef) -> Result<(), String> {
        let level = self.load_level(m)?;
        self.scripted_exit = None;
        self.world.next_level(m.episode, m.map, level)?;
        self.current = m;
        self.start_level();
        Ok(())
    }

    pub(super) fn set_paused(&mut self, on: bool) {
        if self.paused != on {
            self.paused = on;
            self.sound.pause_music(on);
        }
    }
}
