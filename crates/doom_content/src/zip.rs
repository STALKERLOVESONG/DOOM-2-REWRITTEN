//! Zip archives (PK3, PK7-as-zip, IPK3): central directory parsing and
//! entry extraction. Stored and DEFLATE entries are supported, which covers
//! virtually every Doom mod; other methods are listed so the resource stack
//! can warn about them.

use crate::inflate::{crc32, inflate};
use crate::ContentError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflate,
    /// Anything else (bzip2, LZMA, deflate64, zstd...), by method number.
    Other(u16),
}

#[derive(Clone, Debug)]
pub struct ZipEntry {
    /// Path inside the archive with `/` separators, as stored (case kept).
    pub name: String,
    pub method: Method,
    pub compressed_size: u64,
    pub size: u64,
    pub crc: u32,
    pub encrypted: bool,
    /// Offset of the entry's local header.
    local_offset: u64,
}

impl ZipEntry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

#[derive(Clone, Debug)]
pub struct ZipArchive {
    pub entries: Vec<ZipEntry>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

fn bad(what: &str, reason: impl Into<String>) -> ContentError {
    ContentError::malformed(what, reason)
}

impl ZipArchive {
    /// Read the central directory of a zip file held in memory.
    pub fn parse(what: &str, d: &[u8]) -> Result<ZipArchive, ContentError> {
        // The end-of-central-directory record is within the last 64K + 22
        // bytes (it may be followed by a comment).
        let min = d.len().saturating_sub(0xffff + 22);
        let eocd = (min..d.len().saturating_sub(21))
            .rev()
            .find(|&i| u32_at(d, i) == Some(EOCD_SIG))
            .ok_or_else(|| bad(what, "not a zip file (no end of central directory)"))?;
        let mut count = u16_at(d, eocd + 10).unwrap() as u64;
        let mut cd_size = u32_at(d, eocd + 12).unwrap() as u64;
        let mut cd_offset = u32_at(d, eocd + 16).unwrap() as u64;
        // Zip64: the 32-bit fields are saturated and a locator precedes.
        if (count == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff)
            && eocd >= 20
            && u32_at(d, eocd - 20) == Some(EOCD64_LOCATOR_SIG)
        {
            let rec = u64_at(d, eocd - 12).unwrap() as usize;
            if u32_at(d, rec) != Some(EOCD64_SIG) {
                return Err(bad(what, "zip64 end record missing"));
            }
            count = u64_at(d, rec + 32).ok_or_else(|| bad(what, "truncated zip64 record"))?;
            cd_size = u64_at(d, rec + 40).ok_or_else(|| bad(what, "truncated zip64 record"))?;
            cd_offset = u64_at(d, rec + 48).ok_or_else(|| bad(what, "truncated zip64 record"))?;
        }
        let mut pos = cd_offset as usize;
        if pos.checked_add(cd_size as usize).map_or(true, |end| end > d.len()) {
            return Err(bad(what, "central directory out of range"));
        }
        let mut entries = Vec::with_capacity(count as usize);
        for i in 0..count {
            if u32_at(d, pos) != Some(CENTRAL_SIG) {
                return Err(bad(what, format!("central directory entry {i} is corrupt")));
            }
            let field = |o: usize| u16_at(d, pos + o).ok_or_else(|| bad(what, "truncated central directory"));
            let flags = field(8)?;
            let method = match field(10)? {
                0 => Method::Stored,
                8 => Method::Deflate,
                m => Method::Other(m),
            };
            let crc = u32_at(d, pos + 16).unwrap_or(0);
            let mut compressed_size = u32_at(d, pos + 20).unwrap_or(0) as u64;
            let mut size = u32_at(d, pos + 24).unwrap_or(0) as u64;
            let (name_len, extra_len, comment_len) = (field(28)? as usize, field(30)? as usize, field(32)? as usize);
            let mut local_offset = u32_at(d, pos + 42).unwrap_or(0) as u64;
            let name_bytes = d.get(pos + 46..pos + 46 + name_len).ok_or_else(|| bad(what, "truncated file name"))?;
            // Bit 11: UTF-8 names; otherwise CP437, which is ASCII for every
            // name a Doom mod uses.
            let name = String::from_utf8_lossy(name_bytes).replace('\\', "/");
            // Zip64 extended information extra field (id 1).
            let extra = d.get(pos + 46 + name_len..pos + 46 + name_len + extra_len).unwrap_or(&[]);
            let mut e = 0;
            while e + 4 <= extra.len() {
                let (id, len) = (u16_at(extra, e).unwrap(), u16_at(extra, e + 2).unwrap() as usize);
                if id == 1 {
                    let mut p = e + 4;
                    let mut next = |v: &mut u64| {
                        if *v == 0xffff_ffff {
                            if let Some(x) = u64_at(extra, p) {
                                *v = x;
                                p += 8;
                            }
                        }
                    };
                    next(&mut size);
                    next(&mut compressed_size);
                    next(&mut local_offset);
                }
                e += 4 + len;
            }
            entries.push(ZipEntry { name, method, compressed_size, size, crc, encrypted: flags & 1 != 0, local_offset });
            pos += 46 + name_len + extra_len + comment_len;
        }
        Ok(ZipArchive { entries })
    }

    /// The (possibly compressed) bytes of an entry.
    pub fn raw<'a>(&self, d: &'a [u8], e: &ZipEntry) -> Result<&'a [u8], ContentError> {
        let lo = e.local_offset as usize;
        if u32_at(d, lo) != Some(LOCAL_SIG) {
            return Err(bad(&e.name, "local header missing"));
        }
        let start = lo + 30 + u16_at(d, lo + 26).unwrap() as usize + u16_at(d, lo + 28).unwrap() as usize;
        d.get(start..start + e.compressed_size as usize).ok_or_else(|| bad(&e.name, "entry data out of range"))
    }

    /// Extract an entry, checking its CRC.
    pub fn read(&self, d: &[u8], e: &ZipEntry) -> Result<Vec<u8>, ContentError> {
        if e.encrypted {
            return Err(ContentError::Unsupported { what: e.name.clone(), reason: "encrypted zip entry".into() });
        }
        let raw = self.raw(d, e)?;
        let out = match e.method {
            Method::Stored => raw.to_vec(),
            Method::Deflate => inflate(raw, e.size as usize).map_err(|err| bad(&e.name, err.to_string()))?,
            Method::Other(m) => {
                return Err(ContentError::Unsupported {
                    what: e.name.clone(),
                    reason: format!("zip compression method {m} (only stored and deflate are supported)"),
                })
            }
        };
        if out.len() as u64 != e.size {
            return Err(bad(&e.name, format!("size {} but expected {}", out.len(), e.size)));
        }
        if crc32(&out) != e.crc {
            return Err(bad(&e.name, "CRC mismatch"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_python_zip() {
        let d = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/small.pk3")).unwrap();
        let z = ZipArchive::parse("small.pk3", &d).unwrap();
        let names: Vec<_> = z.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["stored.txt", "dir/deflated.txt", "Sprites/TROOA1.png", "textures/"]);
        assert_eq!(z.read(&d, &z.entries[0]).unwrap(), b"hello stored");
        assert_eq!(z.entries[1].method, Method::Deflate);
        assert_eq!(z.read(&d, &z.entries[1]).unwrap(), b"hello deflated ".repeat(100));
        assert!(z.entries[3].is_dir());
        assert!(ZipArchive::parse("x", b"not a zip at all").is_err());
    }
}
