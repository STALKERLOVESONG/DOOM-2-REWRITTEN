use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ContentError {
    Io { path: PathBuf, source: std::io::Error },
    BadMagic { path: PathBuf, magic: [u8; 4] },
    CorruptContainer { path: PathBuf, reason: String },
    MissingLump(String),
    MalformedLump { lump: String, reason: String },
    /// Content that is recognised but not supported yet (e.g. UDMF maps).
    /// Distinct from `MalformedLump` so callers can report it honestly.
    Unsupported { what: String, reason: String },
}

impl ContentError {
    pub fn malformed(lump: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::MalformedLump { lump: lump.into(), reason: reason.into() }
    }
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::BadMagic { path, magic } => {
                write!(f, "{}: not a WAD (bad magic {:?})", path.display(), String::from_utf8_lossy(magic))
            }
            Self::CorruptContainer { path, reason } => write!(f, "{}: corrupt container ({reason})", path.display()),
            Self::MissingLump(n) => write!(f, "required lump `{n}` is missing"),
            Self::MalformedLump { lump, reason } => write!(f, "lump `{lump}` is malformed: {reason}"),
            Self::Unsupported { what, reason } => write!(f, "{what} is not supported yet: {reason}"),
        }
    }
}

impl std::error::Error for ContentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
