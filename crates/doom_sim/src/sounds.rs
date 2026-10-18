//! Sound definitions: logical sound names (SNDINFO) and the resources
//! they play.
//!
//! Gameplay asks for sounds by [`SfxNum`]; actors, weapons and scripts
//! name them logically (`weapons/plasmaf`, `imp/sight1`, `AR15_FP4`) and
//! [`Info::sound`] turns the name into the number once, when definitions
//! load. This module builds that mapping and records, for every sound a
//! mod defines or overrides, the concrete resource it plays
//! ([`SoundDef`]): resolved once against the resource stack (WAD lumps,
//! long PK3 file names and full paths), so playback never searches by
//! name. Doom's own sounds have ZDoom's logical names too, so a mod that
//! redefines `weapons/pistol` changes what Doom's pistol plays.
//!
//! The playsim never decodes audio; the game layer turns a [`SoundDef`]'s
//! resource into PCM (`doom_audio::decode`) and caches it.

use std::collections::HashMap;

use doom_content::{LumpId, ResourceStack};

use crate::info::{Info, SfxInfo, SfxNum};

/// What a mod defined for a sound.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundDef {
    /// The resource it plays (`None`: named but missing; silent).
    pub lump: Option<LumpId>,
    /// The resource as written in SNDINFO.
    pub resource: String,
    /// `$random`: play one of these instead.
    pub random: Vec<SfxNum>,
    /// `$volume` (0..=1).
    pub volume: f32,
    /// `$attenuation`: distance scale (0 = heard everywhere at full volume).
    pub attenuation: f32,
    /// `$limit`: at most this many playing at once (0 = no limit).
    pub limit: u8,
    /// `$singular`: only one at a time, anywhere.
    pub singular: bool,
    /// `$pitchshift` (recorded; pitch shifting is off, as in Chocolate Doom).
    pub pitch_shift: i8,
}

impl Default for SoundDef {
    fn default() -> Self {
        SoundDef { lump: None, resource: String::new(), random: Vec::new(), volume: 1.0, attenuation: 1.0, limit: 0, singular: false, pitch_shift: 0 }
    }
}

/// How an ambient sound repeats (`$ambient`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmbientMode {
    /// Loops for as long as the thing exists.
    Continuous,
    /// Again after a random wait, in tics.
    Random { min: u32, max: u32 },
    /// Again every so many tics.
    Periodic { every: u32 },
}

/// An ambient sound definition (`$ambient`), played by map things
/// 14001-14064 (`AmbientSound`, numbered) and 14065 (`AmbientSoundCustom`,
/// numbered by its first argument).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientDef {
    pub sound: SfxNum,
    /// Comes from the thing's position (else everywhere).
    pub point: bool,
    pub attenuation: f32,
    pub mode: AmbientMode,
    pub volume: f32,
}

/// First editor number of the numbered ambient sound things.
pub const AMBIENT_DOOMEDNUM: i32 = 14000;
/// `AmbientSoundCustom`: the definition number is its first argument.
pub const AMBIENT_CUSTOM_DOOMEDNUM: i32 = 14065;

/// ZDoom's logical names for Doom's sounds (its SNDINFO for Doom), so mods
/// can play and redefine them by those names.
pub const DOOM_LOGICAL_NAMES: &[(&str, &str)] = &[
    ("weapons/pistol", "pistol"),
    ("weapons/shotgf", "shotgn"),
    ("weapons/shotgr", "sgcock"),
    ("weapons/sshotf", "dshtgn"),
    ("weapons/sshoto", "dbopn"),
    ("weapons/sshotc", "dbcls"),
    ("weapons/sshotl", "dbload"),
    ("weapons/chngun", "pistol"),
    ("weapons/rocklx", "barexp"),
    ("weapons/rocklf", "rlaunc"),
    ("weapons/plasmaf", "plasma"),
    ("weapons/plasmax", "firxpl"),
    ("weapons/bfgf", "bfg"),
    ("weapons/bfgx", "rxplod"),
    ("weapons/sawup", "sawup"),
    ("weapons/sawidle", "sawidl"),
    ("weapons/sawfull", "sawful"),
    ("weapons/sawhit", "sawhit"),
    ("weapons/pickup", "wpnup"),
    ("misc/w_pkup", "wpnup"),
    ("misc/p_pkup", "getpow"),
    ("misc/i_pkup", "itemup"),
    ("misc/k_pkup", "itemup"),
    ("misc/spawn", "itmbk"),
    ("misc/teleport", "telept"),
    ("misc/gibbed", "slop"),
    ("misc/keytry", "oof"),
    ("misc/chat", "radio"),
    ("misc/chat2", "tink"),
    ("world/barrelx", "barexp"),
    ("plats/pt1_strt", "pstart"),
    ("plats/pt1_stop", "pstop"),
    ("plats/pt1_mid", "stnmov"),
    ("doors/dr1_open", "doropn"),
    ("doors/dr1_clos", "dorcls"),
    ("doors/dr2_open", "bdopn"),
    ("doors/dr2_clos", "bdcls"),
    ("switches/normbutn", "swtchn"),
    ("switches/exitbutn", "swtchx"),
    ("menu/activate", "swtchn"),
    ("menu/backup", "swtchn"),
    ("menu/prompt", "swtchn"),
    ("menu/cursor", "pstop"),
    ("menu/change", "stnmov"),
    ("menu/invalid", "oof"),
    ("menu/dismiss", "swtchx"),
    ("menu/choose", "pistol"),
    ("menu/clear", "swtchx"),
    ("player/male/death1", "pldeth"),
    ("player/male/xdeath1", "pdiehi"),
    ("player/male/pain100", "plpain"),
    ("player/male/grunt1", "oof"),
    ("player/male/usefail", "noway"),
    ("player/male/land1", "oof"),
    ("grunt/sight1", "posit1"),
    ("grunt/sight2", "posit2"),
    ("grunt/sight3", "posit3"),
    ("grunt/active", "posact"),
    ("grunt/pain", "popain"),
    ("grunt/death1", "podth1"),
    ("grunt/death2", "podth2"),
    ("grunt/death3", "podth3"),
    ("grunt/attack", "pistol"),
    ("shotguy/sight1", "posit1"),
    ("shotguy/sight2", "posit2"),
    ("shotguy/sight3", "posit3"),
    ("shotguy/active", "posact"),
    ("shotguy/pain", "popain"),
    ("shotguy/death1", "podth1"),
    ("shotguy/death2", "podth2"),
    ("shotguy/death3", "podth3"),
    ("shotguy/attack", "shotgn"),
    ("chainguy/sight1", "posit1"),
    ("chainguy/sight2", "posit2"),
    ("chainguy/sight3", "posit3"),
    ("chainguy/active", "posact"),
    ("chainguy/pain", "popain"),
    ("chainguy/death1", "podth1"),
    ("chainguy/death2", "podth2"),
    ("chainguy/death3", "podth3"),
    ("chainguy/attack", "pistol"),
    ("vile/sight", "vilsit"),
    ("vile/active", "vilact"),
    ("vile/pain", "vipain"),
    ("vile/death", "vildth"),
    ("vile/raise", "slop"),
    ("vile/start", "vilatk"),
    ("vile/stop", "barexp"),
    ("vile/firestrt", "flamst"),
    ("vile/firecrkl", "flame"),
    ("skeleton/sight", "skesit"),
    ("skeleton/active", "skeact"),
    ("skeleton/pain", "popain"),
    ("skeleton/melee", "skepch"),
    ("skeleton/swing", "skeswg"),
    ("skeleton/death", "skedth"),
    ("skeleton/attack", "skeatk"),
    ("skeleton/tracex", "barexp"),
    ("fatso/sight", "mansit"),
    ("fatso/active", "posact"),
    ("fatso/pain", "mnpain"),
    ("fatso/raiseguns", "manatk"),
    ("fatso/death", "mandth"),
    ("fatso/attack", "firsht"),
    ("fatso/shotx", "firxpl"),
    ("imp/sight1", "bgsit1"),
    ("imp/sight2", "bgsit2"),
    ("imp/active", "bgact"),
    ("imp/pain", "popain"),
    ("imp/melee", "claw"),
    ("imp/death1", "bgdth1"),
    ("imp/death2", "bgdth2"),
    ("imp/attack", "firsht"),
    ("imp/shotx", "firxpl"),
    ("demon/sight", "sgtsit"),
    ("demon/active", "dmact"),
    ("demon/pain", "dmpain"),
    ("demon/melee", "sgtatk"),
    ("demon/death", "sgtdth"),
    ("spectre/sight", "sgtsit"),
    ("spectre/active", "dmact"),
    ("spectre/pain", "dmpain"),
    ("spectre/melee", "sgtatk"),
    ("spectre/death", "sgtdth"),
    ("caco/sight", "cacsit"),
    ("caco/active", "dmact"),
    ("caco/pain", "dmpain"),
    ("caco/death", "cacdth"),
    ("caco/attack", "firsht"),
    ("caco/shotx", "firxpl"),
    ("baron/sight", "brssit"),
    ("baron/active", "dmact"),
    ("baron/pain", "dmpain"),
    ("baron/melee", "claw"),
    ("baron/death", "brsdth"),
    ("baron/attack", "firsht"),
    ("baron/shotx", "firxpl"),
    ("knight/sight", "kntsit"),
    ("knight/active", "dmact"),
    ("knight/pain", "dmpain"),
    ("knight/death", "kntdth"),
    ("skull/active", "dmact"),
    ("skull/pain", "dmpain"),
    ("skull/melee", "sklatk"),
    ("skull/death", "firxpl"),
    ("spider/sight", "spisit"),
    ("spider/active", "dmact"),
    ("spider/pain", "dmpain"),
    ("spider/attack", "shotgn"),
    ("spider/death", "spidth"),
    ("spider/walk", "metal"),
    ("baby/sight", "bspsit"),
    ("baby/active", "bspact"),
    ("baby/pain", "dmpain"),
    ("baby/death", "bspdth"),
    ("baby/walk", "bspwlk"),
    ("baby/attack", "plasma"),
    ("baby/shotx", "firxpl"),
    ("cyber/sight", "cybsit"),
    ("cyber/active", "dmact"),
    ("cyber/pain", "dmpain"),
    ("cyber/death", "cybdth"),
    ("cyber/hoof", "hoof"),
    ("pain/sight", "pesit"),
    ("pain/active", "dmact"),
    ("pain/pain", "pepain"),
    ("pain/death", "pedth"),
    ("wolfss/sight", "sssit"),
    ("wolfss/active", "posact"),
    ("wolfss/pain", "popain"),
    ("wolfss/death", "ssdth"),
    ("wolfss/attack", "shotgn"),
    ("keen/pain", "keenpn"),
    ("keen/death", "keendt"),
    ("brain/sight", "bossit"),
    ("brain/pain", "bospn"),
    ("brain/death", "bosdth"),
    ("brain/spit", "bospit"),
    ("brain/cube", "boscub"),
    ("brain/cubeboom", "firxpl"),
    ("misc/brainspit", "bospit"),
];

/// Player sound names (`$playersound`) and the Doom sounds they redefine.
const PLAYER_SOUNDS: &[(&str, &str)] = &[
    ("*death", "pldeth"),
    ("*xdeath", "pdiehi"),
    ("*gibbed", "slop"),
    ("*pain", "plpain"),
    ("*grunt", "oof"),
    ("*land", "oof"),
    ("*usefail", "noway"),
];

/// Register Doom's sounds under their ZDoom logical names.
pub fn add_logical_names(info: &mut Info) {
    for &(logical, sfx) in DOOM_LOGICAL_NAMES {
        if let Some(i) = info.sfx.iter().position(|s| s.name == sfx) {
            info.sound_names.entry(logical.to_string()).or_insert(SfxNum(i as u16));
        }
    }
}

/// A token stream over SNDINFO text: words and quoted strings; `//` and
/// `/* */` comments; braces as their own tokens. Newlines matter for
/// `$` commands with optional arguments, so tokens carry their line.
fn tokens(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let (mut i, mut line) = (0, 1usize);
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_ascii_whitespace() || c == b',' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                out.push((line, String::from_utf8_lossy(&b[start..i.min(b.len())]).into_owned()));
                i += 1;
            }
            b'{' | b'}' => {
                out.push((line, (c as char).to_string()));
                i += 1;
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}' | b'"' | b',') {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
                        break;
                    }
                    i += 1;
                }
                out.push((line, String::from_utf8_lossy(&b[start..i]).into_owned()));
            }
        }
    }
    out
}

/// The sound a logical name is (creating it if new).
fn sound_for(info: &mut Info, logical: &str) -> SfxNum {
    let key = logical.to_ascii_lowercase();
    if let Some(&s) = info.sound_names.get(&key) {
        return s;
    }
    let name: &'static str = Box::leak(key.clone().into_boxed_str());
    info.sfx.push(SfxInfo { name, priority: 64, link: None, pitch: -1, volume: -1 });
    let s = SfxNum((info.sfx.len() - 1) as u16);
    info.sound_names.insert(key, s);
    s
}

fn def_mut(info: &mut Info, s: SfxNum) -> &mut SoundDef {
    info.sound_defs.entry(s.0).or_default()
}

/// Read every SNDINFO lump (and file) in load order into `info`. Later
/// definitions of a name replace earlier ones, so a later mod's sounds win.
pub fn load_sndinfo(info: &mut Info, stack: &ResourceStack, warnings: &mut Vec<String>) {
    add_logical_names(info);
    // SNDINFO lumps in WADs and `sndinfo.*` files at a PK3's root (ZDoom
    // reads every lump whose name is SNDINFO).
    let ids: Vec<LumpId> = stack.all_named("SNDINFO").to_vec();
    for id in ids {
        let label = stack.info(id).path.as_deref().map(str::to_string).unwrap_or_else(|| "SNDINFO".into());
        let text = String::from_utf8_lossy(stack.data(id)).into_owned();
        parse_into(info, stack, &text, &label, warnings);
    }
}

/// Parse one SNDINFO text into `info`.
pub fn parse_into(info: &mut Info, stack: &ResourceStack, text: &str, label: &str, warnings: &mut Vec<String>) {
    let toks = tokens(text);
    let mut i = 0;
    // `$ifdoom` ... `$endif` blocks for other games are skipped.
    let mut skipping = false;
    let mut missing = 0usize;
    let line_rest = |i: &mut usize, line: usize| -> Vec<String> {
        let mut v = Vec::new();
        while *i < toks.len() && toks[*i].0 == line {
            v.push(toks[*i].1.clone());
            *i += 1;
        }
        v
    };
    while i < toks.len() {
        let (line, ref word) = toks[i];
        i += 1;
        let lw = word.to_ascii_lowercase();
        if skipping {
            if lw == "$endif" {
                skipping = false;
            }
            continue;
        }
        if let Some(cmd) = lw.strip_prefix('$') {
            let args = line_rest(&mut i, line);
            let arg = |k: usize| args.get(k).map(String::as_str).unwrap_or("");
            let num = |k: usize, d: f32| args.get(k).and_then(|s| s.parse::<f32>().ok()).unwrap_or(d);
            match cmd {
                "ifheretic" | "ifhexen" | "ifstrife" => skipping = true,
                "ifdoom" | "endif" | "registered" | "archivepath" | "edfoverride" | "mididevice" | "musicalias" | "musicvolume" | "map"
                | "rolloff" | "pitchshiftrange" | "playercompat" => {}
                "alias" => {
                    if args.len() >= 2 {
                        let target = sound_for(info, arg(1));
                        info.sound_names.insert(arg(0).to_ascii_lowercase(), target);
                    }
                }
                "random" => {
                    // $random name { a b c } (the braces may span lines).
                    let mut members: Vec<String> = args.iter().skip(1).filter(|t| *t != "{" && *t != "}").cloned().collect();
                    if !args.iter().any(|t| t == "}") {
                        while i < toks.len() {
                            let t = toks[i].1.clone();
                            i += 1;
                            if t == "}" {
                                break;
                            }
                            if t != "{" {
                                members.push(t);
                            }
                        }
                    }
                    let s = sound_for(info, arg(0));
                    let list: Vec<SfxNum> = members.iter().map(|m| sound_for(info, m)).collect();
                    def_mut(info, s).random = list;
                }
                "limit" => {
                    let s = sound_for(info, arg(0));
                    def_mut(info, s).limit = num(1, 0.0).clamp(0.0, 255.0) as u8;
                }
                "singular" => {
                    let s = sound_for(info, arg(0));
                    def_mut(info, s).singular = true;
                }
                "volume" => {
                    let s = sound_for(info, arg(0));
                    def_mut(info, s).volume = num(1, 1.0).clamp(0.0, 1.0);
                }
                "attenuation" => {
                    let s = sound_for(info, arg(0));
                    def_mut(info, s).attenuation = num(1, 1.0).max(0.0);
                }
                "pitchshift" => {
                    let s = sound_for(info, arg(0));
                    def_mut(info, s).pitch_shift = num(1, 0.0) as i8;
                }
                "playersound" | "playersounddup" | "playeralias" => {
                    // $playersound class gender *name resource
                    let (class, name, what) = (arg(0).to_ascii_lowercase(), arg(2).to_ascii_lowercase(), arg(3));
                    let key = format!("{class}/{name}");
                    let s = if cmd == "playersound" {
                        let s = sound_for(info, &format!("$player/{key}"));
                        resolve(info, stack, s, what, label, warnings, &mut missing);
                        s
                    } else if cmd == "playeralias" {
                        sound_for(info, what)
                    } else {
                        // playersounddup: the same as another player sound.
                        sound_for(info, &format!("$player/{class}/{}", what.to_ascii_lowercase()))
                    };
                    info.player_sounds.insert(key, s);
                    // Doom's player is sound class "player": its sounds are
                    // Doom's, so a mod redefining *pain changes what Doom's
                    // player pain sound plays.
                    if class == "player" {
                        let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
                        if let Some(&(_, sfx)) = PLAYER_SOUNDS.iter().find(|(n, _)| *n == base) {
                            if let Some(v) = info.sfx.iter().position(|x| x.name == sfx) {
                                info.sound_defs.insert(v as u16, SoundDef { random: vec![s], ..Default::default() });
                            }
                        }
                    }
                }
                "ambient" => {
                    // $ambient index name [point [atten] | surround | world]
                    //          (continuous | random min max | periodic secs) volume
                    let Ok(index) = arg(0).parse::<u32>() else { continue };
                    let sound = sound_for(info, arg(1));
                    let mut k = 2;
                    let mut point = false;
                    let mut attenuation = 1.0;
                    match arg(k).to_ascii_lowercase().as_str() {
                        "point" => {
                            point = true;
                            k += 1;
                            if let Ok(a) = arg(k).parse::<f32>() {
                                attenuation = a;
                                k += 1;
                            }
                        }
                        "surround" | "world" => k += 1,
                        _ => {}
                    }
                    let secs = |v: f32| (v * 35.0).round().max(1.0) as u32;
                    let mode = match arg(k).to_ascii_lowercase().as_str() {
                        "random" => {
                            let (a, b) = (num(k + 1, 1.0), num(k + 2, 1.0));
                            k += 3;
                            AmbientMode::Random { min: secs(a.min(b)), max: secs(a.max(b)) }
                        }
                        "periodic" => {
                            let p = num(k + 1, 1.0);
                            k += 2;
                            AmbientMode::Periodic { every: secs(p) }
                        }
                        _ => {
                            k += 1;
                            AmbientMode::Continuous
                        }
                    };
                    let volume = num(k, 1.0).clamp(0.0, 1.0);
                    info.ambient_sounds.insert(index, AmbientDef { sound, point, attenuation, mode, volume });
                }
                _ => warnings.push(format!("{label}: line {line}: unknown command ${cmd}")),
            }
            continue;
        }
        // `logical resource`
        let Some((l2, resource)) = toks.get(i).cloned() else { break };
        if l2 != line {
            warnings.push(format!("{label}: line {line}: sound {word} has no resource"));
            continue;
        }
        i += 1;
        let s = sound_for(info, word);
        resolve(info, stack, s, &resource, label, warnings, &mut missing);
    }
    if missing > 0 {
        warnings.push(format!("{label}: {missing} sound definition(s) name files that are not in the loaded resources (those sounds stay silent)"));
    }
}

/// Point sound `s` at `resource`, resolved now.
fn resolve(info: &mut Info, stack: &ResourceStack, s: SfxNum, resource: &str, label: &str, warnings: &mut Vec<String>, missing: &mut usize) {
    let lump = stack.find_sound(resource);
    if lump.is_none() {
        *missing += 1;
        let name = info.sfx.get(s.0 as usize).map_or("?", |x| x.name);
        warnings.push(format!("{label}: sound {name}: {resource} is not in the loaded resources"));
    }
    let d = def_mut(info, s);
    d.lump = lump;
    d.resource = resource.to_string();
    d.random.clear();
}

/// The resource a sound plays, if a definition names one: follows
/// nothing else (callers handle `$random` and Doom's `DS` lumps).
pub fn defined_lump(info: &Info, s: SfxNum) -> Option<LumpId> {
    info.sound_defs.get(&s.0).and_then(|d| d.lump)
}

/// Every definition's resource, for preloading.
pub fn defined_resources(info: &Info) -> HashMap<u16, LumpId> {
    info.sound_defs.iter().filter_map(|(&k, d)| d.lump.map(|l| (k, l))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer() {
        let t: Vec<String> = tokens("a/b \"c d\" // x\n$random r { e\nf } /* g\n h */ i").into_iter().map(|(_, s)| s).collect();
        assert_eq!(t, ["a/b", "c d", "$random", "r", "{", "e", "f", "}", "i"]);
        let lines: Vec<usize> = tokens("a b\nc\n\nd").into_iter().map(|(l, _)| l).collect();
        assert_eq!(lines, [1, 1, 2, 4]);
    }
}
