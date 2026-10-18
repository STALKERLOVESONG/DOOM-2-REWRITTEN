//! Typed cheat codes (`m_cheat.c` behaviour: keys are matched against each
//! sequence as they are typed; any other key resets that sequence).

use crate::session::Session;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cheat {
    God,
    GiveAll,
    /// `idfa`: like `idkfa` without keys.
    GiveWeapons,
    NoClip,
    /// `idclev` + two digits.
    Warp(String),
    /// `iddt` (automap only): cycle map reveal.
    MapReveal,
}

const SEQUENCES: [(&str, fn(&str) -> Cheat); 7] = [
    ("iddqd", |_| Cheat::God),
    ("idkfa", |_| Cheat::GiveAll),
    ("idfa", |_| Cheat::GiveWeapons),
    ("idclip", |_| Cheat::NoClip),
    ("idspispopd", |_| Cheat::NoClip),
    ("idclev??", |args| Cheat::Warp(args.to_string())),
    ("iddt", |_| Cheat::MapReveal),
];

/// Feed typed characters; returns a cheat when a sequence completes.
#[derive(Clone, Debug, Default)]
pub struct CheatMatcher {
    /// Progress into each sequence, and captured `?` characters.
    progress: [(usize, String); SEQUENCES.len()],
}

impl CheatMatcher {
    pub fn feed(&mut self, c: char) -> Option<Cheat> {
        let c = c.to_ascii_lowercase();
        let mut hit = None;
        for (i, (seq, make)) in SEQUENCES.iter().enumerate() {
            let (pos, args) = &mut self.progress[i];
            let want = seq.as_bytes()[*pos] as char;
            if want == '?' {
                args.push(c);
                *pos += 1;
            } else if want == c {
                *pos += 1;
            } else {
                // Restart; the key may begin the sequence again.
                args.clear();
                *pos = usize::from(seq.as_bytes()[0] as char == c);
            }
            if *pos == seq.len() {
                if hit.is_none() {
                    hit = Some(make(args));
                }
                *pos = 0;
                args.clear();
            }
        }
        hit
    }
}

impl Session {
    /// Apply a cheat. Warp failures are shown as messages.
    pub fn apply_cheat(&mut self, cheat: &Cheat) {
        match cheat {
            Cheat::God => self.toggle_god(),
            Cheat::GiveAll => self.give_all(),
            Cheat::GiveWeapons => {
                let keys = self.world.console_player().cards;
                self.give_all();
                let cp = self.world.params.consoleplayer;
                self.world.players[cp].cards = keys;
                self.show_message("Ammo (no keys) Added");
            }
            Cheat::NoClip => self.toggle_noclip(),
            // Handled by the automap (see Session::key_down).
            Cheat::MapReveal => {}
            Cheat::Warp(digits) => {
                // Doom 1: episode digit + map digit; Doom II: two-digit map.
                let d: Vec<char> = digits.chars().collect();
                let arg = if self.game.mode == doom_content::game::GameMode::Commercial {
                    digits.clone()
                } else {
                    format!("{} {}", d[0], d[1])
                };
                if let Err(e) = self.warp(&arg) {
                    self.show_message(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Vec<Cheat> {
        let mut m = CheatMatcher::default();
        s.chars().filter_map(|c| m.feed(c)).collect()
    }

    #[test]
    fn sequences() {
        assert_eq!(typed("iddqd"), vec![Cheat::God]);
        assert_eq!(typed("xxIDKFAxx"), vec![Cheat::GiveAll]);
        assert_eq!(typed("idfa"), vec![Cheat::GiveWeapons]);
        assert_eq!(typed("ididclip"), vec![Cheat::NoClip]);
        assert_eq!(typed("idclev13"), vec![Cheat::Warp("13".into())]);
        assert_eq!(typed("iddq"), vec![]);
    }
}
