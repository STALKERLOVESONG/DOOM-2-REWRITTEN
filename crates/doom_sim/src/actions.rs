//! Action functions with arguments, from DECORATE and ZScript states
//! (`A_Jump(128, "See")`, `A_SpawnItemEx(...)`, `{ ... }` code blocks).
//!
//! [`run`] is called by `set_mobj_state` for states that carry an
//! [`ActionCall`]; it evaluates the arguments (ZScript expressions) against
//! the actor and runs the native implementation. A returned state is a
//! jump: the actor goes there at once, as in ZDoom.
//!
//! Code blocks and ZScript methods run in a small tree-walking interpreter
//! ([`Cx`]): a call to a name that is not a native action looks the name up
//! among the methods of the calling class and the actor's class (and their
//! ancestors) and runs that method's body with its parameters bound, so
//! mod-defined helpers (`action void A_MyFire()`, `int ShotDamage()`)
//! execute like natives and can return values.
//!
//! Weapon states run with a weapon context (the player and psprite), so the
//! weapon actions (`A_FireBullets`, `A_WeaponReady`, Doom's `A_FirePistol`,
//! ...) work from plain state calls, code blocks and methods alike, and
//! `invoker` refers to the weapon.
//!
//! Randomness uses the script RNG, never `P_Random`: mod actors can't
//! disturb vanilla demo sync.

use doom_core::{Angle, Fixed, FRACUNIT};
use doom_script::zscript::ast::{Arg, Block, Expr, Stmt};

use crate::hexen::{self, script_random, SpecialCtx};
use crate::ids::MobjId;
use crate::info::*;
use crate::mobj::flags::*;
use crate::world::{ScriptEvent, SimEvent, SoundSource, World};

/// A value while evaluating arguments.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Num(f64),
    Str(String),
    /// A state to jump to (`ResolveState`, labels).
    State(StateNum),
    Null,
}

impl Value {
    fn num(&self) -> f64 {
        match self {
            Value::Num(n) => *n,
            Value::Str(s) => s.parse().unwrap_or(0.0),
            Value::State(_) | Value::Null => 0.0,
        }
    }
    fn int(&self) -> i32 {
        self.num() as i32
    }
    fn truthy(&self) -> bool {
        match self {
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::State(_) => true,
            Value::Null => false,
        }
    }
    fn text(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Num(n) => n.to_string(),
            _ => String::new(),
        }
    }
}

/// Native actions implemented here (lower case).
const SUPPORTED: &[&str] = &[
    "a_jump", "a_jumpif", "a_jumpifinventory", "a_jumpifcloser", "a_jumpifhealthlower", "a_jumpiftargetinlos", "a_jumpifintargetlos",
    "a_checksight", "a_checkfloor", "a_checkceiling", "a_spawnitem", "a_spawnitemex", "a_custommissile", "a_spawnprojectile",
    "a_custombulletattack", "a_custommeleeattack", "a_meleeattack", "a_explode", "a_playsound", "a_startsound", "a_playsoundex",
    "a_stopsound", "a_activesound", "a_giveinventory", "a_takeinventory", "a_givetotarget", "a_takefromtarget", "a_settics",
    "a_fadeout", "a_fadein", "a_fadeto", "a_settranslucent", "a_setscale", "a_noblocking", "a_screamandunblock", "a_stop",
    "a_recoil", "a_changevelocity", "a_wander", "a_lookex", "a_look", "a_chase", "a_facetarget", "a_cleartarget", "a_setangle",
    "a_setuservar", "a_setuserarray", "a_setarg", "a_setspecial", "a_setshootable", "a_unsetshootable", "a_setsolid", "a_unsetsolid",
    "a_nogravity", "a_gravity", "a_lowgravity", "a_setfloat", "a_unsetfloat", "a_sethealth", "a_log", "a_print", "a_printbold",
    "a_callspecial", "acs_namedexecute", "acs_namedexecutealways", "acs_namedexecutewithresult", "acs_execute", "acs_executealways",
    "callacs", "a_alertmonsters", "a_die", "a_remove", "a_scream", "a_pain", "a_xscream", "a_fall", "a_bossdeath", "a_facetracer",
    "a_setinvulnerable", "a_unsetinvulnerable", "a_setreflective", "a_unsetreflective", "a_settranslation", "a_setrenderstyle",
    "a_setpitch", "a_setroll", "a_quake", "a_quakeex", "a_spawndebris", "a_dropinventory", "a_kill", "a_killchildren", "a_killmaster",
    "a_raiseself", "resolvestate", "a_jumpifmasterclose", "a_checkflag", "a_setmass", "a_setgravity", "a_setspeed",
    // Weapon actions (crate::weapons).
    "a_firebullets", "a_firecustommissile", "a_fireprojectile", "a_custompunch", "a_gunflash", "a_jumpifnoammo", "a_checkreload",
    "a_clearrefire", "a_refire", "a_light", "a_weaponready", "a_lower", "a_raise", "a_light0", "a_light1", "a_light2",
];

pub fn is_supported(name: &str) -> bool {
    SUPPORTED.contains(&name)
}

/// What a call did: nothing, a jump, or it removed the actor.
enum Outcome {
    Done,
    Jump(StateNum),
}

/// Run the call attached to `state` for actor `me`. Returns a state to
/// jump to.
pub fn run(w: &mut World, me: MobjId, state: StateNum, call: &ActionCall) -> Option<StateNum> {
    let mut cx = Cx::new(me, state, call.owner, None);
    let out = match &call.block {
        Some(b) => cx.block(w, b),
        None => cx.call(w, &call.name, &call.args),
    };
    match out {
        Some(Outcome::Jump(s)) => Some(s),
        _ => None,
    }
}

/// Run a call from a weapon (psprite) state for the player's body `me`:
/// labels resolve in the weapon's class only.
pub fn run_for_weapon(w: &mut World, me: MobjId, state: StateNum, call: &ActionCall) -> Option<StateNum> {
    let player = w.mo(me).player.unwrap_or(0);
    run_weapon(w, player, crate::pspr::PS_WEAPON, state, call)
}

/// Run the call of a weapon (psprite `psp`) state of `player`. Returns a
/// psprite state to jump to.
pub fn run_weapon(w: &mut World, player: usize, psp: usize, state: StateNum, call: &ActionCall) -> Option<StateNum> {
    let me = w.players[player].mo?;
    let mut cx = Cx::new(me, state, call.owner, Some((player, psp)));
    let out = match &call.block {
        Some(b) => cx.block(w, b),
        None => cx.call(w, &call.name, &call.args),
    };
    match out {
        Some(Outcome::Jump(s)) => Some(s),
        _ => None,
    }
}

/// Call a ZScript method of `class` (or its ancestors) for actor `me`,
/// with the weapon context of `me`'s player if it is one.
pub fn call_method_on_class(w: &mut World, me: MobjId, class: MobjType, name: &str, args: &[f64]) -> Option<f64> {
    let args: Vec<Arg> = args.iter().map(|&v| Arg { name: None, value: Expr::Float(v) }).collect();
    let player = w.mo(me).player;
    let mut cx = Cx::new(me, S_NULL, class, player.map(|p| (p, crate::pspr::PS_WEAPON)));
    let (_, v) = cx.call_method(w, &name.to_ascii_lowercase(), &args)?;
    Some(v.num())
}

/// Call a ZScript method of `me`'s class by name with numeric arguments
/// (for the engine and tests). Returns its value as a number, or `None` if
/// the class has no such method.
pub fn call_method(w: &mut World, me: MobjId, name: &str, args: &[f64]) -> Option<f64> {
    let class = w.mo(me).mobj_type;
    let args: Vec<Arg> = args.iter().map(|&v| Arg { name: None, value: Expr::Float(v) }).collect();
    let mut cx = Cx::new(me, S_NULL, class, None);
    let (_, v) = cx.call_method(w, &name.to_ascii_lowercase(), &args)?;
    Some(v.num())
}

/// Evaluate argument `i` (or the named one) of a call for actor `me`:
/// (as a number, as text).
pub fn eval_arg(w: &mut World, me: MobjId, state: StateNum, call: &ActionCall, args: &[Arg], i: usize, name: &str) -> Option<(f64, String)> {
    let player = w.mo(me).player;
    let mut cx = Cx::new(me, state, call.owner, player.map(|p| (p, crate::pspr::PS_WEAPON)));
    cx.arg(w, args, i, name).map(|v| (v.num(), v.text()))
}

/// Deepest nesting of method calls (runaway recursion stops here).
const MAX_CALL_DEPTH: u32 = 48;

/// Evaluation context: the actor, the calling state (for relative jumps)
/// and the class whose labels names refer to.
struct Cx {
    me: MobjId,
    state: StateNum,
    owner: MobjType,
    /// Locals of a code block or method (parameters first).
    locals: Vec<(String, Value)>,
    /// A weapon state: labels resolve in the weapon class only.
    weapon: bool,
    /// The weapon context: (player, psprite).
    wpn: Option<(usize, usize)>,
    /// Value of the last `return` in this frame.
    ret: Value,
    /// Value returned by the last method this frame called.
    last_ret: Option<Value>,
    depth: u32,
}

fn deg(a: Angle) -> f64 {
    a.0 as f64 * 360.0 / 4294967296.0
}

fn bam(d: f64) -> Angle {
    Angle((d.rem_euclid(360.0) / 360.0 * 4294967296.0) as u64 as u32)
}

fn fx(v: f64) -> Fixed {
    Fixed((v * FRACUNIT as f64) as i32)
}

fn unfx(f: Fixed) -> f64 {
    f.0 as f64 / FRACUNIT as f64
}

/// `random(a, b)` from the script RNG (inclusive).
fn rand_range(w: &mut World, a: i64, b: i64) -> i64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let r = ((script_random(w) as u64) << 16 | (script_random(w) as u64) << 8 | script_random(w) as u64) as i64;
    lo + r % (hi - lo + 1).max(1)
}

fn frand(w: &mut World, a: f64, b: f64) -> f64 {
    let r = (rand_range(w, 0, 0xffffff) as f64) / 16777215.0;
    a + (b - a) * r
}

/// ZDoom flag constants used in DECORATE arguments.
fn constant(name: &str) -> Option<f64> {
    let v: i64 = match name {
        // A_FireBullets / A_CustomPunch / A_WeaponReady
        "fbf_useammo" => 1,
        "fbf_norandom" => 2,
        "fbf_explicitangle" => 4,
        "fbf_nopitch" => 8,
        "fbf_noflash" => 16,
        "fbf_norandompuffz" => 32,
        "cpf_useammo" => 1,
        "wrf_nobob" => 1,
        "wrf_noswitch" => 2,
        "wrf_noprimary" => 4,
        "wrf_nosecondary" => 8,
        "wrf_nofire" => 12,
        // A_SpawnItemEx
        "sxf_transfertranslation" => 1,
        "sxf_absoluteposition" => 2,
        "sxf_absoluteangle" => 4,
        "sxf_absolutemomentum" | "sxf_absolutevelocity" => 8,
        "sxf_setmaster" => 16,
        "sxf_nocheckposition" => 32,
        "sxf_telefrag" => 64,
        "sxf_clientside" => 128,
        "sxf_transferambushflag" => 256,
        "sxf_transferpitch" => 512,
        "sxf_transferpointers" => 1024,
        "sxf_useblooodcolor" | "sxf_usebloodcolor" => 2048,
        "sxf_clearcallertid" => 4096,
        "sxf_multiplyspeed" => 8192,
        "sxf_transferscale" => 16384,
        "sxf_transferspecial" => 32768,
        "sxf_clearcallerspecial" => 65536,
        "sxf_transferstencilcol" => 1 << 17,
        "sxf_transferalpha" => 1 << 18,
        "sxf_transferrenderstyle" => 1 << 19,
        "sxf_settarget" => 1 << 20,
        "sxf_settracer" => 1 << 21,
        "sxf_nopointers" => 1 << 22,
        "sxf_originator" => 1 << 23,
        "sxf_transferspriteframe" => 1 << 24,
        "sxf_transferroll" => 1 << 25,
        "sxf_istarget" => 1 << 26,
        "sxf_ismaster" => 1 << 27,
        "sxf_istracer" => 1 << 28,
        // A_CustomMissile / A_SpawnProjectile
        "cmf_aimoffset" => 1,
        "cmf_aimdirection" => 2,
        "cmf_trackowner" => 4,
        "cmf_checktargetdead" => 8,
        "cmf_absolutepitch" => 16,
        "cmf_offsetpitch" => 32,
        "cmf_savepitch" => 64,
        "cmf_absoluteangle" => 128,
        // A_CustomBulletAttack
        "cbaf_aimfacing" => 1,
        "cbaf_norandom" => 2,
        "cbaf_explicitangle" => 4,
        "cbaf_nopitch" => 8,
        "cbaf_norandompuffz" => 16,
        // A_ChangeVelocity
        "cvf_relative" => 1,
        "cvf_replace" => 2,
        // A_FadeOut / A_FadeTo
        "ftf_remove" => 1,
        "ftf_clamp" => 2,
        // Sound channels / attenuation
        "chan_auto" => 0,
        "chan_weapon" => 1,
        "chan_voice" => 2,
        "chan_item" => 3,
        "chan_body" => 4,
        "chan_5" => 5,
        "chan_6" => 6,
        "chan_7" => 7,
        "chan_listenerz" => 8,
        "chan_maybe_local" => 16,
        "chan_ui" => 32,
        "chan_nopause" => 64,
        "chan_loop" => 256,
        "attn_none" => 0,
        "attn_norm" | "attn_idle" => 1,
        "attn_static" => 3,
        // Pointers
        "aaptr_default" => 0,
        "aaptr_null" => 1,
        "aaptr_target" => 2,
        "aaptr_master" => 4,
        "aaptr_tracer" => 8,
        "aaptr_player_gettarget" => 16,
        // A_Explode
        "xf_hurtsource" => 1,
        "xf_notmissile" => 4,
        // Misc
        "true" => 1,
        "false" => 0,
        _ => return None,
    };
    Some(v as f64)
}

impl Cx {
    fn new(me: MobjId, state: StateNum, owner: MobjType, wpn: Option<(usize, usize)>) -> Cx {
        Cx { me, state, owner, locals: Vec::new(), weapon: wpn.is_some(), wpn, ret: Value::Null, last_ret: None, depth: 0 }
    }

    // ---- methods ---------------------------------------------------------------------

    /// Run a ZScript method by (lower-case) name, looked up in the calling
    /// class, then the weapon in hand, then the actor's class. Returns its
    /// outcome (a state it returned) and value.
    fn call_method(&mut self, w: &mut World, name: &str, args: &[Arg]) -> Option<(Outcome, Value)> {
        let mut classes = vec![self.owner];
        if let Some((player, _)) = self.wpn {
            classes.extend(crate::weapons::active_class(w, player));
        }
        classes.push(w.mo(self.me).mobj_type);
        let (class, f) = classes.into_iter().find_map(|c| w.info.find_method(c, name))?;
        let body = f.body.as_ref()?;
        if self.depth >= MAX_CALL_DEPTH {
            w.note_unimplemented("ZScript method recursion too deep");
            return Some((Outcome::Done, Value::Null));
        }
        // Bind parameters: named, then positional, then defaults.
        let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
        let mut locals = Vec::with_capacity(f.params.len());
        for (i, p) in f.params.iter().enumerate() {
            let given = args.iter().find(|a| a.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&p.name))).or_else(|| positional.get(i).copied());
            let v = match (given, &p.default) {
                (Some(a), _) => self.eval(w, &a.value),
                (None, Some(d)) => self.eval(w, d),
                (None, None) => Value::Num(0.0),
            };
            locals.push((p.name.clone(), v));
        }
        // Weapon methods keep the weapon context; labels resolve in the
        // class that defines the method.
        let mut callee = Cx { me: self.me, state: self.state, owner: class, locals, weapon: self.weapon, wpn: self.wpn, ret: Value::Null, last_ret: None, depth: self.depth + 1 };
        let out = callee.block(w, body).unwrap_or(Outcome::Done);
        let out = match (&out, &callee.ret) {
            (Outcome::Done, Value::State(s)) => Outcome::Jump(*s),
            _ => out,
        };
        Some((out, callee.ret))
    }

    // ---- invoker (the weapon) ------------------------------------------------------

    /// The weapon class `invoker` means here.
    fn invoker_class(&self, w: &World) -> Option<MobjType> {
        let (player, _) = self.wpn?;
        crate::weapons::active_class(w, player).or(Some(self.owner))
    }

    fn weapon_var_key(&self, w: &World, field: &str) -> Option<(usize, String)> {
        let (player, _) = self.wpn?;
        let class = self.invoker_class(w)?;
        Some((player, format!("{}.{}", w.info.class_name(class), field.to_ascii_lowercase())))
    }

    /// `invoker.<path>`: weapon properties, its ammo and its fields.
    fn invoker_get(&mut self, w: &World, path: &[&str]) -> Value {
        let Some((player, _)) = self.wpn else { return Value::Null };
        let def = self.invoker_class(w).and_then(|c| crate::weapons::def(w, c)).cloned().unwrap_or_default();
        let count = |a: &Option<crate::weapons::AmmoRef>| -> f64 {
            let p = &w.players[player];
            match a {
                Some(crate::weapons::AmmoRef::Vanilla(i)) => p.ammo[*i] as f64,
                Some(crate::weapons::AmmoRef::Named(n)) => p.inventory.get(n).copied().unwrap_or(0) as f64,
                None => 0.0,
            }
        };
        let lower: Vec<String> = path.iter().map(|s| s.to_ascii_lowercase()).collect();
        let parts: Vec<&str> = lower.iter().map(String::as_str).collect();
        Value::Num(match parts.as_slice() {
            ["ammo1", "amount"] | ["ammo1"] => count(&def.ammo),
            ["ammo2", "amount"] | ["ammo2"] => count(&def.ammo2),
            ["ammouse1"] | ["ammouse"] => def.ammo_use as f64,
            ["ammouse2"] => def.ammo_use2 as f64,
            ["ammogive1"] | ["ammogive"] => def.ammo_give as f64,
            ["ammogive2"] => def.ammo_give2 as f64,
            ["slotnumber"] => def.slot as f64,
            ["baltfire"] => (w.players[player].fire_mode == crate::weapons::FireMode::Alt) as i32 as f64,
            [field] => {
                let Some((p, key)) = self.weapon_var_key(w, field) else { return Value::Null };
                w.players[p].weapon_vars.get(&key).copied().unwrap_or(0.0)
            }
            _ => return Value::Null,
        })
    }

    fn invoker_set(&mut self, w: &mut World, path: &[&str], v: f64) {
        let Some((player, _)) = self.wpn else { return };
        let lower: Vec<String> = path.iter().map(|s| s.to_ascii_lowercase()).collect();
        let parts: Vec<&str> = lower.iter().map(String::as_str).collect();
        let def = self.invoker_class(w).and_then(|c| crate::weapons::def(w, c)).cloned().unwrap_or_default();
        let ammo = match parts.as_slice() {
            ["ammo1", "amount"] => Some(def.ammo),
            ["ammo2", "amount"] => Some(def.ammo2),
            _ => None,
        };
        if let Some(a) = ammo {
            let p = &mut w.players[player];
            match a {
                Some(crate::weapons::AmmoRef::Vanilla(i)) => p.ammo[i] = (v as i32).clamp(0, p.maxammo[i]),
                Some(crate::weapons::AmmoRef::Named(n)) => {
                    p.inventory.insert(n, (v as i32).max(0));
                }
                None => {}
            }
            return;
        }
        if let [field] = parts.as_slice() {
            if let Some((p, key)) = self.weapon_var_key(w, field) {
                w.players[p].weapon_vars.insert(key, v);
            }
        }
    }

    /// A field declared by the weapon class (bare names in weapon code).
    fn is_weapon_field(&self, w: &World, name: &str) -> bool {
        let Some(class) = self.invoker_class(w) else { return false };
        let n = name.to_ascii_lowercase();
        w.info.class_vars.get(class.idx()).is_some_and(|v| v.contains(&n)) && !self.locals.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    fn mo<'w>(&self, w: &'w World) -> &'w crate::mobj::Mobj {
        w.mo(self.me)
    }

    fn alive(&self, w: &World) -> bool {
        w.try_mo(Some(self.me)).is_some() && w.thinkers.func(self.me) == Some(crate::thinker::ThinkFn::Active)
    }

    fn arg(&mut self, w: &mut World, args: &[Arg], i: usize, name: &str) -> Option<Value> {
        // Named arguments first, then position.
        let a = args.iter().find(|a| a.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))).or_else(|| {
            let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
            positional.get(i).copied()
        })?;
        Some(self.eval(w, &a.value))
    }

    fn num(&mut self, w: &mut World, args: &[Arg], i: usize, name: &str, default: f64) -> f64 {
        self.arg(w, args, i, name).map_or(default, |v| v.num())
    }

    fn text(&mut self, w: &mut World, args: &[Arg], i: usize, name: &str) -> String {
        self.arg(w, args, i, name).map(|v| v.text()).unwrap_or_default()
    }

    /// A state argument: a label (relative to the defining class), a
    /// number of states ahead, or a state value.
    fn state_arg(&mut self, w: &mut World, v: Value) -> Option<StateNum> {
        match v {
            Value::State(s) => Some(s),
            Value::Str(s) => self.label(w, &s),
            Value::Num(n) if n != 0.0 => Some(StateNum((self.state.0 as i32 + n as i32) as u16)),
            _ => None,
        }
    }

    fn label(&self, w: &World, label: &str) -> Option<StateNum> {
        if label.eq_ignore_ascii_case("null") {
            return Some(S_NULL);
        }
        // `Class::Label`
        if let Some((class, l)) = label.split_once("::") {
            if !class.eq_ignore_ascii_case("super") {
                return w.info.type_by_name(class).and_then(|t| w.info.find_label(t, l));
            }
        }
        // Labels resolve in the class that defines the state, then in the
        // actor's own class (an inherited state jumping to an overridden
        // label goes to the override, as in ZDoom's runtime lookup).
        let actor = w.mo(self.me).mobj_type;
        if !label.contains("::") && !self.weapon {
            if let Some(s) = w.info.find_label(actor, label) {
                return Some(s);
            }
        }
        w.info.find_label(self.owner, label)
    }

    fn target(&self, w: &World) -> Option<MobjId> {
        let t = self.mo(w).target?;
        w.try_mo(Some(t)).map(|_| t)
    }

    fn user_var(&self, w: &World, name: &str) -> Option<f64> {
        let n = name.to_ascii_lowercase();
        self.mo(w).user_vars.iter().find(|(k, _)| *k == n).map(|(_, v)| *v)
    }

    fn set_user_var(&self, w: &mut World, name: &str, v: f64) {
        let n = name.to_ascii_lowercase();
        let m = w.mo_mut(self.me);
        match m.user_vars.iter_mut().find(|(k, _)| *k == n) {
            Some(slot) => slot.1 = v,
            None => m.user_vars.push((n, v)),
        }
    }

    // ---- expressions ---------------------------------------------------------------

    fn eval(&mut self, w: &mut World, e: &Expr) -> Value {
        match e {
            Expr::Int(i) => Value::Num(*i as f64),
            Expr::Float(f) => Value::Num(*f),
            Expr::Bool(b) => Value::Num(*b as i32 as f64),
            Expr::Str(s) | Expr::Name(s) => Value::Str(s.clone()),
            Expr::Null => Value::Null,
            Expr::Ident(name) => self.ident(w, name),
            Expr::Member(obj, field) => {
                // `pos.x`, `vel.z`, `self.health`, `target.health`.
                if let Some(path) = member_path(e) {
                    if path[0].eq_ignore_ascii_case("invoker") && self.wpn.is_some() {
                        return self.invoker_get(w, &path[1..]);
                    }
                    if path[0].eq_ignore_ascii_case("player") && path.len() == 2 && path[1].eq_ignore_ascii_case("refire") {
                        return Value::Num(self.wpn.map_or(0, |(p, _)| w.players[p].refire) as f64);
                    }
                }
                let base = match obj.as_ref() {
                    Expr::SelfRef => Some(self.me),
                    Expr::Ident(n) if n.eq_ignore_ascii_case("owner") && self.wpn.is_some() => Some(self.me),
                    Expr::Ident(n) if n.eq_ignore_ascii_case("target") => self.target(w),
                    Expr::Ident(n) if n.eq_ignore_ascii_case("pos") || n.eq_ignore_ascii_case("vel") => {
                        return self.ident(w, &format!("{n}{field}"));
                    }
                    _ => None,
                };
                match base {
                    Some(m) if m == self.me => self.ident(w, field),
                    Some(m) => {
                        let mut other = Cx::new(m, self.state, self.owner, None);
                        other.ident(w, field)
                    }
                    None => Value::Null,
                }
            }
            Expr::Unary(op @ ("++" | "--"), target) => {
                let Expr::Ident(name) = target.as_ref() else { return Value::Null };
                let old = self.ident(w, name).num();
                let new = if *op == "++" { old + 1.0 } else { old - 1.0 };
                self.assign(w, name, Value::Num(new));
                Value::Num(new)
            }
            Expr::Unary(op, x) => {
                let v = self.eval(w, x);
                match *op {
                    "-" => Value::Num(-v.num()),
                    "!" => Value::Num(!v.truthy() as i32 as f64),
                    "~" => Value::Num(!(v.num() as i64) as f64),
                    _ => v,
                }
            }
            Expr::Binary(op, a, b) => {
                if *op == "&&" {
                    let l = self.eval(w, a).truthy();
                    return Value::Num((l && self.eval(w, b).truthy()) as i32 as f64);
                }
                if *op == "||" {
                    let l = self.eval(w, a).truthy();
                    return Value::Num((l || self.eval(w, b).truthy()) as i32 as f64);
                }
                let (l, r) = (self.eval(w, a), self.eval(w, b));
                if *op == ".." {
                    return Value::Str(l.text() + &r.text());
                }
                if let (Value::Str(x), Value::Str(y)) = (&l, &r) {
                    return match *op {
                        "==" => Value::Num((x == y) as i32 as f64),
                        "!=" => Value::Num((x != y) as i32 as f64),
                        "~==" => Value::Num(x.eq_ignore_ascii_case(y) as i32 as f64),
                        _ => Value::Null,
                    };
                }
                let (x, y) = (l.num(), r.num());
                let (xi, yi) = (x as i64, y as i64);
                Value::Num(match *op {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    "/" => {
                        if y == 0.0 {
                            0.0
                        } else {
                            x / y
                        }
                    }
                    "%" => {
                        if yi == 0 {
                            0.0
                        } else {
                            (xi % yi) as f64
                        }
                    }
                    "**" => x.powf(y),
                    "==" => (x == y) as i32 as f64,
                    "!=" => (x != y) as i32 as f64,
                    "<" => (x < y) as i32 as f64,
                    ">" => (x > y) as i32 as f64,
                    "<=" => (x <= y) as i32 as f64,
                    ">=" => (x >= y) as i32 as f64,
                    "&" => (xi & yi) as f64,
                    "|" => (xi | yi) as f64,
                    "^" => (xi ^ yi) as f64,
                    "<<" => ((xi as i32).wrapping_shl(yi as u32)) as f64,
                    ">>" | ">>>" => ((xi as i32).wrapping_shr(yi as u32)) as f64,
                    _ => 0.0,
                })
            }
            Expr::Ternary(c, a, b) => {
                if self.eval(w, c).truthy() {
                    self.eval(w, a)
                } else {
                    self.eval(w, b)
                }
            }
            Expr::Call(f, args) => self.call_expr(w, f, args),
            Expr::Assign(op, target, value) => {
                let v = self.eval(w, value);
                // `invoker.shots += 1`, `invoker.ammo2.amount = 0`.
                if let Some(path) = member_path(target) {
                    if path[0].eq_ignore_ascii_case("invoker") && self.wpn.is_some() {
                        let old = self.invoker_get(w, &path[1..]).num();
                        let new = apply_op(op, old, v.num());
                        self.invoker_set(w, &path[1..], new);
                        return Value::Num(new);
                    }
                }
                let name = match target.as_ref() {
                    Expr::Ident(n) => n.clone(),
                    Expr::Member(_, n) => n.clone(),
                    _ => return v,
                };
                // Strings and states assign as they are.
                if *op == "=" && !matches!(v, Value::Num(_)) && self.locals.iter().any(|(k, _)| k.eq_ignore_ascii_case(&name)) {
                    self.assign(w, &name, v.clone());
                    return v;
                }
                let old = self.ident(w, &name).num();
                let new = apply_op(op, old, v.num());
                self.assign(w, &name, Value::Num(new));
                Value::Num(new)
            }
            Expr::Postfix(op, target) => {
                let Expr::Ident(name) = target.as_ref() else { return Value::Null };
                let old = self.ident(w, name).num();
                self.assign(w, name, Value::Num(if *op == "++" { old + 1.0 } else { old - 1.0 }));
                Value::Num(old)
            }
            _ => Value::Null,
        }
    }

    fn assign(&mut self, w: &mut World, name: &str, v: Value) {
        if let Some(slot) = self.locals.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            slot.1 = v;
            return;
        }
        if self.is_weapon_field(w, name) {
            self.invoker_set(w, &[name], v.num());
            return;
        }
        let n = name.to_ascii_lowercase();
        let m = w.mo_mut(self.me);
        match n.as_str() {
            "health" => m.health = v.int(),
            "angle" => m.angle = bam(v.num()),
            "alpha" => m.fade = (1.0 - v.num() as f32).clamp(0.0, 1.0),
            "special" => m.special = v.int(),
            "tid" => m.tid = v.int(),
            "threshold" => m.threshold = v.int(),
            "reactiontime" => m.reactiontime = v.int(),
            "tics" => m.tics = v.int(),
            _ => self.set_user_var(w, &n, v.num()),
        }
    }

    fn ident(&mut self, w: &mut World, name: &str) -> Value {
        if let Some((_, v)) = self.locals.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            return v.clone();
        }
        if self.is_weapon_field(w, name) {
            return self.invoker_get(w, &[name]);
        }
        let n = name.to_ascii_lowercase();
        let m = self.mo(w);
        let v = match n.as_str() {
            "health" => m.health as f64,
            "angle" => deg(m.angle),
            "pitch" | "roll" => 0.0,
            "x" | "posx" => unfx(m.x),
            "y" | "posy" => unfx(m.y),
            "z" | "posz" => unfx(m.z),
            "velx" | "momx" => unfx(m.momx),
            "vely" | "momy" => unfx(m.momy),
            "velz" | "momz" => unfx(m.momz),
            "floorz" => unfx(m.floorz),
            "ceilingz" => unfx(m.ceilingz),
            "radius" => unfx(m.radius),
            "height" => unfx(m.height),
            "alpha" => (1.0 - m.fade) as f64,
            "special" => m.special as f64,
            "tid" => m.tid as f64,
            "threshold" => m.threshold as f64,
            "reactiontime" => m.reactiontime as f64,
            "tics" => m.tics as f64,
            "waterlevel" => 0.0,
            "gametic" | "level.time" | "leveltime" => w.leveltime as f64,
            "self" => return Value::Num(1.0),
            "target" => return if self.target(w).is_some() { Value::Num(1.0) } else { Value::Null },
            _ => {
                if let Some(v) = self.user_var(w, &n) {
                    v
                } else if let Some(c) = constant(&n) {
                    c
                } else {
                    // Labels used bare (`A_Jump(128, See)` is a string in
                    // practice; keep unknown names as text).
                    return Value::Str(name.to_string());
                }
            }
        };
        Value::Num(v)
    }

    fn call_expr(&mut self, w: &mut World, f: &Expr, args: &[Arg]) -> Value {
        let name = match f {
            Expr::Ident(n) => n.to_ascii_lowercase(),
            // random[name](a, b)
            Expr::Index(inner, _) => match inner.as_ref() {
                Expr::Ident(n) => n.to_ascii_lowercase(),
                _ => return Value::Null,
            },
            Expr::Member(_, n) => n.to_ascii_lowercase(),
            _ => return Value::Null,
        };
        let nums = |cx: &mut Cx, w: &mut World| -> Vec<f64> { args.iter().map(|a| cx.eval(w, &a.value).num()).collect() };
        match name.as_str() {
            "random" => {
                let v = nums(self, w);
                let (a, b) = (v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(255.0));
                Value::Num(rand_range(w, a as i64, b as i64) as f64)
            }
            "frandom" => {
                let v = nums(self, w);
                Value::Num(frand(w, v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(1.0)))
            }
            "randompick" | "frandompick" => {
                let v = nums(self, w);
                if v.is_empty() {
                    return Value::Num(0.0);
                }
                let i = rand_range(w, 0, v.len() as i64 - 1) as usize;
                Value::Num(v[i])
            }
            "abs" => Value::Num(nums(self, w).first().copied().unwrap_or(0.0).abs()),
            "sqrt" => Value::Num(nums(self, w).first().copied().unwrap_or(0.0).max(0.0).sqrt()),
            "min" => Value::Num(nums(self, w).into_iter().fold(f64::INFINITY, f64::min)),
            "max" => Value::Num(nums(self, w).into_iter().fold(f64::NEG_INFINITY, f64::max)),
            "clamp" => {
                let v = nums(self, w);
                Value::Num(v.first().copied().unwrap_or(0.0).clamp(v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)))
            }
            "sin" => Value::Num(nums(self, w).first().copied().unwrap_or(0.0).to_radians().sin()),
            "cos" => Value::Num(nums(self, w).first().copied().unwrap_or(0.0).to_radians().cos()),
            "int" | "double" | "float" => Value::Num(nums(self, w).first().copied().unwrap_or(0.0)),
            "countinv" | "checkinventory" => {
                let item = args.first().map(|a| self.eval(w, &a.value).text()).unwrap_or_default();
                Value::Num(crate::inventory::count(w, self.me, &item) as f64)
            }
            "getcvar" | "getcvarstring" => Value::Num(0.0),
            "getdistance" => match self.target(w) {
                Some(t) => {
                    let (a, b) = (w.mo(self.me), w.mo(t));
                    Value::Num(unfx(a.x - b.x).hypot(unfx(a.y - b.y)))
                }
                None => Value::Num(0.0),
            },
            "resolvestate" | "state" => match args.first().map(|a| self.eval(w, &a.value)) {
                Some(v) => self.state_arg(w, v).map_or(Value::Null, Value::State),
                None => Value::Null,
            },
            "getclassname" | "getclass" => Value::Str(w.info.class_name(w.mo(self.me).mobj_type).to_string()),
            "checkclass" => {
                let c = args.first().map(|a| self.eval(w, &a.value).text()).unwrap_or_default();
                let mine = w.mo(self.me).mobj_type;
                Value::Num(w.info.type_by_name(&c).is_some_and(|t| w.info.is_a(mine, t)) as i32 as f64)
            }
            // An action or method used in an expression: a method's
            // return value, else success.
            _ => {
                self.last_ret = None;
                match self.call(w, &name, args) {
                    Some(Outcome::Jump(s)) => Value::State(s),
                    Some(Outcome::Done) => match self.last_ret.take() {
                        Some(Value::Null) | None => Value::Num(1.0),
                        Some(v) => v,
                    },
                    None => Value::Num(0.0),
                }
            }
        }
    }

    // ---- code blocks -------------------------------------------------------------------

    fn block(&mut self, w: &mut World, b: &Block) -> Option<Outcome> {
        for s in &b.0 {
            if !self.alive(w) {
                return None;
            }
            match self.stmt(w, s) {
                Flow::Next => {}
                Flow::Return(o) => return o,
                Flow::Break | Flow::Continue => break,
            }
        }
        Some(Outcome::Done)
    }

    fn stmt(&mut self, w: &mut World, s: &Stmt) -> Flow {
        match s {
            Stmt::Expr(e) => {
                if let Expr::Call(f, args) = e {
                    let name = match f.as_ref() {
                        Expr::Ident(n) => Some(n.clone()),
                        // `invoker.Fire()`, `self.A_Foo()`.
                        Expr::Member(obj, n) if matches!(obj.as_ref(), Expr::SelfRef) || matches!(obj.as_ref(), Expr::Ident(o) if o.eq_ignore_ascii_case("invoker")) => Some(n.clone()),
                        _ => None,
                    };
                    if let Some(n) = name {
                        // Statement calls to jumping actions jump too.
                        if let Some(Outcome::Jump(s)) = self.call(w, &n.to_ascii_lowercase(), args) {
                            return Flow::Return(Some(Outcome::Jump(s)));
                        }
                        return Flow::Next;
                    }
                }
                self.eval(w, e);
                Flow::Next
            }
            Stmt::Local { vars, .. } => {
                for v in vars {
                    let init = v.init.as_ref().map_or(Value::Num(0.0), |e| self.eval(w, e));
                    // A redeclared name (a loop body run again) is reset.
                    match self.locals.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&v.name)) {
                        Some(slot) => slot.1 = init,
                        None => self.locals.push((v.name.clone(), init)),
                    }
                }
                Flow::Next
            }
            Stmt::If { cond, then, otherwise } => {
                if self.eval(w, cond).truthy() {
                    self.stmt(w, then)
                } else if let Some(o) = otherwise {
                    self.stmt(w, o)
                } else {
                    Flow::Next
                }
            }
            Stmt::Block(b) => {
                for s in &b.0 {
                    match self.stmt(w, s) {
                        Flow::Next => {}
                        other => return other,
                    }
                }
                Flow::Next
            }
            Stmt::Return(values) => {
                let v = values.first().map(|e| self.eval(w, e));
                self.ret = v.clone().unwrap_or(Value::Null);
                Flow::Return(match v {
                    Some(Value::State(s)) => Some(Outcome::Jump(s)),
                    // A state label returned from state code jumps; a
                    // method returning text just returns it.
                    Some(Value::Str(l)) if self.depth == 0 => self.label(w, &l).map(Outcome::Jump).or(Some(Outcome::Done)),
                    _ => Some(Outcome::Done),
                })
            }
            Stmt::DoWhile { body, cond, until } => {
                for _ in 0..10_000 {
                    match self.stmt(w, body) {
                        Flow::Break => break,
                        Flow::Return(o) => return Flow::Return(o),
                        _ => {}
                    }
                    if self.eval(w, cond).truthy() == *until {
                        break;
                    }
                }
                Flow::Next
            }
            Stmt::Until { cond, body } => {
                for _ in 0..10_000 {
                    if self.eval(w, cond).truthy() {
                        break;
                    }
                    match self.stmt(w, body) {
                        Flow::Break => break,
                        Flow::Return(o) => return Flow::Return(o),
                        _ => {}
                    }
                }
                Flow::Next
            }
            Stmt::Switch { value, body } => {
                let v = self.eval(w, value);
                // Find the matching case (or default), then fall through
                // until `break`.
                let mut start = None;
                for (i, item) in body.iter().enumerate() {
                    if let doom_script::zscript::ast::SwitchItem::Case(c) = item {
                        let cv = self.eval(w, c);
                        let same = match (&v, &cv) {
                            (Value::Str(a), Value::Str(b)) => a.eq_ignore_ascii_case(b),
                            _ => v.num() == cv.num(),
                        };
                        if same {
                            start = Some(i);
                            break;
                        }
                    }
                }
                let start = start.or_else(|| body.iter().position(|i| matches!(i, doom_script::zscript::ast::SwitchItem::Default)));
                if let Some(s) = start {
                    for item in &body[s..] {
                        if let doom_script::zscript::ast::SwitchItem::Stmt(st) = item {
                            match self.stmt(w, st) {
                                Flow::Next | Flow::Continue => {}
                                Flow::Break => break,
                                Flow::Return(o) => return Flow::Return(o),
                            }
                        }
                    }
                }
                Flow::Next
            }
            Stmt::While { cond, body } => {
                for _ in 0..10_000 {
                    if !self.eval(w, cond).truthy() {
                        break;
                    }
                    match self.stmt(w, body) {
                        Flow::Break => break,
                        Flow::Return(o) => return Flow::Return(o),
                        _ => {}
                    }
                }
                Flow::Next
            }
            Stmt::For { init, cond, step, body } => {
                for s in init {
                    self.stmt(w, s);
                }
                for _ in 0..10_000 {
                    if let Some(c) = cond {
                        if !self.eval(w, c).truthy() {
                            break;
                        }
                    }
                    match self.stmt(w, body) {
                        Flow::Break => break,
                        Flow::Return(o) => return Flow::Return(o),
                        _ => {}
                    }
                    for e in step {
                        self.eval(w, e);
                    }
                }
                Flow::Next
            }
            Stmt::Break => Flow::Break,
            Stmt::Continue => Flow::Continue,
            Stmt::Empty => Flow::Next,
            _ => {
                w.note_unimplemented("ZScript statements in state code (switch, do, multi-assign, ...)");
                Flow::Next
            }
        }
    }

    // ---- actions -------------------------------------------------------------------------

    /// Run an action. `None`: unknown action (counted) or no effect.
    fn call(&mut self, w: &mut World, name: &str, args: &[Arg]) -> Option<Outcome> {
        // Vanilla actions called with arguments (or from code blocks):
        // weapon ones on the psprite, others on the actor.
        if let Some(a) = Action::ALL.iter().copied().find(|a| a.name().eq_ignore_ascii_case(name)) {
            if a != Action::None {
                match self.wpn {
                    Some((player, psp)) if crate::pspr::is_psprite_action(a) => {
                        if a == Action::WeaponReady {
                            w.players[player].ready_flags = self.num(w, args, 0, "flags", 0.0) as i32;
                        }
                        crate::pspr::call_psprite_action(w, player, psp, a);
                    }
                    _ => crate::enemy::call_mobj_action(w, self.me, a),
                }
                return Some(Outcome::Done);
            }
        }
        if let Some((player, psp)) = self.wpn {
            if let Some(out) = self.weapon_call(w, player, psp, name, args) {
                return out;
            }
        }
        let me = self.me;
        match name {
            // ---- flow ----
            "a_jump" => {
                let chance = self.num(w, args, 0, "chance", 0.0) as i32;
                let targets: Vec<Value> = args.iter().filter(|a| a.name.is_none()).skip(1).map(|a| self.eval(w, &a.value)).collect();
                if targets.is_empty() || (chance < 256 && script_random(w) >= chance) {
                    return Some(Outcome::Done);
                }
                let pick = targets[if targets.len() > 1 { rand_range(w, 0, targets.len() as i64 - 1) as usize } else { 0 }].clone();
                self.jump(w, pick)
            }
            "a_jumpif" => {
                let ok = self.arg(w, args, 0, "expression").is_some_and(|v| v.truthy());
                self.jump_if(w, ok, args, 1)
            }
            "a_jumpifinventory" => {
                let item = self.text(w, args, 0, "itemtype");
                let amount = self.num(w, args, 1, "itemamount", 0.0) as i32;
                let have = crate::inventory::count(w, me, &item);
                let ok = if amount <= 0 { have > 0 } else { have >= amount };
                self.jump_if(w, ok, args, 2)
            }
            "a_jumpifcloser" | "a_jumpifmasterclose" => {
                let dist = self.num(w, args, 0, "distance", 0.0);
                let ok = self.target(w).is_some_and(|t| {
                    let (a, b) = (w.mo(me), w.mo(t));
                    unfx(a.x - b.x).hypot(unfx(a.y - b.y)) < dist
                });
                self.jump_if(w, ok, args, 1)
            }
            "a_jumpifhealthlower" => {
                let h = self.num(w, args, 0, "health", 0.0) as i32;
                let ok = w.mo(me).health < h;
                self.jump_if(w, ok, args, 1)
            }
            "a_jumpiftargetinlos" | "a_jumpifintargetlos" => {
                let ok = match self.target(w) {
                    Some(t) => crate::sight::check_sight(w, me, t),
                    None => false,
                };
                self.jump_if(w, ok, args, 0)
            }
            "a_checksight" => {
                // Jump if no player can see the actor.
                let players: Vec<MobjId> = (0..crate::player::MAXPLAYERS).filter(|&p| w.playeringame[p]).filter_map(|p| w.players[p].mo).collect();
                let seen = players.into_iter().any(|p| crate::sight::check_sight(w, p, me));
                self.jump_if(w, !seen, args, 0)
            }
            "a_checkfloor" => {
                let ok = w.mo(me).z <= w.mo(me).floorz;
                self.jump_if(w, ok, args, 0)
            }
            "a_checkceiling" => {
                let m = w.mo(me);
                let ok = m.z + m.height >= m.ceilingz;
                self.jump_if(w, ok, args, 0)
            }
            "a_checkflag" => {
                let flag = self.text(w, args, 0, "flagname").to_ascii_lowercase();
                let ok = match flag.as_str() {
                    "shootable" => w.mo(me).flags & MF_SHOOTABLE != 0,
                    "solid" => w.mo(me).flags & MF_SOLID != 0,
                    "countkill" => w.mo(me).flags & MF_COUNTKILL != 0,
                    _ => false,
                };
                self.jump_if(w, ok, args, 1)
            }
            "resolvestate" => {
                let v = self.arg(w, args, 0, "label")?;
                self.jump(w, v)
            }
            // ---- spawning ----
            "a_spawnitem" => {
                let class = self.text(w, args, 0, "itemtype");
                let distance = self.num(w, args, 1, "distance", 0.0);
                let zheight = self.num(w, args, 2, "zheight", 0.0);
                let m = w.mo(me);
                let an = m.angle;
                let dist = distance + unfx(m.radius);
                let (x, y) = (unfx(m.x) + dist * an.to_radians_f32().cos() as f64, unfx(m.y) + dist * an.to_radians_f32().sin() as f64);
                let z = unfx(m.z) + zheight;
                self.spawn(w, &class, x, y, z, an, 0.0, 0.0, 0.0, 0, false).map(|_| Outcome::Done)
            }
            "a_spawnitemex" => {
                let class = self.text(w, args, 0, "missile");
                let class = if class.is_empty() { self.text(w, args, 0, "type") } else { class };
                let mut v = [0.0f64; 7];
                for (i, n) in ["xofs", "yofs", "zofs", "xvel", "yvel", "zvel", "angle"].iter().enumerate() {
                    v[i] = self.num(w, args, i + 1, n, 0.0);
                }
                let flags = self.num(w, args, 8, "flags", 0.0) as i64;
                let chance = self.num(w, args, 9, "failchance", 0.0) as i32;
                let tid = self.num(w, args, 10, "tid", 0.0) as i32;
                if chance > 0 && script_random(w) < chance {
                    return Some(Outcome::Done);
                }
                let m = w.mo(me);
                let base = if flags & 4 != 0 { 0.0 } else { deg(m.angle) };
                let a = (base + v[6]).to_radians();
                // Offsets relative to the actor's facing unless absolute.
                let (xo, yo) = if flags & 2 != 0 {
                    (v[0], v[1])
                } else {
                    let f = deg(m.angle).to_radians();
                    (v[0] * f.cos() - v[1] * f.sin(), v[0] * f.sin() + v[1] * f.cos())
                };
                let (xv, yv) = if flags & 8 != 0 { (v[3], v[4]) } else { (v[3] * a.cos() - v[4] * a.sin(), v[3] * a.sin() + v[4] * a.cos()) };
                let (x, y, z) = (unfx(m.x) + xo, unfx(m.y) + yo, unfx(m.z) + v[2]);
                let check = flags & 32 == 0;
                let spawned = self.spawn(w, &class, x, y, z, bam(a.to_degrees()), xv, yv, v[5], tid, check)?;
                let target = if flags & 1024 != 0 { w.mo(me).target } else { None };
                let sm = w.mo_mut(spawned);
                if flags & 1024 != 0 {
                    sm.target = target;
                } else if sm.flags & MF_MISSILE != 0 {
                    sm.target = Some(me);
                }
                Some(Outcome::Done)
            }
            "a_custommissile" | "a_spawnprojectile" => {
                let class = self.text(w, args, 0, "missiletype");
                let height = self.num(w, args, 1, "spawnheight", 32.0);
                let offset = self.num(w, args, 2, "spawnofs_xy", 0.0);
                let angle = self.num(w, args, 3, "angle", 0.0);
                let flags = self.num(w, args, 4, "flags", 0.0) as i64;
                let pitch = self.num(w, args, 5, "pitch", 0.0);
                self.missile(w, &class, height, offset, angle, flags, pitch).map(|_| Outcome::Done)
            }
            "a_custombulletattack" => {
                let spread_xy = self.num(w, args, 0, "spread_xy", 0.0);
                let _spread_z = self.num(w, args, 1, "spread_z", 0.0);
                let n = self.num(w, args, 2, "numbullets", 1.0) as i32;
                let damage = self.num(w, args, 3, "damageperbullet", 3.0) as i32;
                let range = self.num(w, args, 5, "range", 2048.0);
                let flags = self.num(w, args, 6, "flags", 0.0) as i64;
                let target = self.target(w)?;
                crate::enemy::call_mobj_action(w, me, Action::FaceTarget);
                let _ = target;
                let base = w.mo(me).angle;
                let range = fx(if range <= 0.0 { 2048.0 } else { range });
                let slope = crate::map::aim_line_attack(w, Some(me), base, range);
                w.start_sound(SoundSource::Mobj(me), w.info.mobj(w.mo(me).mobj_type).attacksound);
                for _ in 0..n.max(1) {
                    let spread = frand(w, -spread_xy, spread_xy);
                    let d = if flags & 2 != 0 { damage } else { damage * (rand_range(w, 1, 3) as i32) };
                    crate::map::line_attack(w, me, base + bam(spread), range, slope, d);
                }
                Some(Outcome::Done)
            }
            "a_custommeleeattack" | "a_meleeattack" => {
                let damage = if name == "a_meleeattack" { (rand_range(w, 1, 8) * 3) as i32 } else { self.num(w, args, 0, "damage", 0.0) as i32 };
                let hit_sound = self.text(w, args, 1, "meleesound");
                let target = self.target(w)?;
                crate::enemy::call_mobj_action(w, me, Action::FaceTarget);
                if crate::enemy::check_melee_range(w, me) {
                    if let Some(s) = w.info.sound(&hit_sound) {
                        w.start_sound(SoundSource::Mobj(me), s);
                    }
                    crate::inter::damage_mobj(w, target, Some(me), Some(me), damage);
                }
                Some(Outcome::Done)
            }
            "a_explode" => {
                let damage = self.num(w, args, 0, "damage", -1.0);
                let damage = if damage < 0.0 { 128 } else { damage as i32 };
                let source = w.mo(me).target;
                crate::map::radius_attack(w, me, source, damage);
                Some(Outcome::Done)
            }
            // ---- sound ----
            "a_playsound" | "a_startsound" | "a_playsoundex" => {
                // A_PlaySound(sound, slot, volume, looping, attenuation)
                // A_StartSound(sound, slot, flags, volume, attenuation)
                // A_PlaySoundEx(sound, "channel", looping, attenuation)
                let s = self.text(w, args, 0, "soundname");
                // A name with no definition plays nothing, as in ZDoom.
                let Some(sfx) = w.info.sound(&s) else { return Some(Outcome::Done) };
                let (slot, volume, looping, atten) = match name {
                    "a_playsoundex" => {
                        let ch = self.text(w, args, 1, "channel").to_ascii_lowercase();
                        let slot = ["auto", "weapon", "voice", "item", "body"].iter().position(|c| ch.ends_with(c)).unwrap_or(0) as u8;
                        let looping = self.num(w, args, 2, "looping", 0.0) != 0.0;
                        (slot, 1.0, looping, self.num(w, args, 3, "attenuation", 1.0))
                    }
                    "a_startsound" => {
                        let slot = self.num(w, args, 1, "slot", 4.0) as i64;
                        let flags = self.num(w, args, 2, "flags", 0.0) as i64;
                        // CHANF_LOOP (and the old CHAN_LOOP bit in the slot).
                        let looping = flags & 4 != 0 || slot & 256 != 0;
                        ((slot & 7) as u8, self.num(w, args, 3, "volume", 1.0), looping, self.num(w, args, 4, "attenuation", 1.0))
                    }
                    _ => {
                        let slot = self.num(w, args, 1, "slot", 4.0) as i64;
                        let looping = self.num(w, args, 3, "looping", 0.0) != 0.0 || slot & 256 != 0;
                        ((slot & 7) as u8, self.num(w, args, 2, "volume", 1.0), looping, self.num(w, args, 4, "attenuation", 1.0))
                    }
                };
                w.start_sound_ex(SoundSource::Mobj(me), sfx, slot, volume as f32, atten as f32, looping);
                Some(Outcome::Done)
            }
            "a_stopsound" | "a_stopsoundex" => {
                let slot = if name == "a_stopsoundex" {
                    let ch = self.text(w, args, 0, "channel").to_ascii_lowercase();
                    ["auto", "weapon", "voice", "item", "body"].iter().position(|c| ch.ends_with(c)).unwrap_or(0) as u8
                } else {
                    (self.num(w, args, 0, "slot", 1.0) as i64 & 7) as u8
                };
                w.events.push(SimEvent::StopSound { origin: me, slot });
                Some(Outcome::Done)
            }
            "a_activesound" => {
                let s = w.info.mobj(w.mo(me).mobj_type).activesound;
                w.start_sound(SoundSource::Mobj(me), s);
                Some(Outcome::Done)
            }
            // ---- inventory ----
            "a_giveinventory" | "a_takeinventory" | "a_givetotarget" | "a_takefromtarget" => {
                let item = self.text(w, args, 0, "type");
                let amount = self.num(w, args, 1, "amount", if name.starts_with("a_take") { 0.0 } else { 1.0 }) as i32;
                let who = if name.ends_with("target") { self.target(w)? } else { me };
                if name.starts_with("a_give") {
                    crate::inventory::give(w, who, &item, amount.max(1));
                } else {
                    crate::inventory::take(w, who, &item, amount);
                }
                Some(Outcome::Done)
            }
            "a_dropinventory" => {
                let item = self.text(w, args, 0, "type");
                crate::inventory::take(w, me, &item, 0);
                Some(Outcome::Done)
            }
            // ---- appearance and state ----
            "a_settics" => {
                let t = self.num(w, args, 0, "tics", 0.0) as i32;
                w.mo_mut(me).tics = t;
                Some(Outcome::Done)
            }
            "a_fadeout" | "a_fadein" | "a_fadeto" => {
                let (step, flags) = match name {
                    "a_fadeto" => {
                        let target = self.num(w, args, 0, "target", 0.5) as f32;
                        let amount = self.num(w, args, 1, "amount", 0.1) as f32;
                        let flags = self.num(w, args, 2, "flags", 0.0) as i64;
                        let alpha = 1.0 - w.mo(me).fade;
                        let a = if alpha < target { (alpha + amount).min(target) } else { (alpha - amount).max(target) };
                        w.mo_mut(me).fade = 1.0 - a;
                        (0.0, flags)
                    }
                    _ => (self.num(w, args, 0, "reduce", 0.1) as f32, self.num(w, args, 1, "flags", 1.0) as i64),
                };
                if name == "a_fadeout" {
                    let m = w.mo_mut(me);
                    m.fade = (m.fade + step).min(1.0);
                    if m.fade >= 1.0 && flags & 1 != 0 {
                        crate::mobj::remove_mobj(w, me);
                    }
                    return Some(Outcome::Done);
                } else if name == "a_fadein" {
                    let m = w.mo_mut(me);
                    m.fade = (m.fade - step).max(0.0);
                }
                Some(Outcome::Done)
            }
            "a_settranslucent" => {
                let a = self.num(w, args, 0, "alpha", 1.0) as f32;
                w.mo_mut(me).fade = (1.0 - a).clamp(0.0, 1.0);
                Some(Outcome::Done)
            }
            "a_setscale" => {
                let x = self.num(w, args, 0, "scalex", 1.0) as f32;
                let y = self.num(w, args, 1, "scaley", 0.0) as f32;
                w.mo_mut(me).scale = Some((x, if y == 0.0 { x } else { y }));
                Some(Outcome::Done)
            }
            "a_noblocking" | "a_fall" => {
                crate::enemy::call_mobj_action(w, me, Action::Fall);
                Some(Outcome::Done)
            }
            "a_screamandunblock" => {
                crate::enemy::call_mobj_action(w, me, Action::Scream);
                crate::enemy::call_mobj_action(w, me, Action::Fall);
                Some(Outcome::Done)
            }
            "a_stop" => {
                let m = w.mo_mut(me);
                (m.momx, m.momy, m.momz) = (Fixed(0), Fixed(0), Fixed(0));
                Some(Outcome::Done)
            }
            "a_recoil" => {
                let force = self.num(w, args, 0, "xyvel", 0.0);
                let m = w.mo_mut(me);
                let a = m.angle.to_radians_f32() as f64;
                m.momx -= fx(force * a.cos());
                m.momy -= fx(force * a.sin());
                Some(Outcome::Done)
            }
            "a_changevelocity" => {
                let (x, y, z) = (self.num(w, args, 0, "x", 0.0), self.num(w, args, 1, "y", 0.0), self.num(w, args, 2, "z", 0.0));
                let flags = self.num(w, args, 3, "flags", 0.0) as i64;
                let m = w.mo_mut(me);
                let (vx, vy) = if flags & 1 != 0 {
                    let a = m.angle.to_radians_f32() as f64;
                    (x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos())
                } else {
                    (x, y)
                };
                if flags & 2 != 0 {
                    (m.momx, m.momy, m.momz) = (fx(vx), fx(vy), fx(z));
                } else {
                    m.momx += fx(vx);
                    m.momy += fx(vy);
                    m.momz += fx(z);
                }
                Some(Outcome::Done)
            }
            "a_wander" => {
                w.mo_mut(me).movecount -= 1;
                if w.mo(me).movecount < 0 || !crate::enemy::try_walk(w, me) {
                    crate::enemy::new_chase_dir(w, me);
                }
                Some(Outcome::Done)
            }
            "a_lookex" | "a_look" => {
                crate::enemy::call_mobj_action(w, me, Action::Look);
                Some(Outcome::Done)
            }
            "a_chase" => {
                crate::enemy::call_mobj_action(w, me, Action::Chase);
                Some(Outcome::Done)
            }
            "a_facetarget" | "a_facetracer" => {
                crate::enemy::call_mobj_action(w, me, Action::FaceTarget);
                Some(Outcome::Done)
            }
            "a_cleartarget" => {
                w.mo_mut(me).target = None;
                Some(Outcome::Done)
            }
            "a_setangle" => {
                let a = self.num(w, args, 0, "angle", 0.0);
                w.mo_mut(me).angle = bam(a);
                Some(Outcome::Done)
            }
            "a_setuservar" | "a_setuserarray" => {
                let var = self.text(w, args, 0, "name");
                let value = if name == "a_setuserarray" { self.num(w, args, 2, "value", 0.0) } else { self.num(w, args, 1, "value", 0.0) };
                self.set_user_var(w, &var, value);
                Some(Outcome::Done)
            }
            "a_setarg" => {
                let (i, v) = (self.num(w, args, 0, "pos", 0.0) as usize, self.num(w, args, 1, "value", 0.0) as i32);
                if i < 5 {
                    w.mo_mut(me).args[i] = v;
                }
                Some(Outcome::Done)
            }
            "a_setspecial" => {
                let mut v = [0i32; 6];
                for (i, n) in ["spec", "arg0", "arg1", "arg2", "arg3", "arg4"].iter().enumerate() {
                    v[i] = self.num(w, args, i, n, 0.0) as i32;
                }
                let m = w.mo_mut(me);
                m.special = v[0];
                m.args = [v[1], v[2], v[3], v[4], v[5]];
                Some(Outcome::Done)
            }
            "a_setshootable" | "a_unsetshootable" | "a_setsolid" | "a_unsetsolid" | "a_nogravity" | "a_gravity" | "a_lowgravity"
            | "a_setfloat" | "a_unsetfloat" => {
                let m = w.mo_mut(me);
                match name {
                    "a_setshootable" => m.flags |= MF_SHOOTABLE,
                    "a_unsetshootable" => m.flags &= !MF_SHOOTABLE,
                    "a_setsolid" => m.flags |= MF_SOLID,
                    "a_unsetsolid" => m.flags &= !MF_SOLID,
                    "a_nogravity" => m.flags |= MF_NOGRAVITY,
                    "a_gravity" | "a_lowgravity" => m.flags &= !MF_NOGRAVITY,
                    "a_setfloat" => m.flags |= MF_FLOAT,
                    _ => m.flags &= !MF_FLOAT,
                }
                Some(Outcome::Done)
            }
            "a_sethealth" => {
                let h = self.num(w, args, 0, "health", 1.0) as i32;
                w.mo_mut(me).health = h;
                Some(Outcome::Done)
            }
            "a_setmass" | "a_setgravity" | "a_setspeed" | "a_setinvulnerable" | "a_unsetinvulnerable" | "a_setreflective" | "a_unsetreflective"
            | "a_settranslation" | "a_setrenderstyle" | "a_setpitch" | "a_setroll" | "a_quake" | "a_quakeex" | "a_spawndebris" => Some(Outcome::Done),
            "a_log" | "a_print" | "a_printbold" => {
                let text = self.text(w, args, 0, "whattoprint");
                if name != "a_log" {
                    w.script_events.push(ScriptEvent::Print { player: None, text });
                }
                Some(Outcome::Done)
            }
            // ---- specials and scripts ----
            "a_callspecial" => {
                let mut v = [0i32; 6];
                for (i, n) in ["special", "arg1", "arg2", "arg3", "arg4", "arg5"].iter().enumerate() {
                    v[i] = self.num(w, args, i, n, 0.0) as i32;
                }
                let ctx = SpecialCtx { activator: Some(me), ..Default::default() };
                hexen::execute(w, v[0], [v[1], v[2], v[3], v[4], v[5]], &ctx);
                Some(Outcome::Done)
            }
            "acs_namedexecute" | "acs_namedexecutealways" | "acs_namedexecutewithresult" | "acs_execute" | "acs_executealways" | "callacs" => {
                let named = name.contains("named") || name == "callacs";
                let script = if named {
                    doom_script::acs::ScriptId::Name(self.text(w, args, 0, "script"))
                } else {
                    doom_script::acs::ScriptId::Number(self.num(w, args, 0, "script", 0.0) as i32)
                };
                let with_result = name.ends_with("withresult") || name == "callacs";
                let rest: Vec<i32> = (1..6).map(|i| self.num(w, args, i, "", 0.0) as i32).collect();
                let (map, a) = if with_result { (0, [rest[0], rest[1], rest[2], rest[3]]) } else { (rest[0], [rest[1], rest[2], rest[3], 0]) };
                let kind = if name.ends_with("always") { doom_script::acs::RequestKind::ExecuteAlways } else { doom_script::acs::RequestKind::Execute };
                let act = doom_script::acs::Activation { activator: Some(hexen::actor_handle(me)), ..Default::default() };
                let r = doom_script::acs::ScriptRequest { script, map, args: a, act, kind };
                match w.acs.as_mut() {
                    Some(acs) => {
                        acs.request(&r);
                    }
                    None => w.acs_requests.push(r),
                }
                Some(Outcome::Done)
            }
            "a_alertmonsters" => {
                if let Some(t) = self.target(w) {
                    crate::enemy::noise_alert(w, t, me);
                }
                Some(Outcome::Done)
            }
            "a_die" | "a_kill" => {
                if w.mo(me).health > 0 {
                    crate::inter::damage_mobj(w, me, None, None, 1_000_000);
                }
                Some(Outcome::Done)
            }
            "a_remove" => {
                crate::mobj::remove_mobj(w, me);
                Some(Outcome::Done)
            }
            "a_killchildren" | "a_killmaster" | "a_raiseself" => Some(Outcome::Done),
            // ZScript inventory methods.
            "takeinventory" => {
                let item = self.text(w, args, 0, "itemclass");
                let n = self.num(w, args, 1, "amount", 1.0) as i32;
                crate::inventory::take(w, me, &item, n);
                Some(Outcome::Done)
            }
            "giveinventory" => {
                let item = self.text(w, args, 0, "type");
                let n = self.num(w, args, 1, "amount", 1.0) as i32;
                crate::inventory::give(w, me, &item, n);
                Some(Outcome::Done)
            }
            _ => match self.call_method(w, name, args) {
                Some((out, v)) => {
                    self.last_ret = Some(v);
                    Some(out)
                }
                None => {
                    w.note_unimplemented("DECORATE/ZScript action (other)");
                    None
                }
            },
        }
    }

    /// Weapon actions (a weapon context only). `None`: not a weapon
    /// action; `Some(outcome)` otherwise.
    fn weapon_call(&mut self, w: &mut World, player: usize, psp: usize, name: &str, args: &[Arg]) -> Option<Option<Outcome>> {
        use crate::weapons::{self as wp, FireMode};
        let me = self.me;
        let done = Some(Some(Outcome::Done));
        match name {
            "a_firebullets" => {
                let spread_xy = self.num(w, args, 0, "spread_xy", 0.0);
                let spread_z = self.num(w, args, 1, "spread_z", 0.0);
                let n = self.num(w, args, 2, "numbullets", 1.0) as i32;
                let damage = self.num(w, args, 3, "damageperbullet", 5.0) as i32;
                let puff = self.puff_arg(w, args, 4);
                let flags = self.num(w, args, 5, "flags", 1.0) as i32;
                let range = self.num(w, args, 6, "range", 0.0);
                wp::fire_bullets(w, player, spread_xy, spread_z, n, damage, flags, range, puff);
                done
            }
            "a_firecustommissile" | "a_fireprojectile" => {
                let class = self.text(w, args, 0, "missiletype");
                let angle = self.num(w, args, 1, "angle", 0.0);
                let takes_ammo = self.num(w, args, 2, "useammo", 1.0) != 0.0;
                if takes_ammo {
                    wp::use_ammo(w, player);
                }
                self.fire_missile(w, &class, angle);
                done
            }
            "spawnplayermissile" => {
                let class = self.text(w, args, 0, "type");
                let angle = self.num(w, args, 1, "angle", 0.0);
                self.fire_missile(w, &class, angle);
                done
            }
            "a_custompunch" => {
                let damage = self.num(w, args, 0, "damage", 2.0) as i32;
                let norandom = self.num(w, args, 1, "norandom", 0.0) != 0.0;
                let flags = self.num(w, args, 2, "flags", 0.0) as i32;
                let puff = self.puff_arg(w, args, 3);
                let range = self.num(w, args, 4, "range", 64.0);
                // CPF_USEAMMO
                if flags & 1 != 0 {
                    wp::use_ammo(w, player);
                }
                let d = if norandom { damage } else { damage * ((script_random(w) % 8) + 1) };
                let an = w.mo(me).angle;
                let range = Fixed((range.max(1.0) * FRACUNIT as f64) as i32);
                let slope = crate::map::aim_line_attack(w, Some(me), an, range);
                crate::map::line_attack_puff(w, me, an, range, slope, d, puff);
                done
            }
            "lineattack" => {
                // LineAttack(angle, distance, pitch, damage, damagetype, pufftype, flags)
                let angle = self.num(w, args, 0, "angle", deg(w.mo(me).angle));
                let dist = self.num(w, args, 1, "distance", 8192.0);
                let pitch = self.num(w, args, 2, "pitch", 0.0);
                let damage = self.num(w, args, 3, "damage", 0.0) as i32;
                let puff = self.puff_arg(w, args, 5);
                let slope = Fixed((-(pitch.to_radians().tan()) * FRACUNIT as f64) as i32);
                crate::map::line_attack_puff(w, me, bam(angle), fx(dist), slope, damage, puff);
                done
            }
            "a_gunflash" => {
                let label = self.text(w, args, 0, "flash");
                wp::gun_flash(w, player, me, if label.is_empty() { None } else { Some(label) });
                done
            }
            "a_jumpifnoammo" => {
                if wp::has_ammo(w, player) {
                    return done;
                }
                let target = self.arg(w, args, 0, "label")?;
                Some(self.jump(w, target))
            }
            "checkammo" => {
                // CheckAmmo(fireMode, autoSwitch, requireAmmo): the result
                // is the value; fire mode 1 is AltFire.
                let alt = self.num(w, args, 0, "firemode", 0.0) as i32 == 1;
                let ok = wp::has_ammo_for(w, player, if alt { FireMode::Alt } else { FireMode::Primary });
                self.last_ret = Some(Value::Num(ok as i32 as f64));
                done
            }
            "depleteammo" => {
                // DepleteAmmo(altFire, checkEnough, ammouse)
                let alt = self.num(w, args, 0, "altfire", 0.0) != 0.0;
                let amount = self.arg(w, args, 2, "ammouse").map(|v| v.num() as i32).filter(|&n| n > 0);
                let mode = if alt { FireMode::Alt } else { FireMode::Primary };
                let ok = wp::has_ammo_for(w, player, mode);
                if ok {
                    wp::use_ammo_mode(w, player, mode, amount);
                }
                self.last_ret = Some(Value::Num(ok as i32 as f64));
                done
            }
            "a_checkreload" => {
                crate::pspr::check_ammo(w, player);
                done
            }
            "a_clearrefire" => {
                w.players[player].refire = 0;
                done
            }
            "a_refire" => {
                // A_ReFire(state): the refire state, else Hold/AltHold.
                let target = self.text(w, args, 0, "flash");
                crate::pspr::refire(w, player, if target.is_empty() { None } else { self.label(w, &target) });
                done
            }
            "a_light" => {
                w.players[player].extralight = self.num(w, args, 0, "extralight", 0.0) as i32;
                done
            }
            "a_weaponready" => {
                w.players[player].ready_flags = self.num(w, args, 0, "flags", 0.0) as i32;
                crate::pspr::call_psprite_action(w, player, psp, Action::WeaponReady);
                done
            }
            // Presentation only (no zoom or offsets in this renderer).
            "a_zoomfactor" | "a_weaponoffset" | "a_setcrosshair" | "a_overlay" | "a_overlayoffset" | "a_overlayflags" => done,
            _ => None,
        }
    }

    /// A puff class argument (`"MyPuff"`); `None` for the default puff.
    fn puff_arg(&mut self, w: &mut World, args: &[Arg], i: usize) -> Option<MobjType> {
        let name = self.text(w, args, i, "pufftype");
        if name.is_empty() || name.eq_ignore_ascii_case("bulletpuff") {
            return None;
        }
        let t = w.info.type_by_name(&name);
        if t.is_none() {
            w.note_unimplemented("puff of an unknown class");
        }
        t
    }

    /// A player missile of `class`, turned by `angle` degrees.
    fn fire_missile(&mut self, w: &mut World, class: &str, angle: f64) {
        let me = self.me;
        let Some(t) = w.info.type_by_name(class) else {
            w.note_unimplemented("spawn of an unknown class");
            return;
        };
        let before: Vec<MobjId> = w.thinkers.mobjs().filter(|(_, m)| m.mobj_type == t && m.target == Some(me)).map(|(id, _)| id).collect();
        crate::mobj::spawn_player_missile(w, me, t);
        if angle == 0.0 {
            return;
        }
        let new = w.thinkers.mobjs().filter(|(id, m)| m.mobj_type == t && m.target == Some(me) && !before.contains(id)).map(|(id, _)| id).last();
        if let Some(id) = new {
            let m = w.mo_mut(id);
            let a = m.angle + crate::weapons::deg_to_bam(angle);
            let speed = (m.momx.0 as f64).hypot(m.momy.0 as f64);
            let r = a.to_radians_f32() as f64;
            m.angle = a;
            m.momx = Fixed((speed * r.cos()) as i32);
            m.momy = Fixed((speed * r.sin()) as i32);
        }
    }

    fn jump(&mut self, w: &mut World, target: Value) -> Option<Outcome> {
        self.state_arg(w, target).map(Outcome::Jump).or(Some(Outcome::Done))
    }

    fn jump_if(&mut self, w: &mut World, ok: bool, args: &[Arg], state_index: usize) -> Option<Outcome> {
        if !ok {
            return Some(Outcome::Done);
        }
        let target = self.arg(w, args, state_index, "state")?;
        self.jump(w, target)
    }

    /// Spawn an actor by name at (x, y, z) with velocity; `check`: remove
    /// it again if it doesn't fit there.
    #[allow(clippy::too_many_arguments)]
    fn spawn(&mut self, w: &mut World, class: &str, x: f64, y: f64, z: f64, angle: Angle, vx: f64, vy: f64, vz: f64, tid: i32, check: bool) -> Option<MobjId> {
        let Some(t) = w.info.type_by_name(class) else {
            w.note_unimplemented("spawn of an unknown class");
            return None;
        };
        let id = crate::mobj::spawn_mobj(w, fx(x), fx(y), fx(z), t);
        {
            let m = w.mo_mut(id);
            m.angle = angle;
            m.momx = fx(vx);
            m.momy = fx(vy);
            m.momz = fx(vz);
            if tid != 0 {
                m.tid = tid;
            }
        }
        if check && w.mo(id).flags & MF_MISSILE == 0 && !crate::map::check_position(w, id, fx(x), fx(y)) {
            crate::mobj::remove_mobj(w, id);
            return None;
        }
        if w.mo(id).flags & MF_COUNTKILL != 0 {
            w.totalkills += 1;
        }
        Some(id)
    }

    /// `A_SpawnProjectile`: a missile toward the target (or along the
    /// actor's facing with `CMF_AIMDIRECTION`).
    #[allow(clippy::too_many_arguments)]
    fn missile(&mut self, w: &mut World, class: &str, height: f64, offset: f64, angle: f64, flags: i64, pitch: f64) -> Option<MobjId> {
        let me = self.me;
        let Some(t) = w.info.type_by_name(class) else {
            w.note_unimplemented("spawn of an unknown class");
            return None;
        };
        let target = self.target(w);
        if target.is_none() && flags & 2 == 0 {
            return None;
        }
        let m = w.mo(me);
        let facing = m.angle.to_radians_f32() as f64;
        // Side offset to the right of the facing direction.
        let (ox, oy) = (offset * (facing - std::f64::consts::FRAC_PI_2).cos(), offset * (facing - std::f64::consts::FRAC_PI_2).sin());
        let (sx, sy, sz) = (unfx(m.x) + ox, unfx(m.y) + oy, unfx(m.z) + height);
        let speed = unfx(Fixed(w.info.mobj(t).speed));
        let (dir, vz) = match (flags & 2 != 0, target) {
            (false, Some(tg)) => {
                let d = w.mo(tg);
                let (dx, dy) = (unfx(d.x) - sx, unfx(d.y) - sy);
                let dist = dx.hypot(dy).max(1.0);
                let vz = (unfx(d.z) + unfx(d.height) / 2.0 - sz) / (dist / speed.max(0.1));
                (dy.atan2(dx), vz)
            }
            _ => (facing, -pitch.to_radians().tan() * speed),
        };
        let dir = dir + angle.to_radians();
        let id = crate::mobj::spawn_mobj(w, fx(sx), fx(sy), fx(sz), t);
        let see = w.info.mobj(t).seesound;
        w.start_sound(SoundSource::Mobj(id), see);
        let mm = w.mo_mut(id);
        mm.target = Some(me);
        mm.angle = bam(dir.to_degrees());
        mm.momx = fx(speed * dir.cos());
        mm.momy = fx(speed * dir.sin());
        mm.momz = fx(vz);
        crate::mobj::check_missile_spawn(w, id);
        Some(id)
    }
}

enum Flow {
    Next,
    Break,
    Continue,
    Return(Option<Outcome>),
}

/// `a.b.c` as names (the root must be a plain identifier).
fn member_path(e: &Expr) -> Option<Vec<&str>> {
    match e {
        Expr::Ident(n) => Some(vec![n.as_str()]),
        Expr::Member(obj, field) => {
            let mut p = member_path(obj)?;
            p.push(field.as_str());
            Some(p)
        }
        _ => None,
    }
}

fn apply_op(op: &str, old: f64, v: f64) -> f64 {
    match op {
        "=" => v,
        "+=" => old + v,
        "-=" => old - v,
        "*=" => old * v,
        "/=" if v != 0.0 => old / v,
        "%=" if v as i64 != 0 => ((old as i64) % (v as i64)) as f64,
        "|=" => ((old as i64) | (v as i64)) as f64,
        "&=" => ((old as i64) & (v as i64)) as f64,
        "^=" => ((old as i64) ^ (v as i64)) as f64,
        "<<=" => ((old as i64) << (v as i64).clamp(0, 63)) as f64,
        ">>=" => ((old as i64) >> (v as i64).clamp(0, 63)) as f64,
        _ => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doom_script::zscript::parse_expr;

    #[test]
    fn constants_and_angles() {
        assert_eq!(constant("sxf_nocheckposition"), Some(32.0));
        assert!((deg(bam(90.0)) - 90.0).abs() < 1e-6);
        assert!(parse_expr("SXF_NOCHECKPOSITION|SXF_TRANSFERPOINTERS").is_ok());
    }
}
