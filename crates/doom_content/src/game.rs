//! Game identification (d_main.c `D_IdentifyVersion`, `InitGameVersion`).
//!
//! The playsim has a handful of version-dependent behaviours (lost soul
//! bounce, teleport z, intermission details ...), so which executable we
//! emulate is an explicit, user-overridable setting rather than something
//! inferred silently in gameplay code.

use crate::resources::ResourceStack;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameMode {
    Shareware,
    Registered,
    /// Ultimate Doom (4 episodes).
    Retail,
    /// Doom II / Final Doom.
    Commercial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameMission {
    Doom,
    Doom2,
    Tnt,
    Plutonia,
}

/// Emulated executable (`GameVersion_t`), ordered as in Chocolate Doom so
/// `>=` comparisons mean the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GameVersion {
    Doom1_2,
    Doom1_5,
    Doom1_666,
    Doom1_7,
    Doom1_8,
    Doom1_9,
    Hacx,
    Ultimate,
    Final,
    Final2,
    Chex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameInfo {
    pub mode: GameMode,
    pub mission: GameMission,
    pub version: GameVersion,
}

/// Identify the game from content (the IWAD is assumed to be loaded first).
///
/// Mission: the first `MAP01` or `E1M1` lump decides Doom II vs Doom (as
/// Chocolate does for IWADs with unrecognised file names). TNT/Plutonia
/// cannot be told apart from Doom II by lump names alone; pass a hint (e.g.
/// from the IWAD file name) via `mission_hint`.
pub fn identify(stack: &ResourceStack, mission_hint: Option<GameMission>) -> Option<GameInfo> {
    let mission = mission_hint.or_else(|| {
        (0..stack.num_lumps()).find_map(|i| {
            match stack.info(crate::LumpId(i as u32)).name.as_bytes() {
                b"MAP01" => Some(GameMission::Doom2),
                b"E1M1" => Some(GameMission::Doom),
                _ => None,
            }
        })
    })?;

    let mode = match mission {
        GameMission::Doom if stack.check_num_for_name("E4M1").is_some() => GameMode::Retail,
        GameMission::Doom if stack.check_num_for_name("E3M1").is_some() => GameMode::Registered,
        GameMission::Doom => GameMode::Shareware,
        _ => GameMode::Commercial,
    };

    let version = match (mode, mission) {
        (GameMode::Retail, _) => GameVersion::Ultimate,
        (GameMode::Commercial, GameMission::Tnt | GameMission::Plutonia) => GameVersion::Final,
        _ => demo_version(stack).unwrap_or(GameVersion::Doom1_9),
    };
    Some(GameInfo { mode, mission, version })
}

/// Chocolate Doom's heuristic: the version byte of the first DEMOn lump.
fn demo_version(stack: &ResourceStack) -> Option<GameVersion> {
    for i in 1..=3 {
        let Some(d) = stack.lump(&format!("DEMO{i}")) else { continue };
        let v = match d.first()? {
            0..=4 => GameVersion::Doom1_2,
            106 => GameVersion::Doom1_666,
            107 => GameVersion::Doom1_7,
            108 => GameVersion::Doom1_8,
            109 => GameVersion::Doom1_9,
            _ => continue,
        };
        return Some(v);
    }
    None
}
