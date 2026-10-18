//! DEHACKED patches (DeHackEd 3.0 text format, as read by Chocolate Doom's
//! `deh_*.c`) and the Boom BEX extensions, applied to the actor tables.
//!
//! Supported:
//! - `Thing`, `Frame`, `Pointer`, `Sound` (priority), `Ammo`, `Weapon`,
//!   `Cheat`, `Misc`, `Text`; `Sprite` blocks are accepted and ignored, as
//!   in Chocolate (they only hold a pointer offset);
//! - Thing `Bits` as a number or as Boom flag mnemonics (`SOLID+SHOOTABLE`);
//! - BEX `[CODEPTR]`, `[STRINGS]`, `[PARS]`, and ZDoom's `[SPRITES]`,
//!   `[SOUNDS]` and `[MUSIC]` renames.
//!
//! Not supported (reported as warnings): DEHEXTRA/MBF21 extended tables and
//! fields, MBF code pointers that vanilla doesn't have, `INCLUDE`.
//!
//! Parsing follows GZDoom's tolerance rather than Chocolate's strictness:
//! blank lines don't end a block (only the next header does), keys are
//! case-insensitive, and `Text` replacements may be longer than the
//! original. Patches apply after DECORATE, so a mod's DEHACKED lump still
//! edits Doom's own things and frames by number.
//!
//! Strings work like Chocolate's `DEH_String`: one table of replacements
//! keyed by the original English text. `Text` blocks add to it directly;
//! BEX `[STRINGS]` entries are by mnemonic and are resolved by the game
//! layer, which knows the menu and finale texts ([`DehData::bex_strings`]).

use std::collections::{BTreeSet, HashMap};

use doom_content::ResourceStack;

use crate::info::*;
use crate::items::ammo;
use crate::mobj::flags::*;

/// What a patch changed that the game layer (rather than the playsim)
/// consumes.
#[derive(Clone, Debug, Default)]
pub struct DehData {
    pub strings: StringTable,
    /// BEX `[STRINGS]` entries, mnemonic (upper case) and text, in order.
    /// The game layer turns them into [`DehData::strings`] entries
    /// ([`DehData::resolve_bex_strings`]).
    pub bex_strings: Vec<(String, String)>,
    /// BEX `[PARS]`: par times in seconds by (episode, map); episode 0 for
    /// Doom II's `par <map> <seconds>` form.
    pub pars: HashMap<(i32, i32), i32>,
    /// `Cheat` block entries by DEHACKED name, lower case (`god mode`), as
    /// typed sequences.
    pub cheats: HashMap<String, String>,
}

impl DehData {
    /// Turn BEX string mnemonics into replacements, given each mnemonic's
    /// original text. Returns the mnemonics nobody knows.
    pub fn resolve_bex_strings(&mut self, originals: &dyn Fn(&str) -> Option<&'static str>) -> Vec<String> {
        let mut unknown = Vec::new();
        for (name, text) in std::mem::take(&mut self.bex_strings) {
            match originals(&name) {
                Some(original) => self.strings.insert(original, text),
                // Strings with no use here: start-up banners, obituaries,
                // multiplayer chat, GZDoom's LANGUAGE-only names, and
                // texts we only show formatted (`Marked Spot %d`).
                None if IGNORED_BEX_PREFIXES.iter().any(|p| name.starts_with(p)) => {}
                None => unknown.push(name),
            }
        }
        unknown
    }
}

/// BEX string mnemonics that are accepted without effect (see
/// [`DehData::resolve_bex_strings`]).
const IGNORED_BEX_PREFIXES: &[&str] = &[
    "STARTUP", "OB_", "SAVEGAMENAME", "HUSTR_PLR", "HUSTR_CHAT", "HUSTR_MSGU", "HUSTR_TALKTOSELF", "AMSTR_MARKEDSPOT",
    "TAG_", "SKILL_", "TXT_", "PD_ANY", "PD_ALL", "PD_BLUEC", "PD_REDC", "PD_YELLOWC", "PD_BLUES", "PD_REDS", "PD_YELLOWS",
];

/// String replacements keyed by the original text (`DEH_String`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StringTable(HashMap<String, String>);

impl StringTable {
    /// The replacement for `s`, or `s` itself.
    pub fn get<'a>(&'a self, s: &'a str) -> &'a str {
        self.0.get(s).map_or(s, String::as_str)
    }

    pub fn insert(&mut self, original: impl Into<String>, replacement: impl Into<String>) {
        self.0.insert(original.into(), replacement.into());
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The playsim's own messages by BEX mnemonic (pickups, locked doors).
pub fn sim_strings() -> &'static [(&'static str, &'static str)] {
    use crate::inter::msg::*;
    use crate::specials::doors::*;
    &[
        ("GOTARMOR", GOTARMOR),
        ("GOTMEGA", GOTMEGA),
        ("GOTHTHBONUS", GOTHTHBONUS),
        ("GOTARMBONUS", GOTARMBONUS),
        ("GOTSTIM", GOTSTIM),
        ("GOTMEDINEED", GOTMEDINEED),
        ("GOTMEDIKIT", GOTMEDIKIT),
        ("GOTSUPER", GOTSUPER),
        ("GOTBLUECARD", GOTBLUECARD),
        ("GOTYELWCARD", GOTYELWCARD),
        ("GOTREDCARD", GOTREDCARD),
        ("GOTBLUESKUL", GOTBLUESKUL),
        ("GOTYELWSKUL", GOTYELWSKUL),
        ("GOTREDSKULL", GOTREDSKULL),
        ("GOTINVUL", GOTINVUL),
        ("GOTBERSERK", GOTBERSERK),
        ("GOTINVIS", GOTINVIS),
        ("GOTSUIT", GOTSUIT),
        ("GOTMAP", GOTMAP),
        ("GOTVISOR", GOTVISOR),
        ("GOTMSPHERE", GOTMSPHERE),
        ("GOTCLIP", GOTCLIP),
        ("GOTCLIPBOX", GOTCLIPBOX),
        ("GOTROCKET", GOTROCKET),
        ("GOTROCKBOX", GOTROCKBOX),
        ("GOTCELL", GOTCELL),
        ("GOTCELLBOX", GOTCELLBOX),
        ("GOTSHELLS", GOTSHELLS),
        ("GOTSHELLBOX", GOTSHELLBOX),
        ("GOTBACKPACK", GOTBACKPACK),
        ("GOTBFG9000", GOTBFG9000),
        ("GOTCHAINGUN", GOTCHAINGUN),
        ("GOTCHAINSAW", GOTCHAINSAW),
        ("GOTLAUNCHER", GOTLAUNCHER),
        ("GOTPLASMA", GOTPLASMA),
        ("GOTSHOTGUN", GOTSHOTGUN),
        ("GOTSHOTGUN2", GOTSHOTGUN2),
        ("PD_BLUEO", PD_BLUEO),
        ("PD_REDO", PD_REDO),
        ("PD_YELLOWO", PD_YELLOWO),
        ("PD_BLUEK", PD_BLUEK),
        ("PD_REDK", PD_REDK),
        ("PD_YELLOWK", PD_YELLOWK),
    ]
}

/// Apply every `DEHACKED` lump in load order (the IWAD's first, as Freedoom
/// ships one). Loose `.deh`/`.bex` files are mounted as such lumps by
/// [`ResourceStack::add_file`]. Returns warnings.
pub fn load(info: &mut Info, stack: &ResourceStack) -> Vec<String> {
    let mut warnings = Vec::new();
    for &id in stack.all_named("DEHACKED") {
        let source = &stack.sources()[stack.info(id).source];
        let label = format!("{}:DEHACKED", source.path.display());
        apply(info, &String::from_utf8_lossy(stack.data(id)), &label, &mut warnings);
    }
    warnings
}

/// Apply one patch.
pub fn apply(info: &mut Info, text: &str, label: &str, warnings: &mut Vec<String>) {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut p = Patcher { info, label, warnings, pos: 0, text: &text, things: BTreeSet::new(), weapons: BTreeSet::new(), unknown_keys: BTreeSet::new() };
    p.run();
    p.finish();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    /// Before the first header, or after an unusable one.
    None,
    Thing(usize),
    Frame(usize),
    Pointer(usize),
    Sound(usize),
    Ammo(usize),
    Weapon(usize),
    Cheat,
    Misc,
    CodePtr,
    Strings,
    Pars,
    RenameSprites,
    RenameSounds,
    RenameMusic,
    /// Accepted and skipped (`Sprite`, `[HELPER]`, ...).
    Ignored,
}

struct Patcher<'a> {
    info: &'a mut Info,
    label: &'a str,
    warnings: &'a mut Vec<String>,
    text: &'a str,
    pos: usize,
    /// Thing and weapon numbers changed, to refresh derived class data.
    things: BTreeSet<usize>,
    weapons: BTreeSet<usize>,
    /// Unknown keys already reported, by block kind.
    unknown_keys: BTreeSet<(String, String)>,
}

/// The leading integer of a value, like C's `atoi` (trailing comments and
/// junk are ignored).
fn int(v: &str) -> Option<i64> {
    let v = v.trim_start();
    let (neg, digits) = match v.as_bytes().first() {
        Some(b'-') => (true, &v[1..]),
        Some(b'+') => (false, &v[1..]),
        _ => (false, v),
    };
    let end = digits.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(digits.len());
    let n: i64 = digits[..end].parse().ok()?;
    Some(if neg { -n } else { n })
}

/// Boom's mnemonics for Thing `Bits`.
const FLAG_NAMES: &[(&str, u32)] = &[
    ("SPECIAL", MF_SPECIAL),
    ("SOLID", MF_SOLID),
    ("SHOOTABLE", MF_SHOOTABLE),
    ("NOSECTOR", MF_NOSECTOR),
    ("NOBLOCKMAP", MF_NOBLOCKMAP),
    ("AMBUSH", MF_AMBUSH),
    ("JUSTHIT", MF_JUSTHIT),
    ("JUSTATTACKED", MF_JUSTATTACKED),
    ("SPAWNCEILING", MF_SPAWNCEILING),
    ("NOGRAVITY", MF_NOGRAVITY),
    ("DROPOFF", MF_DROPOFF),
    ("PICKUP", MF_PICKUP),
    ("NOCLIP", MF_NOCLIP),
    ("SLIDE", MF_SLIDE),
    ("FLOAT", MF_FLOAT),
    ("TELEPORT", MF_TELEPORT),
    ("MISSILE", MF_MISSILE),
    ("DROPPED", MF_DROPPED),
    ("SHADOW", MF_SHADOW),
    ("NOBLOOD", MF_NOBLOOD),
    ("CORPSE", MF_CORPSE),
    ("INFLOAT", MF_INFLOAT),
    ("COUNTKILL", MF_COUNTKILL),
    ("COUNTITEM", MF_COUNTITEM),
    ("SKULLFLY", MF_SKULLFLY),
    ("NOTDMATCH", MF_NOTDMATCH),
    ("TRANSLATION", MF_TRANSLATION),
    ("TRANSLATION1", 1 << MF_TRANSSHIFT),
    ("TRANSLATION2", 2 << MF_TRANSSHIFT),
    // Boom's name for the translucency bit vanilla uses as TRANSLATION's high bit.
    ("UNUSED1", 1 << 28),
];

/// `Cheat` block names and the vanilla sequences they replace. A replaced
/// `Level Warp` or `Change music` sequence keeps its two digit arguments.
pub const CHEAT_NAMES: &[(&str, &str)] = &[
    ("change music", "idmus"),
    ("chainsaw", "idchoppers"),
    ("god mode", "iddqd"),
    ("ammo & keys", "idkfa"),
    ("ammo", "idfa"),
    ("no clipping 1", "idspispopd"),
    ("no clipping 2", "idclip"),
    ("invincibility", "idbeholdv"),
    ("berserk", "idbeholds"),
    ("invisibility", "idbeholdi"),
    ("radiation suit", "idbeholdr"),
    ("auto-map", "idbeholda"),
    ("lite-amp goggles", "idbeholdl"),
    ("behold menu", "idbehold"),
    ("level warp", "idclev"),
    ("player position", "idmypos"),
    ("map cheat", "iddt"),
];

impl<'a> Patcher<'a> {
    fn warn(&mut self, msg: impl std::fmt::Display) {
        self.warnings.push(format!("{}: {msg}", self.label));
    }

    /// The next line; borrows the patch text, not the patcher.
    fn next_line(&mut self) -> Option<&'a str> {
        let text = self.text;
        if self.pos >= text.len() {
            return None;
        }
        let end = text[self.pos..].find('\n').map_or(text.len(), |i| self.pos + i);
        let line = &text[self.pos..end];
        self.pos = (end + 1).min(text.len());
        Some(line)
    }

    fn run(&mut self) {
        let mut block = Block::None;
        while let Some(raw) = self.next_line() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // BEX string values may contain anything, including '=' and
            // header-like words, so they are handled before headers.
            if block == Block::Strings && !line.starts_with('[') {
                if let Some((k, v)) = line.split_once('=') {
                    let key = k.trim().to_ascii_uppercase();
                    let value = self.bex_string_value(v.trim_start());
                    self.info.deh.bex_strings.push((key, value));
                    continue;
                }
            }
            if let Some((key, value)) = line.split_once('=') {
                self.field(block, key.trim(), value.trim());
                continue;
            }
            if block == Block::Pars && line.to_ascii_lowercase().starts_with("par") {
                self.par(line);
                continue;
            }
            block = self.header(line);
        }
    }

    /// A BEX string value, following `\` line continuations, with `\n`
    /// escapes expanded.
    fn bex_string_value(&mut self, first: &str) -> String {
        let mut raw = first.trim_end().to_string();
        while raw.ends_with('\\') && !raw.ends_with("\\\\") {
            raw.pop();
            match self.next_line() {
                Some(l) => raw.push_str(l.trim()),
                None => break,
            }
        }
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn header(&mut self, line: &str) -> Block {
        let lower = line.to_ascii_lowercase();
        let mut words = lower.split_whitespace();
        let first = words.next().unwrap_or("");
        let number = words.next().and_then(int);
        let index = |n: Option<i64>, limit: usize| n.filter(|&n| n >= 0 && (n as usize) < limit).map(|n| n as usize);
        match first {
            "thing" => match number.filter(|&n| n >= 1).map(|n| n as usize - 1).filter(|&n| n < NUMMOBJTYPES) {
                Some(t) => Block::Thing(t),
                None => self.bad_header(line, "thing number out of range (DEHEXTRA is not supported)"),
            },
            "frame" => match index(number, NUMSTATES) {
                Some(f) => Block::Frame(f),
                None => self.bad_header(line, "frame number out of range (DEHEXTRA is not supported)"),
            },
            "pointer" => {
                // `Pointer 12 (Frame 45)`: the frame in parentheses is the target.
                let frame = lower.split_once("frame").and_then(|(_, r)| int(r.trim_start()));
                match index(frame, NUMSTATES) {
                    Some(f) => Block::Pointer(f),
                    None => self.bad_header(line, "pointer without a valid frame"),
                }
            }
            "sound" => match index(number, self.info.sfx.len()) {
                Some(s) => Block::Sound(s),
                None => self.bad_header(line, "sound number out of range"),
            },
            "ammo" => match index(number, 4) {
                Some(a) => Block::Ammo(a),
                None => self.bad_header(line, "ammo number out of range"),
            },
            "weapon" => match index(number, 9) {
                Some(w) => Block::Weapon(w),
                None => self.bad_header(line, "weapon number out of range"),
            },
            "cheat" => Block::Cheat,
            "misc" => Block::Misc,
            "sprite" => Block::Ignored,
            "text" => {
                let old = number.unwrap_or(-1);
                let new = words.next().and_then(int).unwrap_or(-1);
                if old < 0 || new < 0 {
                    return self.bad_header(line, "bad text lengths");
                }
                self.text_block(old as usize, new as usize);
                Block::None
            }
            "[codeptr]" => Block::CodePtr,
            "[strings]" => Block::Strings,
            "[pars]" => Block::Pars,
            "[sprites]" => Block::RenameSprites,
            "[sounds]" => Block::RenameSounds,
            "[music]" => Block::RenameMusic,
            "include" => self.bad_header(line, "INCLUDE is not supported"),
            // Patch file banners.
            "patch" | "doom" => Block::None,
            _ if first.starts_with('[') => self.bad_header(line, "unsupported BEX section"),
            _ => self.bad_header(line, "unrecognised line"),
        }
    }

    fn bad_header(&mut self, line: &str, why: &str) -> Block {
        self.warn(format!("{why}: `{line}`"));
        Block::Ignored
    }

    fn unknown_key(&mut self, block: &str, key: &str) {
        if self.unknown_keys.insert((block.to_string(), key.to_ascii_lowercase())) {
            self.warn(format!("{block}: unsupported field `{key}`"));
        }
    }

    fn state_value(&mut self, v: &str, what: &str) -> Option<StateNum> {
        match int(v) {
            Some(n) if n >= 0 && (n as usize) < NUMSTATES => Some(StateNum(n as u16)),
            _ => {
                self.warn(format!("{what}: bad frame number `{v}`"));
                None
            }
        }
    }

    fn sound_value(&mut self, v: &str, what: &str) -> Option<SfxNum> {
        match int(v) {
            Some(n) if n >= 0 && (n as usize) < self.info.sfx.len() => Some(SfxNum(n as u16)),
            _ => {
                self.warn(format!("{what}: bad sound number `{v}`"));
                None
            }
        }
    }

    fn field(&mut self, block: Block, key: &str, value: &str) {
        let k = key.to_ascii_lowercase();
        match block {
            Block::Thing(t) => self.thing_field(t, &k, key, value),
            Block::Frame(f) => self.frame_field(f, &k, key, value),
            Block::Pointer(f) => {
                if k == "codep frame" {
                    self.set_codep_frame(f, value);
                } else {
                    self.unknown_key("Pointer", key);
                }
            }
            Block::Sound(s) => match k.as_str() {
                "value" => {
                    if let Some(n) = int(value) {
                        self.info.sfx[s].priority = n as i32;
                    }
                }
                // Lump offsets and the like: meaningless outside the DOS exe.
                "offset" | "zero/one" | "zero 1" | "zero 2" | "zero 3" | "zero 4" | "neg. one 1" | "neg. one 2" => {}
                _ => self.unknown_key("Sound", key),
            },
            Block::Ammo(a) => match (k.as_str(), int(value)) {
                ("max ammo", Some(n)) => self.info.maxammo[a] = n as i32,
                ("per ammo", Some(n)) => self.info.clipammo[a] = n as i32,
                _ => self.unknown_key("Ammo", key),
            },
            Block::Weapon(w) => self.weapon_field(w, &k, key, value),
            Block::Cheat => match CHEAT_NAMES.iter().find(|(n, _)| *n == k) {
                // Old patches end sequences with 0xFF bytes (decoded lossily).
                Some(_) => {
                    let seq: String = value.chars().filter(|c| c.is_ascii_graphic()).collect::<String>().to_ascii_lowercase();
                    if !seq.is_empty() {
                        self.info.deh.cheats.insert(k, seq);
                    }
                }
                None => self.unknown_key("Cheat", key),
            },
            Block::Misc => self.misc_field(&k, key, value),
            Block::CodePtr => {
                let frame = k.strip_prefix("frame").map(str::trim_start).and_then(int);
                match frame.filter(|&n| n >= 0 && (n as usize) < NUMSTATES) {
                    Some(f) => self.set_code_pointer(f as usize, value),
                    None => self.warn(format!("[CODEPTR]: bad entry `{key} = {value}`")),
                }
            }
            Block::RenameSprites => self.rename_sprite(key, value),
            Block::RenameSounds => self.rename_sound(key, value),
            Block::RenameMusic => {
                let (old, new) = (key.to_ascii_lowercase(), value.to_ascii_lowercase());
                let old = old.strip_prefix("d_").unwrap_or(&old).to_string();
                let new = new.strip_prefix("d_").unwrap_or(&new).to_string();
                self.info.deh.strings.insert(old, new);
            }
            // Banner fields (`Doom version = 19`) and skipped blocks.
            Block::None | Block::Ignored | Block::Strings | Block::Pars => {}
        }
    }

    fn thing_field(&mut self, t: usize, k: &str, key: &str, value: &str) {
        let what = format!("Thing {}", t + 1);
        let n = int(value);
        macro_rules! state {
            ($f:ident) => {
                if let Some(s) = self.state_value(value, &what) {
                    self.info.mobjinfo[t].$f = s;
                }
            };
        }
        macro_rules! sound {
            ($f:ident) => {
                if let Some(s) = self.sound_value(value, &what) {
                    self.info.mobjinfo[t].$f = s;
                }
            };
        }
        macro_rules! num {
            ($f:ident) => {
                match n {
                    Some(n) => self.info.mobjinfo[t].$f = n as i32,
                    None => self.warn(format!("{what}: bad number `{value}`")),
                }
            };
        }
        match k {
            "id #" => num!(doomednum),
            "initial frame" => state!(spawnstate),
            "hit points" => num!(spawnhealth),
            "first moving frame" => state!(seestate),
            "alert sound" => sound!(seesound),
            "reaction time" => num!(reactiontime),
            "attack sound" => sound!(attacksound),
            "injury frame" => state!(painstate),
            "pain chance" => num!(painchance),
            "pain sound" => sound!(painsound),
            "close attack frame" => state!(meleestate),
            "far attack frame" => state!(missilestate),
            "death frame" => state!(deathstate),
            "exploding frame" => state!(xdeathstate),
            "death sound" => sound!(deathsound),
            "speed" => num!(speed),
            "width" => {
                if let Some(n) = n {
                    self.info.mobjinfo[t].radius = doom_core::Fixed(n as i32);
                }
            }
            "height" => {
                if let Some(n) = n {
                    self.info.mobjinfo[t].height = doom_core::Fixed(n as i32);
                }
            }
            "mass" => num!(mass),
            "missile damage" => num!(damage),
            "action sound" => sound!(activesound),
            "bits" => {
                if let Some(bits) = self.parse_bits(value, &what) {
                    self.info.mobjinfo[t].flags = bits;
                }
            }
            "respawn frame" => state!(raisestate),
            _ => return self.unknown_key("Thing", key),
        }
        self.things.insert(t);
    }

    /// `Bits`: a number, or mnemonics joined by `+`, `|`, `,` or spaces.
    fn parse_bits(&mut self, value: &str, what: &str) -> Option<u32> {
        let v = value.trim();
        if v.bytes().next().is_some_and(|b| b.is_ascii_digit() || b == b'-') {
            return int(v).map(|n| n as u32);
        }
        let mut bits = 0u32;
        for word in v.split(|c: char| c == '+' || c == '|' || c == ',' || c.is_whitespace()).filter(|w| !w.is_empty()) {
            let w = word.to_ascii_uppercase();
            let w = w.strip_prefix("MF_").unwrap_or(&w);
            match FLAG_NAMES.iter().find(|(n, _)| *n == w) {
                Some(&(_, b)) => bits |= b,
                None => match int(w) {
                    Some(n) => bits |= n as u32,
                    None => self.warn(format!("{what}: unsupported flag `{word}` (MBF flags are not supported)")),
                },
            }
        }
        Some(bits)
    }

    fn frame_field(&mut self, f: usize, k: &str, key: &str, value: &str) {
        let what = format!("Frame {f}");
        let n = int(value).map(|n| n as i32);
        match k {
            // Some editors write the pointer inside the Frame block.
            "codep frame" => return self.set_codep_frame(f, value),
            // DeHackEd 2 offsets into the executable: meaningless here.
            "action pointer" => return,
            "sprite number" | "sprite subnumber" | "duration" | "next frame" | "unknown 1" | "unknown 2" => {}
            _ => return self.unknown_key("Frame", key),
        }
        let Some(n) = n else {
            return self.warn(format!("{what}: bad value `{key} = {value}`"));
        };
        if k == "next frame" {
            if let Some(s) = self.state_value(value, &what) {
                self.info.states[f].next = s;
            }
            return;
        }
        if k == "sprite number" && !(0..self.info.sprnames.len() as i32).contains(&n) {
            return self.warn(format!("{what}: bad sprite number `{value}`"));
        }
        let st = &mut self.info.states[f];
        match k {
            "sprite number" => st.sprite = SpriteNum(n as u16),
            "sprite subnumber" => st.frame = n,
            "duration" => st.tics = n,
            "unknown 1" => st.misc1 = n,
            _ => st.misc2 = n,
        }
    }

    /// `Codep Frame = N`: the frame takes the action vanilla frame N had.
    fn set_codep_frame(&mut self, f: usize, value: &str) {
        match int(value).filter(|&n| n >= 0 && (n as usize) < NUMSTATES) {
            Some(src) => self.set_action(f, DEFAULT_STATES[src as usize].action),
            None => self.warn(format!("Pointer (Frame {f}): bad frame `{value}`")),
        }
    }

    /// BEX `FRAME n = Name`: `A_` is optional, `NULL` clears.
    fn set_code_pointer(&mut self, f: usize, name: &str) {
        let name = name.trim();
        let bare = name.strip_prefix("A_").or_else(|| name.strip_prefix("a_")).unwrap_or(name);
        let found = Action::ALL.iter().copied().find(|a| {
            let n = a.name();
            n.eq_ignore_ascii_case(name) || n.strip_prefix("A_").is_some_and(|n| n.eq_ignore_ascii_case(bare))
        });
        match found {
            Some(a) => self.set_action(f, a),
            None => self.warn(format!("[CODEPTR]: frame {f}: unsupported code pointer `{name}` (MBF pointers are not supported)")),
        }
    }

    fn set_action(&mut self, f: usize, a: Action) {
        self.info.states[f].action = a;
        // A DECORATE call on the same state would take precedence.
        self.info.state_calls.remove(&(f as u16));
    }

    fn weapon_field(&mut self, w: usize, k: &str, key: &str, value: &str) {
        let what = format!("Weapon {w}");
        if k == "ammo type" {
            match int(value) {
                // 4 is NUMAMMO; vanilla's "no ammo" is 5, but patches use both.
                Some(n @ 0..=3) => self.info.weaponinfo[w].ammo = n as usize,
                Some(4 | 5) => self.info.weaponinfo[w].ammo = ammo::NOAMMO,
                _ => self.warn(format!("{what}: bad ammo type `{value}`")),
            }
            self.weapons.insert(w);
            return;
        }
        let slot = match k {
            "deselect frame" => 0,
            "select frame" => 1,
            "bobbing frame" => 2,
            "shooting frame" => 3,
            "firing frame" => 4,
            _ => return self.unknown_key("Weapon", key),
        };
        let Some(s) = self.state_value(value, &what) else { return };
        let wi = &mut self.info.weaponinfo[w];
        *[&mut wi.downstate, &mut wi.upstate, &mut wi.readystate, &mut wi.atkstate, &mut wi.flashstate][slot] = s;
        self.weapons.insert(w);
    }

    fn misc_field(&mut self, k: &str, key: &str, value: &str) {
        let Some(n) = int(value).map(|n| n as i32) else {
            return self.warn(format!("Misc: bad number `{key} = {value}`"));
        };
        let m = &mut self.info.misc;
        match k {
            "initial health" => m.initial_health = n,
            "initial bullets" => m.initial_bullets = n,
            "max health" => m.max_health = n,
            "max armor" => m.max_armor = n,
            "green armor class" => m.green_armor_class = n,
            "blue armor class" => m.blue_armor_class = n,
            "max soulsphere" => m.max_soulsphere = n,
            "soulsphere health" => m.soulsphere_health = n,
            "megasphere health" => m.megasphere_health = n,
            "god mode health" => m.god_mode_health = n,
            "idfa armor" => m.idfa_armor = n,
            "idfa armor class" => m.idfa_armor_class = n,
            "idkfa armor" => m.idkfa_armor = n,
            "idkfa armor class" => m.idkfa_armor_class = n,
            "bfg cells/shot" => m.bfg_cells_per_shot = n,
            // DeHackEd writes 202 for "on" and 221 for "off".
            "monsters infight" => m.species_infighting = n == 202,
            _ => self.unknown_key("Misc", key),
        }
    }

    /// `Text <old> <new>`: the next `old + new` characters, newlines
    /// included.
    fn text_block(&mut self, old_len: usize, new_len: usize) {
        let rest = &self.text[self.pos..];
        let mut chars = rest.char_indices();
        let mut take = |n: usize| -> (String, bool) {
            let mut s = String::with_capacity(n);
            for _ in 0..n {
                match chars.next() {
                    Some((_, c)) => s.push(c),
                    None => return (s, false),
                }
            }
            (s, true)
        };
        let (old, ok1) = take(old_len);
        let (new, ok2) = take(new_len);
        let consumed = chars.next().map_or(rest.len(), |(i, _)| i);
        self.pos += consumed;
        if !(ok1 && ok2) {
            self.warn(format!("Text {old_len} {new_len}: runs past the end of the patch"));
            return;
        }
        // Sprite and sound names are looked up through DEH_String in
        // Chocolate; renaming the tables has the same effect.
        if old.len() == 4 && self.info.sprnames.iter().any(|s| s.eq_ignore_ascii_case(&old)) {
            self.rename_sprite(&old, &new);
        }
        if self.info.sfx.iter().any(|s| s.name.eq_ignore_ascii_case(&old)) {
            self.rename_sound(&old, &new);
        }
        self.info.deh.strings.insert(old, new);
    }

    fn rename_sprite(&mut self, old: &str, new: &str) {
        let new = new.trim().to_ascii_uppercase();
        if new.len() != 4 {
            return self.warn(format!("sprite rename {old} -> {new}: sprite names have four characters"));
        }
        match self.info.sprnames.iter_mut().find(|s| s.eq_ignore_ascii_case(old.trim())) {
            Some(s) => *s = new,
            None => self.warn(format!("sprite rename: no sprite `{old}`")),
        }
    }

    fn rename_sound(&mut self, old: &str, new: &str) {
        let (old, new) = (old.trim().to_ascii_lowercase(), new.trim().to_ascii_lowercase());
        let Some(s) = self.info.sfx.iter_mut().find(|s| s.name == old) else {
            return self.warn(format!("sound rename: no sound `{old}`"));
        };
        // `SfxInfo::name` is a static string (the tables are generated
        // constants). Renames are rare and happen once per load, so the
        // few bytes are leaked rather than making every name owned.
        s.name = Box::leak(new.into_boxed_str());
    }

    fn par(&mut self, line: &str) {
        let nums: Vec<i64> = line.split_whitespace().skip(1).take_while(|w| !w.starts_with('#')).filter_map(int).collect();
        match nums[..] {
            [e, m, secs] => {
                self.info.deh.pars.insert((e as i32, m as i32), secs as i32);
            }
            [m, secs] => {
                self.info.deh.pars.insert((0, m as i32), secs as i32);
            }
            _ => self.warn(format!("[PARS]: bad line `{line}`")),
        }
    }

    /// Refresh class data derived from the patched tables.
    fn finish(&mut self) {
        for &t in &self.things {
            let labels = standard_labels(&self.info.mobjinfo[t]);
            let l = &mut self.info.class_labels[t];
            for key in ["spawn", "see", "pain", "melee", "missile", "death", "xdeath", "raise"] {
                match labels.get(key) {
                    Some(&s) => l.insert(key.to_string(), s),
                    None => l.remove(key),
                };
            }
        }
        for &w in &self.weapons {
            self.info.refresh_vanilla_weapon_class(crate::player::Weapon::from_index(w as u8));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(text: &str) -> (Info, Vec<String>) {
        let mut info = Info::doom();
        let mut w = Vec::new();
        apply(&mut info, text, "test", &mut w);
        (info, w)
    }

    #[test]
    fn things_frames_pointers() {
        let (info, w) = patch(
            "Patch File for DeHackEd v3.0\r\nDoom version = 21\r\nPatch format = 6\r\n\r\n\
             Thing 1 (Player)\nHit points = 50\nBits = SOLID+SHOOTABLE | DROPOFF\n\n\
             Speed = 3\n\
             Thing 2 (Zombieman)\nWidth = 1310720\nInitial frame = 10\nAlert sound = 1\nBits = 4194310\n\
             Frame 185\nSprite subnumber = 32773\nDuration = 9 # comment\nNext frame = 0\n\
             Pointer 3 (Frame 200)\nCodep Frame = 175\n",
        );
        assert!(w.is_empty(), "{w:?}");
        let p = &info.mobjinfo[MT_PLAYER.idx()];
        assert_eq!(p.spawnhealth, 50);
        assert_eq!(p.flags, MF_SOLID | MF_SHOOTABLE | MF_DROPOFF);
        // A blank line doesn't end the block.
        assert_eq!(p.speed, 3);
        let z = &info.mobjinfo[MT_POSSESSED.idx()];
        assert_eq!(z.radius, doom_core::Fixed::from_int(20));
        assert_eq!((z.spawnstate, z.seesound, z.flags), (StateNum(10), SfxNum(1), 4194310));
        assert_eq!(info.class_labels[MT_POSSESSED.idx()]["spawn"], StateNum(10));
        let f = &info.states[185];
        assert_eq!((f.frame, f.tics, f.next), (32773, 9, S_NULL));
        assert_eq!(info.states[200].action, DEFAULT_STATES[175].action);
    }

    #[test]
    fn text_and_renames() {
        let (info, w) = patch("Text 4 4\nTROOSARGText 6 8\npistolplasmaxxText 17 13\nPicked up a clip.Ammo, hooray!\n");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(info.sprnames[SPR_TROO.idx()], "SARG");
        assert_eq!(info.sfx[SFX_PISTOL.idx()].name, "plasmaxx");
        assert_eq!(info.deh.strings.get("Picked up a clip."), "Ammo, hooray!");
        let (info, _) = patch("[SPRITES]\nPOSS = ZOMB\n[MUSIC]\nD_E1M1 = MYSONG\n[SOUNDS]\nshotgn = blast\n");
        assert_eq!(info.sprnames[SPR_POSS.idx()], "ZOMB");
        assert_eq!(info.deh.strings.get("e1m1"), "mysong");
        assert_eq!(info.sfx[SFX_SHOTGN.idx()].name, "blast");
    }

    #[test]
    fn bex_sections() {
        let (info, w) = patch(
            "[CODEPTR]\nFRAME 1 = Scream\nFRAME 2 = A_Fall\nFRAME 3 = NULL\nFRAME 4 = Spawn\n\
             [PARS]\npar 1 2 99 # comment\npar 15 120\n\
             [STRINGS]\nGOTARMOR = Vest\\n on!\nE1TEXT = first \\\n   second\nHUSTR_E1M1=E1M1: Lobby = start\n",
        );
        assert_eq!(w.len(), 1, "{w:?}"); // MBF's A_Spawn
        assert_eq!(info.states[1].action, Action::Scream);
        assert_eq!(info.states[2].action, Action::Fall);
        assert_eq!(info.states[3].action, Action::None);
        assert_eq!(info.deh.pars[&(1, 2)], 99);
        assert_eq!(info.deh.pars[&(0, 15)], 120);
        assert_eq!(
            info.deh.bex_strings,
            vec![
                ("GOTARMOR".to_string(), "Vest\n on!".to_string()),
                ("E1TEXT".to_string(), "first second".to_string()),
                ("HUSTR_E1M1".to_string(), "E1M1: Lobby = start".to_string()),
            ]
        );
    }

    #[test]
    fn weapons_ammo_misc_cheats() {
        let (info, w) = patch(
            "Weapon 2 (Shotgun)\nAmmo type = 2\nShooting frame = 70\n\
             Ammo 1\nMax ammo = 99\nPer ammo = 7\n\
             Misc 0\nInitial Health = 150\nMonsters Infight = 202\nBFG Cells/Shot = 10\n\
             Cheat 0\nGod mode = idgod\nLevel Warp = idwarp\n\
             Sound 1\nValue = 12\n",
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(info.weaponinfo[2].ammo, ammo::CELL);
        assert_eq!(info.weaponinfo[2].atkstate, StateNum(70));
        let class = info.vanilla_weapon_classes[2];
        assert_eq!(info.find_label(class, "fire"), Some(StateNum(70)));
        assert_eq!(info.weapons[&class.0].ammo, Some(crate::weapons::AmmoRef::Vanilla(ammo::CELL)));
        assert_eq!((info.maxammo[1], info.clipammo[1]), (99, 7));
        assert_eq!(info.misc.initial_health, 150);
        assert!(info.misc.species_infighting);
        assert_eq!(info.misc.bfg_cells_per_shot, 10);
        assert_eq!(info.deh.cheats["god mode"], "idgod");
        assert_eq!(info.deh.cheats["level warp"], "idwarp");
        assert_eq!(info.sfx[1].priority, 12);
    }

    #[test]
    fn bad_input_warns_without_panicking() {
        let (_, w) = patch("Thing 9999\nHit points = 1\nFrame -1\nText 100 100\nshort");
        assert_eq!(w.len(), 3, "{w:?}");
        let (_, w) = patch("Thing 1\nFancy field = 3\nFancy field = 4\n");
        assert_eq!(w.len(), 1, "unknown keys are reported once: {w:?}");
    }

    #[test]
    fn bex_resolution() {
        let mut d = DehData::default();
        d.bex_strings = vec![("GOTARMOR".into(), "Vest".into()), ("STARTUP1".into(), "x".into()), ("NOPE".into(), "y".into())];
        let unknown = d.resolve_bex_strings(&|m| sim_strings().iter().find(|(n, _)| *n == m).map(|&(_, s)| s));
        assert_eq!(unknown, vec!["NOPE".to_string()]);
        assert_eq!(d.strings.get("Picked up the armor."), "Vest");
    }
}
