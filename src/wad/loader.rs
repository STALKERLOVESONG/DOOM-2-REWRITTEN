//! WAD container parsing and IWAD/PWAD directory merging.
//!
//! Every lump of every file is appended to one ordered directory. Name lookups
//! resolve to the *last* lump with that name, which gives classic DOOM override
//! semantics (PWAD over IWAD, later PWAD over earlier PWAD). Map data is found
//! positionally after its marker lump, so a PWAD map replaces the whole map.

use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use bevy::log::{info, warn};
use bevy::prelude::Resource;
use byteorder::{LittleEndian, ReadBytesExt};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WadError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: not a WAD (bad magic {magic:?})")]
    BadMagic { path: PathBuf, magic: [u8; 4] },
    #[error("{path}: corrupt WAD ({reason})")]
    CorruptDirectory { path: PathBuf, reason: String },
    #[error("required lump `{0}` is missing")]
    MissingLump(String),
    #[error("lump `{lump}` is malformed: {reason}")]
    MalformedLump { lump: String, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WadKind {
    Iwad,
    Pwad,
}

#[derive(Debug, Clone)]
pub struct LumpEntry {
    pub name: String,
    wad: usize,
    offset: usize,
    size: usize,
}

struct WadFile {
    path: PathBuf,
    kind: WadKind,
    data: Vec<u8>,
}

/// Names of the lumps that follow a map marker, in canonical order.
const MAP_LUMPS: [&str; 10] = [
    "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT",
    "BLOCKMAP",
];

/// Convert a NUL-padded 8-byte lump name to an uppercase `String`.
pub fn lump_name(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).to_ascii_uppercase()
}

#[derive(Resource)]
pub struct WadDirectory {
    files: Vec<WadFile>,
    entries: Vec<LumpEntry>,
    lookup: HashMap<String, usize>,
}

impl WadDirectory {
    /// Load the IWAD and then each PWAD in order.
    pub fn load(iwad: &Path, pwads: &[PathBuf]) -> Result<Self, WadError> {
        let mut dir = Self {
            files: Vec::new(),
            entries: Vec::new(),
            lookup: HashMap::new(),
        };
        if dir.add_wad(iwad)? != WadKind::Iwad {
            warn!("{} is not tagged IWAD; treating it as the base anyway", iwad.display());
        }
        for p in pwads {
            if dir.add_wad(p)? == WadKind::Iwad {
                warn!("{} is an IWAD; merging it as an override layer", p.display());
            }
        }
        for f in &dir.files {
            info!("WAD {:?}: {} ({} bytes)", f.kind, f.path.display(), f.data.len());
        }
        info!("merged directory: {} lumps ({} unique names)", dir.entries.len(), dir.lookup.len());
        Ok(dir)
    }

    /// Parse one WAD file and layer its lumps on top of the current directory.
    pub fn add_wad(&mut self, path: &Path) -> Result<WadKind, WadError> {
        let data = fs::read(path).map_err(|source| WadError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let corrupt = |reason: &str| WadError::CorruptDirectory {
            path: path.to_path_buf(),
            reason: reason.to_string(),
        };

        if data.len() < 12 {
            return Err(corrupt("shorter than the 12-byte header"));
        }
        let magic: [u8; 4] = [data[0], data[1], data[2], data[3]];
        let kind = match &magic {
            b"IWAD" => WadKind::Iwad,
            b"PWAD" => WadKind::Pwad,
            _ => {
                return Err(WadError::BadMagic {
                    path: path.to_path_buf(),
                    magic,
                })
            }
        };

        let mut hdr = Cursor::new(&data[4..12]);
        let numlumps = hdr
            .read_i32::<LittleEndian>()
            .map_err(|_| corrupt("truncated header"))?;
        let dir_ofs = hdr
            .read_i32::<LittleEndian>()
            .map_err(|_| corrupt("truncated header"))?;
        if numlumps < 0 || dir_ofs < 0 {
            return Err(corrupt("negative header field"));
        }
        let (numlumps, dir_ofs) = (numlumps as usize, dir_ofs as usize);
        let dir_end = numlumps
            .checked_mul(16)
            .and_then(|n| n.checked_add(dir_ofs))
            .ok_or_else(|| corrupt("directory size overflow"))?;
        if dir_end > data.len() {
            return Err(corrupt("directory extends past end of file"));
        }

        let wad_idx = self.files.len();
        let mut cur = Cursor::new(&data[dir_ofs..dir_end]);
        for _ in 0..numlumps {
            let filepos = cur.read_i32::<LittleEndian>().map_err(|_| corrupt("short directory"))?;
            let size = cur.read_i32::<LittleEndian>().map_err(|_| corrupt("short directory"))?;
            let mut raw = [0u8; 8];
            cur.read_exact(&mut raw).map_err(|_| corrupt("short directory"))?;
            let name = lump_name(&raw);
            let (offset, size) = (filepos.max(0) as usize, size.max(0) as usize);
            if size > 0 && offset.checked_add(size).map_or(true, |end| end > data.len()) {
                warn!("{}: lump {name} lies outside the file; skipped", path.display());
                continue;
            }
            let idx = self.entries.len();
            self.entries.push(LumpEntry {
                name: name.clone(),
                wad: wad_idx,
                offset,
                size,
            });
            self.lookup.insert(name, idx); // later definitions win
        }

        self.files.push(WadFile {
            path: path.to_path_buf(),
            kind,
            data,
        });
        Ok(kind)
    }

    fn entry_data(&self, e: &LumpEntry) -> &[u8] {
        &self.files[e.wad].data[e.offset..e.offset + e.size]
    }

    /// Lump data by name (with PWAD override applied).
    pub fn try_lump(&self, name: &str) -> Option<&[u8]> {
        let idx = *self.lookup.get(&name.to_ascii_uppercase())?;
        Some(self.entry_data(&self.entries[idx]))
    }

    pub fn lump(&self, name: &str) -> Result<&[u8], WadError> {
        self.try_lump(name)
            .ok_or_else(|| WadError::MissingLump(name.to_ascii_uppercase()))
    }

    pub fn lump_names(&self) -> impl Iterator<Item = &str> {
        self.lookup.keys().map(String::as_str)
    }

    /// First map present: `E1M1`, otherwise `MAP01`.
    pub fn default_map(&self) -> Option<String> {
        ["E1M1", "MAP01"]
            .into_iter()
            .find(|m| self.lookup.contains_key(*m))
            .map(str::to_string)
    }

    /// The map lumps that follow `marker`, resolved positionally so a PWAD's
    /// map fully replaces the IWAD's.
    pub fn map_lumps(&self, marker: &str) -> Result<HashMap<&'static str, &[u8]>, WadError> {
        let marker = marker.to_ascii_uppercase();
        let idx = *self
            .lookup
            .get(&marker)
            .ok_or_else(|| WadError::MissingLump(marker.clone()))?;
        let mut out = HashMap::new();
        for e in self.entries[idx + 1..].iter().take(MAP_LUMPS.len()) {
            match MAP_LUMPS.iter().find(|n| **n == e.name) {
                Some(n) => {
                    out.insert(*n, self.entry_data(e));
                }
                None => break, // reached the next map or an unrelated lump
            }
        }
        Ok(out)
    }

    /// All 64x64 flats between `F_START`/`F_END` style markers. Later lumps
    /// with the same name override earlier ones when collected into a map.
    pub fn flats(&self) -> Vec<(&str, &[u8])> {
        const STARTS: [&str; 5] = ["F_START", "FF_START", "F1_START", "F2_START", "F3_START"];
        const ENDS: [&str; 5] = ["F_END", "FF_END", "F1_END", "F2_END", "F3_END"];
        let mut inside = false;
        let mut out = Vec::new();
        for e in &self.entries {
            if STARTS.contains(&e.name.as_str()) {
                inside = true;
            } else if ENDS.contains(&e.name.as_str()) {
                inside = false;
            } else if inside && e.size == 4096 {
                out.push((e.name.as_str(), self.entry_data(e)));
            }
        }
        out
    }
}
