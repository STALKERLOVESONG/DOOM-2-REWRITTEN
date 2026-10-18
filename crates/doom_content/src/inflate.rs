//! DEFLATE (RFC 1951) and zlib (RFC 1950) decompression, dependency-free.
//!
//! Used by PK3 (zip) archives, PNG images and compressed ZDoom nodes. The
//! decoder is a straightforward canonical-Huffman implementation with a
//! 9-bit lookup table for the common short codes.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InflateError(pub &'static str);

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "corrupt deflate data: {}", self.0)
    }
}

impl std::error::Error for InflateError {}

type Result<T> = std::result::Result<T, InflateError>;

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, buf: 0, count: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.count <= 56 {
            let b = match self.data.get(self.pos) {
                Some(&b) => b,
                None => {
                    // Past the end: feed zeros, but remember how far we went
                    // so that genuinely truncated input is reported.
                    if self.pos > self.data.len() + 8 {
                        return;
                    }
                    0
                }
            };
            self.pos += 1;
            self.buf |= (b as u64) << self.count;
            self.count += 8;
        }
    }

    #[inline]
    fn peek(&mut self, n: u32) -> u32 {
        if self.count < n {
            self.refill();
        }
        (self.buf & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    fn consume(&mut self, n: u32) {
        self.buf >>= n;
        self.count -= n;
    }

    #[inline]
    fn bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.peek(n);
        if self.count < n {
            return Err(InflateError("unexpected end of data"));
        }
        self.consume(n);
        Ok(v)
    }

    /// Discard to the next byte boundary.
    fn align(&mut self) {
        let r = self.count % 8;
        self.consume(r);
    }

    fn overrun(&self) -> bool {
        // Bytes fetched beyond the input that were actually consumed.
        let fetched_past = self.pos.saturating_sub(self.data.len());
        (fetched_past * 8) as u32 > self.count
    }
}

const FAST_BITS: u32 = 9;

struct Huffman {
    /// Number of codes of each length.
    counts: [u16; 16],
    /// Symbols ordered by code.
    symbols: Vec<u16>,
    /// `symbol << 4 | length` for codes of up to FAST_BITS bits, else 0.
    fast: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // Over-subscribed code sets are errors; incomplete ones are allowed
        // (a single distance code is legal).
        let mut left: i32 = 1;
        for len in 1..16 {
            left <<= 1;
            left -= counts[len] as i32;
            if left < 0 {
                return Err(InflateError("over-subscribed Huffman code"));
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        // Fast table: walk the canonical codes in order.
        let mut fast = vec![0u16; 1 << FAST_BITS];
        let mut code: u32 = 0;
        let mut index = 0usize;
        for len in 1..=FAST_BITS {
            for _ in 0..counts[len as usize] {
                let sym = symbols[index];
                index += 1;
                // Codes are packed MSB-first; the stream is read LSB-first.
                let rev = code.reverse_bits() >> (32 - len);
                let mut k = rev;
                while k < (1 << FAST_BITS) {
                    fast[k as usize] = (sym << 4) | len as u16;
                    k += 1 << len;
                }
                code += 1;
            }
            code <<= 1;
        }
        Ok(Huffman { counts, symbols, fast })
    }

    #[inline]
    fn decode(&self, bits: &mut Bits) -> Result<u16> {
        let e = self.fast[bits.peek(FAST_BITS) as usize];
        if e != 0 {
            let len = (e & 15) as u32;
            if bits.count < len {
                return Err(InflateError("unexpected end of data"));
            }
            bits.consume(len);
            return Ok(e >> 4);
        }
        // Slow path: canonical decode one bit at a time.
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError("invalid Huffman code"))
    }
}

const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Decompress a raw DEFLATE stream. `size_hint` preallocates the output.
pub fn inflate(data: &[u8], size_hint: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size_hint);
    let mut bits = Bits::new(data);
    let fixed = fixed_tables()?;
    loop {
        let last = bits.bits(1)?;
        match bits.bits(2)? {
            0 => {
                bits.align();
                let len = bits.bits(16)?;
                let nlen = bits.bits(16)?;
                if len != !nlen & 0xffff {
                    return Err(InflateError("stored block length mismatch"));
                }
                // Drain whole bytes still in the bit buffer first.
                let mut remaining = len as usize;
                while remaining > 0 && bits.count >= 8 {
                    out.push(bits.bits(8)? as u8);
                    remaining -= 1;
                }
                let start = bits.pos - (bits.count / 8) as usize;
                debug_assert_eq!(bits.count % 8, 0);
                let end = start + remaining;
                if end > data.len() {
                    return Err(InflateError("stored block past end of data"));
                }
                out.extend_from_slice(&data[start..end]);
                bits = Bits { data, pos: end, buf: 0, count: 0 };
            }
            1 => codes(&mut bits, &mut out, &fixed.0, &fixed.1)?,
            2 => {
                let (lit, dist) = dynamic_tables(&mut bits)?;
                codes(&mut bits, &mut out, &lit, &dist)?;
            }
            _ => return Err(InflateError("invalid block type")),
        }
        if bits.overrun() {
            return Err(InflateError("unexpected end of data"));
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

fn fixed_tables() -> Result<(Huffman, Huffman)> {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    Ok((Huffman::new(&l)?, Huffman::new(&[5u8; 30])?))
}

fn dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman)> {
    let nlen = bits.bits(5)? as usize + 257;
    let ndist = bits.bits(5)? as usize + 1;
    let ncode = bits.bits(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(InflateError("bad code counts"));
    }
    let mut cl = [0u8; 19];
    for &i in &CL_ORDER[..ncode] {
        cl[i] = bits.bits(3)? as u8;
    }
    let clh = Huffman::new(&cl)?;
    let mut lengths = vec![0u8; nlen + ndist];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = clh.decode(bits)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(InflateError("repeat with no previous length"));
                }
                let prev = lengths[i - 1];
                let n = 3 + bits.bits(2)? as usize;
                if i + n > lengths.len() {
                    return Err(InflateError("too many lengths"));
                }
                lengths[i..i + n].fill(prev);
                i += n;
            }
            17 | 18 => {
                let n = if sym == 17 { 3 + bits.bits(3)? } else { 11 + bits.bits(7)? } as usize;
                if i + n > lengths.len() {
                    return Err(InflateError("too many lengths"));
                }
                i += n; // already zero
            }
            _ => return Err(InflateError("bad code length symbol")),
        }
    }
    if lengths[256] == 0 {
        return Err(InflateError("no end-of-block code"));
    }
    Ok((Huffman::new(&lengths[..nlen])?, Huffman::new(&lengths[nlen..])?))
}

fn codes(bits: &mut Bits, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Result<()> {
    loop {
        let sym = lit.decode(bits)?;
        if sym < 256 {
            out.push(sym as u8);
            continue;
        }
        if sym == 256 {
            return Ok(());
        }
        let s = (sym - 257) as usize;
        if s >= 29 {
            return Err(InflateError("bad length symbol"));
        }
        let len = LEN_BASE[s] as usize + bits.bits(LEN_EXTRA[s] as u32)? as usize;
        let d = dist.decode(bits)? as usize;
        if d >= 30 {
            return Err(InflateError("bad distance symbol"));
        }
        let distance = DIST_BASE[d] as usize + bits.bits(DIST_EXTRA[d] as u32)? as usize;
        if distance > out.len() {
            return Err(InflateError("distance too far back"));
        }
        let start = out.len() - distance;
        if distance >= len {
            out.extend_from_within(start..start + len);
        } else {
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
}

/// Decompress a zlib stream (2-byte header, DEFLATE data, Adler-32).
pub fn zlib_decompress(data: &[u8], size_hint: usize) -> Result<Vec<u8>> {
    if data.len() < 2 {
        return Err(InflateError("zlib header missing"));
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0f != 8 || ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return Err(InflateError("bad zlib header"));
    }
    if flg & 0x20 != 0 {
        return Err(InflateError("zlib preset dictionary not supported"));
    }
    let out = inflate(&data[2..], size_hint)?;
    // The Adler-32 trailer is not checked: callers validate structure, and
    // some tools write zero checksums.
    Ok(out)
}

/// CRC-32 (zip, PNG).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_block() {
        // Python: zlib.compress(b"hello", 0)
        let z = [0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o', 0x06, 0x2c, 0x02, 0x15];
        assert_eq!(zlib_decompress(&z, 0).unwrap(), b"hello");
    }

    #[test]
    fn fixed_huffman_with_matches() {
        // Python: zlib.compress(b"abcabcabcabcabcabc", 9)
        let z = [0x78, 0xda, 0x4b, 0x4c, 0x4a, 0x4e, 0x44, 0x45, 0x00, 0x41, 0x7c, 0x06, 0xe5];
        assert_eq!(zlib_decompress(&z, 0).unwrap(), b"abcabcabcabcabcabc");
    }

    #[test]
    fn rejects_garbage() {
        assert!(inflate(&[0xff, 0xff, 0xff], 0).is_err());
    }

    #[test]
    fn crc() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
