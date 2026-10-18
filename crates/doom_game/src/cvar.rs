//! Console variables: a typed, thread-safe settings registry with config
//! file persistence.
//!
//! Every setting a player can change lives here under a GZDoom-style name
//! (`cl_freelook`, `r_render_mode`, ...). Values are typed ([`CVarValue`]) and
//! validated against the variable's spec (ranges, choice lists). The
//! registry is shared (`Arc<CVarRegistry>`) between the session, the menu
//! and the host, so a host can read mouse settings on its input thread and
//! register host-only variables of its own.
//!
//! Consumers poll [`CVarRegistry::generation`], a counter bumped on every
//! change, and re-read values when it moves; there are no callbacks to keep
//! alive.
//!
//! The config file is a TOML subset (`name = value` lines, `#` comments,
//! booleans, integers, floats and strings). Values for variables that are
//! not registered (yet) are kept and written back, so settings for a mod or
//! host feature survive a run without it.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::RwLock;

/// A variable's value.
#[derive(Clone, Debug, PartialEq)]
pub enum CVarValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(String),
}

impl CVarValue {
    fn same_kind(&self, other: &CVarValue) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// The value as written in the config file.
    fn to_toml(&self) -> String {
        match self {
            CVarValue::Bool(b) => b.to_string(),
            CVarValue::Int(i) => i.to_string(),
            // `{:?}` always keeps a decimal point, so it reads back as a float.
            CVarValue::Float(f) => format!("{f:?}"),
            CVarValue::Str(s) => quote(s),
        }
    }
}

impl fmt::Display for CVarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CVarValue::Bool(b) => write!(f, "{}", if *b { "on" } else { "off" }),
            CVarValue::Int(i) => write!(f, "{i}"),
            CVarValue::Float(x) => write!(f, "{x:.2}"),
            CVarValue::Str(s) => write!(f, "{s}"),
        }
    }
}

/// Rust types a variable can be read and written as.
pub trait CVarType: Sized {
    fn from_value(v: &CVarValue) -> Option<Self>;
    fn into_value(self) -> CVarValue;
}

impl CVarType for bool {
    fn from_value(v: &CVarValue) -> Option<Self> {
        match v {
            CVarValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn into_value(self) -> CVarValue {
        CVarValue::Bool(self)
    }
}

impl CVarType for i32 {
    fn from_value(v: &CVarValue) -> Option<Self> {
        match v {
            CVarValue::Int(i) => Some(*i),
            _ => None,
        }
    }
    fn into_value(self) -> CVarValue {
        CVarValue::Int(self)
    }
}

impl CVarType for f32 {
    fn from_value(v: &CVarValue) -> Option<Self> {
        match v {
            CVarValue::Float(f) => Some(*f),
            _ => None,
        }
    }
    fn into_value(self) -> CVarValue {
        CVarValue::Float(self)
    }
}

impl CVarType for String {
    fn from_value(v: &CVarValue) -> Option<Self> {
        match v {
            CVarValue::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn into_value(self) -> CVarValue {
        CVarValue::Str(self)
    }
}

/// A typed name for a variable, so call sites can't read `cl_freelook` as
/// an integer: `cvars.get(CL_FREELOOK)` is a `bool`.
pub struct CVar<T> {
    pub name: &'static str,
    _type: PhantomData<fn() -> T>,
}

impl<T> CVar<T> {
    pub const fn new(name: &'static str) -> Self {
        CVar { name, _type: PhantomData }
    }
}

impl<T> Clone for CVar<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for CVar<T> {}

/// How a variable is declared: type and default, limits, and how it is
/// presented.
#[derive(Clone, Debug)]
pub struct CVarSpec {
    pub name: &'static str,
    pub help: &'static str,
    pub default: CVarValue,
    /// Inclusive range for numbers; values outside are clamped.
    pub range: Option<(f32, f32)>,
    /// Step for menu adjustment of numbers.
    pub step: f32,
    /// Allowed values for strings (compared case-insensitively, stored as
    /// listed). Empty: any string.
    pub choices: &'static [&'static str],
    /// Saved to the config file.
    pub archive: bool,
    /// Shown on the engine options menu page with this label.
    pub menu: Option<&'static str>,
}

impl CVarSpec {
    fn new(name: &'static str, default: CVarValue) -> Self {
        CVarSpec { name, help: "", default, range: None, step: 1.0, choices: &[], archive: true, menu: None }
    }

    pub fn bool(name: &'static str, default: bool) -> Self {
        Self::new(name, CVarValue::Bool(default))
    }

    pub fn int(name: &'static str, default: i32, min: i32, max: i32) -> Self {
        CVarSpec { range: Some((min as f32, max as f32)), ..Self::new(name, CVarValue::Int(default)) }
    }

    pub fn float(name: &'static str, default: f32, min: f32, max: f32, step: f32) -> Self {
        CVarSpec { range: Some((min, max)), step, ..Self::new(name, CVarValue::Float(default)) }
    }

    pub fn string(name: &'static str, default: &str) -> Self {
        Self::new(name, CVarValue::Str(default.to_string()))
    }

    /// A string restricted to `choices` (an enum setting).
    pub fn choice(name: &'static str, default: &'static str, choices: &'static [&'static str]) -> Self {
        CVarSpec { choices, ..Self::string(name, default) }
    }

    pub fn help(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }

    pub fn menu(mut self, label: &'static str) -> Self {
        self.menu = Some(label);
        self
    }

    /// Not saved to the config file.
    pub fn transient(mut self) -> Self {
        self.archive = false;
        self
    }

    /// Bring a value of the right kind into range / onto a listed choice.
    fn validate(&self, v: CVarValue) -> Result<CVarValue, CVarError> {
        if !v.same_kind(&self.default) {
            return Err(CVarError::WrongType(self.name));
        }
        Ok(match v {
            CVarValue::Int(i) => match self.range {
                Some((lo, hi)) => CVarValue::Int(i.clamp(lo as i32, hi as i32)),
                None => CVarValue::Int(i),
            },
            CVarValue::Float(f) if f.is_nan() => return Err(CVarError::BadValue(self.name, "NaN".into())),
            CVarValue::Float(f) => match self.range {
                Some((lo, hi)) => CVarValue::Float(f.clamp(lo, hi)),
                None => CVarValue::Float(f),
            },
            CVarValue::Str(s) if !self.choices.is_empty() => match self.choices.iter().find(|c| c.eq_ignore_ascii_case(&s)) {
                Some(c) => CVarValue::Str(c.to_string()),
                None => return Err(CVarError::BadValue(self.name, s)),
            },
            v => v,
        })
    }

    /// Parse console/config text as this variable's type. Booleans also
    /// take `1/0`, `on/off`, `yes/no` (GZDoom ini style).
    fn parse(&self, text: &str) -> Result<CVarValue, CVarError> {
        let t = text.trim();
        let bad = || CVarError::BadValue(self.name, t.to_string());
        let v = match &self.default {
            CVarValue::Bool(_) => CVarValue::Bool(match t.to_ascii_lowercase().as_str() {
                "true" | "1" | "on" | "yes" => true,
                "false" | "0" | "off" | "no" => false,
                _ => return Err(bad()),
            }),
            CVarValue::Int(_) => CVarValue::Int(match t.parse::<i32>() {
                Ok(i) => i,
                // An integral float ("3.0") is accepted.
                Err(_) => match t.parse::<f64>() {
                    Ok(f) if f.fract() == 0.0 && f.abs() <= i32::MAX as f64 => f as i32,
                    _ => return Err(bad()),
                },
            }),
            CVarValue::Float(_) => CVarValue::Float(t.parse::<f32>().map_err(|_| bad())?),
            CVarValue::Str(_) => CVarValue::Str(t.to_string()),
        };
        self.validate(v)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CVarError {
    Unknown(String),
    WrongType(&'static str),
    BadValue(&'static str, String),
    AlreadyRegistered(&'static str),
}

impl fmt::Display for CVarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CVarError::Unknown(n) => write!(f, "unknown variable {n}"),
            CVarError::WrongType(n) => write!(f, "{n}: wrong value type"),
            CVarError::BadValue(n, v) => write!(f, "{n}: bad value {v:?}"),
            CVarError::AlreadyRegistered(n) => write!(f, "{n} is already registered"),
        }
    }
}

impl std::error::Error for CVarError {}

/// A variable as listed for menus and consoles.
#[derive(Clone, Debug)]
pub struct CVarInfo {
    pub name: &'static str,
    pub help: &'static str,
    pub menu: Option<&'static str>,
    pub value: CVarValue,
    pub default: CVarValue,
    /// Saved to the config file (transient variables are debug switches).
    pub archive: bool,
    pub choices: &'static [&'static str],
    pub range: Option<(f32, f32)>,
}

struct Entry {
    spec: CVarSpec,
    value: CVarValue,
}

fn info_of(e: &Entry) -> CVarInfo {
    CVarInfo {
        name: e.spec.name,
        help: e.spec.help,
        menu: e.spec.menu,
        value: e.value.clone(),
        default: e.spec.default.clone(),
        archive: e.spec.archive,
        choices: e.spec.choices,
        range: e.spec.range,
    }
}

#[derive(Default)]
struct Inner {
    /// Registration order (menus and the config file follow it).
    vars: Vec<Entry>,
    index: HashMap<&'static str, usize>,
    /// Config values for names nobody has registered, as raw TOML values.
    pending: BTreeMap<String, String>,
    generation: u64,
}

impl Inner {
    fn entry(&self, name: &str) -> Option<&Entry> {
        self.index.get(name).map(|&i| &self.vars[i])
    }

    fn assign(&mut self, i: usize, v: CVarValue) -> bool {
        let e = &mut self.vars[i];
        if e.value == v {
            return false;
        }
        e.value = v;
        self.generation += 1;
        true
    }
}

#[derive(Default)]
pub struct CVarRegistry {
    inner: RwLock<Inner>,
}

impl fmt::Debug for CVarRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut m = f.debug_map();
        for v in self.list() {
            m.entry(&v.name, &v.value);
        }
        m.finish()
    }
}

impl CVarRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry holding the engine's variables ([`engine_specs`]).
    pub fn with_engine_vars() -> Self {
        let r = Self::new();
        for spec in engine_specs() {
            r.register(spec).expect("engine cvar names are unique");
        }
        r
    }

    // A panic while holding the lock leaves plain data behind; keep going.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Add a variable at its default, or at a value the config file gave
    /// before it was registered.
    pub fn register(&self, spec: CVarSpec) -> Result<(), CVarError> {
        let mut g = self.write();
        if g.index.contains_key(spec.name) {
            return Err(CVarError::AlreadyRegistered(spec.name));
        }
        let value = match g.pending.remove(spec.name).map(|raw| parse_toml_value(&raw).and_then(|v| coerce(&spec, v))) {
            Some(Ok(v)) => v,
            _ => spec.default.clone(),
        };
        let i = g.vars.len();
        g.index.insert(spec.name, i);
        g.vars.push(Entry { spec, value });
        g.generation += 1;
        Ok(())
    }

    /// Bumped by every change; poll it to notice updates.
    pub fn generation(&self) -> u64 {
        self.read().generation
    }

    pub fn contains(&self, name: &str) -> bool {
        self.read().index.contains_key(name)
    }

    /// The current value. Panics if the variable is not registered or has
    /// another type: typed handles name engine variables, so that is a bug.
    pub fn get<T: CVarType>(&self, var: CVar<T>) -> T {
        self.try_get(var.name).unwrap_or_else(|| panic!("cvar {} is not registered with this type", var.name))
    }

    pub fn try_get<T: CVarType>(&self, name: &str) -> Option<T> {
        self.read().entry(name).and_then(|e| T::from_value(&e.value))
    }

    pub fn value(&self, name: &str) -> Option<CVarValue> {
        self.read().entry(name).map(|e| e.value.clone())
    }

    /// Set a typed value (clamped / matched to the spec). Returns whether it
    /// changed.
    pub fn set<T: CVarType>(&self, var: CVar<T>, v: T) -> Result<bool, CVarError> {
        self.set_value(var.name, v.into_value())
    }

    pub fn set_value(&self, name: &str, v: CVarValue) -> Result<bool, CVarError> {
        let mut g = self.write();
        let i = *g.index.get(name).ok_or_else(|| CVarError::Unknown(name.to_string()))?;
        let v = g.vars[i].spec.validate(v)?;
        Ok(g.assign(i, v))
    }

    /// Set from text, as typed at a console (`cl_freelook 0`).
    pub fn set_str(&self, name: &str, text: &str) -> Result<bool, CVarError> {
        let mut g = self.write();
        let i = *g.index.get(name).ok_or_else(|| CVarError::Unknown(name.to_string()))?;
        let v = g.vars[i].spec.parse(text)?;
        Ok(g.assign(i, v))
    }

    pub fn reset(&self, name: &str) -> Result<bool, CVarError> {
        let mut g = self.write();
        let i = *g.index.get(name).ok_or_else(|| CVarError::Unknown(name.to_string()))?;
        let v = g.vars[i].spec.default.clone();
        Ok(g.assign(i, v))
    }

    /// Menu adjustment: toggle a boolean, step a number by its step (`dir`
    /// is the sign) or move to the next/previous choice (wrapping).
    pub fn step(&self, name: &str, dir: i32) -> Result<bool, CVarError> {
        let mut g = self.write();
        let i = *g.index.get(name).ok_or_else(|| CVarError::Unknown(name.to_string()))?;
        let spec = &g.vars[i].spec;
        let next = match &g.vars[i].value {
            CVarValue::Bool(b) => CVarValue::Bool(!b),
            CVarValue::Int(v) => CVarValue::Int(v.saturating_add(dir.signum() * spec.step.max(1.0) as i32)),
            CVarValue::Float(v) => CVarValue::Float(v + dir.signum() as f32 * spec.step),
            CVarValue::Str(s) if !spec.choices.is_empty() => {
                let n = spec.choices.len() as i32;
                let at = spec.choices.iter().position(|c| c.eq_ignore_ascii_case(s)).unwrap_or(0) as i32;
                CVarValue::Str(spec.choices[(at + dir.signum()).rem_euclid(n) as usize].to_string())
            }
            CVarValue::Str(_) => return Ok(false),
        };
        let v = spec.validate(next)?;
        Ok(g.assign(i, v))
    }

    /// All variables in registration order.
    pub fn list(&self) -> Vec<CVarInfo> {
        self.read()
            .vars
            .iter()
            .map(|e| info_of(e))
            .collect()
    }

    /// One variable's listing.
    pub fn info(&self, name: &str) -> Option<CVarInfo> {
        self.read().entry(name).map(info_of)
    }

    // ---- config file --------------------------------------------------------

    /// The archived variables (and kept unknown ones) as config text.
    pub fn to_config_string(&self) -> String {
        let g = self.read();
        let mut out = String::from("# doom_bevy configuration: written on exit, edit freely.\n");
        for e in g.vars.iter().filter(|e| e.spec.archive) {
            out.push('\n');
            if !e.spec.help.is_empty() {
                out.push_str(&format!("# {}\n", e.spec.help));
            }
            out.push_str(&format!("{} = {}\n", e.spec.name, e.value.to_toml()));
        }
        if !g.pending.is_empty() {
            out.push_str("\n# Not used by this run; kept for the feature or mod that sets them.\n");
            for (k, v) in &g.pending {
                out.push_str(&format!("{k} = {v}\n"));
            }
        }
        out
    }

    /// Apply config text. Bad lines and values are skipped and reported.
    pub fn load_config_str(&self, text: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("config line {}: expected `name = value`", n + 1));
                continue;
            };
            let (key, value) = (key.trim().trim_matches('"'), value.trim());
            let mut g = self.write();
            let Some(&i) = g.index.get(key) else {
                g.pending.insert(key.to_string(), value.to_string());
                continue;
            };
            match parse_toml_value(value).and_then(|v| coerce(&g.vars[i].spec, v)) {
                Ok(v) => {
                    g.assign(i, v);
                }
                Err(e) => warnings.push(format!("config line {}: {e}", n + 1)),
            }
        }
        warnings
    }

    /// Load `path` if it exists (a missing file is not an error).
    pub fn load_file(&self, path: &Path) -> Vec<String> {
        match std::fs::read_to_string(path) {
            Ok(text) => self.load_config_str(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => vec![format!("{}: {e}", path.display())],
        }
    }

    /// Write the config, via a temporary file so a crash mid-write can't
    /// leave a truncated config behind.
    pub fn save_file(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_config_string())?;
        std::fs::rename(&tmp, path)
    }
}

/// Convert a parsed config value to the variable's type (`1`/`0` for
/// booleans and whole floats for integers are accepted).
fn coerce(spec: &CVarSpec, v: CVarValue) -> Result<CVarValue, CVarError> {
    let v = match (&spec.default, v) {
        (CVarValue::Bool(_), CVarValue::Int(i @ (0 | 1))) => CVarValue::Bool(i == 1),
        (CVarValue::Float(_), CVarValue::Int(i)) => CVarValue::Float(i as f32),
        (CVarValue::Int(_), CVarValue::Float(f)) if f.fract() == 0.0 => CVarValue::Int(f as i32),
        (CVarValue::Str(_), CVarValue::Bool(b)) => CVarValue::Str(b.to_string()),
        (_, v) => v,
    };
    spec.validate(v)
}

/// Everything before a `#` that is not inside a string.
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (Some(q), c) if c == q && !escaped => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => return &line[..i],
            _ => {}
        }
        escaped = false;
    }
    line
}

fn parse_toml_value(s: &str) -> Result<CVarValue, CVarError> {
    let bad = || CVarError::BadValue("config", s.to_string());
    let s = s.trim();
    if let Some(body) = s.strip_prefix('"') {
        let body = body.strip_suffix('"').ok_or_else(bad)?;
        return unquote(body).map(CVarValue::Str).ok_or_else(bad);
    }
    if let Some(body) = s.strip_prefix('\'') {
        return body.strip_suffix('\'').map(|b| CVarValue::Str(b.to_string())).ok_or_else(bad);
    }
    match s {
        "true" => return Ok(CVarValue::Bool(true)),
        "false" => return Ok(CVarValue::Bool(false)),
        _ => {}
    }
    let digits = s.replace('_', "");
    if let Ok(i) = digits.parse::<i32>() {
        return Ok(CVarValue::Int(i));
    }
    digits.parse::<f32>().map(CVarValue::Float).map_err(|_| bad())
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn unquote(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            'n' => '\n',
            't' => '\t',
            '"' => '"',
            '\\' => '\\',
            _ => return None,
        });
    }
    Some(out)
}

// ---- engine variables --------------------------------------------------------

pub const CL_FREELOOK: CVar<bool> = CVar::new("cl_freelook");
pub const R_RENDER_MODE: CVar<String> = CVar::new("r_render_mode");
pub const SV_AUTOAIM: CVar<bool> = CVar::new("sv_autoaim");
pub const R_WIPE: CVar<bool> = CVar::new("r_wipe");
pub const R_SCREENBLOCKS: CVar<i32> = CVar::new("r_screenblocks");
pub const R_LOWDETAIL: CVar<bool> = CVar::new("r_lowdetail");
pub const VID_GAMMA: CVar<i32> = CVar::new("vid_gamma");
pub const HUD_MESSAGES: CVar<bool> = CVar::new("hud_messages");
pub const SND_SFXVOLUME: CVar<i32> = CVar::new("snd_sfxvolume");
pub const SND_MUSICVOLUME: CVar<i32> = CVar::new("snd_musicvolume");
pub const M_SENSITIVITY: CVar<i32> = CVar::new("m_sensitivity");

// Player controller and skateboarding.
pub const PLAYER_CONTROLLER: CVar<String> = CVar::new("player_controller");
pub const SKATE_ACCELERATION: CVar<f32> = CVar::new("skate_acceleration");
pub const SKATE_MAX_SPEED: CVar<f32> = CVar::new("skate_max_speed");
pub const SKATE_TURN_RATE: CVar<f32> = CVar::new("skate_turn_rate");
pub const SKATE_CARVE_STRENGTH: CVar<f32> = CVar::new("skate_carve_strength");
pub const SKATE_FRICTION: CVar<f32> = CVar::new("skate_friction");
pub const SKATE_BRAKE_STRENGTH: CVar<f32> = CVar::new("skate_brake_strength");
pub const SKATE_JUMP_STRENGTH: CVar<f32> = CVar::new("skate_jump_strength");
pub const SKATE_AIR_CONTROL: CVar<f32> = CVar::new("skate_air_control");
pub const SKATE_GRAVITY: CVar<f32> = CVar::new("skate_gravity");
pub const SKATE_BOARD_LENGTH: CVar<f32> = CVar::new("skate_board_length");
pub const SKATE_GRIND_SNAP_DISTANCE: CVar<f32> = CVar::new("skate_grind_snap_distance");
pub const SKATE_GRIND_MAX_ANGLE: CVar<f32> = CVar::new("skate_grind_max_angle");
pub const SKATE_GRIND_MAX_HEIGHT: CVar<f32> = CVar::new("skate_grind_max_height");
pub const SKATE_GRIND_ASSIST: CVar<bool> = CVar::new("skate_grind_assist");
pub const SKATE_CAMERA_DISTANCE: CVar<f32> = CVar::new("skate_camera_distance");
pub const SKATE_CAMERA_HEIGHT: CVar<f32> = CVar::new("skate_camera_height");
pub const SKATE_CAMERA_LOOK_HEIGHT: CVar<f32> = CVar::new("skate_camera_look_height");
pub const SKATE_CAMERA_FOLLOW: CVar<f32> = CVar::new("skate_camera_follow");
pub const SKATE_CAMERA_ROTATION: CVar<f32> = CVar::new("skate_camera_rotation");
pub const SKATE_CAMERA_RADIUS: CVar<f32> = CVar::new("skate_camera_radius");
pub const SKATE_CAMERA_MIN_DISTANCE: CVar<f32> = CVar::new("skate_camera_min_distance");
pub const SKATE_CAMERA_MAX_DISTANCE: CVar<f32> = CVar::new("skate_camera_max_distance");

// Debug switches (not saved).
pub const TIMESCALE: CVar<f32> = CVar::new("timescale");
pub const SHOW_PLAYER: CVar<bool> = CVar::new("show_player");
pub const SHOW_COLLISION: CVar<bool> = CVar::new("show_collision");
pub const SHOW_VELOCITY: CVar<bool> = CVar::new("show_velocity");
pub const SHOW_GRINDABLES: CVar<bool> = CVar::new("show_grindables");
pub const SHOW_SKATE_STATE: CVar<bool> = CVar::new("show_skate_state");
pub const SKATE_DEBUG: CVar<bool> = CVar::new("skate_debug");
pub const R_WIREFRAME: CVar<bool> = CVar::new("r_wireframe");

/// The variables the session owns. Defaults are vanilla's.
pub fn engine_specs() -> Vec<CVarSpec> {
    vec![
        CVarSpec::choice(R_RENDER_MODE.name, "Classic", &["Classic", "GZDoomClean"])
            .help("Classic: 320x200 palettized Doom. GZDoomClean: truecolor 3D at the display resolution.")
            .menu("Render mode"),
        CVarSpec::bool(CL_FREELOOK.name, false)
            .help("Look up and down with a real camera pitch. Off keeps Doom's fixed horizon.")
            .menu("Freelook"),
        CVarSpec::bool(SV_AUTOAIM.name, true)
            .help("Aim shots up/down at targets like Doom. Off: shots follow the view pitch (with freelook).")
            .menu("Autoaim"),
        CVarSpec::bool(R_WIPE.name, true).help("Melt the screen on level and screen changes.").menu("Screen melt"),
        CVarSpec::int(R_SCREENBLOCKS.name, 10, 3, 11).help("View size: 3..9 bordered, 10 status bar, 11 full screen."),
        CVarSpec::bool(R_LOWDETAIL.name, false).help("Low detail: double-wide pixels."),
        CVarSpec::int(VID_GAMMA.name, 0, 0, 4).help("Gamma correction level (F11)."),
        CVarSpec::bool(HUD_MESSAGES.name, true).help("Show pickup and status messages."),
        CVarSpec::int(SND_SFXVOLUME.name, 8, 0, 15).help("Sound effects volume."),
        CVarSpec::int(SND_MUSICVOLUME.name, 8, 0, 15).help("Music volume."),
        CVarSpec::int(M_SENSITIVITY.name, 5, 0, 9).help("Mouse sensitivity (menu slider)."),
    ]
    .into_iter()
    .chain(skate_specs())
    .chain(debug_specs())
    .collect()
}

/// The skateboard controller's variables (defaults from
/// `doom_sim::skate::SkateParams`).
fn skate_specs() -> Vec<CVarSpec> {
    let d = doom_sim::skate::SkateParams::default();
    vec![
        CVarSpec::choice(PLAYER_CONTROLLER.name, "Classic", &["Classic", "Skate"])
            .help("How the player moves: Classic Doom walking, or Skate (skateboard, third-person camera)."),
        CVarSpec::float(SKATE_ACCELERATION.name, d.acceleration, 50.0, 3000.0, 20.0).help("Skate: push acceleration (units/s^2)."),
        CVarSpec::float(SKATE_MAX_SPEED.name, d.max_speed, 100.0, 2000.0, 20.0).help("Skate: top pushing speed (units/s)."),
        CVarSpec::float(SKATE_TURN_RATE.name, d.turn_rate, 30.0, 720.0, 10.0).help("Skate: steering rate (degrees/s)."),
        CVarSpec::float(SKATE_CARVE_STRENGTH.name, d.carve_strength, 0.0, 2.0, 0.1).help("Skate: carving turn and wheel grip (0..2)."),
        CVarSpec::float(SKATE_FRICTION.name, d.friction, 0.0, 5.0, 0.05).help("Skate: rolling resistance."),
        CVarSpec::float(SKATE_BRAKE_STRENGTH.name, d.brake_strength, 50.0, 5000.0, 50.0).help("Skate: braking (units/s^2)."),
        CVarSpec::float(SKATE_JUMP_STRENGTH.name, d.jump_strength, 50.0, 1200.0, 10.0).help("Skate: ollie take-off speed (units/s)."),
        CVarSpec::float(SKATE_AIR_CONTROL.name, d.air_control, 0.0, 1.0, 0.05).help("Skate: steering in the air (0..1)."),
        CVarSpec::float(SKATE_GRAVITY.name, d.gravity, 100.0, 4000.0, 25.0).help("Skate: gravity (units/s^2; Doom uses 1225)."),
        CVarSpec::float(SKATE_BOARD_LENGTH.name, d.board_length, 16.0, 96.0, 2.0).help("Skate: board length (units)."),
        CVarSpec::float(SKATE_GRIND_SNAP_DISTANCE.name, d.grind_snap_distance, 1.0, 64.0, 1.0)
            .help("Skate: how far from an edge the board snaps onto it (units)."),
        CVarSpec::float(SKATE_GRIND_MAX_ANGLE.name, d.grind_max_angle, 5.0, 89.0, 5.0)
            .help("Skate: largest approach angle to an edge that still grinds (degrees)."),
        CVarSpec::float(SKATE_GRIND_MAX_HEIGHT.name, d.grind_max_height, 8.0, 256.0, 4.0)
            .help("Skate: tallest ledge or rail treated as grindable (units)."),
        CVarSpec::bool(SKATE_GRIND_ASSIST.name, d.grind_assist).help("Skate: grind magnetism, balance help and corner following."),
        CVarSpec::float(SKATE_CAMERA_DISTANCE.name, 110.0, 32.0, 400.0, 8.0).help("Skate: chase camera distance at rest (units); it backs off with speed."),
        CVarSpec::float(SKATE_CAMERA_HEIGHT.name, 46.0, 0.0, 200.0, 4.0).help("Skate: chase camera height above the board (units)."),
        CVarSpec::float(SKATE_CAMERA_LOOK_HEIGHT.name, 24.0, 0.0, 96.0, 2.0).help("Skate: height above the board the camera looks at (units)."),
        CVarSpec::float(SKATE_CAMERA_FOLLOW.name, 9.0, 0.5, 60.0, 0.5).help("Skate: camera position smoothing (higher follows tighter, per second)."),
        CVarSpec::float(SKATE_CAMERA_ROTATION.name, 5.0, 0.5, 60.0, 0.5).help("Skate: camera turning smoothing (higher turns faster, per second)."),
        CVarSpec::float(SKATE_CAMERA_RADIUS.name, 8.0, 0.0, 32.0, 1.0).help("Skate: how far the camera keeps from walls (units)."),
        CVarSpec::float(SKATE_CAMERA_MIN_DISTANCE.name, 64.0, 16.0, 400.0, 8.0).help("Skate: closest the camera sits behind the board (units)."),
        CVarSpec::float(SKATE_CAMERA_MAX_DISTANCE.name, 190.0, 16.0, 600.0, 8.0).help("Skate: farthest the camera backs off at speed (units)."),
    ]
}

/// Developer switches; transient, so a session always starts clean.
fn debug_specs() -> Vec<CVarSpec> {
    vec![
        CVarSpec::float(TIMESCALE.name, 1.0, 0.05, 10.0, 0.05).help("Game speed multiplier.").transient(),
        CVarSpec::bool(SHOW_PLAYER.name, false).help("Show player position and state.").transient(),
        CVarSpec::bool(SHOW_COLLISION.name, false).help("Draw collision boxes and blocking lines.").transient(),
        CVarSpec::bool(SHOW_VELOCITY.name, false).help("Draw the player's velocity.").transient(),
        CVarSpec::bool(SHOW_GRINDABLES.name, false).help("Draw grindable edges and their connections.").transient(),
        CVarSpec::bool(SHOW_SKATE_STATE.name, false).help("Show the skate controller's state.").transient(),
        CVarSpec::bool(SKATE_DEBUG.name, false).help("Skate debug view: wheel contacts, board axes, state text.").transient(),
        CVarSpec::bool(R_WIREFRAME.name, false).help("Draw the level as wireframe.").transient(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_get_set_and_validation() {
        let r = CVarRegistry::with_engine_vars();
        assert!(!r.get(CL_FREELOOK));
        let g = r.generation();
        assert_eq!(r.set(CL_FREELOOK, true), Ok(true));
        assert_eq!(r.set(CL_FREELOOK, true), Ok(false), "no change, no bump");
        assert_eq!(r.generation(), g + 1);
        // Ranges clamp; choices match case-insensitively.
        r.set(SND_SFXVOLUME, 99).unwrap();
        assert_eq!(r.get(SND_SFXVOLUME), 15);
        assert_eq!(r.get(R_RENDER_MODE), "Classic", "classic is the default");
        r.set(R_RENDER_MODE, "gzdoomclean".into()).unwrap();
        assert_eq!(r.get(R_RENDER_MODE), "GZDoomClean");
        assert!(matches!(r.set(R_RENDER_MODE, "voxel".into()), Err(CVarError::BadValue(..))));
        assert!(matches!(r.set_value("cl_freelook", CVarValue::Int(1)), Err(CVarError::WrongType(_))));
        assert!(matches!(r.set_str("nope", "1"), Err(CVarError::Unknown(_))));
        assert!(matches!(r.register(CVarSpec::bool("cl_freelook", true)), Err(CVarError::AlreadyRegistered(_))));
    }

    #[test]
    fn console_text_and_menu_steps() {
        let r = CVarRegistry::with_engine_vars();
        r.set_str("cl_freelook", "on").unwrap();
        assert!(r.get(CL_FREELOOK));
        r.set_str("cl_freelook", "off").unwrap();
        assert!(!r.get(CL_FREELOOK));
        r.set_str("r_screenblocks", "7.0").unwrap();
        assert_eq!(r.get(R_SCREENBLOCKS), 7);
        assert!(r.set_str("r_screenblocks", "seven").is_err());
        r.step("cl_freelook", 1).unwrap();
        assert!(r.get(CL_FREELOOK));
        r.step("r_render_mode", 1).unwrap();
        r.step("r_render_mode", 1).unwrap();
        assert_eq!(r.get(R_RENDER_MODE), "Classic", "wraps around");
        r.step("r_screenblocks", -1).unwrap();
        assert_eq!(r.get(R_SCREENBLOCKS), 6);
        r.register(CVarSpec::float("m_pitch", 1.0, 0.0, 4.0, 0.25)).unwrap();
        r.step("m_pitch", -1).unwrap();
        assert_eq!(r.try_get::<f32>("m_pitch"), Some(0.75));
    }

    #[test]
    fn config_round_trip_keeps_unknown_values() {
        let r = CVarRegistry::with_engine_vars();
        r.set(CL_FREELOOK, true).unwrap();
        r.set(R_RENDER_MODE, "GZDoomClean".into()).unwrap();
        r.set(SND_MUSICVOLUME, 3).unwrap();
        let name = "Doom \"Guy\" #1\\\t";
        r.register(CVarSpec::string("player_name", "")).unwrap();
        r.set_str("player_name", name).unwrap();
        r.register(CVarSpec::bool("dev_trace", true).transient()).unwrap();
        r.register(CVarSpec::float("m_yaw", 1.5, 0.0, 10.0, 0.1)).unwrap();
        let mut text = r.to_config_string();
        assert!(!text.contains("dev_trace"), "transient variables are not saved");
        text.push_str("mod_flashlight = \"bright\" # set by some mod\n");
        let r2 = CVarRegistry::with_engine_vars();
        r2.register(CVarSpec::string("player_name", "")).unwrap();
        let warnings = r2.load_config_str(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(r2.get(CL_FREELOOK));
        assert_eq!(r2.get(R_RENDER_MODE), "GZDoomClean");
        assert_eq!(r2.get(SND_MUSICVOLUME), 3);
        assert_eq!(r2.try_get::<String>("player_name").unwrap(), name.trim(), "quotes, escapes and # survive");
        // Registered after loading: picks up the saved value.
        r2.register(CVarSpec::float("m_yaw", 1.0, 0.0, 10.0, 0.1)).unwrap();
        assert_eq!(r2.try_get::<f32>("m_yaw"), Some(1.5));
        // Never registered: written back untouched.
        assert!(r2.to_config_string().contains("mod_flashlight = \"bright\""));
    }

    #[test]
    fn config_parsing_is_tolerant() {
        let r = CVarRegistry::with_engine_vars();
        r.register(CVarSpec::string("name", "")).unwrap();
        let warnings = r.load_config_str(
            "[cvars]\n\
             cl_freelook = 1          # ini-style boolean\n\
             r_screenblocks = 1_1\n\
             snd_sfxvolume = \"loud\"\n\
             garbage line\n\
             name = \"a # not a comment \\\" q\"\n",
        );
        assert!(r.get(CL_FREELOOK));
        assert_eq!(r.get(R_SCREENBLOCKS), 11);
        assert_eq!(r.try_get::<String>("name").unwrap(), "a # not a comment \" q");
        assert_eq!(r.get(SND_SFXVOLUME), 8, "bad value skipped");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn file_round_trip() {
        let dir = std::env::temp_dir().join(format!("doom_cvar_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let r = CVarRegistry::with_engine_vars();
        assert!(r.load_file(&path).is_empty(), "a missing file is fine");
        r.set(VID_GAMMA, 2).unwrap();
        r.save_file(&path).unwrap();
        let r2 = CVarRegistry::with_engine_vars();
        assert!(r2.load_file(&path).is_empty());
        assert_eq!(r2.get(VID_GAMMA), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn registry_is_shareable() {
        fn check<T: Send + Sync + 'static>() {}
        check::<CVarRegistry>();
    }
}
