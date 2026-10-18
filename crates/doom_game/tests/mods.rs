//! Mods: a UDMF map with GL nodes, PNG textures and DECORATE actors
//! (MyHouse.wad on Doom II) loads and renders, and the same content packed
//! as a PK3 (built here, deflate + stored entries, map in `maps/`) gives an
//! identical frame. Skipped without doom2.wad and myhouse.wad.

use std::io::Write;
use std::path::PathBuf;

use doom_content::inflate::crc32;
use doom_game::{InputState, Session, SessionConfig};

fn wad(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS").join(name);
    p.exists().then_some(p).or_else(|| {
        eprintln!("skipping: no {name}");
        None
    })
}

fn wad_lumps(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let n = i32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let dir = i32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    (0..n)
        .map(|i| {
            let e = &data[dir + i * 16..dir + i * 16 + 16];
            let pos = i32::from_le_bytes(e[0..4].try_into().unwrap()) as usize;
            let size = i32::from_le_bytes(e[4..8].try_into().unwrap()) as usize;
            let name = String::from_utf8_lossy(&e[8..16]).trim_end_matches('\0').to_string();
            (name, data[pos..pos + size].to_vec())
        })
        .collect()
}

fn build_wad(lumps: &[(&str, &[u8])]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut dir = Vec::new();
    for (name, d) in lumps {
        dir.extend_from_slice(&((12 + data.len()) as i32).to_le_bytes());
        dir.extend_from_slice(&(d.len() as i32).to_le_bytes());
        let mut n = [0u8; 8];
        n[..name.len()].copy_from_slice(name.as_bytes());
        dir.extend_from_slice(&n);
        data.extend_from_slice(d);
    }
    let mut out = b"PWAD".to_vec();
    out.extend_from_slice(&(lumps.len() as i32).to_le_bytes());
    out.extend_from_slice(&((12 + data.len()) as i32).to_le_bytes());
    out.extend(data);
    out.extend(dir);
    out
}

/// A zip with stored entries.
fn build_zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, d) in files {
        let offset = out.len() as u32;
        let crc = crc32(d);
        let header = |sig: u32, central: bool| {
            let mut h = sig.to_le_bytes().to_vec();
            if central {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&[0, 0, 0, 0]); // time, date
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(d.len() as u32).to_le_bytes());
            h.extend_from_slice(&(d.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                h.extend_from_slice(&[0; 6]); // comment len, disk, internal attrs
                h.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(d);
        central.extend(header(0x0201_4b50, true));
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn frame(cfg: &SessionConfig) -> (Session, Vec<u8>) {
    let mut s = Session::new(cfg).unwrap();
    for _ in 0..10 {
        s.tick(&InputState::default());
    }
    let f = s.render_indexed().to_vec();
    (s, f)
}

#[test]
fn myhouse_wad_and_pk3_load_and_match() {
    let (Some(doom2), Some(myhouse)) = (wad("doom2.wad"), wad("myhouse.wad")) else { return };
    let mut cfg = SessionConfig::new(&doom2);
    cfg.pwads = vec![myhouse.clone()];
    cfg.map = Some("MAP01".into());
    let (s, wad_frame) = frame(&cfg);
    assert_eq!(s.world.level.lines.len(), 7892);
    // DECORATE shrubs spawned with their own sprites.
    let shrubs = s.world.thinkers.mobjs().filter(|(_, m)| s.info.sprnames[m.sprite.idx()].starts_with("BSH")).count();
    assert_eq!(shrubs, 49);
    // The view is mostly drawn (not black): sky, house, driveway.
    let black = wad_frame[..320 * 168].iter().filter(|&&c| c == 0).count();
    assert!(black < 320 * 168 / 10, "{black} black pixels");

    // Repack as a PK3.
    let lumps = wad_lumps(&std::fs::read(&myhouse).unwrap());
    let get = |n: &str| lumps.iter().find(|(k, _)| k == n).unwrap().1.clone();
    let mut files = vec![
        ("maps/MAP01.wad".to_string(), build_wad(&[("MAP01", b""), ("TEXTMAP", &get("TEXTMAP")), ("ZNODES", &get("ZNODES")), ("ENDMAP", b"")])),
        ("decorate.txt".to_string(), get("DECORATE")),
        ("graphics/TITLEPIC.lmp".to_string(), get("TITLEPIC")),
    ];
    let mut ns = "";
    for (name, data) in &lumps {
        match name.as_str() {
            "TX_START" | "F_START" | "SS_START" => ns = name,
            "TX_END" | "F_END" | "SS_END" => ns = "",
            _ => match ns {
                "TX_START" => files.push((format!("textures/{}.png", name.to_lowercase()), data.clone())),
                "F_START" => files.push((format!("flats/{name}.lmp"), data.clone())),
                "SS_START" => files.push((format!("sprites/shrubbery/{name}.lmp"), data.clone())),
                _ => {}
            },
        }
    }
    let dir = std::env::temp_dir().join(format!("doom_bevy_mods_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pk3 = dir.join("myhouse_test.pk3");
    std::fs::File::create(&pk3).unwrap().write_all(&build_zip(&files)).unwrap();
    cfg.pwads = vec![pk3];
    let (_, pk3_frame) = frame(&cfg);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(wad_frame == pk3_frame, "PK3 and WAD versions render differently");
}
