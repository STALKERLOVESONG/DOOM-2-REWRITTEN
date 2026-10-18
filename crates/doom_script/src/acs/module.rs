//! Compiled ACS modules: `BEHAVIOR` lumps and ACS libraries.
//!
//! Three layouts exist (ZDoom `FBehavior::Init`):
//! - **ACS0** (Hexen): `"ACS\0"`, a directory of scripts and a string table;
//!   every instruction and operand is 4 bytes.
//! - **ACSE / ACSe** (ZDoom): `"ACSE"`/`"ACSe"` followed by the offset of a
//!   list of tagged chunks (`SPTR`, `STRL`, `FUNC`, ...). In `ACSe`
//!   ("little enhanced") instructions and some operands are compressed to
//!   one or two bytes.
//! - **Redesigned enhanced** (what current ACC writes): an ACS0 header whose
//!   directory is only a compatibility stub; the 4 bytes before it hold the
//!   real `ACSE`/`ACSe` tag and the 4 before those the chunk offset.

use std::collections::HashMap;

/// Instruction encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Old,
    Enhanced,
    /// Compressed pcodes (`ACSe`).
    LittleEnhanced,
}

/// When a script starts by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    /// Started by specials or other scripts.
    Closed,
    /// When the level starts.
    Open,
    /// When a player respawns.
    Respawn,
    /// When a player dies.
    Death,
    /// When a player enters the level.
    Enter,
    Pickup,
    BlueReturn,
    RedReturn,
    WhiteReturn,
    Lightning,
    Unloading,
    Disconnect,
    /// When a player returns to a hub level.
    Return,
    Event,
    Kill,
    Reopen,
    Other(u8),
}

impl ScriptKind {
    pub fn from_code(c: u32) -> ScriptKind {
        match c {
            0 => ScriptKind::Closed,
            1 => ScriptKind::Open,
            2 => ScriptKind::Respawn,
            3 => ScriptKind::Death,
            4 => ScriptKind::Enter,
            5 => ScriptKind::Pickup,
            6 => ScriptKind::BlueReturn,
            7 => ScriptKind::RedReturn,
            8 => ScriptKind::WhiteReturn,
            12 => ScriptKind::Lightning,
            13 => ScriptKind::Unloading,
            14 => ScriptKind::Disconnect,
            15 => ScriptKind::Return,
            16 => ScriptKind::Event,
            17 => ScriptKind::Kill,
            18 => ScriptKind::Reopen,
            n => ScriptKind::Other(n as u8),
        }
    }
}

/// Script flags (`SFLG`).
pub mod script_flags {
    pub const NET: u16 = 1;
    pub const CLIENTSIDE: u16 = 2;
}

#[derive(Clone, Debug)]
pub struct ScriptDef {
    /// Script number; named scripts have negative numbers (`-1 - index`
    /// into the `SNAM` names).
    pub number: i32,
    pub name: Option<String>,
    pub kind: ScriptKind,
    pub arg_count: u32,
    /// Byte offset of the first instruction in the module.
    pub address: u32,
    /// Local variable slots (args first). 20 unless `SVCT` says otherwise.
    pub var_count: u32,
    pub flags: u16,
    /// Sizes of script-local arrays (`SARY`).
    pub arrays: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct FuncDef {
    pub arg_count: u8,
    /// Locals including the arguments.
    pub local_count: u8,
    pub has_return: bool,
    /// Non-zero: the body is in library `import - 1` of this module's
    /// `LOAD` list (the address is then unused); resolved by name.
    pub import: u8,
    pub address: u32,
    pub name: Option<String>,
    /// Sizes of function-local arrays (`FARY`).
    pub arrays: Vec<u32>,
}

/// Map variables per module (ZDoom `NUM_MAPVARS`).
pub const NUM_MAPVARS: usize = 128;
/// Script variables when `SVCT` gives no count (`LOCAL_SIZE`).
pub const DEFAULT_SCRIPT_VARS: u32 = 20;

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub data: Vec<u8>,
    pub format: Format,
    pub scripts: Vec<ScriptDef>,
    pub strings: Vec<String>,
    pub functions: Vec<FuncDef>,
    /// Initial map variable values.
    pub map_vars: [i32; NUM_MAPVARS],
    /// Map arrays: initial contents (their length is the array size).
    pub arrays: Vec<Vec<i32>>,
    /// Libraries this module loads (`LOAD`), by lump name.
    pub libraries: Vec<String>,
    /// Map variables exported by name (`MEXP`), index = variable.
    pub exports: Vec<String>,
    /// Map variables imported from libraries (`MIMP`): (variable, name).
    pub var_imports: Vec<(u32, String)>,
    /// Arrays imported from libraries (`AIMP`): (array, size, name).
    pub array_imports: Vec<(u32, u32, String)>,
    /// Map variables / arrays holding strings (`MSTR`/`ASTR`): their
    /// initial values are tagged with the library number when loaded.
    pub string_vars: Vec<u32>,
    pub string_arrays: Vec<u32>,
    /// `JUMP` table for `GOTOSTACK`.
    pub jump_targets: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

/// NUL-terminated string at `o` (Latin-1).
fn cstr(d: &[u8], o: usize) -> String {
    let s = d.get(o..).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s[..end].iter().map(|&c| c as char).collect()
}

impl Module {
    /// Is this an ACS module at all?
    pub fn is_acs(data: &[u8]) -> bool {
        data.len() >= 8 && &data[..3] == b"ACS" && matches!(data[3], 0 | b'E' | b'e')
    }

    pub fn load(name: &str, data: Vec<u8>) -> Result<Module, LoadError> {
        let err = |m: &str| LoadError(format!("{name}: {m}"));
        if !Self::is_acs(&data) {
            return Err(err("not an ACS module"));
        }
        let mut format = match data[3] {
            0 => Format::Old,
            b'E' => Format::Enhanced,
            _ => Format::LittleEnhanced,
        };
        // Where chunks are, and where they end.
        let mut chunks: Option<(usize, usize)> = None;
        let dirofs = u32_at(&data, 4).ok_or_else(|| err("truncated header"))? as usize;
        let mut old_dir = None;
        if format == Format::Old {
            let pretag = if dirofs >= 8 { data.get(dirofs - 4..dirofs) } else { None };
            match pretag {
                Some(t) if dirofs >= 24 && (t == b"ACSE" || t == b"ACSe") => {
                    format = if t == b"ACSe" { Format::LittleEnhanced } else { Format::Enhanced };
                    let start = u32_at(&data, dirofs - 8).ok_or_else(|| err("truncated header"))? as usize;
                    chunks = Some((start, dirofs - 8));
                }
                _ => old_dir = Some(dirofs),
            }
        } else {
            chunks = Some((dirofs, data.len()));
        }
        // The redesigned format's SPTR uses 8-byte entries; the original
        // ACSE/ACSe header uses 12-byte ones.
        let short_sptr = data[3] == 0;

        let mut m = Module {
            name: name.to_string(),
            data: Vec::new(),
            format,
            scripts: Vec::new(),
            strings: Vec::new(),
            functions: Vec::new(),
            map_vars: [0; NUM_MAPVARS],
            arrays: Vec::new(),
            libraries: Vec::new(),
            exports: Vec::new(),
            var_imports: Vec::new(),
            array_imports: Vec::new(),
            string_vars: Vec::new(),
            string_arrays: Vec::new(),
            jump_targets: Vec::new(),
        };

        if let Some(dir) = old_dir {
            let n = u32_at(&data, dir).ok_or_else(|| err("truncated directory"))? as usize;
            for i in 0..n {
                let e = dir + 4 + i * 12;
                let (Some(num), Some(addr), Some(argc)) = (u32_at(&data, e), u32_at(&data, e + 4), u32_at(&data, e + 8)) else {
                    return Err(err("truncated directory"));
                };
                m.scripts.push(ScriptDef {
                    number: (num % 1000) as i32,
                    name: None,
                    kind: ScriptKind::from_code(num / 1000),
                    arg_count: argc,
                    address: addr,
                    var_count: DEFAULT_SCRIPT_VARS,
                    flags: 0,
                    arrays: Vec::new(),
                });
            }
            let st = dir + 4 + n * 12;
            let ns = u32_at(&data, st).unwrap_or(0) as usize;
            for i in 0..ns {
                let o = u32_at(&data, st + 4 + i * 4).unwrap_or(0) as usize;
                m.strings.push(cstr(&data, o));
            }
            m.data = data;
            return Ok(m);
        }

        let (start, end) = chunks.unwrap_or((0, 0));
        let end = end.min(data.len());
        let mut list: Vec<([u8; 4], usize, usize)> = Vec::new();
        let mut p = start;
        while p + 8 <= end {
            let id = [data[p], data[p + 1], data[p + 2], data[p + 3]];
            let len = u32_at(&data, p + 4).unwrap_or(0) as usize;
            let body = p + 8;
            if body + len > data.len() {
                break;
            }
            list.push((id, body, len));
            p = body + len;
        }
        let chunk = |id: &[u8; 4]| -> Vec<(usize, usize)> { list.iter().filter(|(i, _, _)| i == id).map(|&(_, b, l)| (b, l)).collect() };
        let d = &data;
        // Name lists: count, then offsets relative to the chunk body.
        let names = |b: usize, _l: usize| -> Vec<String> {
            let n = u32_at(d, b).unwrap_or(0) as usize;
            (0..n).map(|i| cstr(d, b + u32_at(d, b + 4 + i * 4).unwrap_or(0) as usize)).collect()
        };

        for (b, l) in chunk(b"SPTR") {
            let size = if short_sptr { 8 } else { 12 };
            for i in 0..l / size {
                let e = b + i * size;
                let (number, kind, argc, address) = if short_sptr {
                    (u16_at(d, e).unwrap_or(0) as i16 as i32, d[e + 2] as u32, d[e + 3] as u32, u32_at(d, e + 4).unwrap_or(0))
                } else {
                    (
                        u16_at(d, e).unwrap_or(0) as i16 as i32,
                        u16_at(d, e + 2).unwrap_or(0) as u32 & 0xff,
                        u32_at(d, e + 8).unwrap_or(0),
                        u32_at(d, e + 4).unwrap_or(0),
                    )
                };
                m.scripts.push(ScriptDef {
                    number,
                    name: None,
                    kind: ScriptKind::from_code(kind),
                    arg_count: argc,
                    address,
                    var_count: DEFAULT_SCRIPT_VARS,
                    flags: 0,
                    arrays: Vec::new(),
                });
            }
        }
        for (b, l) in chunk(b"SFLG") {
            for i in 0..l / 4 {
                let (num, flags) = (u16_at(d, b + i * 4).unwrap_or(0) as i16 as i32, u16_at(d, b + i * 4 + 2).unwrap_or(0));
                if let Some(s) = m.scripts.iter_mut().find(|s| s.number == num) {
                    s.flags = flags;
                }
            }
        }
        for (b, l) in chunk(b"SVCT") {
            for i in 0..l / 4 {
                let (num, count) = (u16_at(d, b + i * 4).unwrap_or(0) as i16 as i32, u16_at(d, b + i * 4 + 2).unwrap_or(0));
                if let Some(s) = m.scripts.iter_mut().find(|s| s.number == num) {
                    s.var_count = (count as u32).max(s.arg_count);
                }
            }
        }
        for (b, _) in chunk(b"SNAM") {
            for (i, name) in names(b, 0).into_iter().enumerate() {
                let number = -1 - i as i32;
                if let Some(s) = m.scripts.iter_mut().find(|s| s.number == number) {
                    s.name = Some(name);
                }
            }
        }
        // Strings: reserved, count, reserved, offsets.
        for (id, encrypted) in [(b"STRL", false), (b"STRE", true)] {
            for (b, _) in chunk(id) {
                let n = u32_at(d, b + 4).unwrap_or(0) as usize;
                for i in 0..n {
                    let ofs = u32_at(d, b + 12 + i * 4).unwrap_or(0) as usize;
                    let at = b + ofs;
                    if encrypted {
                        let key = (ofs as u32).wrapping_mul(157135);
                        let mut s = String::new();
                        for k in 0.. {
                            let Some(&c) = d.get(at + k) else { break };
                            let c = c ^ (key.wrapping_add(k as u32 / 2) as u8);
                            if c == 0 {
                                break;
                            }
                            s.push(c as char);
                        }
                        m.strings.push(s);
                    } else {
                        m.strings.push(cstr(d, at));
                    }
                }
            }
        }
        for (b, l) in chunk(b"FUNC") {
            for i in 0..l / 8 {
                let e = b + i * 8;
                m.functions.push(FuncDef {
                    arg_count: d[e],
                    local_count: d[e + 1],
                    has_return: d[e + 2] != 0,
                    import: d[e + 3],
                    address: u32_at(d, e + 4).unwrap_or(0),
                    name: None,
                    arrays: Vec::new(),
                });
            }
        }
        for (b, l) in chunk(b"FNAM") {
            for (f, name) in m.functions.iter_mut().zip(names(b, l)) {
                f.name = Some(name);
            }
        }
        for (b, l) in chunk(b"MINI") {
            let first = u32_at(d, b).unwrap_or(0) as usize;
            for i in 0..(l / 4).saturating_sub(1) {
                if let Some(v) = m.map_vars.get_mut(first + i) {
                    *v = u32_at(d, b + 4 + i * 4).unwrap_or(0) as i32;
                }
            }
        }
        let mut sizes: HashMap<u32, u32> = HashMap::new();
        for (b, l) in chunk(b"ARAY") {
            for i in 0..l / 8 {
                sizes.insert(u32_at(d, b + i * 8).unwrap_or(0), u32_at(d, b + i * 8 + 4).unwrap_or(0));
            }
        }
        let max = sizes.keys().map(|&k| k as usize + 1).max().unwrap_or(0).min(NUM_MAPVARS);
        m.arrays = (0..max).map(|i| vec![0; sizes.get(&(i as u32)).copied().unwrap_or(0).min(1 << 20) as usize]).collect();
        for (b, l) in chunk(b"AINI") {
            let arr = u32_at(d, b).unwrap_or(0) as usize;
            if let Some(a) = m.arrays.get_mut(arr) {
                for (i, v) in a.iter_mut().enumerate().take((l / 4).saturating_sub(1)) {
                    *v = u32_at(d, b + 4 + i * 4).unwrap_or(0) as i32;
                }
            }
        }
        for (b, l) in chunk(b"LOAD") {
            m.libraries.extend(d[b..b + l].split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| s.iter().map(|&c| c as char).collect()));
        }
        for (b, l) in chunk(b"MEXP") {
            m.exports = names(b, l);
        }
        for (b, l) in chunk(b"MIMP") {
            let mut p = b;
            while p + 4 < b + l {
                let var = u32_at(d, p).unwrap_or(0);
                let name = cstr(d, p + 4);
                p += 4 + name.len() + 1;
                m.var_imports.push((var, name));
            }
        }
        for (b, _) in chunk(b"AIMP") {
            let n = u32_at(d, b).unwrap_or(0) as usize;
            let mut p = b + 4;
            for _ in 0..n {
                let var = u32_at(d, p).unwrap_or(0);
                let size = u32_at(d, p + 4).unwrap_or(0);
                let name = cstr(d, p + 8);
                p += 8 + name.len() + 1;
                m.array_imports.push((var, size, name));
            }
        }
        for (b, l) in chunk(b"MSTR") {
            m.string_vars.extend((0..l / 4).map(|i| u32_at(d, b + i * 4).unwrap_or(0)));
        }
        for (b, l) in chunk(b"ASTR") {
            m.string_arrays.extend((0..l / 4).map(|i| u32_at(d, b + i * 4).unwrap_or(0)));
        }
        for (b, l) in chunk(b"JUMP") {
            m.jump_targets.extend((0..l / 4).map(|i| u32_at(d, b + i * 4).unwrap_or(0)));
        }
        // Local arrays: owner number (u16), then sizes.
        for (b, l) in chunk(b"SARY") {
            let num = u16_at(d, b).unwrap_or(0) as i16 as i32;
            let arrays = (0..(l.saturating_sub(2)) / 4).map(|i| u32_at(d, b + 2 + i * 4).unwrap_or(0)).collect();
            if let Some(s) = m.scripts.iter_mut().find(|s| s.number == num) {
                s.arrays = arrays;
            }
        }
        for (b, l) in chunk(b"FARY") {
            let f = u16_at(d, b).unwrap_or(0) as usize;
            let arrays = (0..(l.saturating_sub(2)) / 4).map(|i| u32_at(d, b + 2 + i * 4).unwrap_or(0)).collect();
            if let Some(func) = m.functions.get_mut(f) {
                func.arrays = arrays;
            }
        }
        m.data = data;
        Ok(m)
    }

    pub fn script(&self, number: i32) -> Option<&ScriptDef> {
        self.scripts.iter().find(|s| s.number == number)
    }

    pub fn named_script(&self, name: &str) -> Option<&ScriptDef> {
        self.scripts.iter().find(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }

    pub fn function_by_name(&self, name: &str) -> Option<usize> {
        self.functions.iter().position(|f| f.import == 0 && f.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }
}
