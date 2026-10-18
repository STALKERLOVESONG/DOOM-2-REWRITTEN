//! Decode the IWAD's sounds and render music. Skipped without the IWAD.
//! `AUDIO_WAV_OUT=<dir>` writes WAV files for listening.

use std::path::PathBuf;
use std::sync::Arc;

use doom_audio::{AudioCommand, Genmidi, Mixer, Song, Sound};
use doom_content::ResourceStack;

fn stack() -> Option<ResourceStack> {
    let p = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    p.exists().then(|| ResourceStack::load(&p, &[]).unwrap())
}

fn write_wav(path: &std::path::Path, rate: u32, samples: &[f32]) {
    let data: Vec<u8> = samples.iter().flat_map(|s| ((s * 32767.0) as i16).to_le_bytes()).collect();
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    std::fs::write(path, w).unwrap();
}

#[test]
fn all_sound_lumps_decode() {
    let Some(stack) = stack() else { return };
    let names: Vec<String> = (0..stack.num_lumps())
        .map(|i| stack.info(doom_content::LumpId(i as u32)).name.to_string())
        .filter(|n| n.starts_with("DS"))
        .collect();
    assert!(names.len() > 50);
    for n in &names {
        let s = Sound::parse_dmx(stack.lump(n).unwrap()).unwrap_or_else(|| panic!("{n} failed to decode"));
        assert!(s.rate == 11025 || s.rate == 22050, "{n}: {}", s.rate);
    }
}

#[test]
fn music_renders() {
    let Some(stack) = stack() else { return };
    let bank = Arc::new(Genmidi::parse(stack.lump("GENMIDI").unwrap()).unwrap());
    let rate = 44100;
    let mut mixer = Mixer::new(rate, 8, Some(bank));
    mixer.apply(AudioCommand::SetMusicVolume(100));
    for name in ["D_E1M1", "D_INTRO", "D_E2M1", "D_INTER", "D_VICTOR"] {
        let Some(lump) = stack.lump(name) else { continue };
        let song = Arc::new(Song::parse(lump).unwrap());
        assert!(matches!(&*song, Song::Mus(m) if m.events.len() > 10));
        mixer.apply(AudioCommand::PlayMusic { song, looping: false });
        let mut out = vec![0f32; rate as usize * 2 * 8];
        mixer.render(&mut out);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        let dc = out.iter().sum::<f32>() / out.len() as f32;
        eprintln!("{name}: rms {rms:.4} dc {dc:.5}");
        assert!(rms > 0.002, "{name} is silent");
        assert!(dc.abs() < 0.02, "{name} has a DC offset");
        if let Some(dir) = std::env::var_os("AUDIO_WAV_OUT") {
            std::fs::create_dir_all(&dir).unwrap();
            write_wav(&PathBuf::from(&dir).join(format!("{name}.wav")), rate, &out);
        }
    }
    // Sound effects mix on top.
    let pistol = Arc::new(Sound::parse_dmx(stack.lump("DSPISTOL").unwrap()).unwrap());
    mixer.apply(AudioCommand::StopMusic);
    mixer.apply(AudioCommand::StartSound { channel: 0, sound: pistol.clone(), volume: 127, separation: 128, looping: false });
    let mut out = vec![0f32; 4410 * 2];
    mixer.render(&mut out);
    assert!(out.iter().any(|s| s.abs() > 0.05));
}
