//! Finding and recognising game data: which file is which IWAD, and where
//! the usual installs (Steam, GOG, a `WADS` folder) keep them.
//!
//! Only the WAD directory is read (header and lump names), never the whole
//! file. Plain std, no engine crates, so the launcher starts instantly.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// What a WAD file is, from its header and lump names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WadInfo {
    /// `IWAD` header (a game) rather than `PWAD` (a mod).
    pub iwad: bool,
    /// The game, for IWADs ("DOOM II: Hell on Earth").
    pub title: String,
    pub lumps: usize,
}

/// File names IWADs ship under, lower case.
pub const IWAD_NAMES: &[&str] =
    &["doom.wad", "doom1.wad", "doomu.wad", "doom2.wad", "doom2f.wad", "tnt.wad", "plutonia.wad", "freedoom1.wad", "freedoom2.wad", "freedm.wad"];

/// Read a WAD's header and directory.
pub fn identify(path: &Path) -> Result<WadInfo, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut header = [0u8; 12];
    f.read_exact(&mut header).map_err(|_| "too short to be a WAD".to_string())?;
    let iwad = match &header[0..4] {
        b"IWAD" => true,
        b"PWAD" => false,
        _ => return Err("not a WAD file".into()),
    };
    let numlumps = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let dir_ofs = i32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if !(0..=1_000_000).contains(&numlumps) || dir_ofs < 0 {
        return Err("corrupt WAD header".into());
    }
    let mut dir = vec![0u8; numlumps as usize * 16];
    f.seek(SeekFrom::Start(dir_ofs as u64)).map_err(|e| e.to_string())?;
    f.read_exact(&mut dir).map_err(|_| "WAD directory is cut short".to_string())?;
    let names: HashSet<String> = dir
        .chunks_exact(16)
        .map(|e| {
            let raw = &e[8..16];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(8);
            String::from_utf8_lossy(&raw[..end]).to_ascii_uppercase()
        })
        .collect();
    Ok(WadInfo { iwad, title: game_title(&names, iwad), lumps: numlumps as usize })
}

/// Which game a set of lump names makes, like the engine's IWAD
/// identification (and GZDoom's lump tests for Freedoom, TNT, Plutonia).
fn game_title(lumps: &HashSet<String>, iwad: bool) -> String {
    let has = |n: &str| lumps.contains(n);
    let freedoom = has("FREEDOOM");
    let t = if has("MAP01") {
        if freedoom && has("FREEDM") {
            "FreeDM"
        } else if freedoom {
            "Freedoom: Phase 2"
        } else if has("REDTNT2") {
            "Final DOOM: TNT - Evilution"
        } else if has("CAMO1") {
            "Final DOOM: The Plutonia Experiment"
        } else {
            "DOOM II: Hell on Earth"
        }
    } else if has("E1M1") {
        if freedoom {
            "Freedoom: Phase 1"
        } else if has("E4M1") {
            "The Ultimate DOOM"
        } else if has("E2M1") {
            "DOOM (registered)"
        } else {
            "DOOM Shareware"
        }
    } else if iwad {
        "Unknown game"
    } else {
        "Mod"
    };
    t.to_string()
}

/// Places IWADs are usually found, in a sensible order: next to the
/// launcher, a `WADS` folder, `DOOMWADDIR`/`DOOMWADPATH`, then Steam and
/// GOG installs.
pub fn find_iwads(exe_dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![exe_dir.to_path_buf(), exe_dir.join("WADS")];
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("WADS"));
        dirs.push(cwd);
    }
    for var in ["DOOMWADDIR", "DOOMWADPATH"] {
        if let Some(v) = std::env::var_os(var) {
            dirs.extend(std::env::split_paths(&v));
        }
    }
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |p: PathBuf, found: &mut Vec<PathBuf>| {
        let key = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if seen.insert(key) {
            found.push(p);
        }
    };
    for d in &dirs {
        for p in iwads_in(d) {
            add(p, &mut found);
        }
    }
    for root in store_game_dirs() {
        for p in search_install(&root, 3) {
            add(p, &mut found);
        }
    }
    found
}

/// IWAD-named files directly in `dir`.
fn iwads_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| IWAD_NAMES.contains(&n.to_ascii_lowercase().as_str())))
        .collect();
    v.sort();
    v
}

/// IWADs anywhere up to `depth` folders below a game install folder
/// (Steam keeps them in `base/`, the 2024 re-release in `rerelease/`).
fn search_install(dir: &Path, depth: u32) -> Vec<PathBuf> {
    let mut out = iwads_in(dir);
    if depth > 0 {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut subs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
            subs.sort();
            for s in subs {
                out.extend(search_install(&s, depth - 1));
            }
        }
    }
    out
}

/// Game folders from Steam libraries and GOG whose name mentions Doom.
fn store_game_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    // Steam: the default library plus every library in libraryfolders.vdf.
    let steam = program_files_x86.join("Steam");
    let mut libraries = vec![steam.clone()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        libraries.extend(steam_library_paths(&vdf));
    }
    for lib in libraries {
        roots.push(lib.join("steamapps").join("common"));
    }
    // GOG Galaxy and the classic GOG installer folders.
    roots.push(program_files_x86.join("GOG Galaxy").join("Games"));
    roots.push(program_files.join("GOG Galaxy").join("Games"));
    roots.push(PathBuf::from(r"C:\GOG Games"));
    roots.push(PathBuf::from(r"C:\GOG Galaxy\Games"));

    let mut dirs = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() && e.file_name().to_string_lossy().to_ascii_lowercase().contains("doom") {
                dirs.push(p);
            }
        }
    }
    dirs.sort();
    dirs
}

/// `"path"   "D:\\SteamLibrary"` lines from Steam's libraryfolders.vdf.
pub fn steam_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("\"path\"")?.trim();
            let v = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(PathBuf::from(v.replace("\\\\", "\\")))
        })
        .collect()
}

/// Is this a file the engine can load as a mod?
pub fn is_mod_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| MOD_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Mod file types offered in the file picker and accepted by drag and drop.
pub const MOD_EXTENSIONS: &[&str] = &["wad", "pk3", "zip", "deh", "bex"];

#[cfg(test)]
mod tests {
    use super::*;

    fn wad(kind: &[u8; 4], names: &[&str]) -> PathBuf {
        let mut data = kind.to_vec();
        data.extend_from_slice(&(names.len() as i32).to_le_bytes());
        data.extend_from_slice(&12i32.to_le_bytes());
        for n in names {
            data.extend_from_slice(&[0; 8]);
            let mut raw = [0u8; 8];
            raw[..n.len()].copy_from_slice(n.as_bytes());
            data.extend_from_slice(&raw);
        }
        let p = std::env::temp_dir().join(format!("launcher_test_{}_{}.wad", std::process::id(), names.join("_")));
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn identifies_games() {
        let cases: &[(&[u8; 4], &[&str], bool, &str)] = &[
            (b"IWAD", &["PLAYPAL", "MAP01"], true, "DOOM II: Hell on Earth"),
            (b"IWAD", &["MAP01", "REDTNT2"], true, "Final DOOM: TNT - Evilution"),
            (b"IWAD", &["MAP01", "CAMO1"], true, "Final DOOM: The Plutonia Experiment"),
            (b"IWAD", &["E1M1", "E2M1", "E4M1"], true, "The Ultimate DOOM"),
            (b"IWAD", &["E1M1"], true, "DOOM Shareware"),
            (b"IWAD", &["FREEDOOM", "MAP01"], true, "Freedoom: Phase 2"),
            (b"PWAD", &["MAP07", "TEXTMAP"], false, "Mod"),
        ];
        for (kind, names, iwad, title) in cases {
            let p = wad(kind, names);
            let info = identify(&p).unwrap();
            std::fs::remove_file(&p).ok();
            assert_eq!((info.iwad, info.title.as_str(), info.lumps), (*iwad, *title, names.len()));
        }
    }

    #[test]
    fn rejects_non_wads() {
        let p = std::env::temp_dir().join(format!("launcher_test_{}.txt", std::process::id()));
        std::fs::write(&p, b"hello, this is not a wad").unwrap();
        assert!(identify(&p).is_err());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn steam_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(steam_library_paths(vdf), vec![PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn mod_extensions() {
        assert!(is_mod_file(Path::new("x/MyHouse.PK3")));
        assert!(is_mod_file(Path::new("patch.deh")));
        assert!(!is_mod_file(Path::new("readme.txt")));
    }
}
