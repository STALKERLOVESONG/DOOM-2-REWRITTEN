//! Eight-character lump names.

use std::fmt;

/// A lump name: up to 8 bytes, uppercased, NUL-padded.
///
/// Vanilla compares names case-insensitively (`W_CheckNumForName` uppercases
/// the query; Chocolate Doom uses `strncasecmp`). We normalise to uppercase at
/// construction so comparisons and hashing are plain byte operations.
/// Bytes after the first NUL are discarded, as the C string compare would.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct LumpName(pub [u8; 8]);

impl LumpName {
    pub fn from_raw(raw: &[u8]) -> Self {
        let mut out = [0u8; 8];
        for (o, &b) in out.iter_mut().zip(raw.iter().take(8)) {
            if b == 0 {
                break;
            }
            *o = b.to_ascii_uppercase();
        }
        LumpName(out)
    }

    /// From a string; longer names are truncated to 8 bytes like the C code.
    pub fn new(s: &str) -> Self {
        Self::from_raw(s.as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8] {
        let len = self.0.iter().position(|&b| b == 0).unwrap_or(8);
        &self.0[..len]
    }

    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.as_bytes())
    }

    pub fn is_empty(&self) -> bool {
        self.0[0] == 0
    }

    pub fn starts_with(&self, prefix: &[u8]) -> bool {
        self.as_bytes().starts_with(prefix)
    }

    /// Sidedef texture fields use "-" for "no texture".
    pub fn is_no_texture(&self) -> bool {
        self.is_empty() || self.as_bytes() == b"-"
    }
}

impl fmt::Display for LumpName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl fmt::Debug for LumpName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LumpName({:?})", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises() {
        assert_eq!(LumpName::from_raw(b"e1m1\0xyz"), LumpName::new("E1M1"));
        assert_eq!(LumpName::new("TOOLONGNAME").to_string(), "TOOLONGN");
        assert!(LumpName::new("-").is_no_texture());
    }
}
