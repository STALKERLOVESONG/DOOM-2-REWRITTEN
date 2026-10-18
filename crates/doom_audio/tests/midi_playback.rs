//! MIDI and digital music.
//!
//! - Every MUS song in the IWAD, converted to MIDI, must drive the OPL with
//!   exactly the register writes of the MUS player (which matches Chocolate
//!   Doom; see `opl_reference.rs`). Chocolate itself plays MUS by converting
//!   it to MIDI, so this checks the MIDI path against the same behaviour.
//! - Freedoom's MIDI soundtrack parses and renders.
//! - WAV music is played as a digital stream and loops.
//!
//! IWAD tests are skipped without `DOOM_IWAD` / `WADS/doom1.wad`
//! (and `FREEDOOM2_IWAD` / `WADS/freedoom2.wad`).

use std::path::PathBuf;
use std::sync::Arc;

use doom_audio::music::MusEvent;
use doom_audio::{AudioCommand, Genmidi, Mixer, MusicPlayer, Song};
use doom_content::{LumpId, ResourceStack};

fn wad(file: &str, var: &str) -> Option<ResourceStack> {
    let p = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS").join(file));
    p.exists().then(|| ResourceStack::load(&p, &[]).unwrap())
}

/// `D_*` music lumps, each name once (the latest definition wins).
fn music_lumps(stack: &ResourceStack) -> Vec<String> {
    let mut names: Vec<String> = (0..stack.num_lumps())
        .map(|i| stack.info(LumpId(i as u32)).name.to_string())
        .filter(|n| n.starts_with("D_"))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn var_len(out: &mut Vec<u8>, mut v: u32) {
    let mut bytes = vec![(v & 0x7f) as u8];
    v >>= 7;
    while v > 0 {
        bytes.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.extend(bytes.iter().rev());
}

/// MUS to a format-0 MIDI file, like Chocolate's `mus2mid` (70 ticks per
/// quarter note at the default 500000 µs tempo = 140 Hz), except for the
/// channel numbers: instead of allocating MIDI channels in order of use,
/// MUS channel `c` becomes the MIDI channel that `TrackChannelForEvent`
/// maps back to `c`, so both players use the same channel indices.
fn mus_to_midi(events: &[(MusEvent, u32)]) -> Vec<u8> {
    let midi_channel = |c: u8| match c {
        15 => 9,
        9 => 15,
        c => c,
    };
    let mut track = Vec::new();
    let mut velocity = [127u8; 16];
    let mut delta = 0u32;
    for &(ev, delay) in events {
        let mut emit = |bytes: &[u8], track: &mut Vec<u8>| {
            var_len(track, delta);
            track.extend_from_slice(bytes);
            delta = 0;
        };
        match ev {
            MusEvent::ReleaseNote { channel, note } => emit(&[0x80 | midi_channel(channel), note, 0x40], &mut track),
            MusEvent::PlayNote { channel, note, volume } => {
                if let Some(v) = volume {
                    velocity[channel as usize] = v;
                }
                emit(&[0x90 | midi_channel(channel), note, velocity[channel as usize]], &mut track);
            }
            MusEvent::PitchBend { channel, bend } => {
                let b = (bend as u16) << 6;
                emit(&[0xe0 | midi_channel(channel), (b & 0x7f) as u8, (b >> 7) as u8], &mut track);
            }
            MusEvent::System { channel, controller } => {
                let c = match controller {
                    10 => 120,
                    11 => 123,
                    12 => 126,
                    13 => 127,
                    _ => 121,
                };
                emit(&[0xb0 | midi_channel(channel), c, 0], &mut track);
            }
            MusEvent::Controller { channel, controller: 0, value } => emit(&[0xc0 | midi_channel(channel), value], &mut track),
            MusEvent::Controller { channel, controller, value } => {
                let c = [0, 0, 1, 7, 10, 11, 91, 93, 64, 67].get(controller as usize).copied().unwrap_or(0);
                emit(&[0xb0 | midi_channel(channel), c, value], &mut track);
            }
            MusEvent::ScoreEnd => {
                emit(&[0xff, 0x2f, 0x00], &mut track);
                break;
            }
            MusEvent::Nop => {}
        }
        delta += delay;
    }
    let mut file = b"MThd\0\0\0\x06\0\0\0\x01\0\x46MTrk".to_vec();
    file.extend_from_slice(&(track.len() as u32).to_be_bytes());
    file.extend_from_slice(&track);
    file
}

fn trace(bank: &Arc<Genmidi>, song: Song, seconds: u32) -> Vec<(u16, u8)> {
    let mut p = MusicPlayer::new(bank.clone(), 44100);
    p.set_volume(100);
    p.start_trace();
    p.play(Arc::new(song), true);
    let mut buf = vec![0i16; 2 * 44100];
    for _ in 0..seconds {
        p.render(&mut buf);
    }
    p.take_trace().iter().map(|w| (w.1, w.2)).collect()
}

#[test]
fn midi_matches_mus_player() {
    let Some(stack) = wad("doom1.wad", "DOOM_IWAD") else { return };
    let bank = Arc::new(Genmidi::parse(stack.lump("GENMIDI").unwrap()).unwrap());
    let mut songs = 0;
    for name in music_lumps(&stack) {
        let Ok(Song::Mus(score)) = Song::parse(stack.lump(&name).unwrap()) else { continue };
        let midi = Song::parse(&mus_to_midi(&score.events)).unwrap();
        assert!(matches!(midi, Song::Midi(_)));
        let a = trace(&bank, Song::Mus(score), 30);
        let b = trace(&bank, midi, 30);
        let n = a.len().min(b.len());
        if let Some(i) = (0..n).find(|&i| a[i] != b[i]) {
            panic!("{name}: write {i} of {n} differs: MUS {:x?}, MIDI {:x?}", a[i], b[i]);
        }
        // Chocolate truncates every MIDI delay to whole microseconds
        // (7142 µs for a 140 Hz tick), so MIDI runs up to ~0.01% fast and
        // has made a few more writes by the end.
        assert!(a.len().max(b.len()) - n <= n / 100 + 16, "{name}: {} vs {} writes", a.len(), b.len());
        songs += 1;
    }
    eprintln!("{songs} songs identical");
    assert!(songs > 0);
}

#[test]
fn freedoom_midi_renders() {
    let Some(stack) = wad("freedoom2.wad", "FREEDOOM2_IWAD") else { return };
    let bank = Arc::new(Genmidi::parse(stack.lump("GENMIDI").unwrap()).unwrap());
    let rate = 44100;
    let mut songs = 0;
    for name in music_lumps(&stack) {
        let song = Song::parse(stack.lump(&name).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(matches!(song, Song::Midi(_)), "{name} is {}", song.format_name());
        let mut mixer = Mixer::new(rate, 8, Some(bank.clone()));
        mixer.apply(AudioCommand::SetMusicVolume(100));
        mixer.apply(AudioCommand::PlayMusic { song: Arc::new(song), looping: true });
        let mut out = vec![0f32; rate as usize * 2 * 10];
        mixer.render(&mut out);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.002, "{name} is silent");
        songs += 1;
    }
    eprintln!("{songs} Freedoom songs render");
    assert!(songs > 30);
}

fn wav(rate: u32, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut w = b"RIFF".to_vec();
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(b"\x02\0\x10\0data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    w
}

#[test]
fn wav_music_streams_and_loops() {
    // 0.1 s of a constant level at 22050 Hz, played at 44100 Hz for 1 s.
    let song = Song::parse(&wav(22050, &[16384; 2205])).unwrap();
    let Song::Stream(ref s) = song else { panic!("expected a stream, got {}", song.format_name()) };
    assert_eq!((s.rate, s.channels, s.frames()), (22050, 1, 2205));
    let song = Arc::new(song);

    // No GENMIDI: digital music still plays.
    let mut mixer = Mixer::new(44100, 8, None);
    mixer.apply(AudioCommand::SetMusicVolume(127));
    mixer.apply(AudioCommand::PlayMusic { song: song.clone(), looping: true });
    let mut out = vec![0f32; 44100 * 2];
    mixer.render(&mut out);
    assert!(out.iter().all(|&x| (x - 0.5).abs() < 1e-3), "looping stream should be continuous");

    mixer.apply(AudioCommand::PauseMusic(true));
    mixer.render(&mut out);
    assert!(out.iter().all(|&x| x == 0.0), "paused");
    mixer.apply(AudioCommand::PauseMusic(false));

    mixer.apply(AudioCommand::PlayMusic { song, looping: false });
    mixer.render(&mut out);
    let playing = out.chunks(2).take_while(|f| f[0] != 0.0).count();
    assert!((4400..=4420).contains(&playing), "non-looping stream ended after {playing} frames");
    assert!(out[44100..].iter().all(|&x| x == 0.0));
}
