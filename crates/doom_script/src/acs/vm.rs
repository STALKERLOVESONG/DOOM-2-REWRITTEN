//! The ACS virtual machine (ZDoom `DLevelScript` / `DACSThinker`).
//!
//! An [`AcsRuntime`] holds the map's module and its libraries, the
//! variables, and the running scripts ("threads"). The engine calls
//! [`AcsRuntime::tick`] once per tic from the playsim; each thread runs until
//! it delays, waits, suspends or ends. Threads started during a tic first run
//! on the next one, except `ACS_ExecuteWithResult`, which runs at once.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::host::{AcsHost, Activation, Builtin, PrintKind, Strings};
use super::module::{Format, Module, ScriptKind, NUM_MAPVARS};
use super::pcode::*;

/// A script by number or by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptId {
    Number(i32),
    Name(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// `ACS_Execute`: start unless running (resumes a suspended script).
    Execute,
    /// `ACS_ExecuteAlways`: start another instance even if running.
    ExecuteAlways,
    Suspend,
    Terminate,
}

/// A script start/stop asked for by the engine (a line, a thing's special).
#[derive(Clone, Debug)]
pub struct ScriptRequest {
    pub script: ScriptId,
    /// Map number the script belongs to; 0 = the current map.
    pub map: i32,
    pub args: [i32; 4],
    pub act: Activation,
    pub kind: RequestKind,
}

/// Variables that outlive a map: world (hub) and global scope, and starts
/// deferred to other maps.
#[derive(Clone, Debug)]
pub struct AcsGlobals {
    pub world_vars: Vec<i32>,
    pub global_vars: Vec<i32>,
    pub world_arrays: Vec<HashMap<i32, i32>>,
    pub global_arrays: Vec<HashMap<i32, i32>>,
    /// (map number, request) for `ACS_Execute` on another map.
    pub deferred: Vec<(i32, ScriptRequest)>,
}

impl Default for AcsGlobals {
    fn default() -> Self {
        AcsGlobals {
            world_vars: vec![0; 256],
            global_vars: vec![0; 64],
            world_arrays: vec![HashMap::new(); 256],
            global_arrays: vec![HashMap::new(); 64],
            deferred: Vec::new(),
        }
    }
}

/// Instructions one script may run in one tic before it is stopped as a
/// runaway (ZDoom's limit).
pub const RUNAWAY_LIMIT: u32 = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Running,
    Delayed(i32),
    TagWait(i32),
    PolyWait(i32),
    /// Waiting for a script (module, number) to finish.
    ScriptWait(usize, i32),
    Suspended,
    Done,
}

#[derive(Debug)]
struct Frame {
    module: usize,
    ret_pc: usize,
    locals_base: usize,
    arrays: Vec<Vec<i32>>,
    discard: bool,
}

#[derive(Debug)]
struct Thread {
    module: usize,
    number: i32,
    start: usize,
    pc: usize,
    stack: Vec<i32>,
    locals: Vec<i32>,
    locals_base: usize,
    arrays: Vec<Vec<i32>>,
    frames: Vec<Frame>,
    state: State,
    act: Activation,
    print: String,
    hud_opt: Option<usize>,
    result: i32,
}

/// The compiled code of one module, shared so threads can hold it while the
/// runtime is mutated.
#[derive(Clone)]
struct Code {
    data: Arc<[u8]>,
    format: Format,
}

impl Code {
    fn u8(&self, pc: &mut usize) -> Option<i32> {
        let v = *self.data.get(*pc)?;
        *pc += 1;
        Some(v as i32)
    }
    fn u16(&self, pc: &mut usize) -> Option<i32> {
        let b = self.data.get(*pc..*pc + 2)?;
        *pc += 2;
        Some(u16::from_le_bytes([b[0], b[1]]) as i32)
    }
    fn word(&self, pc: &mut usize) -> Option<i32> {
        let b = self.data.get(*pc..*pc + 4)?;
        *pc += 4;
        Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// `NEXTBYTE`: one byte in compressed code, else a word.
    fn byte_op(&self, pc: &mut usize) -> Option<i32> {
        if self.format == Format::LittleEnhanced {
            self.u8(pc)
        } else {
            self.word(pc)
        }
    }
    /// `NEXTSHORT`.
    fn short_op(&self, pc: &mut usize) -> Option<i32> {
        if self.format == Format::LittleEnhanced {
            self.u16(pc)
        } else {
            self.word(pc)
        }
    }
    fn pcode(&self, pc: &mut usize) -> Option<u32> {
        if self.format == Format::LittleEnhanced {
            let b = self.u8(pc)? as u32;
            if b >= 240 {
                let lo = self.u8(pc)? as u32;
                return Some(240 + ((b - 240) << 8) + lo);
            }
            Some(b)
        } else {
            self.word(pc).map(|w| w as u32)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Script,
    Map,
    World,
    Global,
    ScriptArray,
    MapArray,
    WorldArray,
    GlobalArray,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VarOp {
    Assign,
    Push,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Inc,
    Dec,
    And,
    Eor,
    Or,
    Shl,
    Shr,
}

/// Variable instructions as (scope, operation).
fn var_op(p: u32) -> Option<(Scope, VarOp)> {
    use Scope::*;
    use VarOp::*;
    let t = |base: u32, ops: [VarOp; 1]| (p >= base && p < base + 3).then(|| ([Script, Map, World][(p - base) as usize], ops[0]));
    if let Some(r) = t(ASSIGNSCRIPTVAR, [Assign])
        .or_else(|| t(PUSHSCRIPTVAR, [Push]))
        .or_else(|| t(ADDSCRIPTVAR, [Add]))
        .or_else(|| t(SUBSCRIPTVAR, [Sub]))
        .or_else(|| t(MULSCRIPTVAR, [Mul]))
        .or_else(|| t(DIVSCRIPTVAR, [Div]))
        .or_else(|| t(MODSCRIPTVAR, [Mod]))
        .or_else(|| t(INCSCRIPTVAR, [Inc]))
        .or_else(|| t(DECSCRIPTVAR, [Dec]))
    {
        return Some(r);
    }
    Some(match p {
        ASSIGNGLOBALVAR => (Global, Assign),
        PUSHGLOBALVAR => (Global, Push),
        ADDGLOBALVAR => (Global, Add),
        SUBGLOBALVAR => (Global, Sub),
        MULGLOBALVAR => (Global, Mul),
        DIVGLOBALVAR => (Global, Div),
        MODGLOBALVAR => (Global, Mod),
        INCGLOBALVAR => (Global, Inc),
        DECGLOBALVAR => (Global, Dec),
        PUSHMAPARRAY => (MapArray, Push),
        ASSIGNMAPARRAY => (MapArray, Assign),
        ADDMAPARRAY => (MapArray, Add),
        SUBMAPARRAY => (MapArray, Sub),
        MULMAPARRAY => (MapArray, Mul),
        DIVMAPARRAY => (MapArray, Div),
        MODMAPARRAY => (MapArray, Mod),
        INCMAPARRAY => (MapArray, Inc),
        DECMAPARRAY => (MapArray, Dec),
        PUSHWORLDARRAY => (WorldArray, Push),
        ASSIGNWORLDARRAY => (WorldArray, Assign),
        ADDWORLDARRAY => (WorldArray, Add),
        SUBWORLDARRAY => (WorldArray, Sub),
        MULWORLDARRAY => (WorldArray, Mul),
        DIVWORLDARRAY => (WorldArray, Div),
        MODWORLDARRAY => (WorldArray, Mod),
        INCWORLDARRAY => (WorldArray, Inc),
        DECWORLDARRAY => (WorldArray, Dec),
        PUSHGLOBALARRAY => (GlobalArray, Push),
        ASSIGNGLOBALARRAY => (GlobalArray, Assign),
        ADDGLOBALARRAY => (GlobalArray, Add),
        SUBGLOBALARRAY => (GlobalArray, Sub),
        MULGLOBALARRAY => (GlobalArray, Mul),
        DIVGLOBALARRAY => (GlobalArray, Div),
        MODGLOBALARRAY => (GlobalArray, Mod),
        INCGLOBALARRAY => (GlobalArray, Inc),
        DECGLOBALARRAY => (GlobalArray, Dec),
        ASSIGNSCRIPTARRAY => (ScriptArray, Assign),
        PUSHSCRIPTARRAY => (ScriptArray, Push),
        ADDSCRIPTARRAY => (ScriptArray, Add),
        SUBSCRIPTARRAY => (ScriptArray, Sub),
        MULSCRIPTARRAY => (ScriptArray, Mul),
        DIVSCRIPTARRAY => (ScriptArray, Div),
        MODSCRIPTARRAY => (ScriptArray, Mod),
        INCSCRIPTARRAY => (ScriptArray, Inc),
        DECSCRIPTARRAY => (ScriptArray, Dec),
        ANDSCRIPTARRAY => (ScriptArray, And),
        EORSCRIPTARRAY => (ScriptArray, Eor),
        ORSCRIPTARRAY => (ScriptArray, Or),
        LSSCRIPTARRAY => (ScriptArray, Shl),
        RSSCRIPTARRAY => (ScriptArray, Shr),
        // And/Eor/Or/Ls/Rs on scalars and arrays: 7 scopes each.
        ANDSCRIPTVAR..=RSGLOBALARRAY => {
            let k = p - ANDSCRIPTVAR;
            let op = [And, Eor, Or, Shl, Shr][(k / 7) as usize];
            let scope = [Script, Map, World, Global, MapArray, WorldArray, GlobalArray][(k % 7) as usize];
            (scope, op)
        }
        _ => return None,
    })
}

pub struct AcsRuntime {
    pub modules: Vec<Module>,
    code: Vec<Code>,
    pub strings: Strings,
    map_vars: Vec<i32>,
    var_slots: Vec<[usize; NUM_MAPVARS]>,
    arrays: Vec<Vec<i32>>,
    array_slots: Vec<Vec<usize>>,
    /// Per module and function: where its body is (imports resolved).
    func_targets: Vec<Vec<Option<(usize, usize)>>>,
    threads: Vec<Thread>,
    pub globals: AcsGlobals,
    /// Load and link problems, and runtime errors (division by zero,
    /// runaway scripts...).
    pub warnings: Vec<String>,
    /// Instructions and extension functions met but not implemented, with
    /// counts.
    pub unimplemented: BTreeMap<String, u64>,
    /// The current map's number, for `ACS_Execute(script, map)`.
    pub map_number: i32,
}

impl std::fmt::Debug for AcsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcsRuntime").field("modules", &self.modules.len()).field("threads", &self.threads.len()).finish()
    }
}

fn arith(op: VarOp, a: i32, b: i32) -> Option<i32> {
    Some(match op {
        VarOp::Add => a.wrapping_add(b),
        VarOp::Sub => a.wrapping_sub(b),
        VarOp::Mul => a.wrapping_mul(b),
        VarOp::Div => a.checked_div(b)?,
        VarOp::Mod => a.checked_rem(b)?,
        VarOp::And => a & b,
        VarOp::Eor => a ^ b,
        VarOp::Or => a | b,
        VarOp::Shl => a.wrapping_shl(b as u32),
        VarOp::Shr => a.wrapping_shr(b as u32),
        VarOp::Inc => a.wrapping_add(1),
        VarOp::Dec => a.wrapping_sub(1),
        VarOp::Assign => b,
        VarOp::Push => a,
    })
}

impl AcsRuntime {
    /// Link a map's module (if any) with its libraries. `libraries` are the
    /// `LOADACS` modules plus everything named by `LOAD` chunks; `find`
    /// loads a library by lump name.
    pub fn new(map: Option<Module>, loadacs: Vec<Module>, mut find: impl FnMut(&str) -> Option<Module>, globals: AcsGlobals) -> AcsRuntime {
        let mut warnings = Vec::new();
        let mut modules = vec![map.unwrap_or_else(|| Module::empty("map"))];
        modules.extend(loadacs);
        // Pull in libraries named by LOAD chunks (transitively).
        let mut i = 0;
        while i < modules.len() {
            for lib in modules[i].libraries.clone() {
                if modules.iter().any(|m| m.name.eq_ignore_ascii_case(&lib)) {
                    continue;
                }
                match find(&lib) {
                    Some(m) => modules.push(m),
                    None => warnings.push(format!("{}: library {lib} not found", modules[i].name)),
                }
            }
            i += 1;
        }
        let mut rt = AcsRuntime {
            code: modules.iter().map(|m| Code { data: Arc::from(m.data.as_slice()), format: m.format }).collect(),
            strings: Strings { tables: modules.iter().map(|m| m.strings.clone()).collect(), dynamic: Vec::new() },
            map_vars: Vec::new(),
            var_slots: Vec::new(),
            arrays: Vec::new(),
            array_slots: Vec::new(),
            func_targets: Vec::new(),
            threads: Vec::new(),
            globals,
            warnings,
            unimplemented: BTreeMap::new(),
            map_number: 0,
            modules,
        };
        rt.link();
        rt
    }

    fn link(&mut self) {
        for (mi, m) in self.modules.iter().enumerate() {
            let mut slots = [0usize; NUM_MAPVARS];
            for (v, slot) in slots.iter_mut().enumerate() {
                let mut value = m.map_vars[v];
                if mi > 0 && m.string_vars.contains(&(v as u32)) {
                    value |= (mi as i32) << 16;
                }
                *slot = self.map_vars.len();
                self.map_vars.push(value);
            }
            self.var_slots.push(slots);
            let mut aslots = Vec::new();
            for (a, init) in m.arrays.iter().enumerate() {
                let mut init = init.clone();
                if mi > 0 && m.string_arrays.contains(&(a as u32)) {
                    for v in &mut init {
                        *v |= (mi as i32) << 16;
                    }
                }
                aslots.push(self.arrays.len());
                self.arrays.push(init);
            }
            self.array_slots.push(aslots);
        }
        // Imports by exported name.
        for mi in 0..self.modules.len() {
            let find_export = |name: &str| {
                self.modules.iter().enumerate().find_map(|(ei, e)| {
                    (ei != mi).then(|| e.exports.iter().position(|x| x.eq_ignore_ascii_case(name)).map(|i| (ei, i))).flatten()
                })
            };
            for (var, name) in self.modules[mi].var_imports.clone() {
                match find_export(&name) {
                    Some((ei, i)) if (var as usize) < NUM_MAPVARS && i < NUM_MAPVARS => self.var_slots[mi][var as usize] = self.var_slots[ei][i],
                    _ => self.warnings.push(format!("{}: imported variable {name} not found", self.modules[mi].name)),
                }
            }
            for (arr, _, name) in self.modules[mi].array_imports.clone() {
                match find_export(&name).and_then(|(ei, i)| self.array_slots[ei].get(i).copied()) {
                    Some(slot) => {
                        let a = &mut self.array_slots[mi];
                        if a.len() <= arr as usize {
                            a.resize(arr as usize + 1, usize::MAX);
                        }
                        a[arr as usize] = slot;
                    }
                    None => self.warnings.push(format!("{}: imported array {name} not found", self.modules[mi].name)),
                }
            }
        }
        for (mi, m) in self.modules.iter().enumerate() {
            let targets = m
                .functions
                .iter()
                .enumerate()
                .map(|(fi, f)| {
                    if f.import == 0 {
                        return Some((mi, fi));
                    }
                    let name = f.name.as_deref()?;
                    self.modules.iter().enumerate().skip(1).find_map(|(li, l)| l.function_by_name(name).map(|i| (li, i)))
                })
                .collect::<Vec<_>>();
            for (fi, t) in targets.iter().enumerate() {
                if t.is_none() {
                    self.warnings.push(format!("{}: function {:?} not found", m.name, m.functions[fi].name));
                }
            }
            self.func_targets.push(targets);
        }
    }

    // ---- starting and stopping ------------------------------------------------

    fn find_script(&self, id: &ScriptId) -> Option<(usize, usize)> {
        self.modules.iter().enumerate().find_map(|(mi, m)| {
            let i = match id {
                ScriptId::Number(n) => m.scripts.iter().position(|s| s.number == *n),
                ScriptId::Name(name) => m.scripts.iter().position(|s| s.name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(name))),
            }?;
            Some((mi, i))
        })
    }

    fn is_running(&self, module: usize, number: i32) -> bool {
        self.threads.iter().any(|t| t.module == module && t.number == number && t.state != State::Done)
    }

    /// Start (or resume, suspend, stop) a script. Returns whether a script
    /// was started.
    pub fn request(&mut self, r: &ScriptRequest) -> bool {
        if r.map != 0 && r.map != self.map_number {
            if matches!(r.kind, RequestKind::Execute | RequestKind::ExecuteAlways) {
                self.globals.deferred.push((r.map, r.clone()));
            }
            return false;
        }
        let Some((mi, si)) = self.find_script(&r.script) else {
            self.warnings.push(format!("script {:?} not found", r.script));
            return false;
        };
        let number = self.modules[mi].scripts[si].number;
        match r.kind {
            RequestKind::Suspend => {
                for t in self.threads.iter_mut().filter(|t| t.module == mi && t.number == number && t.state != State::Done) {
                    t.state = State::Suspended;
                }
                false
            }
            RequestKind::Terminate => {
                for t in self.threads.iter_mut().filter(|t| t.module == mi && t.number == number) {
                    t.state = State::Done;
                }
                false
            }
            RequestKind::Execute | RequestKind::ExecuteAlways => {
                if r.kind == RequestKind::Execute {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.module == mi && t.number == number && t.state != State::Done) {
                        if t.state == State::Suspended {
                            t.state = State::Running;
                        }
                        return false;
                    }
                }
                self.spawn_thread(mi, si, &r.args, r.act);
                true
            }
        }
    }

    fn spawn_thread(&mut self, mi: usize, si: usize, args: &[i32], act: Activation) -> usize {
        let s = &self.modules[mi].scripts[si];
        let mut locals = vec![0; s.var_count.max(s.arg_count).max(1) as usize];
        for (l, a) in locals.iter_mut().zip(args.iter().take(s.arg_count as usize)) {
            *l = *a;
        }
        self.threads.push(Thread {
            module: mi,
            number: s.number,
            start: s.address as usize,
            pc: s.address as usize,
            stack: Vec::with_capacity(16),
            locals,
            locals_base: 0,
            arrays: s.arrays.iter().map(|&n| vec![0; n.min(1 << 16) as usize]).collect(),
            frames: Vec::new(),
            state: State::Running,
            act,
            print: String::new(),
            hud_opt: None,
            result: 0,
        });
        self.threads.len() - 1
    }

    /// Start every script of a kind (OPEN at level start, ENTER for a
    /// player, ...).
    pub fn start_kind(&mut self, kind: ScriptKind, act: Activation, args: &[i32]) {
        let starts: Vec<(usize, usize)> = self
            .modules
            .iter()
            .enumerate()
            .flat_map(|(mi, m)| m.scripts.iter().enumerate().filter(|(_, s)| s.kind == kind).map(move |(si, _)| (mi, si)))
            .collect();
        for (mi, si) in starts {
            self.spawn_thread(mi, si, args, act);
        }
    }

    /// Run deferred starts recorded for this map (`ACS_Execute` aimed at it
    /// from another map).
    pub fn run_deferred(&mut self) {
        let map = self.map_number;
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.globals.deferred).into_iter().partition(|(m, _)| *m == map);
        self.globals.deferred = rest;
        for (_, mut r) in mine {
            r.map = 0;
            self.request(&r);
        }
    }

    pub fn running_scripts(&self) -> usize {
        self.threads.iter().filter(|t| t.state != State::Done).count()
    }

    // ---- running -----------------------------------------------------------

    /// One tic: wake waiting threads and run everything runnable.
    pub fn tick(&mut self, host: &mut dyn AcsHost) {
        for r in host.take_requests() {
            self.request(&r);
        }
        let n = self.threads.len();
        for i in 0..n {
            let ready = match self.threads[i].state {
                State::Running => true,
                State::Delayed(d) => {
                    if d > 1 {
                        self.threads[i].state = State::Delayed(d - 1);
                        false
                    } else {
                        true
                    }
                }
                State::TagWait(tag) => !host.tag_busy(tag),
                State::PolyWait(po) => !host.poly_busy(po),
                State::ScriptWait(m, num) => !self.is_running(m, num),
                State::Suspended | State::Done => false,
            };
            if ready {
                self.threads[i].state = State::Running;
                self.run_thread(i, host);
            }
        }
        self.threads.retain(|t| t.state != State::Done);
    }

    fn run_thread(&mut self, i: usize, host: &mut dyn AcsHost) {
        let mut t = std::mem::replace(&mut self.threads[i], Thread::placeholder());
        self.exec(&mut t, host);
        self.threads[i] = t;
    }

    fn note(&mut self, what: String) {
        *self.unimplemented.entry(what).or_default() += 1;
    }

    fn fail(&mut self, t: &mut Thread, why: &str) {
        let m = &self.modules[t.module].name;
        self.warnings.push(format!("{m}: script {} terminated: {why}", t.number));
        t.state = State::Done;
    }

    /// A string operand embedded in this module's code.
    fn tag(module: usize, v: i32) -> i32 {
        ((module as i32) << 16) | (v & 0xffff)
    }

    fn get_var<'a>(&'a mut self, t: &'a mut Thread, scope: Scope, idx: i32, elem: i32) -> Option<&'a mut i32> {
        let idx = idx as usize;
        Some(match scope {
            Scope::Script => t.locals.get_mut(t.locals_base + idx)?,
            Scope::Map => {
                let slot = *self.var_slots[t.module].get(idx)?;
                &mut self.map_vars[slot]
            }
            Scope::World => self.globals.world_vars.get_mut(idx)?,
            Scope::Global => self.globals.global_vars.get_mut(idx)?,
            Scope::ScriptArray => t.arrays.get_mut(idx)?.get_mut(elem as usize)?,
            Scope::MapArray => {
                let slot = *self.array_slots[t.module].get(idx)?;
                self.arrays.get_mut(slot)?.get_mut(elem as usize)?
            }
            Scope::WorldArray => self.globals.world_arrays.get_mut(idx)?.entry(elem).or_insert(0),
            Scope::GlobalArray => self.globals.global_arrays.get_mut(idx)?.entry(elem).or_insert(0),
        })
    }

    fn print_value(s: &mut String, p: u32, v: i32, strings: &Strings) {
        match p {
            PRINTSTRING => s.push_str(strings.get(v)),
            PRINTNUMBER => s.push_str(&v.to_string()),
            PRINTCHARACTER => s.push(char::from_u32(v as u32 & 0xff).unwrap_or('?')),
            PRINTFIXED => {
                let f = v as f64 / 65536.0;
                let txt = format!("{f:.5}");
                let txt = txt.trim_end_matches('0').trim_end_matches('.');
                s.push_str(txt);
            }
            PRINTBINARY => s.push_str(&format!("{:b}", v as u32)),
            PRINTHEX => s.push_str(&format!("{:X}", v as u32)),
            _ => {}
        }
    }

    /// Run a thread until it yields.
    fn exec(&mut self, t: &mut Thread, host: &mut dyn AcsHost) {
        let mut code = self.code[t.module].clone();
        let mut budget = RUNAWAY_LIMIT;
        macro_rules! pop {
            () => {
                match t.stack.pop() {
                    Some(v) => v,
                    None => return self.fail(t, "stack underflow"),
                }
            };
        }
        macro_rules! operand {
            ($e:expr) => {
                match $e {
                    Some(v) => v,
                    None => return self.fail(t, "truncated instruction"),
                }
            };
        }
        macro_rules! pop_n {
            ($n:expr) => {{
                let n = $n;
                if t.stack.len() < n {
                    return self.fail(t, "stack underflow");
                }
                t.stack.split_off(t.stack.len() - n)
            }};
        }
        macro_rules! builtin {
            ($f:expr, $args:expr) => {{
                let args: Vec<i32> = $args;
                host.builtin(&mut t.act, $f, &args, &mut self.strings)
            }};
        }
        loop {
            if budget == 0 {
                return self.fail(t, "runaway script (too many instructions in one tic)");
            }
            budget -= 1;
            let at = t.pc;
            let Some(p) = code.pcode(&mut t.pc) else { return self.fail(t, "ran off the end of the code") };
            if let Some((scope, op)) = var_op(p) {
                let idx = operand!(code.byte_op(&mut t.pc));
                let is_array = matches!(scope, Scope::ScriptArray | Scope::MapArray | Scope::WorldArray | Scope::GlobalArray);
                let value = match op {
                    VarOp::Push | VarOp::Inc | VarOp::Dec => 0,
                    _ => pop!(),
                };
                let elem = if is_array { pop!() } else { 0 };
                let Some(var) = self.get_var(t, scope, idx, elem) else { return self.fail(t, "variable index out of range") };
                if op == VarOp::Push {
                    let v = *var;
                    t.stack.push(v);
                    continue;
                }
                match arith(op, *var, value) {
                    Some(v) => *var = v,
                    None => return self.fail(t, "division by zero"),
                }
                continue;
            }
            match p {
                NOP => {}
                TERMINATE => {
                    t.state = State::Done;
                    return;
                }
                SUSPEND => {
                    t.state = State::Suspended;
                    return;
                }
                PUSHNUMBER => {
                    let v = operand!(code.word(&mut t.pc));
                    t.stack.push(v);
                }
                PUSHBYTE => {
                    let v = operand!(code.u8(&mut t.pc));
                    t.stack.push(v);
                }
                PUSHBYTES => {
                    let n = operand!(code.u8(&mut t.pc));
                    for _ in 0..n {
                        let v = operand!(code.u8(&mut t.pc));
                        t.stack.push(v);
                    }
                }
                PUSH2BYTES..=PUSH5BYTES => {
                    for _ in 0..(p - PUSH2BYTES + 2) {
                        let v = operand!(code.u8(&mut t.pc));
                        t.stack.push(v);
                    }
                }
                LSPEC1..=LSPEC5 | LSPEC5RESULT | LSPEC5EX | LSPEC5EXRESULT => {
                    let special = if matches!(p, LSPEC5EX | LSPEC5EXRESULT) { operand!(code.word(&mut t.pc)) } else { operand!(code.byte_op(&mut t.pc)) };
                    let n = match p {
                        LSPEC1..=LSPEC5 => (p - LSPEC1 + 1) as usize,
                        _ => 5,
                    };
                    let a = pop_n!(n);
                    let mut args = [0; 5];
                    args[..n].copy_from_slice(&a);
                    let r = match self.script_special(t, host, special, args) {
                        Some(r) => r,
                        None => return,
                    };
                    if matches!(p, LSPEC5RESULT | LSPEC5EXRESULT) {
                        t.stack.push(r);
                    }
                }
                LSPEC1DIRECT..=LSPEC5DIRECT | LSPEC1DIRECTB..=LSPEC5DIRECTB => {
                    let b = p >= LSPEC1DIRECTB;
                    let n = if b { p - LSPEC1DIRECTB + 1 } else { p - LSPEC1DIRECT + 1 } as usize;
                    let special = if b { operand!(code.u8(&mut t.pc)) } else { operand!(code.byte_op(&mut t.pc)) };
                    let mut args = [0; 5];
                    for a in args.iter_mut().take(n) {
                        *a = if b { operand!(code.u8(&mut t.pc)) } else { operand!(code.word(&mut t.pc)) };
                    }
                    if self.script_special(t, host, special, args).is_none() {
                        return;
                    }
                }
                ADD | SUBTRACT | MULTIPLY | DIVIDE | MODULUS | EQ | NE | LT | GT | LE | GE | ANDLOGICAL | ORLOGICAL | ANDBITWISE
                | ORBITWISE | EORBITWISE | LSHIFT | RSHIFT | FIXEDMUL | FIXEDDIV => {
                    let b = pop!();
                    let a = pop!();
                    let r = match p {
                        ADD => a.wrapping_add(b),
                        SUBTRACT => a.wrapping_sub(b),
                        MULTIPLY => a.wrapping_mul(b),
                        DIVIDE => match a.checked_div(b) {
                            Some(v) => v,
                            None => return self.fail(t, "division by zero"),
                        },
                        MODULUS => match a.checked_rem(b) {
                            Some(v) => v,
                            None => return self.fail(t, "modulus by zero"),
                        },
                        EQ => (a == b) as i32,
                        NE => (a != b) as i32,
                        LT => (a < b) as i32,
                        GT => (a > b) as i32,
                        LE => (a <= b) as i32,
                        GE => (a >= b) as i32,
                        ANDLOGICAL => (a != 0 && b != 0) as i32,
                        ORLOGICAL => (a != 0 || b != 0) as i32,
                        ANDBITWISE => a & b,
                        ORBITWISE => a | b,
                        EORBITWISE => a ^ b,
                        LSHIFT => a.wrapping_shl(b as u32),
                        RSHIFT => a.wrapping_shr(b as u32),
                        FIXEDMUL => ((a as i64 * b as i64) >> 16) as i32,
                        _ => {
                            if b == 0 {
                                return self.fail(t, "fixed division by zero");
                            }
                            (((a as i64) << 16) / b as i64) as i32
                        }
                    };
                    t.stack.push(r);
                }
                NEGATELOGICAL => {
                    let v = pop!();
                    t.stack.push((v == 0) as i32);
                }
                NEGATEBINARY => {
                    let v = pop!();
                    t.stack.push(!v);
                }
                UNARYMINUS => {
                    let v = pop!();
                    t.stack.push(v.wrapping_neg());
                }
                GOTO => {
                    let target = operand!(code.word(&mut t.pc));
                    t.pc = target as usize;
                }
                IFGOTO | IFNOTGOTO => {
                    let target = operand!(code.word(&mut t.pc));
                    let v = pop!();
                    if (v != 0) == (p == IFGOTO) {
                        t.pc = target as usize;
                    }
                }
                CASEGOTO => {
                    let value = operand!(code.word(&mut t.pc));
                    let target = operand!(code.word(&mut t.pc));
                    if t.stack.last() == Some(&value) {
                        t.stack.pop();
                        t.pc = target as usize;
                    }
                }
                CASEGOTOSORTED => {
                    t.pc = (t.pc + 3) & !3;
                    let n = operand!(code.word(&mut t.pc)) as usize;
                    let table = t.pc;
                    let top = *t.stack.last().unwrap_or(&0);
                    t.pc = table + n * 8;
                    let (mut lo, mut hi) = (0usize, n);
                    while lo < hi {
                        let mid = (lo + hi) / 2;
                        let mut q = table + mid * 8;
                        let v = operand!(code.word(&mut q));
                        if v == top {
                            let target = operand!(code.word(&mut q));
                            t.stack.pop();
                            t.pc = target as usize;
                            break;
                        } else if v < top {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                }
                GOTOSTACK => {
                    let j = pop!();
                    match self.modules[t.module].jump_targets.get(j as usize) {
                        Some(&target) => t.pc = target as usize,
                        None => return self.fail(t, "bad computed jump"),
                    }
                }
                DROP => {
                    pop!();
                }
                DUP => {
                    let v = *t.stack.last().unwrap_or(&0);
                    t.stack.push(v);
                }
                SWAP => {
                    let n = t.stack.len();
                    if n < 2 {
                        return self.fail(t, "stack underflow");
                    }
                    t.stack.swap(n - 1, n - 2);
                }
                DELAY | DELAYDIRECT | DELAYDIRECTB => {
                    let d = match p {
                        DELAY => pop!(),
                        DELAYDIRECT => operand!(code.word(&mut t.pc)),
                        _ => operand!(code.u8(&mut t.pc)),
                    };
                    if d > 0 {
                        t.state = State::Delayed(d);
                        return;
                    }
                }
                RANDOM | RANDOMDIRECT | RANDOMDIRECTB => {
                    let (lo, hi) = match p {
                        RANDOM => {
                            let hi = pop!();
                            (pop!(), hi)
                        }
                        RANDOMDIRECT => (operand!(code.word(&mut t.pc)), operand!(code.word(&mut t.pc))),
                        _ => (operand!(code.u8(&mut t.pc)), operand!(code.u8(&mut t.pc))),
                    };
                    let r = host.random(lo, hi);
                    t.stack.push(r);
                }
                TAGWAIT | TAGWAITDIRECT | POLYWAIT | POLYWAITDIRECT => {
                    let v = if matches!(p, TAGWAIT | POLYWAIT) { pop!() } else { operand!(code.word(&mut t.pc)) };
                    t.state = if matches!(p, TAGWAIT | TAGWAITDIRECT) { State::TagWait(v) } else { State::PolyWait(v) };
                    return;
                }
                SCRIPTWAIT | SCRIPTWAITDIRECT | SCRIPTWAITNAMED => {
                    let id = match p {
                        SCRIPTWAIT => ScriptId::Number(pop!()),
                        SCRIPTWAITDIRECT => ScriptId::Number(operand!(code.word(&mut t.pc))),
                        _ => ScriptId::Name(self.strings.get(pop!()).to_string()),
                    };
                    if let Some((mi, si)) = self.find_script(&id) {
                        let number = self.modules[mi].scripts[si].number;
                        if self.is_running(mi, number) {
                            t.state = State::ScriptWait(mi, number);
                            return;
                        }
                    }
                }
                RESTART => t.pc = t.start,
                PLAYERBLUESKULL..=PLAYERSILVERCARD => {
                    let r = builtin!(Builtin::PlayerKey, vec![(p - PLAYERBLUESKULL) as i32]);
                    t.stack.push(r);
                }
                TAGSTRING => {
                    let v = pop!();
                    t.stack.push(Self::tag(t.module, v));
                }
                SETRESULTVALUE => t.result = pop!(),
                LINESIDE => t.stack.push(t.act.side as i32),
                TIMER => t.stack.push(host.level_time()),
                SIN | COS => {
                    let a = pop!();
                    let angle = doom_core::Angle((a as u32) << 16);
                    let v = if p == SIN { doom_core::tables::sine(angle) } else { doom_core::tables::cosine(angle) };
                    t.stack.push(v.0);
                }
                VECTORANGLE => {
                    let y = pop!();
                    let x = pop!();
                    let a = doom_core::geom::point_to_angle(doom_core::Fixed(x), doom_core::Fixed(y));
                    t.stack.push((a.0 >> 16) as i32);
                }
                STRLEN => {
                    let s = pop!();
                    let n = self.strings.get(s).chars().count() as i32;
                    t.stack.push(n);
                }
                // ---- printing ----
                BEGINPRINT => t.print.clear(),
                PRINTSTRING | PRINTNUMBER | PRINTCHARACTER | PRINTFIXED | PRINTBINARY | PRINTHEX => {
                    let v = pop!();
                    Self::print_value(&mut t.print, p, v, &self.strings);
                }
                PRINTNAME => {
                    let v = pop!();
                    let name = host.player_name(&t.act, v);
                    t.print.push_str(&name);
                }
                PRINTLOCALIZED => {
                    let v = pop!();
                    let s = host.localize(self.strings.get(v));
                    t.print.push_str(&s);
                }
                PRINTBIND => {
                    pop!();
                    t.print.push_str("??");
                }
                ENDPRINT | ENDPRINTBOLD | ENDLOG => {
                    let kind = match p {
                        ENDPRINT => PrintKind::Print,
                        ENDPRINTBOLD => PrintKind::Bold,
                        _ => PrintKind::Log,
                    };
                    host.print(&t.act, kind, &t.print);
                }
                SAVESTRING => {
                    let s = std::mem::take(&mut t.print);
                    let v = self.strings.add(s);
                    t.stack.push(v);
                }
                MOREHUDMESSAGE => t.hud_opt = None,
                OPTHUDMESSAGE => t.hud_opt = Some(t.stack.len()),
                ENDHUDMESSAGE | ENDHUDMESSAGEBOLD => {
                    let opt = t.hud_opt.take().unwrap_or(t.stack.len());
                    if opt < 6 || opt > t.stack.len() {
                        return self.fail(t, "bad HudMessage arguments");
                    }
                    let vals = t.stack.split_off(opt - 6);
                    let mut args = [0; 9];
                    let count = vals.len().min(9);
                    args[..count].copy_from_slice(&vals[..count]);
                    host.print(&t.act, PrintKind::HudMessage { bold: p == ENDHUDMESSAGEBOLD, args, count }, &t.print);
                }
                PRINTMAPCHARARRAY | PRINTWORLDCHARARRAY | PRINTGLOBALCHARARRAY | PRINTSCRIPTCHARARRAY => {
                    let arr = pop!();
                    let offset = pop!();
                    let scope = match p {
                        PRINTMAPCHARARRAY => Scope::MapArray,
                        PRINTWORLDCHARARRAY => Scope::WorldArray,
                        PRINTGLOBALCHARARRAY => Scope::GlobalArray,
                        _ => Scope::ScriptArray,
                    };
                    for k in 0..4096 {
                        let c = self.get_var(t, scope, arr, offset + k).map_or(0, |v| *v);
                        if c == 0 {
                            break;
                        }
                        t.print.push(char::from_u32(c as u32 & 0xff).unwrap_or('?'));
                    }
                }
                // ---- functions ----
                CALL | CALLDISCARD | CALLSTACK => {
                    let (fm, fi) = if p == CALLSTACK {
                        let v = pop!();
                        (((v >> 16) & 0xffff) as usize, (v & 0xffff) as usize)
                    } else {
                        (t.module, operand!(code.byte_op(&mut t.pc)) as usize)
                    };
                    let Some(&Some((tm, tf))) = self.func_targets.get(fm).and_then(|f| f.get(fi)) else {
                        return self.fail(t, "call to a missing function");
                    };
                    let f = &self.modules[tm].functions[tf];
                    let (argc, localc, address) = (f.arg_count as usize, f.local_count as usize, f.address as usize);
                    let arrays = f.arrays.iter().map(|&n| vec![0; n.min(1 << 16) as usize]).collect();
                    let args = pop_n!(argc);
                    let base = t.locals.len();
                    t.locals.extend_from_slice(&args);
                    t.locals.resize(base + localc.max(argc), 0);
                    t.frames.push(Frame {
                        module: t.module,
                        ret_pc: t.pc,
                        locals_base: t.locals_base,
                        arrays: std::mem::replace(&mut t.arrays, arrays),
                        discard: p == CALLDISCARD,
                    });
                    if t.frames.len() > 1000 {
                        return self.fail(t, "call stack overflow");
                    }
                    t.locals_base = base;
                    t.module = tm;
                    t.pc = address;
                    code = self.code[tm].clone();
                }
                PUSHFUNCTION => {
                    let f = operand!(code.byte_op(&mut t.pc));
                    t.stack.push(Self::tag(t.module, f));
                }
                RETURNVOID | RETURNVAL => {
                    let v = if p == RETURNVAL { pop!() } else { 0 };
                    let Some(frame) = t.frames.pop() else { return self.fail(t, "return outside a function") };
                    t.locals.truncate(t.locals_base);
                    t.locals_base = frame.locals_base;
                    t.arrays = frame.arrays;
                    t.module = frame.module;
                    t.pc = frame.ret_pc;
                    code = self.code[t.module].clone();
                    if !frame.discard {
                        t.stack.push(v);
                    }
                }
                CALLFUNC => {
                    let argc = operand!(code.byte_op(&mut t.pc)) as usize;
                    let func = operand!(code.short_op(&mut t.pc));
                    let args = pop_n!(argc);
                    let r = match self.callfunc(t, host, func, &args) {
                        Some(r) => r,
                        None => return,
                    };
                    t.stack.push(r);
                }
                CONSOLECOMMAND => {
                    // Never run console commands from map scripts.
                    let _ = pop_n!(3);
                }
                CONSOLECOMMANDDIRECT => {
                    for _ in 0..3 {
                        operand!(code.word(&mut t.pc));
                    }
                }
                WRITETOINI => {
                    let _ = pop_n!(3);
                }
                GETFROMINI => {
                    let _ = pop_n!(3);
                    t.stack.push(0);
                }
                SETSTYLE => {
                    pop!();
                }
                SETSTYLEDIRECT => {
                    operand!(code.word(&mut t.pc));
                }
                _ => {
                    // Everything that touches the world: (builtin, stack
                    // args, direct args, string direct args, pushes result).
                    let Some((f, n_stack, n_direct, strings_mask, pushes)) = world_op(p) else {
                        self.note(format!("pcode {} ({p})", name(p)));
                        let m = self.modules[t.module].name.clone();
                        self.warnings.push(format!("{m}: unimplemented pcode {} ({p}) at {at}", name(p)));
                        t.state = State::Done;
                        return;
                    };
                    let mut args = pop_n!(n_stack);
                    for k in 0..n_direct {
                        let v = operand!(code.word(&mut t.pc));
                        args.push(if strings_mask & (1 << k) != 0 { Self::tag(t.module, v) } else { v });
                    }
                    if let Some(extra) = implicit_arg(f, &t.act) {
                        args.insert(0, extra);
                    }
                    let r = builtin!(f, args);
                    if pushes {
                        t.stack.push(r);
                    }
                    if self.after_host(host) {
                        // A request from the host may have stopped us.
                        if t.state == State::Done {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Process starts the host queued during a call. True if any.
    fn after_host(&mut self, host: &mut dyn AcsHost) -> bool {
        let reqs = host.take_requests();
        let any = !reqs.is_empty();
        for r in reqs {
            self.request(&r);
        }
        any
    }

    /// A line special called from a script: script control is handled here,
    /// the rest by the host. `None`: the thread ended.
    fn script_special(&mut self, t: &mut Thread, host: &mut dyn AcsHost, special: i32, args: [i32; 5]) -> Option<i32> {
        let req = |kind, script: ScriptId, map: i32, a: [i32; 4]| ScriptRequest { script, map, args: a, act: t.act, kind };
        let r = match special {
            // ACS_Execute(script, map, a1, a2, a3), ACS_ExecuteAlways.
            80 | 226 => {
                let kind = if special == 80 { RequestKind::Execute } else { RequestKind::ExecuteAlways };
                self.request(&req(kind, ScriptId::Number(args[0]), args[1], [args[2], args[3], args[4], 0])) as i32
            }
            81 => self.request(&req(RequestKind::Suspend, ScriptId::Number(args[0]), args[1], [0; 4])) as i32,
            82 => self.request(&req(RequestKind::Terminate, ScriptId::Number(args[0]), args[1], [0; 4])) as i32,
            // ACS_LockedExecute(script, map, a1, a2, lock): the lock is
            // the host's to check; a script caller has no key to show.
            83 | 85 => self.request(&req(RequestKind::Execute, ScriptId::Number(args[0]), args[1], [args[2], args[3], 0, 0])) as i32,
            // ACS_ExecuteWithResult(script, a1, a2, a3, a4): runs now.
            84 => self.execute_with_result(host, ScriptId::Number(args[0]), [args[1], args[2], args[3], args[4]], t.act),
            _ => {
                let r = host.line_special(&t.act, special, args, &self.strings);
                self.after_host(host);
                r
            }
        };
        (t.state != State::Done).then_some(r)
    }

    /// Start a script and run it right away; its `SetResultValue` (or 1)
    /// is returned.
    pub fn execute_with_result(&mut self, host: &mut dyn AcsHost, id: ScriptId, args: [i32; 4], act: Activation) -> i32 {
        let Some((mi, si)) = self.find_script(&id) else { return 0 };
        let i = self.spawn_thread(mi, si, &args, act);
        self.threads[i].result = 1;
        self.run_thread(i, host);
        self.threads[i].result
    }

    /// `CallFunc` extension functions: string and script functions here,
    /// the rest from the host.
    fn callfunc(&mut self, t: &mut Thread, host: &mut dyn AcsHost, func: i32, args: &[i32]) -> Option<i32> {
        let a = |i: usize| args.get(i).copied().unwrap_or(0);
        let named = |rt: &Self, i: usize| ScriptId::Name(rt.strings.get(a(i)).to_string());
        let req = |kind, script: ScriptId, map: i32, args4: [i32; 4]| ScriptRequest { script, map, args: args4, act: t.act, kind };
        Some(match func {
            // GetChar(string, index)
            15 => self.strings.get(a(0)).as_bytes().get(a(1) as usize).map_or(0, |&c| c as i32),
            // ACS_NamedExecute(name, map, a1, a2, a3)
            39 => {
                let id = named(self, 0);
                self.request(&req(RequestKind::Execute, id, a(1), [a(2), a(3), a(4), 0])) as i32
            }
            40 => {
                let id = named(self, 0);
                self.request(&req(RequestKind::Suspend, id, a(1), [0; 4])) as i32
            }
            41 => {
                let id = named(self, 0);
                self.request(&req(RequestKind::Terminate, id, a(1), [0; 4])) as i32
            }
            // Locked variants: (name, map, a1, a2, lock).
            42 | 43 => {
                let id = named(self, 0);
                self.request(&req(RequestKind::Execute, id, a(1), [a(2), a(3), 0, 0])) as i32
            }
            // ACS_NamedExecuteWithResult(name, a1, a2, a3, a4)
            44 => {
                let id = named(self, 0);
                self.execute_with_result(host, id, [a(1), a(2), a(3), a(4)], t.act)
            }
            45 => {
                let id = named(self, 0);
                self.request(&req(RequestKind::ExecuteAlways, id, a(1), [a(2), a(3), a(4), 0])) as i32
            }
            // Sqrt(int), FixedSqrt(fixed), VectorLength(x, y)
            48 => (a(0).max(0) as f64).sqrt() as i32,
            49 => ((a(0).max(0) as f64 / 65536.0).sqrt() * 65536.0) as i32,
            50 => ((a(0) as f64).hypot(a(1) as f64)) as i32,
            // strcmp / stricmp (string, string[, max])
            63 | 64 => {
                let (x, y) = (self.strings.get(a(0)), self.strings.get(a(1)));
                let max = if args.len() > 2 { a(2).max(0) as usize } else { usize::MAX };
                let (x, y): (String, String) = (x.chars().take(max).collect(), y.chars().take(max).collect());
                let (x, y) = if func == 64 { (x.to_ascii_lowercase(), y.to_ascii_lowercase()) } else { (x, y) };
                x.cmp(&y) as i32
            }
            // StrLeft(s, n), StrRight(s, n), StrMid(s, start, n)
            65..=67 => {
                let s: Vec<char> = self.strings.get(a(0)).chars().collect();
                let n = s.len();
                let part: String = match func {
                    65 => s[..(a(1).max(0) as usize).min(n)].iter().collect(),
                    66 => s[n - (a(1).max(0) as usize).min(n)..].iter().collect(),
                    _ => {
                        let start = (a(1).max(0) as usize).min(n);
                        s[start..(start + a(2).max(0) as usize).min(n)].iter().collect()
                    }
                };
                self.strings.add(part)
            }
            _ => {
                let r = host.builtin(&mut t.act, Builtin::Func(func as u16), args, &mut self.strings);
                self.after_host(host);
                if t.state == State::Done {
                    return None;
                }
                r
            }
        })
    }
}

/// The activator-relative builtins whose ACS form has no TID argument get
/// none; nothing to add. (Kept as a hook for builtins that need the line.)
fn implicit_arg(f: Builtin, act: &Activation) -> Option<i32> {
    match f {
        Builtin::ClearLineSpecial | Builtin::GetLineRowOffset => Some(act.line.map_or(-1, |l| l as i32)),
        _ => None,
    }
}

/// World instructions: (builtin, stack args, direct word args, which direct
/// args are strings (bit mask), pushes a result).
fn world_op(p: u32) -> Option<(Builtin, usize, usize, u32, bool)> {
    use Builtin as B;
    Some(match p {
        THINGCOUNT => (B::ThingCount, 2, 0, 0, true),
        THINGCOUNTDIRECT => (B::ThingCount, 0, 2, 0, true),
        THINGCOUNTNAME => (B::ThingCountName, 2, 0, 0, true),
        THINGCOUNTSECTOR => (B::ThingCountSector, 3, 0, 0, true),
        THINGCOUNTNAMESECTOR => (B::ThingCountNameSector, 3, 0, 0, true),
        CHANGEFLOOR => (B::ChangeFloor, 2, 0, 0, false),
        CHANGEFLOORDIRECT => (B::ChangeFloor, 0, 2, 0b10, false),
        CHANGECEILING => (B::ChangeCeiling, 2, 0, 0, false),
        CHANGECEILINGDIRECT => (B::ChangeCeiling, 0, 2, 0b10, false),
        CLEARLINESPECIAL => (B::ClearLineSpecial, 0, 0, 0, false),
        PLAYERCOUNT => (B::PlayerCount, 0, 0, 0, true),
        GAMETYPE => (B::GameType, 0, 0, 0, true),
        GAMESKILL => (B::GameSkill, 0, 0, 0, true),
        SECTORSOUND => (B::SectorSound, 2, 0, 0, false),
        AMBIENTSOUND => (B::AmbientSound, 2, 0, 0, false),
        LOCALAMBIENTSOUND => (B::LocalAmbientSound, 2, 0, 0, false),
        SOUNDSEQUENCE => (B::SoundSequence, 1, 0, 0, false),
        SETLINETEXTURE => (B::SetLineTexture, 4, 0, 0, false),
        SETLINEBLOCKING => (B::SetLineBlocking, 2, 0, 0, false),
        SETLINEMONSTERBLOCKING => (B::SetLineMonsterBlocking, 2, 0, 0, false),
        SETLINESPECIAL => (B::SetLineSpecial, 7, 0, 0, false),
        THINGSOUND => (B::ThingSound, 3, 0, 0, false),
        ACTIVATORSOUND => (B::ActivatorSound, 2, 0, 0, false),
        ISNETWORKGAME => (B::IsNetworkGame, 0, 0, 0, true),
        PLAYERTEAM | PLAYEREXPERT | BLUETEAMCOUNT | REDTEAMCOUNT | BLUETEAMSCORE | REDTEAMSCORE | ISONEFLAGCTF | GETINVASIONWAVE
        | GETINVASIONSTATE | GETSIGILPIECES => (B::Func(0), 0, 0, 0, true),
        PLAYERHEALTH => (B::PlayerHealth, 0, 0, 0, true),
        PLAYERARMORPOINTS => (B::PlayerArmorPoints, 0, 0, 0, true),
        PLAYERFRAGS => (B::PlayerFrags, 0, 0, 0, true),
        MUSICCHANGE => (B::SetMusic, 2, 0, 0, false),
        SINGLEPLAYER => (B::SinglePlayer, 0, 0, 0, true),
        SETGRAVITY => (B::SetGravity, 1, 0, 0, false),
        SETGRAVITYDIRECT => (B::SetGravity, 0, 1, 0, false),
        SETAIRCONTROL => (B::SetAirControl, 1, 0, 0, false),
        SETAIRCONTROLDIRECT => (B::SetAirControl, 0, 1, 0, false),
        CLEARINVENTORY => (B::ClearInventory, 0, 0, 0, false),
        GIVEINVENTORY => (B::GiveInventory, 2, 0, 0, false),
        GIVEINVENTORYDIRECT => (B::GiveInventory, 0, 2, 0b1, false),
        TAKEINVENTORY => (B::TakeInventory, 2, 0, 0, false),
        TAKEINVENTORYDIRECT => (B::TakeInventory, 0, 2, 0b1, false),
        CHECKINVENTORY => (B::CheckInventory, 1, 0, 0, true),
        CHECKINVENTORYDIRECT => (B::CheckInventory, 0, 1, 0b1, true),
        SPAWN => (B::Spawn, 6, 0, 0, true),
        SPAWNDIRECT => (B::Spawn, 0, 6, 0b1, true),
        SPAWNSPOT => (B::SpawnSpot, 4, 0, 0, true),
        SPAWNSPOTDIRECT => (B::SpawnSpot, 0, 4, 0b1, true),
        SPAWNSPOTFACING => (B::SpawnSpotFacing, 3, 0, 0, true),
        SETMUSIC => (B::SetMusic, 3, 0, 0, false),
        SETMUSICDIRECT => (B::SetMusic, 0, 3, 0b1, false),
        LOCALSETMUSIC => (B::LocalSetMusic, 3, 0, 0, false),
        LOCALSETMUSICDIRECT => (B::LocalSetMusic, 0, 3, 0b1, false),
        SETFONT => (B::SetFont, 1, 0, 0, false),
        SETFONTDIRECT => (B::SetFont, 0, 1, 0b1, false),
        SETTHINGSPECIAL => (B::SetThingSpecial, 7, 0, 0, false),
        FADETO => (B::FadeTo, 5, 0, 0, false),
        FADERANGE => (B::FadeRange, 9, 0, 0, false),
        CANCELFADE => (B::CancelFade, 0, 0, 0, false),
        PLAYMOVIE => (B::Func(0), 1, 0, 0, true),
        SETFLOORTRIGGER => (B::SetFloorTrigger, 8, 0, 0, false),
        SETCEILINGTRIGGER => (B::SetCeilingTrigger, 8, 0, 0, false),
        GETACTORX => (B::GetActorX, 1, 0, 0, true),
        GETACTORY => (B::GetActorY, 1, 0, 0, true),
        GETACTORZ => (B::GetActorZ, 1, 0, 0, true),
        STARTTRANSLATION => (B::StartTranslation, 1, 0, 0, false),
        TRANSLATIONRANGE1 => (B::TranslationRange1, 4, 0, 0, false),
        TRANSLATIONRANGE2 => (B::TranslationRange2, 8, 0, 0, false),
        TRANSLATIONRANGE3 => (B::TranslationRange3, 8, 0, 0, false),
        TRANSLATIONRANGE4 => (B::TranslationRange4, 5, 0, 0, false),
        TRANSLATIONRANGE5 => (B::TranslationRange5, 6, 0, 0, false),
        ENDTRANSLATION => (B::EndTranslation, 0, 0, 0, false),
        CHECKWEAPON => (B::CheckWeapon, 1, 0, 0, true),
        SETWEAPON => (B::SetWeapon, 1, 0, 0, true),
        SETMARINEWEAPON | SETMARINESPRITE => (B::Func(0), 2, 0, 0, false),
        SETACTORPROPERTY => (B::SetActorProperty, 3, 0, 0, false),
        GETACTORPROPERTY => (B::GetActorProperty, 2, 0, 0, true),
        PLAYERNUMBER => (B::PlayerNumber, 0, 0, 0, true),
        ACTIVATORTID => (B::ActivatorTid, 0, 0, 0, true),
        GETSCREENWIDTH => (B::GetScreenWidth, 0, 0, 0, true),
        GETSCREENHEIGHT => (B::GetScreenHeight, 0, 0, 0, true),
        THING_PROJECTILE2 => (B::ThingProjectile2, 7, 0, 0, false),
        SETHUDSIZE => (B::SetHudSize, 3, 0, 0, false),
        GETCVAR => (B::GetCvar, 1, 0, 0, true),
        GETLINEROWOFFSET => (B::GetLineRowOffset, 0, 0, 0, true),
        GETACTORFLOORZ => (B::GetActorFloorZ, 1, 0, 0, true),
        GETACTORANGLE => (B::GetActorAngle, 1, 0, 0, true),
        GETSECTORFLOORZ => (B::GetSectorFloorZ, 3, 0, 0, true),
        GETSECTORCEILINGZ => (B::GetSectorCeilingZ, 3, 0, 0, true),
        GETLEVELINFO => (B::GetLevelInfo, 1, 0, 0, true),
        CHANGESKY => (B::ChangeSky, 2, 0, 0, false),
        PLAYERINGAME => (B::PlayerInGame, 1, 0, 0, true),
        PLAYERISBOT => (B::PlayerIsBot, 1, 0, 0, true),
        SETCAMERATOTEXTURE => (B::SetCameraToTexture, 3, 0, 0, false),
        GETAMMOCAPACITY => (B::GetAmmoCapacity, 1, 0, 0, true),
        SETAMMOCAPACITY => (B::SetAmmoCapacity, 2, 0, 0, false),
        SETACTORANGLE => (B::SetActorAngle, 2, 0, 0, false),
        GRABINPUT | MOVEMOUSEPOINTER => (B::Func(0), 2, 0, 0, false),
        SETMOUSEPOINTER => (B::Func(0), 3, 0, 0, false),
        SPAWNPROJECTILE => (B::SpawnProjectile, 7, 0, 0, false),
        GETSECTORLIGHTLEVEL => (B::GetSectorLightLevel, 1, 0, 0, true),
        GETACTORCEILINGZ => (B::GetActorCeilingZ, 1, 0, 0, true),
        SETACTORPOSITION => (B::SetActorPosition, 5, 0, 0, true),
        CLEARACTORINVENTORY => (B::ClearActorInventory, 1, 0, 0, false),
        GIVEACTORINVENTORY => (B::GiveActorInventory, 3, 0, 0, false),
        TAKEACTORINVENTORY => (B::TakeActorInventory, 3, 0, 0, false),
        CHECKACTORINVENTORY => (B::CheckActorInventory, 2, 0, 0, true),
        PLAYERCLASS => (B::PlayerClass, 1, 0, 0, true),
        GETPLAYERINFO => (B::GetPlayerInfo, 2, 0, 0, true),
        CHANGELEVEL => (B::ChangeLevel, 4, 0, 0, false),
        SECTORDAMAGE => (B::SectorDamage, 5, 0, 0, false),
        REPLACETEXTURES => (B::ReplaceTextures, 3, 0, 0, false),
        GETACTORPITCH => (B::GetActorPitch, 1, 0, 0, true),
        SETACTORPITCH => (B::SetActorPitch, 2, 0, 0, false),
        SETACTORSTATE => (B::SetActorState, 3, 0, 0, true),
        THINGDAMAGE2 => (B::ThingDamage2, 3, 0, 0, true),
        USEINVENTORY => (B::UseInventory, 1, 0, 0, true),
        USEACTORINVENTORY => (B::UseActorInventory, 2, 0, 0, true),
        CHECKACTORCEILINGTEXTURE => (B::CheckActorCeilingTexture, 2, 0, 0, true),
        CHECKACTORFLOORTEXTURE => (B::CheckActorFloorTexture, 2, 0, 0, true),
        GETACTORLIGHTLEVEL => (B::GetActorLightLevel, 1, 0, 0, true),
        SETMUGSHOTSTATE => (B::SetMugShotState, 1, 0, 0, false),
        CHECKPLAYERCAMERA => (B::CheckPlayerCamera, 1, 0, 0, true),
        MORPHACTOR => (B::MorphActor, 7, 0, 0, true),
        UNMORPHACTOR => (B::UnmorphActor, 2, 0, 0, true),
        GETPLAYERINPUT => (B::GetPlayerInput, 2, 0, 0, true),
        CLASSIFYACTOR => (B::ClassifyActor, 1, 0, 0, true),
        _ => return None,
    })
}

impl Thread {
    fn placeholder() -> Thread {
        Thread {
            module: 0,
            number: 0,
            start: 0,
            pc: 0,
            stack: Vec::new(),
            locals: Vec::new(),
            locals_base: 0,
            arrays: Vec::new(),
            frames: Vec::new(),
            state: State::Done,
            act: Activation::default(),
            print: String::new(),
            hud_opt: None,
            result: 0,
        }
    }
}

impl Module {
    /// A module with no scripts (maps without BEHAVIOR that still load
    /// libraries).
    pub fn empty(name: &str) -> Module {
        Module {
            name: name.to_string(),
            data: Vec::new(),
            format: Format::Old,
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
        }
    }
}
