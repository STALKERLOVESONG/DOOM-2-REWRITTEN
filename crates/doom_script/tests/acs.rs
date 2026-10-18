//! ACS VM tests: hand-assembled modules for exact semantics, and the real
//! BEHAVIOR lump of MyHouse.pk3 (skipped when the mod is absent) run against
//! a recording fake host.

use std::path::PathBuf;

use doom_script::acs::pcode::*;
use doom_script::acs::*;

#[derive(Default)]
struct FakeHost {
    specials: Vec<(i32, [i32; 5])>,
    calls: Vec<(Builtin, Vec<i32>, Vec<String>)>,
    prints: Vec<(String, PrintKind)>,
    busy_tag: Option<i32>,
    time: i32,
    requests: Vec<ScriptRequest>,
}

impl AcsHost for FakeHost {
    fn line_special(&mut self, _act: &Activation, special: i32, args: [i32; 5], _strings: &Strings) -> i32 {
        self.specials.push((special, args));
        1
    }
    fn tag_busy(&self, tag: i32) -> bool {
        self.busy_tag == Some(tag)
    }
    fn random(&mut self, min: i32, _max: i32) -> i32 {
        min
    }
    fn level_time(&self) -> i32 {
        self.time
    }
    fn builtin(&mut self, _act: &mut Activation, f: Builtin, args: &[i32], strings: &mut Strings) -> i32 {
        let texts = args.iter().map(|&a| strings.get(a).to_string()).collect();
        self.calls.push((f, args.to_vec(), texts));
        match f {
            Builtin::CheckInventory => 3,
            _ => 0,
        }
    }
    fn print(&mut self, _act: &Activation, kind: PrintKind, text: &str) {
        self.prints.push((text.to_string(), kind));
    }
    fn take_requests(&mut self) -> Vec<ScriptRequest> {
        std::mem::take(&mut self.requests)
    }
}

/// A tiny ACS0 assembler: words for every instruction and operand.
struct Asm {
    code: Vec<u8>,
}

impl Asm {
    fn new() -> Asm {
        // Header: "ACS\0", directory offset (patched).
        Asm { code: b"ACS\0\0\0\0\0".to_vec() }
    }
    fn here(&self) -> i32 {
        self.code.len() as i32
    }
    fn op(&mut self, p: u32, operands: &[i32]) -> &mut Self {
        self.code.extend_from_slice(&(p as i32).to_le_bytes());
        for o in operands {
            self.code.extend_from_slice(&o.to_le_bytes());
        }
        self
    }
    fn push(&mut self, v: i32) -> &mut Self {
        self.op(PUSHNUMBER, &[v])
    }
    /// Finish with scripts (number + kind*1000, address, argc) and strings.
    fn finish(mut self, scripts: &[(i32, i32, i32)], strings: &[&str]) -> Module {
        let dir = self.code.len() as u32;
        self.code[4..8].copy_from_slice(&dir.to_le_bytes());
        self.code.extend_from_slice(&(scripts.len() as i32).to_le_bytes());
        for &(n, a, c) in scripts {
            for v in [n, a, c] {
                self.code.extend_from_slice(&v.to_le_bytes());
            }
        }
        self.code.extend_from_slice(&(strings.len() as i32).to_le_bytes());
        let table = self.code.len();
        self.code.resize(table + strings.len() * 4, 0);
        for (i, s) in strings.iter().enumerate() {
            let at = self.code.len() as i32;
            self.code[table + i * 4..table + i * 4 + 4].copy_from_slice(&at.to_le_bytes());
            self.code.extend_from_slice(s.as_bytes());
            self.code.push(0);
        }
        Module::load("test", self.code).unwrap()
    }
}

fn runtime(m: Module) -> AcsRuntime {
    AcsRuntime::new(Some(m), vec![], |_| None, AcsGlobals::default())
}

fn start(rt: &mut AcsRuntime, n: i32, args: [i32; 4]) {
    rt.request(&ScriptRequest { script: ScriptId::Number(n), map: 0, args, act: Activation::default(), kind: RequestKind::Execute });
}

#[test]
fn arithmetic_loops_and_specials() {
    // script 1 (int a): for (i = 0; i < a; i++) Door_Open(i * 10 + 1, 16, 0); Print(d: a*a);
    let mut a = Asm::new();
    let s1 = a.here();
    a.push(0).op(ASSIGNSCRIPTVAR, &[1]);
    let top = a.here();
    a.op(PUSHSCRIPTVAR, &[1]).op(PUSHSCRIPTVAR, &[0]).op(LT, &[]);
    let exit_patch = a.here() + 4;
    a.op(IFNOTGOTO, &[0]);
    a.op(PUSHSCRIPTVAR, &[1]).push(10).op(MULTIPLY, &[]).push(1).op(ADD, &[]).push(16).push(0);
    a.op(LSPEC3, &[10]);
    a.op(INCSCRIPTVAR, &[1]).op(GOTO, &[top]);
    let exit = a.here();
    a.code[exit_patch as usize..exit_patch as usize + 4].copy_from_slice(&exit.to_le_bytes());
    a.op(BEGINPRINT, &[]).op(PUSHSCRIPTVAR, &[0]).op(DUP, &[]).op(MULTIPLY, &[]).op(PRINTNUMBER, &[]).op(ENDPRINT, &[]);
    a.op(TERMINATE, &[]);
    let mut rt = runtime(a.finish(&[(1, s1, 1)], &[]));
    let mut host = FakeHost::default();
    start(&mut rt, 1, [3, 0, 0, 0]);
    rt.tick(&mut host);
    let doors: Vec<i32> = host.specials.iter().map(|(s, a)| if *s == 10 { a[0] } else { -1 }).collect();
    assert_eq!(doors, [1, 11, 21]);
    assert_eq!(host.prints[0].0, "9");
    assert_eq!(rt.running_scripts(), 0);
    assert!(rt.warnings.is_empty(), "{:?}", rt.warnings);
}

#[test]
fn delays_tag_waits_and_map_vars() {
    // OPEN script: mapvar0 = 5; delay(3); mapvar0 += 1; tagwait(7); mapvar0 *= 2; print(d: mapvar0)
    let mut a = Asm::new();
    let s = a.here();
    a.push(5).op(ASSIGNMAPVAR, &[0]).op(DELAYDIRECT, &[3]);
    a.push(1).op(ADDMAPVAR, &[0]).op(TAGWAITDIRECT, &[7]);
    a.push(2).op(MULMAPVAR, &[0]).op(BEGINPRINT, &[]).op(PUSHMAPVAR, &[0]).op(PRINTNUMBER, &[]).op(ENDPRINT, &[]).op(TERMINATE, &[]);
    let mut rt = runtime(a.finish(&[(1000 + 2, s, 0)], &[]));
    let mut host = FakeHost { busy_tag: Some(7), ..Default::default() };
    rt.start_kind(ScriptKind::Open, Activation::default(), &[]);
    for _ in 0..3 {
        rt.tick(&mut host);
    }
    assert!(host.prints.is_empty(), "still delaying");
    rt.tick(&mut host); // delay over, now waiting for tag 7
    rt.tick(&mut host);
    assert!(host.prints.is_empty(), "waiting for the tag");
    host.busy_tag = None;
    rt.tick(&mut host);
    assert_eq!(host.prints[0].0, "12");
}

#[test]
fn functions_strings_and_script_control() {
    // function 0 (int x) returns x * 3; script 1: print(s:"hp ", d:f(4)); ACS_Execute(2); script 2: SetResultValue(7)
    let mut a = Asm::new();
    let f0 = a.here();
    a.op(PUSHSCRIPTVAR, &[0]).push(3).op(MULTIPLY, &[]).op(RETURNVAL, &[]);
    let s1 = a.here();
    a.op(BEGINPRINT, &[]).push(0).op(PRINTSTRING, &[]).push(4).op(CALL, &[0]).op(PRINTNUMBER, &[]).op(ENDPRINT, &[]);
    // ACS_ExecuteWithResult(2) -> printed.
    a.push(2).push(0).push(0).push(0).push(0).op(LSPEC5RESULT, &[84]);
    a.op(BEGINPRINT, &[]).op(PRINTNUMBER, &[]).op(ENDPRINT, &[]);
    // CheckInventory("Shell") from the host.
    a.op(CHECKINVENTORYDIRECT, &[1]).op(DROP, &[]);
    a.op(TERMINATE, &[]);
    let s2 = a.here();
    a.push(7).op(SETRESULTVALUE, &[]).op(TERMINATE, &[]);
    let mut m = a.finish(&[(1, s1, 0), (2, s2, 0)], &["hp ", "Shell"]);
    m.functions.push(FuncDef { arg_count: 1, local_count: 1, has_return: true, import: 0, address: f0 as u32, name: Some("f".into()), arrays: vec![] });
    let mut rt = runtime(m);
    let mut host = FakeHost::default();
    start(&mut rt, 1, [0; 4]);
    rt.tick(&mut host);
    assert_eq!(host.prints.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["hp 12", "7"]);
    let (f, _, texts) = &host.calls[0];
    assert_eq!((*f, texts[0].as_str()), (Builtin::CheckInventory, "Shell"));
    assert!(rt.warnings.is_empty(), "{:?}", rt.warnings);
}

#[test]
fn errors_terminate_the_script_not_the_game() {
    let mut a = Asm::new();
    let s = a.here();
    a.push(1).push(0).op(DIVIDE, &[]).op(TERMINATE, &[]);
    let r = a.here();
    // Runaway: an endless loop without delay.
    a.op(GOTO, &[r]);
    let mut rt = runtime(a.finish(&[(1, s, 0), (2, r, 0)], &[]));
    let mut host = FakeHost::default();
    start(&mut rt, 1, [0; 4]);
    start(&mut rt, 2, [0; 4]);
    rt.tick(&mut host);
    assert_eq!(rt.running_scripts(), 0);
    assert_eq!(rt.warnings.len(), 2, "{:?}", rt.warnings);
}

// ---- real content -------------------------------------------------------------

fn wads() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS")
}

/// The BEHAVIOR lump of MyHouse.pk3's house map.
fn myhouse_behavior() -> Option<Vec<u8>> {
    let (doom2, pk3) = (wads().join("doom2.wad"), wads().join("myhouse.pk3"));
    if !doom2.exists() || !pk3.exists() {
        eprintln!("skipping: no doom2.wad / myhouse.pk3");
        return None;
    }
    let stack = doom_content::ResourceStack::load(&doom2, &[pk3]).unwrap();
    let mut id = stack.check_num(doom_content::LumpName::new("MAP01"))?;
    while let Some(next) = stack.next_in_source(id) {
        id = next;
        match stack.info(id).name.as_bytes() {
            b"BEHAVIOR" => return Some(stack.data(id).to_vec()),
            b"ENDMAP" => break,
            _ => {}
        }
    }
    None
}

#[test]
fn myhouse_behavior_loads_and_runs() {
    let Some(data) = myhouse_behavior() else { return };
    let m = Module::load("MAP01", data).unwrap();
    assert_eq!(m.format, Format::LittleEnhanced);
    assert!(m.scripts.len() > 40, "{} scripts", m.scripts.len());
    // Named scripts ("FadeMusicIn" is only referenced, from script 21).
    assert!(m.named_script("AED").is_some() && m.named_script("scale setup").is_some());
    assert!(m.strings.iter().any(|s| s == "RedBall"));
    let mut rt = runtime(m);
    assert!(rt.warnings.is_empty(), "{:?}", rt.warnings);
    let mut host = FakeHost::default();

    // script 1: Ceiling_RaiseInstant(428, 0, 14); Floor_LowerInstant(1366, 0, 8);
    start(&mut rt, 1, [0; 4]);
    rt.tick(&mut host);
    let args: Vec<[i32; 3]> = host.specials.iter().map(|(_, a)| [a[0], a[1], a[2]]).collect();
    assert_eq!(args, [[428, 0, 14], [1366, 0, 8]]);

    // script 6: SpawnSpotFacing("RedBall", 1671, 1292) and three more.
    start(&mut rt, 6, [0; 4]);
    rt.tick(&mut host);
    let spawns: Vec<(String, i32)> = host
        .calls
        .iter()
        .filter(|(f, ..)| *f == Builtin::SpawnSpotFacing)
        .map(|(_, a, t)| (t[0].clone(), a[1]))
        .collect();
    assert_eq!(spawns[0], ("RedBall".to_string(), 1671));
    assert_eq!(spawns.len(), 4);

    // script 21: fades the music out over 20 tics with Delay(1), then
    // gives FadeToken and ends.
    host.calls.clear();
    start(&mut rt, 21, [0; 4]);
    for _ in 0..30 {
        rt.tick(&mut host);
    }
    let volumes: Vec<i32> = host.calls.iter().filter(|(f, ..)| matches!(f, Builtin::Func(_))).map(|(_, a, _)| a[0]).collect();
    assert!(volumes.len() >= 19, "{volumes:?}");
    assert!(volumes.windows(2).all(|w| w[1] <= w[0]), "volume falls: {volumes:?}");
    assert!(host.calls.iter().any(|(f, _, t)| *f == Builtin::GiveInventory && t[0] == "FadeToken"));
    assert_eq!(rt.running_scripts(), 0);
    assert!(rt.unimplemented.is_empty(), "{:?}", rt.unimplemented);
}

#[test]
fn brutal_doom_libraries_load() {
    let (doom2, pk3) = (wads().join("doom2.wad"), wads().join("brutalv21.pk3"));
    if !doom2.exists() || !pk3.exists() {
        return;
    }
    let stack = doom_content::ResourceStack::load(&doom2, &[pk3]).unwrap();
    let mut loaded = 0;
    for &id in stack.namespace(doom_content::Namespace::Acs) {
        let data = stack.data(id).to_vec();
        if !Module::is_acs(&data) {
            continue;
        }
        let name = stack.info(id).name.to_string();
        let m = Module::load(&name, data).unwrap_or_else(|e| panic!("{e}"));
        assert!(!m.scripts.is_empty() || !m.functions.is_empty(), "{name}: empty");
        loaded += 1;
    }
    assert!(loaded >= 15, "{loaded} libraries");
}
