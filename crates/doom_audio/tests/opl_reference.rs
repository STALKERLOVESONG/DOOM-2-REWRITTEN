//! Music against the Chocolate Doom reference (fixtures from
//! tools/refopl/gen_fixtures.sh: 10 s of each song at 44100 Hz, volume 64).
//!
//! - `emulator_matches_reference`: replaying the reference's register writes
//!   through our `Opl` (same timing, buffered writes) reproduces its PCM
//!   bit for bit. Needs no WAD.
//! - `player_writes_match_reference`: our `MusicPlayer` (MUS + GENMIDI, DMX
//!   1.9 voice logic) issues the same register writes in the same order.
//!   Timing is not compared: the reference schedules each event from the
//!   moment the previous one was handled, so it falls slightly behind exact
//!   140 Hz (about 0.06%); we keep DMX's exact tick.

use std::path::PathBuf;
use std::sync::Arc;

use doom_audio::opl::Opl;
use doom_audio::{Genmidi, MusicPlayer, Song};
use doom_content::ResourceStack;

const SONGS: [&str; 3] = ["D_E1M1", "D_INTER", "D_E2M8"];

struct Fixture {
    pcm_hash: u64,
    pcm_bytes: usize,
    /// Song start, in output samples.
    start: u64,
    /// Index of the first write after the song started.
    first_song_write: usize,
    /// `(sample, register, value)` from chip reset.
    writes: Vec<(u64, u16, u8)>,
}

fn fixture(song: &str) -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{song}.opl"));
    let text = std::fs::read_to_string(path).unwrap();
    let mut f = Fixture { pcm_hash: 0, pcm_bytes: 0, start: 0, first_song_write: 0, writes: Vec::new() };
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["#", "pcm_fnv1a64", h, "bytes", n] => {
                f.pcm_hash = u64::from_str_radix(h, 16).unwrap();
                f.pcm_bytes = n.parse().unwrap();
            }
            ["#", "start", s, ..] => {
                f.start = s.parse().unwrap();
                f.first_song_write = f.writes.len();
            }
            ["#", ..] => {}
            [t, r, v] => f.writes.push((t.parse().unwrap(), u16::from_str_radix(r, 16).unwrap(), u8::from_str_radix(v, 16).unwrap())),
            _ => panic!("bad fixture line {line}"),
        }
    }
    f
}

fn fnv1a64(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

#[test]
fn emulator_matches_reference() {
    for song in SONGS {
        let f = fixture(song);
        let mut opl = Opl::new(44100);
        let frames = (f.pcm_bytes / 4) as u64;
        let mut pcm = Vec::with_capacity(f.pcm_bytes);
        let mut next = 0;
        for i in 0..f.start + frames {
            while next < f.writes.len() && f.writes[next].0 <= i {
                let (_, reg, v) = f.writes[next];
                // Timer registers belong to the driver, not the chip.
                if !(2..=4).contains(&reg) {
                    opl.write_buffered(reg, v);
                }
                next += 1;
            }
            let s = opl.generate();
            if i >= f.start {
                pcm.extend_from_slice(&s[0].to_le_bytes());
                pcm.extend_from_slice(&s[1].to_le_bytes());
            }
        }
        assert_eq!(fnv1a64(pcm.into_iter()), f.pcm_hash, "{song}: OPL output differs from the reference");
    }
}

#[test]
fn player_writes_match_reference() {
    let iwad = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    if !iwad.exists() {
        eprintln!("skipping: no IWAD");
        return;
    }
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let bank = Arc::new(Genmidi::parse(stack.lump("GENMIDI").unwrap()).unwrap());
    for song in SONGS {
        // The episode 2 song is missing from the shareware IWAD.
        let Some(lump) = stack.lump(song) else {
            eprintln!("{song}: not in this IWAD, skipped");
            continue;
        };
        let f = fixture(song);
        let reference: Vec<(u16, u8)> = f.writes[f.first_song_write..].iter().map(|w| (w.1, w.2)).collect();
        let mut p = MusicPlayer::new(bank.clone(), 44100);
        p.set_volume(64);
        p.start_trace();
        p.play(Arc::new(Song::parse(lump).unwrap()), true);
        let mut buf = vec![0i16; 2 * 44100 * 10];
        p.render(&mut buf);
        let ours: Vec<(u16, u8)> = p.take_trace().iter().map(|w| (w.1, w.2)).collect();
        // We run marginally ahead of the reference, so compare its writes.
        let n = reference.len().min(ours.len());
        assert!(n + 50 >= reference.len(), "{song}: only {} of {} writes", ours.len(), reference.len());
        if let Some(i) = (0..n).find(|&i| ours[i] != reference[i]) {
            panic!("{song}: write {i} differs: ours {:03x}={:02x}, reference {:03x}={:02x}", ours[i].0, ours[i].1, reference[i].0, reference[i].1);
        }
    }
}
