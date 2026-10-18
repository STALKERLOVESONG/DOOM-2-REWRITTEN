//! Map naming and level progression (`G_DoCompleted` / `G_WorldDone`).

use doom_content::game::GameMode;
use doom_content::ResourceStack;

/// A map slot: `ExMy` (episode, map) or `MAPxx` (episode 1, map).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapRef {
    pub episode: i32,
    pub map: i32,
}

impl MapRef {
    pub fn new(episode: i32, map: i32) -> Self {
        MapRef { episode, map }
    }

    /// The map's lump name for the game mode.
    pub fn lump_name(self, mode: GameMode) -> String {
        match mode {
            GameMode::Commercial => format!("MAP{:02}", self.map),
            _ => format!("E{}M{}", self.episode, self.map),
        }
    }

    /// Parse `E1M1`, `MAP07`, `e2m3`, `map7`, or the `-warp` forms `"1 3"`
    /// and `"7"`.
    pub fn parse(s: &str, mode: GameMode) -> Option<MapRef> {
        let t = s.trim().to_ascii_uppercase();
        // MAPxx names only exist in Doom II, ExMy only in Doom 1: a name
        // of the other kind is rejected rather than reinterpreted.
        if let Some(rest) = t.strip_prefix("MAP") {
            if mode != GameMode::Commercial {
                return None;
            }
            return rest.parse().ok().map(|m| MapRef::new(1, m));
        }
        if let Some(rest) = t.strip_prefix('E') {
            if mode == GameMode::Commercial {
                return None;
            }
            let (e, m) = rest.split_once('M')?;
            return Some(MapRef::new(e.parse().ok()?, m.parse().ok()?));
        }
        let nums: Vec<i32> = t.split_whitespace().map(|n| n.parse().ok()).collect::<Option<_>>()?;
        // Chocolate's -warp: Doom II takes a map number; Doom 1 takes an
        // episode and an optional map (default 1).
        match (nums.as_slice(), mode) {
            ([m], GameMode::Commercial) | ([_, m], GameMode::Commercial) => Some(MapRef::new(1, *m)),
            ([e], _) => Some(MapRef::new(*e, 1)),
            ([e, m], _) => Some(MapRef::new(*e, *m)),
            _ => None,
        }
    }

    pub fn exists(self, stack: &ResourceStack, mode: GameMode) -> bool {
        stack.check_num_for_name(&self.lump_name(mode)).is_some()
    }
}

/// Where the game goes after completing a level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Continue to this map, keeping the player's inventory.
    Map(MapRef),
    /// The episode (or game) ends here: vanilla shows the finale.
    EpisodeEnd,
}

/// `G_DoCompleted`'s choice of `wminfo.next`, plus the finale cases from
/// `G_WorldDone` (Doom 1 map 8, Doom II map 30).
pub fn next_map(mode: GameMode, cur: MapRef, secret: bool) -> Next {
    if mode == GameMode::Commercial {
        let next = match (cur.map, secret) {
            (30, _) => return Next::EpisodeEnd,
            (15, true) => 31,
            (31, true) => 32,
            (31, false) | (32, _) => 16,
            (m, _) => m + 1,
        };
        return Next::Map(MapRef::new(cur.episode, next));
    }
    if cur.map == 8 {
        return Next::EpisodeEnd;
    }
    if secret {
        return Next::Map(MapRef::new(cur.episode, 9));
    }
    if cur.map == 9 {
        // Returning from the secret level.
        let back = match cur.episode {
            1 => 4,
            2 => 6,
            3 => 7,
            4 => 3,
            _ => 1,
        };
        return Next::Map(MapRef::new(cur.episode, back));
    }
    Next::Map(MapRef::new(cur.episode, cur.map + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        assert_eq!(MapRef::parse("e2m3", GameMode::Retail), Some(MapRef::new(2, 3)));
        assert_eq!(MapRef::parse("MAP07", GameMode::Commercial), Some(MapRef::new(1, 7)));
        assert_eq!(MapRef::parse("3 4", GameMode::Retail), Some(MapRef::new(3, 4)));
        assert_eq!(MapRef::parse("12", GameMode::Commercial), Some(MapRef::new(1, 12)));
        assert_eq!(MapRef::parse("2", GameMode::Retail), Some(MapRef::new(2, 1)));
        assert_eq!(MapRef::parse("bogus", GameMode::Retail), None);
        // Names of the other game are rejected, not reinterpreted.
        assert_eq!(MapRef::parse("MAP01", GameMode::Retail), None);
        assert_eq!(MapRef::parse("E1M1", GameMode::Commercial), None);
        assert_eq!(MapRef::new(1, 3).lump_name(GameMode::Commercial), "MAP03");
    }

    #[test]
    fn progression() {
        use GameMode::*;
        assert_eq!(next_map(Retail, MapRef::new(1, 3), true), Next::Map(MapRef::new(1, 9)));
        assert_eq!(next_map(Retail, MapRef::new(1, 9), false), Next::Map(MapRef::new(1, 4)));
        assert_eq!(next_map(Retail, MapRef::new(4, 9), false), Next::Map(MapRef::new(4, 3)));
        assert_eq!(next_map(Retail, MapRef::new(2, 8), false), Next::EpisodeEnd);
        assert_eq!(next_map(Commercial, MapRef::new(1, 15), true), Next::Map(MapRef::new(1, 31)));
        assert_eq!(next_map(Commercial, MapRef::new(1, 31), false), Next::Map(MapRef::new(1, 16)));
        assert_eq!(next_map(Commercial, MapRef::new(1, 30), false), Next::EpisodeEnd);
    }
}
