//! Decompress zlib streams produced by Python's zlib at every level and
//! strategy (stored, fixed and dynamic Huffman blocks, long matches,
//! overlapping copies) and compare with the originals.
//!
//! Fixture: per input `len, count, data` then `count` x `zlen, zlib bytes`.

use doom_content::inflate::zlib_decompress;

fn u32_at(d: &[u8], p: usize) -> usize {
    u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize
}

#[test]
fn matches_reference_zlib() {
    let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/zlib_cases.bin")).unwrap();
    let mut pos = 0;
    let mut cases = 0;
    while pos < data.len() {
        let (len, count) = (u32_at(&data, pos), u32_at(&data, pos + 4));
        pos += 8;
        let original = &data[pos..pos + len];
        pos += len;
        for _ in 0..count {
            let zlen = u32_at(&data, pos);
            let z = &data[pos + 4..pos + 4 + zlen];
            pos += 4 + zlen;
            assert_eq!(zlib_decompress(z, len).unwrap(), original, "case {cases}");
            // Truncated input must fail cleanly, never panic.
            if zlen > 4 {
                let _ = zlib_decompress(&z[..zlen / 2], len);
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 96);
}
