//! Decode every sound a mod defines (SNDINFO resources and Doom's `DS`
//! lumps), timing each: finds files the decoders reject or choke on.
//!
//! cargo run -p doom_game --release --example decode_all -- <iwad> [pwad ...]

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pwads: Vec<std::path::PathBuf> = args[2..].iter().map(Into::into).collect();
    let stack = doom_content::ResourceStack::load(args[1].as_ref(), &pwads).expect("load");
    let mut info = doom_sim::info::Info::doom();
    let mut warnings = Vec::new();
    doom_sim::sounds::load_sndinfo(&mut info, &stack, &mut warnings);
    let mut lumps: Vec<doom_content::LumpId> = info.sound_defs.values().filter_map(|d| d.lump).collect();
    for s in &info.sfx {
        if let Some(l) = stack.find_sound(&format!("DS{}", s.name.to_ascii_uppercase())) {
            lumps.push(l);
        }
    }
    lumps.sort();
    lumps.dedup();
    let (mut ok, mut bad) = (0, 0);
    let mut by_format = std::collections::BTreeMap::new();
    for l in lumps {
        let li = stack.info(l);
        let name = li.path.as_deref().map(str::to_string).unwrap_or_else(|| li.name.to_string());
        let data = stack.data(l);
        let f = doom_audio::sniff(data);
        eprintln!("decoding {name} ({})", f.name());
        let t = Instant::now();
        let r = doom_audio::decode(data);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        *by_format.entry(f.name()).or_insert(0) += 1;
        match r {
            Ok(_) => ok += 1,
            Err(e) => {
                bad += 1;
                println!("FAIL {name}: {e}");
            }
        }
        if ms > 200.0 {
            println!("SLOW {name}: {ms:.0} ms");
        }
    }
    println!("{ok} decoded, {bad} failed; formats {by_format:?}");
}
