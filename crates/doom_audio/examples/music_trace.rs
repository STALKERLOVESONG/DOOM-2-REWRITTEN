//! Render a music lump through `MusicPlayer` and write its OPL register
//! writes (`sample register value`, hex like tools/refopl) and a WAV, for
//! comparison with the Chocolate Doom reference (tools/refopl).
//!
//! cargo run -p doom_audio --release --example music_trace -- \
//!     <iwad> <lump> <seconds> <out.wav> <out.log> [rate] [volume 0-127]
//!
//! Replay mode feeds a tools/refopl log (written from chip reset) through
//! our `Opl` with the same timing and compares the audio with the
//! reference WAV, isolating the emulator from the music logic:
//!
//!     music_trace replay <ref.log> <ref.wav>

use std::io::Write;
use std::sync::Arc;

use doom_audio::{Genmidi, MusicPlayer, Song};
use doom_content::ResourceStack;

fn read_wav(path: &str) -> (u32, Vec<i16>) {
    let d = std::fs::read(path).unwrap();
    let rate = u32::from_le_bytes(d[24..28].try_into().unwrap());
    (rate, d[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
}

fn replay(log: &str, wav: &str) {
    let (rate, reference) = read_wav(wav);
    let mut writes = Vec::new();
    let mut start = 0u64;
    for line in std::fs::read_to_string(log).unwrap().lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f[0] == "#" {
            start = f[2].parse().unwrap();
            continue;
        }
        let reg = u16::from_str_radix(f[1], 16).unwrap();
        // Timer registers go to the driver, not the chip.
        if !(2..=4).contains(&reg) {
            writes.push((f[0].parse::<u64>().unwrap(), reg, u8::from_str_radix(f[2], 16).unwrap()));
        }
    }
    let mut opl = doom_audio::opl::Opl::new(rate);
    let mut next = 0;
    let frames = reference.len() / 2;
    let mut ours = Vec::with_capacity(reference.len());
    for i in 0..start + frames as u64 {
        while next < writes.len() && writes[next].0 <= i {
            opl.write_buffered(writes[next].1, writes[next].2);
            next += 1;
        }
        let s = opl.generate();
        if i >= start {
            ours.extend_from_slice(&s);
        }
    }
    let (mut exact, mut max_diff, mut err, mut sig) = (0usize, 0i32, 0f64, 0f64);
    for (&a, &b) in ours.iter().zip(&reference) {
        let d = (a as i32 - b as i32).abs();
        exact += (d == 0) as usize;
        max_diff = max_diff.max(d);
        err += (d as f64).powi(2);
        sig += (b as f64).powi(2);
    }
    let n = reference.len();
    println!(
        "replay: {} samples, identical {:.3}%, max |diff| {max_diff}, error/signal {:.2e} ({:.1} dB)",
        n,
        exact as f64 * 100.0 / n as f64,
        err / sig.max(1.0),
        10.0 * (err.max(1e-9) / sig.max(1.0)).log10()
    );
    // Same, allowing the reference's right channel to lag one sample
    // (Nuked's OPL_QUIRK_CHANNELSAMPLEDELAY).
    let (mut l_exact, mut r_exact) = (0usize, 0usize);
    for i in 0..frames - 1 {
        l_exact += (ours[2 * i] == reference[2 * i]) as usize;
        r_exact += (ours[2 * i + 1] == reference[2 * i + 3]) as usize;
    }
    println!("  left identical {:.3}%, right (ref +1 sample) identical {:.3}%", l_exact as f64 * 100.0 / frames as f64, r_exact as f64 * 100.0 / frames as f64);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "replay" {
        return replay(&a[2], &a[3]);
    }
    let stack = ResourceStack::load(std::path::Path::new(&a[1]), &[]).unwrap();
    let rate: u32 = a.get(6).map_or(44100, |r| r.parse().unwrap());
    let volume: u32 = a.get(7).map_or(127, |v| v.parse().unwrap());
    let bank = Arc::new(Genmidi::parse(stack.lump("GENMIDI").unwrap()).unwrap());
    let song = Arc::new(Song::parse(stack.lump(&a[2].to_ascii_uppercase()).unwrap()).unwrap());
    let mut p = MusicPlayer::new(bank, rate);
    p.set_volume(volume);
    p.start_trace();
    p.play(song, true);
    let total = (a[3].parse::<f64>().unwrap() * rate as f64) as usize;
    let mut pcm = vec![0i16; total * 2];
    for chunk in pcm.chunks_mut(1024) {
        p.render(chunk);
    }
    let mut log = std::io::BufWriter::new(std::fs::File::create(&a[5]).unwrap());
    writeln!(log, "# start 0 rate {rate}").unwrap();
    for (t, r, v) in p.take_trace() {
        writeln!(log, "{t} {r:03x} {v:02x}").unwrap();
    }
    let mut wav = Vec::new();
    let data_len = (total * 4) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&a[4], wav).unwrap();
}
