//! Trace a mod's sound through the whole pipeline and prove it plays:
//! definition -> logical name -> resource -> source file -> decoder ->
//! PCM -> gameplay event -> mixer command -> audible output.
//!
//! cargo run -p doom_game --release --example sound_trace -- <iwad> <pwad> <sound name>
//!     [--weapon <class> --slot <n>] [--actor <class>] [--map MAP01]
//!
//! With `--weapon`, the weapon is given, selected and fired, and the
//! sound must come from that. With `--actor`, the actor is spawned in
//! front of the player and its sounds are traced as it wakes and attacks.

use std::sync::Arc;

use doom_audio::{AudioCommand, Mixer, Sound};
use doom_game::{InputState, Session, SessionConfig};
use doom_sim::world::SimEvent;

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len().max(1) as f32).sqrt()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: sound_trace <iwad> <pwad> <sound name> [--weapon CLASS --slot N] [--actor CLASS] [--map MAP]");
        std::process::exit(2);
    }
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let sound_name = args[3].clone();
    let mut cfg = SessionConfig::new(&args[1]);
    cfg.pwads = vec![args[2].clone().into()];
    cfg.map = Some(opt("--map").unwrap_or_else(|| "MAP01".into()));
    cfg.nomonsters = opt("--actor").is_none();
    let mut s = Session::new(&cfg).expect("session");

    println!("== {sound_name}");
    // 1-2. Definition and logical name.
    let Some(sfx) = s.world.info.sound(&sound_name) else {
        println!("1. FAIL: no sound is defined under the name {sound_name:?}");
        std::process::exit(1);
    };
    let def = s.world.info.sound_defs.get(&sfx.0).cloned();
    println!("1. definition: {:?}", def.as_ref().map(|d| (&d.resource, d.volume, d.attenuation, d.limit)));
    println!("2. logical name {sound_name:?} -> sound #{} ({})", sfx.0, s.world.info.sfx[sfx.0 as usize].name);
    // `$random`: trace the members (any of them may play).
    let members: Vec<doom_sim::info::SfxNum> = match &def {
        Some(d) if !d.random.is_empty() => {
            println!("   $random of {} sounds", d.random.len());
            d.random.clone()
        }
        _ => vec![sfx],
    };
    let resource_of = |s: &Session, m: doom_sim::info::SfxNum| {
        s.world.info.sound_defs.get(&m.0).and_then(|d| d.lump).or_else(|| s.stack.find_sound(&format!("DS{}", s.world.info.sfx[m.0 as usize].name)))
    };
    let pcms: Vec<Sound> = members.iter().filter_map(|&m| resource_of(&s, m)).filter_map(|l| doom_audio::decode(s.stack.data(l)).ok()).collect();
    // 3-4. Resource and where it comes from.
    let lump = resource_of(&s, members[0]);
    let Some(lump) = lump else {
        println!("3. FAIL: the resource is not in the loaded files");
        std::process::exit(1);
    };
    let li = s.stack.info(lump);
    let source = &s.stack.sources()[li.source];
    println!("3. resource: lump {} {:?}, {} bytes", li.name, li.path, li.size());
    println!("4. from: {} ({:?})", source.path.display(), source.kind);
    // 5-6. Decoder and PCM.
    let data = s.stack.data(lump);
    let format = doom_audio::sniff(data);
    println!("5. decoder: {}", format.name());
    let decoded: Sound = doom_audio::decode(data).unwrap_or_else(|e| {
        println!("6. FAIL: {e}");
        std::process::exit(1)
    });
    println!("6. PCM: {} Hz, {} channel(s), {} frames ({:.2} s)", decoded.rate, decoded.channels, decoded.frames(), decoded.seconds());

    // 7. A gameplay event asks for it.
    let idle = InputState::default();
    for _ in 0..5 {
        s.tick(&idle);
    }
    s.take_events();
    s.take_audio_commands();
    let mut events: Vec<SimEvent> = Vec::new();
    let mut commands: Vec<AudioCommand> = Vec::new();
    if let Some(class) = opt("--weapon") {
        s.exec(&format!("give {class}")).expect("give the weapon");
        if let Some(slot) = opt("--slot") {
            s.exec(&format!("slot {slot}")).expect("select it");
        }
        for _ in 0..80 {
            s.tick(&idle);
        }
        s.take_events();
        s.take_audio_commands();
        let fire = InputState { fire: true, ..Default::default() };
        for _ in 0..20 {
            s.tick(&fire);
            events.extend(s.take_events());
            commands.extend(s.take_audio_commands());
        }
        println!("7. fired {class} for 20 tics");
    } else if let Some(class) = opt("--actor") {
        s.exec(&format!("summon {class}")).expect("summon");
        let verbose = std::env::var_os("TRACE_TICS").is_some();
        for t in 0..140 {
            if verbose {
                let st: Vec<String> = s.world.thinkers.mobjs().filter(|(_, m)| m.flags & doom_sim::mobj::flags::MF_COUNTKILL != 0).map(|(_, m)| format!("{}:{}", m.state.0, m.tics)).collect();
                eprintln!("tic {t} {st:?}");
            }
            s.tick(&idle);
            events.extend(s.take_events());
            commands.extend(s.take_audio_commands());
        }
        println!("7. spawned {class}, ran 140 tics");
    } else {
        println!("7. (no --weapon or --actor: nothing triggers it)");
        return;
    }
    let requested = events.iter().filter(|e| matches!(e, SimEvent::StartSound { sfx: x, .. } if *x == sfx)).count();
    println!("   sim events asking for sound #{}: {requested}", sfx.0);
    // 8. The mixer is told to play it (the same decoded PCM).
    let started: Vec<(usize, Arc<Sound>, i32)> = commands
        .iter()
        .filter_map(|c| match c {
            AudioCommand::StartSound { channel, sound, volume, .. } if pcms.iter().any(|p| p.samples == sound.samples) => {
                Some((*channel, sound.clone(), *volume))
            }
            _ => None,
        })
        .collect();
    println!("8. mixer StartSound commands for it: {} (channel/volume {:?})", started.len(), started.iter().map(|(c, _, v)| (*c, *v)).collect::<Vec<_>>());
    if requested == 0 || started.is_empty() {
        println!("FAIL: the sound was not played");
        std::process::exit(1);
    }
    // 9. It is audible: run a mixer on those commands.
    let mut mixer = Mixer::new(44100, doom_game::sound::NUM_CHANNELS, None);
    let quiet = {
        let mut b = vec![0f32; 4410 * 2];
        mixer.render(&mut b);
        rms(&b)
    };
    let (ch, sound, volume) = started[0].clone();
    mixer.apply(AudioCommand::StartSound { channel: ch, sound, volume, separation: 128, looping: false });
    let mut b = vec![0f32; 4410 * 2];
    mixer.render(&mut b);
    let loud = rms(&b);
    println!("9. output level: {loud:.4} RMS over the first 0.1 s (silence: {quiet:.4})");
    if loud <= quiet + 1e-4 {
        println!("FAIL: silent output");
        std::process::exit(1);
    }
    println!("OK: {sound_name} plays");
}
