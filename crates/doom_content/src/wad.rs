//! WAD container format: 12-byte header + 16-byte directory entries.

use std::path::{Path, PathBuf};

use crate::le;
use crate::name::LumpName;
use crate::ContentError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WadKind {
    Iwad,
    Pwad,
}

#[derive(Debug, Clone)]
pub struct WadEntry {
    pub name: LumpName,
    pub offset: usize,
    pub size: usize,
    /// The directory entry pointed outside the file. Vanilla only fails when
    /// such a lump is actually read, so we keep the entry (it still occupies a
    /// lump number and can shadow other lumps) but expose it as empty.
    pub broken: bool,
}

/// A parsed WAD file. The whole file is kept in memory; lumps are slices of it.
pub struct WadFile {
    pub path: PathBuf,
    pub kind: WadKind,
    pub data: Vec<u8>,
    pub entries: Vec<WadEntry>,
    /// Non-fatal problems found while parsing.
    pub warnings: Vec<String>,
}

impl WadFile {
    pub fn open(path: &Path) -> Result<Self, ContentError> {
        let data = std::fs::read(path).map_err(|source| ContentError::Io { path: path.to_path_buf(), source })?;
        Self::parse(path.to_path_buf(), data)
    }

    pub fn parse(path: PathBuf, data: Vec<u8>) -> Result<Self, ContentError> {
        let corrupt = |reason: &str| ContentError::CorruptContainer { path: path.clone(), reason: reason.into() };
        if data.len() < 12 {
            return Err(corrupt("shorter than the 12-byte header"));
        }
        let magic = [data[0], data[1], data[2], data[3]];
        let kind = match &magic {
            b"IWAD" => WadKind::Iwad,
            b"PWAD" => WadKind::Pwad,
            _ => return Err(ContentError::BadMagic { path, magic }),
        };
        let numlumps = le::i32(&data, 4);
        let dir_ofs = le::i32(&data, 8);
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

        let mut entries = Vec::with_capacity(numlumps);
        let mut warnings = Vec::new();
        for i in 0..numlumps {
            let e = dir_ofs + i * 16;
            let filepos = le::i32(&data, e);
            let size = le::i32(&data, e + 4);
            let name = LumpName::from_raw(&data[e + 8..e + 16]);
            let in_bounds = filepos >= 0
                && size >= 0
                && (filepos as usize).checked_add(size as usize).is_some_and(|end| end <= data.len());
            if in_bounds {
                entries.push(WadEntry { name, offset: filepos as usize, size: size as usize, broken: false });
            } else {
                warnings.push(format!(
                    "{}: lump {i} ({name}) has an invalid offset/size ({filepos}, {size}); treated as empty",
                    path.display()
                ));
                entries.push(WadEntry { name, offset: 0, size: 0, broken: true });
            }
        }
        Ok(Self { path, kind, data, entries, warnings })
    }

    pub fn lump_data(&self, e: &WadEntry) -> &[u8] {
        &self.data[e.offset..e.offset + e.size]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build an in-memory WAD for tests.
    pub fn build_wad(kind: &[u8; 4], lumps: &[(&str, &[u8])]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(kind);
        data.extend_from_slice(&(lumps.len() as i32).to_le_bytes());
        data.extend_from_slice(&0i32.to_le_bytes()); // patched below
        let mut dir = Vec::new();
        for (name, bytes) in lumps {
            let ofs = data.len() as i32;
            data.extend_from_slice(bytes);
            dir.extend_from_slice(&ofs.to_le_bytes());
            dir.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
            let mut n = [0u8; 8];
            n[..name.len()].copy_from_slice(name.as_bytes());
            dir.extend_from_slice(&n);
        }
        let dir_ofs = data.len() as i32;
        data[8..12].copy_from_slice(&dir_ofs.to_le_bytes());
        data.extend_from_slice(&dir);
        data
    }

    #[test]
    fn parses_header_and_directory() {
        let bytes = build_wad(b"PWAD", &[("FOO", b"abc"), ("BAR", b"")]);
        let w = WadFile::parse("t.wad".into(), bytes).unwrap();
        assert_eq!(w.kind, WadKind::Pwad);
        assert_eq!(w.entries.len(), 2);
        assert_eq!(w.lump_data(&w.entries[0]), b"abc");
        assert_eq!(w.entries[1].name, LumpName::new("BAR"));
    }

    #[test]
    fn rejects_bad_magic() {
        let bytes = build_wad(b"ZWAD", &[]);
        assert!(matches!(WadFile::parse("t.wad".into(), bytes), Err(ContentError::BadMagic { .. })));
    }
}
