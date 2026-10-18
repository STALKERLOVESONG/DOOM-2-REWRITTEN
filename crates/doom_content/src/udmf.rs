//! UDMF maps (`TEXTMAP` ... `ENDMAP`) into [`RawMap`] records.
//!
//! Namespaces `doom`, `zdoom`, `hexen`, `heretic`, `strife` and
//! `zdoomtranslated` are read. Lines in Hexen-style namespaces (`zdoom`,
//! `hexen`, ...) keep their special, arguments and activation flags as a
//! [`LineAction`] for the playsim's Hexen special executor; Doom-namespace
//! lines keep Doom specials. Sector specials are translated to Doom's.
//! Geometry, textures, things (with TIDs and specials), per-part texture
//! offsets, per-side and per-plane light, flat panning/scaling/rotation and
//! the map's BEHAVIOR lump are kept.

use std::borrow::Cow;

use crate::map::{
    LineAction, MapFormat, MapLinedef, MapSector, MapSidedef, MapThing, MapVertex, PlaneXform, RawMap, ml, mtf, spac, NO_SIDEDEF,
};
use crate::name::LumpName;
use crate::resources::{LumpId, ResourceStack};
use crate::ContentError;

#[derive(Clone, Debug)]
enum Value<'a> {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Cow<'a, str>),
}

impl Value<'_> {
    fn f(&self) -> f64 {
        match *self {
            Value::Int(i) => i as f64,
            Value::Float(f) => f,
            Value::Bool(b) => b as i32 as f64,
            Value::Str(_) => 0.0,
        }
    }
    fn i(&self) -> i64 {
        match *self {
            Value::Int(i) => i,
            Value::Float(f) => f as i64,
            Value::Bool(b) => b as i64,
            Value::Str(_) => 0,
        }
    }
    fn b(&self) -> bool {
        match *self {
            Value::Bool(b) => b,
            Value::Int(i) => i != 0,
            _ => false,
        }
    }
}

/// One UDMF block. Keys borrow from the TEXTMAP text and are compared
/// case-insensitively; blocks have few fields, so a list beats a map.
struct Block<'a> {
    /// Lower-case block type (`linedef`, `sidedef`, ...).
    kind: String,
    fields: Vec<(&'a str, Value<'a>)>,
}

impl<'a> Block<'a> {
    fn get(&self, k: &str) -> Option<&Value<'a>> {
        // Later assignments win, as with a map.
        self.fields.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v)
    }
    fn f(&self, k: &str, default: f64) -> f64 {
        self.get(k).map_or(default, Value::f)
    }
    fn i(&self, k: &str, default: i64) -> i64 {
        self.get(k).map_or(default, Value::i)
    }
    fn b(&self, k: &str) -> bool {
        self.get(k).is_some_and(Value::b)
    }
    fn s(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }
}

/// A single-pass, allocation-light UDMF reader. TEXTMAPs of big maps run to
/// tens of megabytes, so this avoids a token list and per-token strings.
struct Reader<'a> {
    text: &'a str,
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn line(&self) -> usize {
        1 + self.b[..self.pos.min(self.b.len())].iter().filter(|&&c| c == b'\n').count()
    }

    fn err(&self, what: &str) -> String {
        format!("line {}: {what}", self.line())
    }

    /// Skip whitespace and comments; the next byte, if any.
    fn peek(&mut self) -> Option<u8> {
        let b = self.b;
        loop {
            while self.pos < b.len() && b[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if b.get(self.pos) == Some(&b'/') && b.get(self.pos + 1) == Some(&b'/') {
                while self.pos < b.len() && b[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else if b.get(self.pos) == Some(&b'/') && b.get(self.pos + 1) == Some(&b'*') {
                self.pos += 2;
                while self.pos < b.len() && !(b[self.pos] == b'*' && b.get(self.pos + 1) == Some(&b'/')) {
                    self.pos += 1;
                }
                self.pos = (self.pos + 2).min(b.len());
            } else {
                return b.get(self.pos).copied();
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ident(&mut self) -> Option<&'a str> {
        let c = self.peek()?;
        if !(c.is_ascii_alphabetic() || c == b'_') {
            return None;
        }
        let start = self.pos;
        while self.pos < self.b.len() && (self.b[self.pos].is_ascii_alphanumeric() || self.b[self.pos] == b'_') {
            self.pos += 1;
        }
        Some(&self.text[start..self.pos])
    }

    fn value(&mut self) -> Result<Value<'a>, String> {
        let c = self.peek().ok_or_else(|| self.err("value expected"))?;
        let v = if c == b'"' {
            self.pos += 1;
            let start = self.pos;
            let mut escaped = false;
            while self.pos < self.b.len() && self.b[self.pos] != b'"' {
                if self.b[self.pos] == b'\\' {
                    escaped = true;
                    self.pos += 1;
                }
                self.pos += 1;
            }
            let raw = &self.text[start..self.pos.min(self.b.len())];
            self.pos += 1;
            if escaped {
                let mut out = String::with_capacity(raw.len());
                let mut chars = raw.chars();
                while let Some(ch) = chars.next() {
                    if ch == '\\' {
                        match chars.next() {
                            Some('n') => out.push('\n'),
                            Some('t') => out.push('\t'),
                            Some(other) => out.push(other),
                            None => {}
                        }
                    } else {
                        out.push(ch);
                    }
                }
                Value::Str(Cow::Owned(out))
            } else {
                Value::Str(Cow::Borrowed(raw))
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let w = self.ident().unwrap_or("");
            if w.eq_ignore_ascii_case("true") {
                Value::Bool(true)
            } else if w.eq_ignore_ascii_case("false") {
                Value::Bool(false)
            } else {
                Value::Str(Cow::Borrowed(w))
            }
        } else {
            let start = self.pos;
            if matches!(c, b'-' | b'+') {
                self.pos += 1;
            }
            let digits = self.pos;
            let hex = self.b.get(self.pos) == Some(&b'0') && matches!(self.b.get(self.pos + 1), Some(b'x' | b'X'));
            if hex {
                self.pos += 2;
                while self.pos < self.b.len() && self.b[self.pos].is_ascii_hexdigit() {
                    self.pos += 1;
                }
                let v = i64::from_str_radix(&self.text[digits + 2..self.pos], 16).map_err(|_| self.err("bad number"))?;
                Value::Int(if c == b'-' { -v } else { v })
            } else {
                let mut float = false;
                while self.pos < self.b.len() {
                    match self.b[self.pos] {
                        b'0'..=b'9' => {}
                        b'.' | b'e' | b'E' => float = true,
                        b'-' | b'+' if float && matches!(self.b[self.pos - 1], b'e' | b'E') => {}
                        _ => break,
                    }
                    self.pos += 1;
                }
                let t = &self.text[start..self.pos];
                if t.len() == digits - start {
                    return Err(self.err("bad value"));
                }
                if float {
                    Value::Float(t.parse().map_err(|_| self.err("bad number"))?)
                } else {
                    Value::Int(t.parse().map_err(|_| self.err("bad number"))?)
                }
            }
        };
        if !self.eat(b';') {
            return Err(self.err("expected ';'"));
        }
        Ok(v)
    }
}

fn parse(text: &str) -> Result<(String, Vec<Block<'_>>), String> {
    let mut r = Reader { text, b: text.as_bytes(), pos: 0 };
    let mut namespace = String::new();
    let mut blocks = Vec::new();
    while r.peek().is_some() {
        let Some(name) = r.ident() else { return Err(r.err("expected an identifier")) };
        if r.eat(b'=') {
            let v = r.value()?;
            if name.eq_ignore_ascii_case("namespace") {
                if let Value::Str(s) = v {
                    namespace = s.to_ascii_lowercase();
                }
            }
            continue;
        }
        if !r.eat(b'{') {
            return Err(r.err(&format!("expected '{{' after {name}")));
        }
        let mut fields = Vec::with_capacity(8);
        loop {
            if r.eat(b'}') {
                break;
            }
            let Some(key) = r.ident() else { return Err(r.err("expected a key")) };
            if !r.eat(b'=') {
                return Err(r.err(&format!("expected '=' after {key}")));
            }
            fields.push((key, r.value()?));
        }
        blocks.push(Block { kind: name.to_ascii_lowercase(), fields });
    }
    Ok((namespace, blocks))
}

fn tex(s: Option<&str>, long: &mut Vec<String>) -> LumpName {
    let s = s.unwrap_or("-");
    if s.len() > 8 && !long.iter().any(|l| l == s) {
        long.push(s.to_string());
    }
    LumpName::new(s)
}

/// Load the UDMF map whose marker is `marker`.
pub fn load(stack: &ResourceStack, marker: LumpId, name: LumpName) -> Result<RawMap, ContentError> {
    // Lumps between the marker and ENDMAP, by name.
    let mut textmap = None;
    let mut znodes = None;
    let mut behavior = None;
    let mut id = marker;
    while let Some(next) = stack.next_in_source(id) {
        id = next;
        match stack.info(id).name.as_bytes() {
            b"TEXTMAP" => textmap = Some(id),
            b"ZNODES" => znodes = Some(id),
            b"BEHAVIOR" => behavior = Some(stack.data(id).to_vec()),
            b"ENDMAP" => break,
            _ => {}
        }
    }
    let textmap = textmap.ok_or_else(|| ContentError::MissingLump(format!("{name}/TEXTMAP")))?;
    let text = String::from_utf8_lossy(stack.data(textmap));
    let (namespace, blocks) = parse(&text).map_err(|e| ContentError::malformed(format!("{name}/TEXTMAP"), e))?;
    let mut warnings = Vec::new();
    let hexen_specials = matches!(namespace.as_str(), "zdoom" | "hexen" | "vavoom" | "eternity");
    if !matches!(namespace.as_str(), "doom" | "zdoom" | "hexen" | "heretic" | "strife" | "zdoomtranslated" | "vavoom" | "eternity") {
        warnings.push(format!("{name}: unknown UDMF namespace {namespace:?}"));
    }

    let mut map = RawMap {
        name,
        format: MapFormat::Udmf,
        things: Vec::new(),
        linedefs: Vec::new(),
        sidedefs: Vec::new(),
        vertexes: Vec::new(),
        segs: Vec::new(),
        subsectors: Vec::new(),
        nodes: Vec::new(),
        sectors: Vec::new(),
        reject: Vec::new(),
        blockmap: Vec::new(),
        behavior,
        warnings: Vec::new(),
    };
    let fx = |v: f64| (v * 65536.0).round() as i32;
    let mut long_names = Vec::new();
    for b in &blocks {
        match b.kind.as_str() {
            "vertex" => map.vertexes.push(MapVertex { x: fx(b.f("x", 0.0)), y: fx(b.f("y", 0.0)) }),
            "linedef" => {
                let mut flags = 0u16;
                for (k, bit) in [
                    ("blocking", ml::BLOCKING),
                    ("blockmonsters", ml::BLOCKMONSTERS),
                    ("twosided", ml::TWOSIDED),
                    ("dontpegtop", ml::DONTPEGTOP),
                    ("dontpegbottom", ml::DONTPEGBOTTOM),
                    ("secret", ml::SECRET),
                    ("blocksound", ml::SOUNDBLOCK),
                    ("dontdraw", ml::DONTDRAW),
                    ("mapped", ml::MAPPED),
                    // Closest vanilla behaviour for "blocks everything".
                    ("blockeverything", ml::BLOCKING),
                    ("blockplayers", ml::BLOCKING),
                ] {
                    if b.b(k) {
                        flags |= bit;
                    }
                }
                let side = |v: i64| if v < 0 { NO_SIDEDEF } else { v as u32 };
                let special = b.i("special", 0) as i32;
                let args = [b.i("arg0", 0), b.i("arg1", 0), b.i("arg2", 0), b.i("arg3", 0), b.i("arg4", 0)].map(|a| a as i32);
                let (doom_special, tag, action, id) = if hexen_specials {
                    let mut activation = 0;
                    for (k, bit) in [
                        ("playercross", spac::CROSS),
                        ("playeruse", spac::USE),
                        ("monstercross", spac::MCROSS),
                        ("impact", spac::IMPACT),
                        ("playerpush", spac::PUSH),
                        ("missilecross", spac::PCROSS),
                        ("passuse", spac::USETHROUGH),
                        ("anycross", spac::ANYCROSS),
                        ("monsteruse", spac::MUSE),
                        ("monsterpush", spac::MPUSH),
                        ("playeruseback", spac::USEBACK),
                        ("firstsideonly", spac::FIRSTSIDEONLY),
                    ] {
                        if b.b(k) {
                            activation |= bit;
                        }
                    }
                    let action = (special != 0).then_some(LineAction {
                        special,
                        args,
                        activation,
                        repeat: b.b("repeatspecial"),
                        lock: b.i("locknumber", 0) as i32,
                    });
                    (0, 0, action, b.i("id", -1) as i32)
                } else {
                    // Doom namespace: the line id is the tag.
                    (special as i16, b.i("id", 0).max(0) as i16, None, -1)
                };
                map.linedefs.push(MapLinedef {
                    v1: b.i("v1", 0) as u32,
                    v2: b.i("v2", 0) as u32,
                    flags,
                    special: doom_special,
                    tag,
                    sidenum: [side(b.i("sidefront", -1)), side(b.i("sideback", -1))],
                    alpha: b.f("alpha", 1.0) as f32,
                    action,
                    id,
                });
            }
            "sidedef" => map.sidedefs.push(MapSidedef {
                texture_offset: b.f("offsetx", 0.0) as f32,
                row_offset: b.f("offsety", 0.0) as f32,
                top: tex(b.s("texturetop"), &mut long_names),
                bottom: tex(b.s("texturebottom"), &mut long_names),
                mid: tex(b.s("texturemiddle"), &mut long_names),
                sector: b.i("sector", 0) as u32,
                part_offsets: [
                    [b.f("offsetx_top", 0.0) as f32, b.f("offsety_top", 0.0) as f32],
                    [b.f("offsetx_mid", 0.0) as f32, b.f("offsety_mid", 0.0) as f32],
                    [b.f("offsetx_bottom", 0.0) as f32, b.f("offsety_bottom", 0.0) as f32],
                ],
                light: b.i("light", 0) as i16,
                light_absolute: b.b("lightabsolute"),
            }),
            "sector" => {
                let xform = |p: &str| PlaneXform {
                    x_offset: b.f(&format!("xpanning{p}"), 0.0) as f32,
                    y_offset: b.f(&format!("ypanning{p}"), 0.0) as f32,
                    x_scale: b.f(&format!("xscale{p}"), 1.0) as f32,
                    y_scale: b.f(&format!("yscale{p}"), 1.0) as f32,
                    rotation: b.f(&format!("rotation{p}"), 0.0) as f32,
                };
                let raw_special = b.i("special", 0);
                let special = if hexen_specials { translate_sector_special(raw_special) } else { raw_special as i16 };
                map.sectors.push(MapSector {
                    floor_height: fx(b.f("heightfloor", 0.0)),
                    ceiling_height: fx(b.f("heightceiling", 0.0)),
                    floor_pic: tex(b.s("texturefloor"), &mut long_names),
                    ceiling_pic: tex(b.s("textureceiling"), &mut long_names),
                    light_level: b.i("lightlevel", 160) as i16,
                    special,
                    tag: b.i("id", 0) as i16,
                    floor_xform: xform("floor"),
                    ceiling_xform: xform("ceiling"),
                    floor_light: (b.i("lightfloor", 0) as i16, b.b("lightfloorabsolute")),
                    ceiling_light: (b.i("lightceiling", 0) as i16, b.b("lightceilingabsolute")),
                });
            }
            "thing" => {
                let mut options = 0i16;
                if b.b("skill1") || b.b("skill2") {
                    options |= mtf::EASY;
                }
                if b.b("skill3") {
                    options |= mtf::NORMAL;
                }
                if b.b("skill4") || b.b("skill5") {
                    options |= mtf::HARD;
                }
                if b.b("ambush") {
                    options |= mtf::AMBUSH;
                }
                if !b.b("single") {
                    options |= mtf::NOTSINGLE;
                }
                map.things.push(MapThing {
                    x: fx(b.f("x", 0.0)),
                    y: fx(b.f("y", 0.0)),
                    z: fx(b.f("height", 0.0)),
                    angle: b.i("angle", 0) as i16,
                    kind: b.i("type", 0) as i16,
                    options,
                    tid: b.i("id", 0) as i32,
                    special: b.i("special", 0) as i32,
                    args: [b.i("arg0", 0), b.i("arg1", 0), b.i("arg2", 0), b.i("arg3", 0), b.i("arg4", 0)].map(|a| a as i32),
                });
            }
            _ => {}
        }
    }
    for l in long_names.iter().take(10) {
        warnings.push(format!("{name}: texture name {l:?} is longer than 8 characters (truncated)"));
    }
    map.warnings = warnings;

    match znodes {
        Some(znodes) => crate::nodes::load_zdoom(&mut map, stack.data(znodes))?,
        // GZDoom builds nodes itself, so many UDMF maps ship without them.
        None => crate::nodebuild::build_nodes(&mut map),
    }
    Ok(map)
}

/// ZDoom/Hexen sector types to Doom ones: 65..=81 are Doom's 1..=17;
/// bit 1024 is "secret", bits 32/64 Boom-style damage.
fn translate_sector_special(s: i64) -> i16 {
    let base = s & 0xff;
    let mut doom = if (65..=81).contains(&base) { (base - 64) as i16 } else { 0 };
    if doom == 0 {
        if s & 1024 != 0 {
            doom = 9;
        } else {
            doom = match (s >> 5) & 3 {
                1 => 7,
                2 => 5,
                3 => 16,
                _ => 0,
            };
        }
    }
    doom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let (ns, blocks) = parse(
            "namespace = \"zdoom\";\n// comment\nvertex { x = -16.5; y = 0x20; }\nlinedef // 0\n{ v1 = 0; blocking = true; arg0 = -3; }",
        )
        .unwrap();
        assert_eq!(ns, "zdoom");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].f("x", 0.0), -16.5);
        assert_eq!(blocks[0].i("y", 0), 32);
        assert!(blocks[1].b("blocking"));
        assert_eq!(blocks[1].i("arg0", 0), -3);
    }

    #[test]
    fn door_translation() {
        let (_, blocks) = parse("linedef { playeruse = true; repeatspecial = true; locknumber = 2; arg0 = 7; }").unwrap();
        assert!(blocks[0].b("playeruse") && blocks[0].b("repeatspecial"));
        assert_eq!(blocks[0].i("locknumber", 0), 2);
    }
}
