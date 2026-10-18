//! Actor definitions from mods: DECORATE and ZScript classes, built into
//! the [`Info`] tables the playsim runs on.
//!
//! Both languages are parsed by `doom_script` (DECORATE through the same
//! state and expression grammar as ZScript) and land here as the same
//! things: a class with a parent, properties and flags, user variables and
//! states. States keep their action calls with argument expressions
//! ([`ActionCall`]); `crate::actions` runs them. Vanilla actions without
//! arguments use the vanilla code directly.
//!
//! Order follows ZDoom: ZScript classes first (parents before children),
//! then DECORATE in load order, then MAPINFO `DoomEdNums`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use doom_content::ResourceStack;
use doom_core::Fixed;
use doom_script::zscript::ast::{self, DefaultItem, Expr, StateAction, StateItem, StatesDecl, Tics};
use doom_script::zscript::{parse, parse_decorate};

use crate::info::*;
use crate::mobj::flags::*;

/// Doom classes by their ZDoom names.
pub(crate) const CLASSES: &[(&str, MobjType)] = &[
    ("doomplayer", MT_PLAYER),
    ("zombieman", MT_POSSESSED),
    ("shotgunguy", MT_SHOTGUY),
    ("archvile", MT_VILE),
    ("archvilefire", MT_FIRE),
    ("revenant", MT_UNDEAD),
    ("revenanttracer", MT_TRACER),
    ("revenanttracersmoke", MT_SMOKE),
    ("fatso", MT_FATSO),
    ("fatshot", MT_FATSHOT),
    ("chaingunguy", MT_CHAINGUY),
    ("doomimp", MT_TROOP),
    ("demon", MT_SERGEANT),
    ("spectre", MT_SHADOWS),
    ("cacodemon", MT_HEAD),
    ("baronofhell", MT_BRUISER),
    ("baronball", MT_BRUISERSHOT),
    ("hellknight", MT_KNIGHT),
    ("lostsoul", MT_SKULL),
    ("spidermastermind", MT_SPIDER),
    ("arachnotron", MT_BABY),
    ("cyberdemon", MT_CYBORG),
    ("painelemental", MT_PAIN),
    ("wolfensteinss", MT_WOLFSS),
    ("commanderkeen", MT_KEEN),
    ("bossbrain", MT_BOSSBRAIN),
    ("bosseye", MT_BOSSSPIT),
    ("bosstarget", MT_BOSSTARGET),
    ("spawnshot", MT_SPAWNSHOT),
    ("spawnfire", MT_SPAWNFIRE),
    ("explosivebarrel", MT_BARREL),
    ("doomimpball", MT_TROOPSHOT),
    ("cacodemonball", MT_HEADSHOT),
    ("rocket", MT_ROCKET),
    ("plasmaball", MT_PLASMA),
    ("bfgball", MT_BFG),
    ("arachnotronplasma", MT_ARACHPLAZ),
    ("bulletpuff", MT_PUFF),
    ("blood", MT_BLOOD),
    ("teleportfog", MT_TFOG),
    ("itemfog", MT_IFOG),
    ("teleportdest", MT_TELEPORTMAN),
    ("bfgextra", MT_EXTRABFG),
    ("greenarmor", MT_MISC0),
    ("bluearmor", MT_MISC1),
    ("healthbonus", MT_MISC2),
    ("armorbonus", MT_MISC3),
    ("bluecard", MT_MISC4),
    ("redcard", MT_MISC5),
    ("yellowcard", MT_MISC6),
    ("yellowskull", MT_MISC7),
    ("redskull", MT_MISC8),
    ("blueskull", MT_MISC9),
    ("stimpack", MT_MISC10),
    ("medikit", MT_MISC11),
    ("soulsphere", MT_MISC12),
    ("invulnerabilitysphere", MT_INV),
    ("berserk", MT_MISC13),
    ("blursphere", MT_INS),
    ("radsuit", MT_MISC14),
    ("allmap", MT_MISC15),
    ("infrared", MT_MISC16),
    ("megasphere", MT_MEGA),
    ("clip", MT_CLIP),
    ("clipbox", MT_MISC17),
    ("rocketammo", MT_MISC18),
    ("rocketbox", MT_MISC19),
    ("cell", MT_MISC20),
    ("cellpack", MT_MISC21),
    ("shell", MT_MISC22),
    ("shellbox", MT_MISC23),
    ("backpack", MT_MISC24),
    ("bfg9000", MT_MISC25),
    ("chaingun", MT_CHAINGUN),
    ("chainsaw", MT_MISC26),
    ("rocketlauncher", MT_MISC27),
    ("plasmarifle", MT_MISC28),
    ("shotgun", MT_SHOTGUN),
    ("supershotgun", MT_SUPERSHOTGUN),
    ("techlamp", MT_MISC29),
    ("techlamp2", MT_MISC30),
    ("column", MT_MISC31),
];

/// ZDoom inventory base classes: actors derived from them are pickups.
const INVENTORY_BASES: &[&str] = &[
    "inventory",
    "custominventory",
    "key",
    "puzzleitem",
    "health",
    "healthpickup",
    "armor",
    "basicarmor",
    "basicarmorpickup",
    "basicarmorbonus",
    "ammo",
    "backpackitem",
    "weapon",
    "doomweapon",
    "powerupgiver",
    "powerup",
    "mapreveal",
    "fakeinventory",
    "weaponpiece",
    "weaponholder",
];

/// Other ZDoom base classes with no vanilla equivalent: used as plain
/// actors (their special behaviour is not implemented yet).
const OTHER_BASES: &[&str] = &[
    "actor",
    "randomspawner",
    "switchabledecoration",
    "switchingdecoration",
    "fastprojectile",
    "spark",
    "playerpawn",
    "morphprojectile",
    "pointlight",
    "dynamiclight",
    "mapmarker",
    "hatetarget",
    "bloodsplatter",
    "axeblood",
    "smokepuff",
];

/// Non-actor ZScript roots (event handlers, thinkers, plain objects): not
/// actor classes.
const NON_ACTOR_ROOTS: &[&str] = &["object", "thinker", "eventhandler", "staticeventhandler", "menu", "optionmenu", "basestatusbar", "doomstatusbar"];

const FLAGS: &[(&str, u32)] = &[
    ("special", MF_SPECIAL),
    ("solid", MF_SOLID),
    ("shootable", MF_SHOOTABLE),
    ("nosector", MF_NOSECTOR),
    ("noblockmap", MF_NOBLOCKMAP),
    ("ambush", MF_AMBUSH),
    ("justhit", MF_JUSTHIT),
    ("justattacked", MF_JUSTATTACKED),
    ("spawnceiling", MF_SPAWNCEILING),
    ("nogravity", MF_NOGRAVITY),
    ("dropoff", MF_DROPOFF),
    ("pickup", MF_PICKUP),
    ("noclip", MF_NOCLIP),
    ("slide", MF_SLIDE),
    ("float", MF_FLOAT),
    ("teleport", MF_TELEPORT),
    ("missile", MF_MISSILE),
    ("dropped", MF_DROPPED),
    ("shadow", MF_SHADOW),
    ("noblood", MF_NOBLOOD),
    ("corpse", MF_CORPSE),
    ("infloat", MF_INFLOAT),
    ("countkill", MF_COUNTKILL),
    ("countitem", MF_COUNTITEM),
    ("skullfly", MF_SKULLFLY),
    ("notdmatch", MF_NOTDMATCH),
];

/// Flags that only change rendering or behaviour we don't model; accepted
/// without a warning.
const QUIET_FLAGS: &[&str] = &[
    "forcexybillboard", "forceybillboard", "clientsideonly", "noblockmonst", "dontsplash", "notelestomp", "noteleport", "floorclip",
    "noskin", "donttranslate", "nointeraction", "thruactors", "bright", "randomize", "ghost", "dontblast", "missilemore",
    "missileevenmore", "noradiusdmg", "nodamagethrust", "puffonactors", "bloodsplatter", "fullvolactive", "fullvoldeath",
    "noiceDeath", "dontgib", "boss", "quicktoretaliate", "lookallaround", "floatbob", "invisible", "nodropoff", "notarget",
    "dontfall", "ismonster", "friendly", "noinfighting", "seekermissile", "screenseeker", "alwayspuff", "dontthrust",
    "dontmorph", "noteamdamage", "hexenbounce", "doombounce", "bounceonactors", "canbouncewater", "explodeonwater",
    "inventory.autoactivate", "inventory.alwayspickup", "inventory.bigpowerup", "inventory.undroppable", "inventory.invbar",
    "inventory.quiet", "inventory.persistentpower", "inventory.hubpower", "inventory.ignoreskill", "inventory.keepdepleted",
    "weapon.noautofire", "weapon.noautoaim", "weapon.wimpy_weapon", "weapon.cheatnotweapon", "weapon.ammo_optional",
    "weapon.alt_ammo_optional", "weapon.noalert", "weapon.meleeweapon", "weapon.dontbob", "weapon.explosive", "weapon.bfg",
    "weapon.axeblood", "weapon.powered_up", "weapon.primary_uses_both", "weapon.ammo_checkboth", "weapon.noautoswitchto",
    "noextremedeath", "extremedeath", "stealth", "nopain", "nevertarget", "nogravity", "rollsprite", "wallsprite",
    "flatsprite", "nodecal", "forcedecal", "oldradiusdmg", "dontrip", "ripper", "noexplodefloor", "skyexplode",
    "activatemcross", "activateimpact", "canpushwalls", "canusewalls", "nobossrip", "forceradiusdmg", "cannotpush",
    "pushable", "blockedbysolidactors", "thrughost", "spectral", "telestomp", "movewithsector", "relativetofloor",
    "crashed", "slidesonwalls", "allowparticles", "nofear", "frightened", "dormant", "reflective", "shieldreflect",
    "deflect", "notautoaimed", "notonautomap", "touchy", "noicedeath", "shadowaim", "nosprite", "seesdaggers",
    "bumpspecial", "usespecial", "noverticalmeleerange", "avoidmelee", "staymorphed", "nonshootable", "noblooddecals",
    "cantleavefloorpic", "fireresist", "lowgravity", "windthrust", "nolifedrain", "jumpdown", "noforwardfall",
    "cannotpush", "solid", "missile", "alwaysrespawn", "neverrespawn", "nosplashalert", "synchronized", "faster",
    "fastmelee", "standstill", "ceilinghugger", "dontcorpse", "dontharmclass", "dontharmspecies", "hitmaster",
    "hittarget", "hittracer", "hitowner", "e1m8boss", "e2m8boss", "e3m8boss", "e4m6boss", "e4m8boss", "map07boss1",
    "map07boss2", "bossdeath", "harmfriends", "doharmspecies", "dontinterpolate", "usebouncestate", "mthruspecies",
    "notdmatch", "grenadetrail", "zdoomtrans", "invulnerable", "buddha", "absmaskangle", "absmaskpitch", "noblockmonst",
];

/// A class definition from either language.
struct ActorSpec<'a> {
    name: &'a str,
    parent: Option<&'a str>,
    replaces: Option<&'a str>,
    doomednum: Option<i32>,
    defaults: &'a [DefaultItem],
    vars: &'a [String],
    states: &'a [StatesDecl],
    source: &'static str,
    /// ZScript methods with bodies.
    methods: &'a [Arc<ast::FuncDecl>],
}

struct Builder<'a> {
    info: &'a mut Info,
    classes: HashMap<String, MobjType>,
    warnings: Vec<String>,
    /// Methods of the ZScript class being built (its states may call them).
    method_names: Vec<String>,
    ignored: BTreeSet<String>,
}

/// Load every actor definition in the stack (ZScript, DECORATE, MAPINFO
/// editor numbers) and SNDINFO sound names into `info`. Returns warnings.
pub fn load(info: &mut Info, stack: &ResourceStack) -> Vec<String> {
    let mut warnings = Vec::new();
    crate::sounds::load_sndinfo(info, stack, &mut warnings);
    let mut b = Builder::new(info);

    // ---- ZScript ----
    let mut files = Vec::new();
    for (name, text) in read_with_includes(stack, "ZSCRIPT", |t| parse(t).0.includes, &mut b.warnings) {
        let (file, errors) = parse(&text);
        b.warnings.extend(errors.iter().take(20).map(|e| format!("ZScript {name}: {e}")));
        files.push(file);
    }
    let classes: Vec<&ast::ClassDecl> = files.iter().flat_map(|f| f.decls.iter()).filter_map(|d| if let ast::Decl::Class(c) = d { Some(c) } else { None }).collect();
    // Which classes are actors: derived (through other ZScript classes) from
    // a known actor class.
    let by_name: HashMap<String, &ast::ClassDecl> = classes.iter().map(|c| (c.name.to_ascii_lowercase(), *c)).collect();
    let is_actor = |c: &ast::ClassDecl, b: &Builder| -> bool {
        let mut p = c.parent.as_deref().map(str::to_ascii_lowercase);
        for _ in 0..64 {
            let Some(name) = p else { return false };
            if NON_ACTOR_ROOTS.contains(&name.as_str()) {
                return false;
            }
            if b.classes.contains_key(&name) || INVENTORY_BASES.contains(&name.as_str()) || OTHER_BASES.contains(&name.as_str()) {
                return true;
            }
            p = by_name.get(&name).and_then(|c| c.parent.as_deref().map(str::to_ascii_lowercase));
        }
        false
    };
    let actors: Vec<&ast::ClassDecl> = classes.iter().copied().filter(|c| !c.extend && !c.mixin && is_actor(c, &b)).collect();
    // Parents first.
    let mut done = vec![false; actors.len()];
    loop {
        let mut progress = false;
        for (i, c) in actors.iter().enumerate() {
            if done[i] {
                continue;
            }
            let parent_ready = c.parent.as_deref().map(str::to_ascii_lowercase).is_none_or(|p| !by_name.contains_key(&p) || b.classes.contains_key(&p));
            if parent_ready {
                b.zscript_class(c);
                done[i] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    for (i, c) in actors.iter().enumerate() {
        if !done[i] {
            b.zscript_class(c);
        }
    }
    for c in classes.iter().filter(|c| c.extend) {
        b.zscript_extend(c);
    }

    // ---- DECORATE ----
    for (name, text) in read_with_includes(stack, "DECORATE", |t| parse_decorate(t).0.includes, &mut b.warnings) {
        let (file, errors) = parse_decorate(&text);
        b.warnings.extend(errors.iter().take(20).map(|e| format!("DECORATE {name}: {e}")));
        for a in &file.actors {
            b.actor(&ActorSpec {
                name: &a.name,
                parent: a.parent.as_deref(),
                replaces: a.replaces.as_deref(),
                doomednum: a.doomednum.map(|n| n as i32),
                defaults: &a.defaults,
                vars: &a.vars,
                states: &a.states,
                source: "DECORATE",
                methods: &[],
            });
        }
    }

    // ---- MAPINFO DoomEdNums ----
    let mapinfo = doom_content::mapinfo::MapInfo::load(stack);
    for (&num, class) in &mapinfo.doomednums {
        match b.info.type_by_name(class) {
            Some(t) => {
                for m in b.info.mobjinfo.iter_mut().filter(|m| m.doomednum == num) {
                    m.doomednum = -1;
                }
                b.info.mobjinfo[t.idx()].doomednum = num;
            }
            None => b.warnings.push(format!("DoomEdNums: {num} = unknown class {class}")),
        }
    }

    if !b.ignored.is_empty() {
        let list: Vec<_> = b.ignored.iter().cloned().collect();
        b.warnings.push(format!("actors: not supported yet: {}", list.join(", ")));
    }
    warnings.append(&mut b.warnings);
    warnings
}

/// Load actor definitions from text (tools and tests): ZScript, then
/// DECORATE. Returns warnings.
pub fn load_text(info: &mut Info, decorate: &str, zscript: &str) -> Vec<String> {
    let mut b = Builder::new(info);
    let (z, errors) = parse(zscript);
    b.warnings.extend(errors.iter().map(|e| format!("ZScript: {e}")));
    for d in &z.decls {
        if let ast::Decl::Class(c) = d {
            if c.extend {
                b.zscript_extend(c);
            } else {
                b.zscript_class(c);
            }
        }
    }
    let (f, errors) = parse_decorate(decorate);
    b.warnings.extend(errors.iter().map(|e| format!("DECORATE: {e}")));
    for a in &f.actors {
        b.actor(&ActorSpec {
            name: &a.name,
            parent: a.parent.as_deref(),
            replaces: a.replaces.as_deref(),
            doomednum: a.doomednum.map(|n| n as i32),
            defaults: &a.defaults,
            vars: &a.vars,
            states: &a.states,
            source: "DECORATE",
            methods: &[],
        });
    }
    b.warnings
}

/// The text of every lump with this name plus the files they include
/// (each once), in load order.
fn read_with_includes(stack: &ResourceStack, lump: &str, includes: impl Fn(&str) -> Vec<String>, warnings: &mut Vec<String>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    fn visit(
        stack: &ResourceStack,
        name: String,
        text: String,
        includes: &dyn Fn(&str) -> Vec<String>,
        seen: &mut BTreeSet<String>,
        out: &mut Vec<(String, String)>,
        warnings: &mut Vec<String>,
        depth: u32,
    ) {
        // Included files come first: they define what the includer uses.
        for inc in includes(&text) {
            let key = inc.to_ascii_lowercase();
            if !seen.insert(key) || depth > 32 {
                continue;
            }
            match stack.find_path(&inc).or_else(|| stack.check_num_for_name(&inc)) {
                Some(id) => {
                    let t = String::from_utf8_lossy(stack.data(id)).into_owned();
                    visit(stack, inc, t, includes, seen, out, warnings, depth + 1);
                }
                None => warnings.push(format!("#include {inc:?} not found")),
            }
        }
        out.push((name, text));
    }
    for &id in stack.all_named(lump) {
        let text = String::from_utf8_lossy(stack.data(id)).into_owned();
        visit(stack, format!("{lump}#{}", id.0), text, &includes, &mut seen, &mut out, warnings, 0);
    }
    out
}

/// A constant number from a property value (`8`, `-0.5`, `(5)`, `2*8`).
fn const_num(e: &Expr) -> Option<f64> {
    Some(match e {
        Expr::Int(i) => *i as f64,
        Expr::Float(f) => *f,
        Expr::Unary("-", x) => -const_num(x)?,
        Expr::Unary("+", x) => const_num(x)?,
        Expr::Binary(op, a, b) => {
            let (a, b) = (const_num(a)?, const_num(b)?);
            match *op {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" if b != 0.0 => a / b,
                _ => return None,
            }
        }
        _ => return None,
    })
}

fn const_str(e: &Expr) -> Option<&str> {
    match e {
        Expr::Str(s) | Expr::Name(s) | Expr::Ident(s) => Some(s),
        _ => None,
    }
}

fn actor_defaults() -> MobjInfo {
    MobjInfo {
        doomednum: -1,
        spawnstate: S_NULL,
        spawnhealth: 1000,
        seestate: S_NULL,
        seesound: SfxNum(0),
        reactiontime: 8,
        attacksound: SfxNum(0),
        painstate: S_NULL,
        painchance: 0,
        painsound: SfxNum(0),
        meleestate: S_NULL,
        missilestate: S_NULL,
        deathstate: S_NULL,
        xdeathstate: S_NULL,
        deathsound: SfxNum(0),
        speed: 0,
        radius: Fixed::from_int(20),
        height: Fixed::from_int(16),
        mass: 100,
        damage: 0,
        activesound: SfxNum(0),
        flags: 0,
        raisestate: S_NULL,
    }
}

impl<'a> Builder<'a> {
    fn new(info: &'a mut Info) -> Self {
        let mut classes = HashMap::new();
        for (i, n) in info.class_names.iter().enumerate() {
            if !n.is_empty() {
                classes.insert(n.clone(), MobjType(i as u16));
            }
        }
        // ZDoom's player class names.
        classes.insert("playerpawn".to_string(), MT_PLAYER);
        Builder { info, classes, warnings: Vec::new(), ignored: BTreeSet::new(), method_names: Vec::new() }
    }

    fn ignore(&mut self, what: String) {
        self.ignored.insert(what);
    }

    fn zscript_class(&mut self, c: &ast::ClassDecl) {
        let defaults: Vec<DefaultItem> = c.members.iter().filter_map(|m| if let ast::Member::Default(d) = m { Some(d.clone()) } else { None }).flatten().collect();
        let states: Vec<StatesDecl> = c.members.iter().filter_map(|m| if let ast::Member::States(s) = m { Some(s.clone()) } else { None }).collect();
        // Methods with bodies run in crate::actions; natives are the engine's.
        let methods: Vec<Arc<ast::FuncDecl>> = c
            .members
            .iter()
            .filter_map(|m| if let ast::Member::Function(f) = m { Some(f) } else { None })
            .filter(|f| f.body.is_some())
            .map(|f| Arc::new(f.clone()))
            .collect();
        // Fields are per-instance numbers (weapon fields through `invoker`).
        let fields: Vec<String> = c
            .members
            .iter()
            .filter_map(|m| if let ast::Member::Field(f) = m { Some(f) } else { None })
            .flat_map(|f| f.names.iter().map(|(n, _)| n.to_ascii_lowercase()))
            .collect();
        self.method_names = methods.iter().map(|f| f.name.to_ascii_lowercase()).collect();
        self.actor(&ActorSpec {
            name: &c.name,
            parent: c.parent.as_deref(),
            replaces: c.replaces.as_deref(),
            doomednum: None,
            defaults: &defaults,
            vars: &fields,
            states: &states,
            source: "ZScript",
            methods: &methods,
        });
        self.method_names.clear();
    }

    /// `extend class X`: more methods (and fields) for a class already
    /// built.
    fn zscript_extend(&mut self, c: &ast::ClassDecl) {
        let Some(&t) = self.classes.get(&c.name.to_ascii_lowercase()) else { return };
        for m in &c.members {
            match m {
                ast::Member::Function(f) if f.body.is_some() => {
                    self.info.class_methods[t.idx()].insert(f.name.to_ascii_lowercase(), Arc::new(f.clone()));
                }
                ast::Member::Field(f) => {
                    for (n, _) in &f.names {
                        let n = n.to_ascii_lowercase();
                        if !self.info.class_vars[t.idx()].contains(&n) {
                            self.info.class_vars[t.idx()].push(n);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn actor(&mut self, a: &ActorSpec) {
        let parent_name = a.parent.map(str::to_ascii_lowercase);
        let parent = parent_name.as_deref().and_then(|p| self.classes.get(p).copied());
        let mut mi = match parent {
            Some(p) => *self.info.mobj(p),
            None => {
                let mut mi = actor_defaults();
                match parent_name.as_deref() {
                    Some(p) if INVENTORY_BASES.contains(&p) => mi.flags |= MF_SPECIAL,
                    Some("fastprojectile") => mi.flags |= MF_NOBLOCKMAP | MF_NOGRAVITY | MF_DROPOFF | MF_MISSILE,
                    Some(p) if OTHER_BASES.contains(&p) => {}
                    Some(p) => self.warnings.push(format!("{} {}: unknown parent {p}; using Actor defaults", a.source, a.name)),
                    None => {}
                }
                mi
            }
        };
        mi.doomednum = -1;
        let mtype = self.info.add_type(mi, a.name, parent);
        for v in a.vars {
            if !self.info.class_vars[mtype.idx()].contains(v) {
                self.info.class_vars[mtype.idx()].push(v.clone());
            }
        }
        for f in a.methods {
            self.info.class_methods[mtype.idx()].insert(f.name.to_ascii_lowercase(), f.clone());
        }

        // Properties and flags, in order.
        let mut speed: Option<f64> = None;
        for item in a.defaults {
            match item {
                DefaultItem::Flag { on, name } => {
                    let lname = name.to_ascii_lowercase();
                    let short = lname.rsplit('.').next().unwrap_or(&lname);
                    // ZDoom flags without a vanilla bit (puff and weapon behaviour).
                    if let Some(&(_, bit)) = zflags::NAMES.iter().find(|(n, _)| *n == short) {
                        let f = &mut self.info.class_zflags[mtype.idx()];
                        if *on {
                            *f |= bit;
                        } else {
                            *f &= !bit;
                        }
                        continue;
                    }
                    match FLAGS.iter().find(|(n, _)| *n == short) {
                        Some(&(_, bit)) => {
                            if *on {
                                mi.flags |= bit;
                            } else {
                                mi.flags &= !bit;
                            }
                        }
                        None if QUIET_FLAGS.contains(&lname.as_str()) || QUIET_FLAGS.contains(&short) => {}
                        None => self.ignore(format!("flag {lname}")),
                    }
                }
                DefaultItem::Property { name, values } => {
                    let key = name.to_ascii_lowercase();
                    let num = values.last().and_then(const_num);
                    let sound = |b: &Builder| values.first().and_then(const_str).and_then(|s| b.info.sound(s)).unwrap_or(SfxNum(0));
                    match key.as_str() {
                        "monster" => mi.flags |= MF_SHOOTABLE | MF_COUNTKILL | MF_SOLID,
                        "projectile" => mi.flags |= MF_NOBLOCKMAP | MF_NOGRAVITY | MF_DROPOFF | MF_MISSILE,
                        "clearflags" => mi.flags = 0,
                        "skip_super" => mi = MobjInfo { doomednum: -1, ..actor_defaults() },
                        "radius" => mi.radius = Fixed((num.unwrap_or(20.0) * 65536.0) as i32),
                        "height" => mi.height = Fixed((num.unwrap_or(16.0) * 65536.0) as i32),
                        "health" => mi.spawnhealth = num.unwrap_or(1000.0) as i32,
                        "speed" => speed = num,
                        "vspeed" => self.info.class_vspeed[mtype.idx()] = num.map(|v| v as f32),
                        "mass" => mi.mass = num.unwrap_or(100.0) as i32,
                        "painchance" => mi.painchance = num.unwrap_or(0.0) as i32,
                        "damage" => mi.damage = num.unwrap_or(0.0) as i32,
                        "reactiontime" => mi.reactiontime = num.unwrap_or(8.0) as i32,
                        "seesound" => mi.seesound = sound(self),
                        "attacksound" => mi.attacksound = sound(self),
                        "painsound" => mi.painsound = sound(self),
                        "deathsound" => mi.deathsound = sound(self),
                        "activesound" => mi.activesound = sound(self),
                        // Presentation and texts: no gameplay effect here yet.
                        "obituary" | "hitobituary" | "tag" | "game" | "spawnid" | "conversationid" | "renderstyle" | "alpha" | "scale"
                        | "xscale" | "yscale" | "translation" | "bloodcolor" | "bloodtype" | "decal" | "inventory.pickupmessage"
                        | "inventory.pickupsound" | "inventory.icon" | "inventory.usesound" | "stencilcolor" | "distancecheck"
                        | "maxstepheight" | "maxdropoffheight" | "gravity" | "minmissilechance" | "meleerange" | "meleethreshold"
                        | "maxtargetrange" | "deathheight" | "burnheight" | "projectilepassheight" | "radiusdamagefactor"
                        | "selfdamagefactor" | "fastspeed" | "floatspeed" | "floatbobphase" | "wallbouncefactor" | "bouncefactor"
                        | "bouncecount" | "bouncetype" | "visibleangles" | "visiblepitch" | "renderradius" | "cameraheight" => {}
                        other if other.starts_with("weapon.") => {
                            // Bob, kickback and sister weapons are presentation
                            // or Heretic/Hexen features.
                            let quiet = matches!(
                                other,
                                "weapon.kickback" | "weapon.yadjust" | "weapon.bobstyle" | "weapon.bobspeed" | "weapon.bobrangex" | "weapon.bobrangey"
                                    | "weapon.upsound" | "weapon.readysound" | "weapon.minselectionammo1" | "weapon.minselectionammo2"
                            );
                            if !crate::weapons::KNOWN_PROPERTIES.contains(&other) && !quiet {
                                self.ignore(format!("property {other}"));
                            }
                        }
                        other => self.ignore(format!("property {other}")),
                    }
                }
            }
        }
        if let Some(v) = speed {
            // Missiles move in fixed point per tic; monsters in whole units.
            mi.speed = if mi.flags & MF_MISSILE != 0 { (v * 65536.0) as i32 } else { v as i32 };
        }

        // Weapon classes: `Weapon.*` properties, inherited.
        let props: Vec<(String, Option<String>, Option<f64>)> = a
            .defaults
            .iter()
            .filter_map(|d| match d {
                DefaultItem::Property { name, values } => {
                    Some((name.to_ascii_lowercase(), values.first().and_then(const_str).map(str::to_string), values.last().and_then(const_num)))
                }
                _ => None,
            })
            .collect();
        let parent_weapon = parent.and_then(|p| self.info.weapons.get(&p.0).cloned());
        let is_weapon = parent_weapon.is_some()
            || parent_name.as_deref().is_some_and(|p| matches!(p, "weapon" | "doomweapon"))
            || props.iter().any(|(n, ..)| n.starts_with("weapon."));
        if is_weapon {
            let d = crate::weapons::def_from_properties(parent_weapon.as_ref(), &props).unwrap_or_default();
            self.info.weapons.insert(mtype.0, d);
        }

        // States.
        let mut labels: HashMap<String, StateNum> = HashMap::new();
        for s in a.states {
            self.states(a.name, mtype, parent, &s.items, &mut labels);
        }
        let own = |l: &str, inherited: StateNum| labels.get(l).copied().unwrap_or(inherited);
        mi.spawnstate = own("spawn", mi.spawnstate);
        mi.seestate = own("see", mi.seestate);
        mi.painstate = own("pain", mi.painstate);
        mi.meleestate = own("melee", mi.meleestate);
        mi.missilestate = own("missile", mi.missilestate);
        mi.deathstate = own("death", mi.deathstate);
        mi.xdeathstate = own("xdeath", mi.xdeathstate);
        mi.raisestate = own("raise", mi.raisestate);
        if let Some(n) = a.doomednum.filter(|&n| n > 0) {
            // The last definition of an editor number wins.
            for m in self.info.mobjinfo.iter_mut().filter(|m| m.doomednum == n) {
                m.doomednum = -1;
            }
            mi.doomednum = n;
        }
        self.info.mobjinfo[mtype.idx()] = mi;
        self.info.class_labels[mtype.idx()] = labels;
        if let Some(r) = a.replaces {
            match self.classes.get(&r.to_ascii_lowercase()).copied() {
                Some(target) => self.replace(target, mtype, mi),
                None => self.warnings.push(format!("{} {}: replaces unknown actor {r}", a.source, a.name)),
            }
        }
        self.classes.insert(a.name.to_ascii_lowercase(), mtype);
    }

    /// `replaces`: the replaced type spawns (and, for Doom's weapons, is
    /// used as) the replacement: same behaviour, the old editor number.
    ///
    /// The replacement usually inherits from what it replaces
    /// (`MyPistol : Pistol replaces Pistol`), so the target keeps its own
    /// parent (no cycle) and gets the replacement's whole resolved label and
    /// method tables, including what it inherited from the original.
    fn replace(&mut self, target: MobjType, mtype: MobjType, mi: MobjInfo) {
        let info = &mut *self.info;
        let keep = info.mobjinfo[target.idx()].doomednum;
        info.mobjinfo[target.idx()] = MobjInfo { doomednum: keep, ..mi };
        let mut labels: HashMap<String, StateNum> = HashMap::new();
        let mut methods = HashMap::new();
        let mut c = Some(mtype);
        for _ in 0..64 {
            let Some(t) = c else { break };
            for (k, v) in &info.class_labels[t.idx()] {
                labels.entry(k.clone()).or_insert(*v);
            }
            for (k, v) in &info.class_methods[t.idx()] {
                methods.entry(k.clone()).or_insert_with(|| v.clone());
            }
            c = info.class_parents[t.idx()];
        }
        info.class_labels[target.idx()] = labels;
        info.class_methods[target.idx()] = methods;
        if !info.is_a(mtype, target) {
            info.class_parents[target.idx()] = Some(mtype);
        }
        info.class_vars[target.idx()] = info.class_vars[mtype.idx()].clone();
        info.class_zflags[target.idx()] = info.class_zflags[mtype.idx()];
        info.class_vspeed[target.idx()] = info.class_vspeed[mtype.idx()];
        if let Some(d) = info.weapons.get(&mtype.0).cloned() {
            info.weapons.insert(target.0, d);
        }
        if let Some(wpn) = info.vanilla_weapon_of(target) {
            info.vanilla_weapon_replaced[wpn as usize] = true;
        }
    }

    fn sprite(&mut self, name: &str) -> SpriteNum {
        let up = name.to_ascii_uppercase();
        match self.info.sprnames.iter().position(|s| *s == up) {
            Some(i) => SpriteNum(i as u16),
            None => {
                self.info.sprnames.push(up);
                SpriteNum((self.info.sprnames.len() - 1) as u16)
            }
        }
    }

    /// Build states from a `States` block: labels, frames (one state per
    /// frame letter), flow (`Stop`/`Loop`/`Wait`/`Goto`).
    fn states(&mut self, actor: &str, mtype: MobjType, parent: Option<MobjType>, items: &[StateItem], labels: &mut HashMap<String, StateNum>) {
        let mut pending: Vec<String> = Vec::new();
        let mut loop_start: Option<usize> = None;
        let mut gotos: Vec<(Option<usize>, Vec<String>, String, i32)> = Vec::new();
        let mut last: Option<usize> = None;
        for item in items {
            match item {
                StateItem::Label(l) => pending.push(l.to_ascii_lowercase()),
                StateItem::Stop | StateItem::Fail => {
                    if let Some(i) = last {
                        self.info.states[i].next = S_NULL;
                    }
                    for l in pending.drain(..) {
                        labels.insert(l, S_NULL);
                    }
                    last = None;
                    loop_start = None;
                }
                StateItem::Wait => {
                    if let Some(i) = last {
                        self.info.states[i].next = StateNum(i as u16);
                    }
                    last = None;
                    loop_start = None;
                }
                StateItem::Loop => {
                    if let (Some(i), Some(s)) = (last, loop_start) {
                        self.info.states[i].next = StateNum(s as u16);
                    }
                    last = None;
                    loop_start = None;
                }
                StateItem::Goto { target, offset } => {
                    gotos.push((last, std::mem::take(&mut pending), target.to_ascii_lowercase(), *offset));
                    last = None;
                    loop_start = None;
                }
                StateItem::Frame(f) => {
                    let keep_sprite = f.sprite == "####" || f.sprite == "----";
                    let spr = if keep_sprite { last.map_or(SpriteNum(0), |i| self.info.states[i].sprite) } else { self.sprite(&f.sprite) };
                    let (tics, random) = match f.tics {
                        Tics::Fixed(t) => (t, None),
                        Tics::Random(a, b) => (a, Some((a, b))),
                    };
                    let (action, call) = self.action(f.action.as_ref(), mtype);
                    for ch in f.frames.chars() {
                        let frame = if ch == '#' {
                            last.map_or(0, |i| self.info.states[i].frame & FF_FRAMEMASK)
                        } else {
                            (ch.to_ascii_uppercase() as i32 - 'A' as i32).clamp(0, 28)
                        };
                        let idx = self.info.states.len();
                        if let Some(prev) = last {
                            self.info.states[prev].next = StateNum(idx as u16);
                        }
                        self.info.states.push(State {
                            sprite: spr,
                            frame: frame | if f.bright { FF_FULLBRIGHT } else { 0 },
                            tics,
                            action,
                            next: S_NULL,
                            misc1: 0,
                            misc2: 0,
                        });
                        if let Some(c) = &call {
                            self.info.state_calls.insert(idx as u16, c.clone());
                        }
                        if let Some(r) = random {
                            self.info.state_random_tics.insert(idx as u16, r);
                        }
                        let labelled = !pending.is_empty();
                        for l in pending.drain(..) {
                            labels.insert(l, StateNum(idx as u16));
                        }
                        if labelled || loop_start.is_none() {
                            loop_start = Some(idx);
                        }
                        last = Some(idx);
                    }
                }
            }
        }
        // A trailing label without frames points at nothing.
        for l in pending.drain(..) {
            labels.insert(l, S_NULL);
        }
        // Resolve gotos now that every label of the block is known.
        for (from, label_names, key, offset) in gotos {
            let target = if let Some((class, l)) = key.split_once("::") {
                let owner = if class == "super" { parent } else { self.classes.get(class).copied() };
                owner.and_then(|o| self.info.find_label(o, l))
            } else {
                labels.get(&key).copied().or_else(|| parent.and_then(|p| self.info.find_label(p, &key)))
            };
            let Some(t) = target else {
                self.warnings.push(format!("{actor}: goto {key}: unknown label"));
                continue;
            };
            let t = StateNum((t.0 as i32 + offset) as u16);
            if let Some(i) = from {
                self.info.states[i].next = t;
            }
            for l in label_names {
                labels.insert(l, t);
            }
        }
    }

    /// A state's action: a vanilla action (no arguments), or a call kept
    /// for `crate::actions`.
    fn action(&mut self, a: Option<&StateAction>, owner: MobjType) -> (Action, Option<Arc<ActionCall>>) {
        match a {
            None => (Action::None, None),
            Some(StateAction::Call { name, args }) => {
                if args.is_empty() {
                    if let Some(v) = Action::ALL.iter().copied().find(|x| x.name().eq_ignore_ascii_case(name)) {
                        return (v, None);
                    }
                }
                let lname = name.to_ascii_lowercase();
                let method = self.method_names.contains(&lname) || self.info.find_method(owner, &lname).is_some();
                if !crate::actions::is_supported(&lname) && !method {
                    self.ignore(format!("action {lname}"));
                }
                (Action::None, Some(Arc::new(ActionCall { name: lname, args: args.clone(), block: None, owner })))
            }
            Some(StateAction::Block(b)) => (Action::None, Some(Arc::new(ActionCall { name: String::new(), args: Vec::new(), block: Some(b.clone()), owner }))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(decorate: &str, zscript: &str) -> (Info, Vec<String>) {
        let mut info = Info::doom();
        let mut b = Builder::new(&mut info);
        let (z, _) = parse(zscript);
        for d in &z.decls {
            if let ast::Decl::Class(c) = d {
                b.zscript_class(c);
            }
        }
        let (f, errors) = parse_decorate(decorate);
        assert!(errors.is_empty(), "{errors:?}");
        for a in &f.actors {
            b.actor(&ActorSpec {
                name: &a.name,
                parent: a.parent.as_deref(),
                replaces: a.replaces.as_deref(),
                doomednum: a.doomednum.map(|n| n as i32),
                defaults: &a.defaults,
                vars: &a.vars,
                states: &a.states,
                source: "DECORATE",
                methods: &[],
            });
        }
        let w = b.warnings.clone();
        (info, w)
    }

    #[test]
    fn decorations_and_inheritance() {
        let base_states = Info::doom().states.len();
        let (info, warnings) = build(
            "ACTOR Shrubbery01 21006\n{\n  Radius 8\n  Height 16\n  +NOBLOCKMAP\n  +MOVEWITHSECTOR\n  States\n  {\n  Spawn:\n    BSH2 A -1\n    Stop\n  }\n}\n\
             ACTOR Shrubbery02 : Shrubbery01 21007\n{\n  States\n  {\n  Spawn:\n    BSH3 AB 5 Bright\n    Loop\n  }\n}\n\
             actor FastImp : DoomImp replaces DoomImp { Speed 16 States { See: TROO AABB 2 A_Chase Loop } }",
            "",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let t = info.type_for_doomednum(21006).unwrap();
        let m = *info.mobj(t);
        assert_eq!(m.radius, Fixed::from_int(8));
        assert_eq!(m.flags & MF_NOBLOCKMAP, MF_NOBLOCKMAP);
        let s = info.state(m.spawnstate);
        assert_eq!(info.sprnames[s.sprite.idx()], "BSH2");
        assert_eq!((s.tics, s.next), (-1, StateNum(0)));
        // Inherited radius, own looping two-frame bright spawn.
        let t2 = info.type_for_doomednum(21007).unwrap();
        let m2 = *info.mobj(t2);
        assert_eq!(m2.radius, Fixed::from_int(8));
        let a = m2.spawnstate;
        let b = info.state(a).next;
        assert_eq!(info.state(b).next, a);
        assert_eq!(info.state(a).frame, FF_FULLBRIGHT);
        assert!(info.is_a(t2, t));
        // Replacement keeps the imp's editor number, takes the new states.
        let imp = info.type_for_doomednum(3001).unwrap();
        assert_eq!(imp, MT_TROOP);
        let see = info.mobj(MT_TROOP).seestate;
        assert!(see.idx() >= base_states);
        assert_eq!(info.state(see).action, Action::Chase);
        assert_eq!(info.mobj(MT_TROOP).speed, 16);
        assert_eq!(info.mobj(MT_TROOP).spawnstate, S_TROO_STND);
    }

    #[test]
    fn zscript_actors_calls_and_labels() {
        let (info, warnings) = build(
            "actor ZChild : ZImp 30002 { States { Spawn: TROO A 1 A_Jump(256, \"Super::Spawn\") Loop } }",
            r#"
            class ZImp : DoomImp
            {
                Default { Health 77; +NOGRAVITY }
                States
                {
                Spawn:
                    TROO AB 10 A_Look;
                    Loop;
                Rage:
                    TROO E 4 { A_FaceTarget(); }
                    TROO F random(2, 6) A_SpawnItemEx("DoomImpBall", 0, 0, 32);
                    Goto See+1;
                }
            }
        "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let z = info.type_by_name("zimp").unwrap();
        assert_eq!(info.mobj(z).spawnhealth, 77);
        assert!(info.mobj(z).flags & MF_NOGRAVITY != 0);
        assert!(info.mobj(z).flags & MF_SHOOTABLE != 0, "inherited from DoomImp");
        let rage = info.find_label(z, "rage").unwrap();
        assert!(info.state_calls[&rage.0].block.is_some());
        let spawn_item = info.state(rage).next;
        let call = &info.state_calls[&spawn_item.0];
        assert_eq!((call.name.as_str(), call.args.len(), call.owner), ("a_spawnitemex", 4, z));
        assert_eq!(info.state_random_tics[&spawn_item.0], (2, 6));
        // Goto See+1: DoomImp's see state, plus one.
        assert_eq!(info.state(spawn_item).next.0, S_TROO_RUN1.0 + 1);
        // DECORATE child of a ZScript class; Super:: resolves to ZImp.
        let child = info.type_by_name("zchild").unwrap();
        assert_eq!(info.type_for_doomednum(30002), Some(child));
        assert_eq!(info.find_label(child, "super::spawn"), info.find_label(z, "spawn"));
        assert_eq!(info.find_label(child, "rage"), Some(rage), "inherited label");
    }
}
