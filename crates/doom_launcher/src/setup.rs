//! What the player picked, how it is saved between runs, and the command
//! line it becomes for `doom_bevy`. Plain std (no GUI), so it is testable
//! anywhere.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub iwad: Option<PathBuf>,
    /// IWADs the player browsed to (shown alongside the detected ones).
    pub extra_iwads: Vec<PathBuf>,
    /// Mods in load order (later ones override earlier ones).
    pub mods: Vec<PathBuf>,
    /// Empty: start at the title screen.
    pub map: String,
    /// 1..=5.
    pub skill: u8,
    pub classic: bool,
    pub fullscreen: bool,
    pub nomonsters: bool,
    pub fast: bool,
    pub respawn: bool,
    pub nosound: bool,
    /// Anything else for the game's command line (see `doom_bevy --help`).
    pub extra_args: String,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            iwad: None,
            extra_iwads: Vec::new(),
            mods: Vec::new(),
            map: String::new(),
            skill: 3,
            classic: false,
            fullscreen: false,
            nomonsters: false,
            fast: false,
            respawn: false,
            nosound: false,
            extra_args: String::new(),
        }
    }
}

pub const SKILLS: [&str; 5] = ["I'm too young to die", "Hey, not too rough", "Hurt me plenty", "Ultra-Violence", "Nightmare!"];

impl Setup {
    /// The game's command line (without the program).
    pub fn args(&self) -> Vec<OsString> {
        let mut a: Vec<OsString> = Vec::new();
        if let Some(iwad) = &self.iwad {
            a.push("--iwad".into());
            a.push(iwad.into());
        }
        for m in &self.mods {
            a.push("--pwad".into());
            a.push(m.into());
        }
        let map = self.map.trim();
        if !map.is_empty() {
            a.push("--map".into());
            a.push(map.into());
            a.push("--skill".into());
            a.push(self.skill.clamp(1, 5).to_string().into());
        }
        for (on, flag) in [
            (self.classic, "--classic"),
            (self.fullscreen, "--fullscreen"),
            (self.nomonsters, "--nomonsters"),
            (self.fast, "--fast"),
            (self.respawn, "--respawn"),
            (self.nosound, "--nosound"),
        ] {
            if on {
                a.push(flag.into());
            }
        }
        a.extend(split_args(&self.extra_args).into_iter().map(OsString::from));
        a
    }

    /// `key=value` lines; repeated keys for lists. Unknown keys are ignored
    /// so older launchers can read newer files.
    pub fn to_text(&self) -> String {
        let mut s = String::from("# doom_launcher settings (written on launch and exit)\n");
        let line = |s: &mut String, k: &str, v: &str| {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push('\n');
        };
        if let Some(i) = &self.iwad {
            line(&mut s, "iwad", &i.to_string_lossy());
        }
        for i in &self.extra_iwads {
            line(&mut s, "extra_iwad", &i.to_string_lossy());
        }
        for m in &self.mods {
            line(&mut s, "mod", &m.to_string_lossy());
        }
        line(&mut s, "map", &self.map);
        line(&mut s, "skill", &self.skill.to_string());
        for (k, v) in [
            ("classic", self.classic),
            ("fullscreen", self.fullscreen),
            ("nomonsters", self.nomonsters),
            ("fast", self.fast),
            ("respawn", self.respawn),
            ("nosound", self.nosound),
        ] {
            line(&mut s, k, if v { "1" } else { "0" });
        }
        line(&mut s, "extra_args", &self.extra_args);
        s
    }

    pub fn from_text(text: &str) -> Setup {
        let mut s = Setup::default();
        for l in text.lines() {
            let l = l.trim_end_matches('\r');
            if l.starts_with('#') {
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let flag = v.trim() == "1";
            match k.trim() {
                "iwad" if !v.is_empty() => s.iwad = Some(PathBuf::from(v)),
                "extra_iwad" if !v.is_empty() => s.extra_iwads.push(PathBuf::from(v)),
                "mod" if !v.is_empty() => s.mods.push(PathBuf::from(v)),
                "map" => s.map = v.to_string(),
                "skill" => s.skill = v.trim().parse().unwrap_or(3u8).clamp(1, 5),
                "classic" => s.classic = flag,
                "fullscreen" => s.fullscreen = flag,
                "nomonsters" => s.nomonsters = flag,
                "fast" => s.fast = flag,
                "respawn" => s.respawn = flag,
                "nosound" => s.nosound = flag,
                "extra_args" => s.extra_args = v.to_string(),
                _ => {}
            }
        }
        s
    }

    pub fn load(path: &Path) -> Setup {
        std::fs::read_to_string(path).map(|t| Setup::from_text(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_text())
    }
}

/// Split a command line on whitespace, keeping `"quoted parts"` together.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Setup {
        Setup {
            iwad: Some(PathBuf::from(r"C:\Games\Doom 2\doom2.wad")),
            extra_iwads: vec![PathBuf::from(r"D:\wads\tnt.wad")],
            mods: vec![PathBuf::from(r"C:\mods\myhouse.pk3"), PathBuf::from(r"C:\mods\fix.deh")],
            map: "MAP01".into(),
            skill: 4,
            fullscreen: true,
            fast: true,
            extra_args: r#"--scale 4 --config "my config.toml""#.into(),
            ..Default::default()
        }
    }

    #[test]
    fn command_line() {
        let a: Vec<String> = sample().args().iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(
            a,
            [
                "--iwad", r"C:\Games\Doom 2\doom2.wad", "--pwad", r"C:\mods\myhouse.pk3", "--pwad", r"C:\mods\fix.deh", "--map", "MAP01",
                "--skill", "4", "--fullscreen", "--fast", "--scale", "4", "--config", "my config.toml"
            ]
        );
        // No map: title screen, and no skill either.
        let s = Setup { iwad: Some("doom.wad".into()), ..Default::default() };
        assert_eq!(s.args(), vec![OsString::from("--iwad"), OsString::from("doom.wad")]);
    }

    #[test]
    fn settings_round_trip() {
        let s = sample();
        assert_eq!(Setup::from_text(&s.to_text()), s);
        assert_eq!(Setup::from_text("skill=9\nnonsense\nunknown=1\r\nmod=a.wad\r\n").skill, 5);
        assert_eq!(Setup::from_text("mod=a.wad\r\n").mods, vec![PathBuf::from("a.wad")]);
    }

    #[test]
    fn splitting() {
        assert_eq!(split_args(r#"  a "b c"  d"" "#), vec!["a", "b c", "d"]);
        assert_eq!(split_args(r#""""#), vec![""]);
        assert!(split_args("   ").is_empty());
    }
}
