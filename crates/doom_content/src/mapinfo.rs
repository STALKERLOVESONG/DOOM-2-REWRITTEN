//! Map definitions from ZDoom `MAPINFO`/`ZMAPINFO` (old and new syntax)
//! and `UMAPINFO`, subset: level name, sky, music, next/secret-next map,
//! title patch. Other keys are ignored.

use std::collections::HashMap;

use crate::script::{Scanner, Token};
use crate::ResourceStack;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapEntry {
    /// Display name (`LOOKUP` strings are kept as their id).
    pub name: Option<String>,
    pub sky: Option<String>,
    pub music: Option<String>,
    pub next: Option<String>,
    pub secret_next: Option<String>,
    pub title_patch: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MapInfo {
    /// By upper-case map lump name.
    pub maps: HashMap<String, MapEntry>,
    /// `DoomEdNums { 12345 = ClassName }`: editor numbers for classes
    /// (how ZScript actors get placed in maps).
    pub doomednums: HashMap<i32, String>,
    pub warnings: Vec<String>,
}

impl MapInfo {
    /// Read every map definition lump. Like GZDoom, `ZMAPINFO` is used
    /// instead of `MAPINFO` when present; `UMAPINFO` entries are applied
    /// first so ZDoom definitions override them.
    pub fn load(stack: &ResourceStack) -> MapInfo {
        let mut mi = MapInfo::default();
        for &id in stack.all_named("UMAPINFO") {
            mi.parse(&String::from_utf8_lossy(stack.data(id)), stack, 0);
        }
        let z = stack.all_named("ZMAPINFO");
        let lumps = if z.is_empty() { stack.all_named("MAPINFO") } else { z };
        for &id in lumps {
            mi.parse(&String::from_utf8_lossy(stack.data(id)), stack, 0);
        }
        mi
    }

    pub fn get(&self, map: &str) -> Option<&MapEntry> {
        self.maps.get(&map.to_ascii_uppercase())
    }

    pub fn parse(&mut self, text: &str, stack: &ResourceStack, depth: u32) {
        let mut sc = Scanner::new(text);
        while let Some(t) = sc.next() {
            let Some(word) = t.text().map(str::to_ascii_lowercase) else { continue };
            match word.as_str() {
                "include" => {
                    let Some(path) = sc.next().and_then(|t| t.text().map(str::to_string)) else { continue };
                    let id = stack.find_path(&path).or_else(|| stack.check_num_for_name(&path));
                    match id {
                        Some(id) if depth < 8 => {
                            let text = String::from_utf8_lossy(stack.data(id)).into_owned();
                            self.parse(&text, stack, depth + 1);
                        }
                        _ => self.warnings.push(format!("MAPINFO: include {path:?} not found")),
                    }
                }
                "map" => self.map(&mut sc),
                "doomednums" if sc.eat('{') => loop {
                    let Some(t) = sc.next() else { break };
                    if t.is_punct('}') {
                        break;
                    }
                    let Token::Int(n) = t else { continue };
                    let line = sc.line();
                    if sc.eat('=') {
                        if let Some(class) = sc.next().and_then(|t| t.text().map(str::to_string)) {
                            self.doomednums.insert(n as i32, class);
                        }
                    }
                    // Optional special/args on the rest of the line.
                    while sc.line() == line && sc.peek().is_some_and(|t| !t.is_punct('}')) {
                        sc.next();
                    }
                },
                _ => {
                    // Other blocks (gameinfo, cluster, episode, ...) are skipped.
                    if sc.peek().is_some_and(|t| t.is_punct('{')) {
                        sc.skip_block();
                    }
                }
            }
        }
    }

    fn map(&mut self, sc: &mut Scanner) {
        let Some(lump) = sc.next().and_then(|t| t.text().map(str::to_ascii_uppercase)) else { return };
        let mut e = self.maps.get(&lump).cloned().unwrap_or_default();
        // Optional display name: "name", or lookup "ID".
        if sc.eat_word("lookup") {
            e.name = sc.next().and_then(|t| t.text().map(str::to_string));
        } else if let Some(Token::Str(_)) = sc.peek() {
            e.name = sc.next().and_then(|t| t.text().map(str::to_string));
        }
        let text = |sc: &mut Scanner| sc.next().and_then(|t| t.text().map(str::to_string));
        if sc.eat('{') {
            // New syntax: key = value[, more] until '}'.
            loop {
                let Some(t) = sc.next() else { break };
                if t.is_punct('}') {
                    break;
                }
                let Some(key) = t.text().map(str::to_ascii_lowercase) else { continue };
                let line = sc.line();
                if !sc.eat('=') {
                    continue; // flag keyword
                }
                match key.as_str() {
                    "sky1" | "skytexture" => e.sky = text(sc),
                    "music" => e.music = text(sc),
                    "next" => e.next = text(sc),
                    "secretnext" | "nextsecret" => e.secret_next = text(sc),
                    "titlepatch" | "levelpic" => e.title_patch = text(sc),
                    "levelname" => e.name = text(sc),
                    _ => {}
                }
                // Skip the rest of the value (", 0" etc.).
                while sc.line() == line && sc.peek().is_some_and(|t| !t.is_punct('}')) {
                    sc.next();
                }
            }
        } else {
            // Old syntax: keywords with values until the next top-level word.
            loop {
                let Some(t) = sc.peek().cloned() else { break };
                let Some(key) = t.text().map(str::to_ascii_lowercase) else {
                    sc.next();
                    continue;
                };
                match key.as_str() {
                    "map" | "defaultmap" | "cluster" | "episode" | "clearepisodes" | "gameinfo" | "include" | "skill" => break,
                    _ => {
                        sc.next();
                        match key.as_str() {
                            "sky1" => e.sky = text(sc),
                            "music" => e.music = text(sc),
                            "next" => e.next = text(sc),
                            "secretnext" => e.secret_next = text(sc),
                            "titlepatch" => e.title_patch = text(sc),
                            _ => {}
                        }
                    }
                }
            }
        }
        self.maps.insert(lump, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_syntaxes() {
        let stack = ResourceStack::new();
        let mut mi = MapInfo::default();
        mi.parse(
            "map MAP01 \"My House\"\n{\n  sky1 = \"SKY3\", 0\n  music = \"D_RUNNIN\"\n  next = \"MAP02\"\n  nointermission\n}\n\
             gameinfo { titlepage = \"TITLEPIC\" }\n\
             map MAP02 lookup HUSTR_2\nsky1 SKY2 0\nmusic D_STALKS\nsecretnext MAP31\nmap MAP03 \"Three\"\n",
            &stack,
            0,
        );
        let m1 = mi.get("map01").unwrap();
        assert_eq!(m1.name.as_deref(), Some("My House"));
        assert_eq!(m1.sky.as_deref(), Some("SKY3"));
        assert_eq!(m1.music.as_deref(), Some("D_RUNNIN"));
        assert_eq!(m1.next.as_deref(), Some("MAP02"));
        let m2 = mi.get("MAP02").unwrap();
        assert_eq!(m2.name.as_deref(), Some("HUSTR_2"));
        assert_eq!(m2.sky.as_deref(), Some("SKY2"));
        assert_eq!(m2.secret_next.as_deref(), Some("MAP31"));
        assert_eq!(mi.get("MAP03").unwrap().name.as_deref(), Some("Three"));
    }
}
