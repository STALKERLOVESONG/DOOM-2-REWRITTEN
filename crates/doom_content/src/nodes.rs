//! ZDoom extended nodes: `XNOD`/`ZNOD` (normal) and `XGLN`/`ZGLN`,
//! `XGL2`/`ZGL2`, `XGL3`/`ZGL3` (GL nodes), the `Z` variants being
//! zlib-compressed. Used by UDMF maps (`ZNODES`) and large Doom-format maps
//! (in `NODES`).
//!
//! GL nodes contain "minisegs" along partition lines that belong to no
//! linedef. The software renderers and the playsim only want real segs, so
//! minisegs are dropped and the subsector seg ranges recomputed; a
//! subsector left with no segs records a neighbouring sidedef (found through
//! miniseg partners) so its sector is still known. Seg angles and offsets,
//! which these formats do not store, are computed from the geometry.

use crate::inflate::zlib_decompress;
use crate::map::{MapNode, MapSeg, MapSubsector, MapVertex, RawMap, NF_SUBSECTOR};
use crate::ContentError;

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], ContentError> {
        let s = self.d.get(self.pos..self.pos + n).ok_or_else(|| ContentError::malformed("ZNODES", "truncated"))?;
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, ContentError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ContentError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16, ContentError> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32, ContentError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, ContentError> {
        Ok(self.u32()? as i32)
    }
    /// A count that must fit in the remaining data at `size` bytes each.
    fn count(&mut self, size: usize) -> Result<usize, ContentError> {
        let n = self.u32()? as usize;
        if n.checked_mul(size).map_or(true, |b| b > self.d.len() - self.pos.min(self.d.len())) {
            return Err(ContentError::malformed("ZNODES", format!("count {n} exceeds the lump")));
        }
        Ok(n)
    }
}

/// BAM angle of the vector (dx, dy).
pub fn bam(dx: f64, dy: f64) -> u32 {
    let a = dy.atan2(dx) / std::f64::consts::TAU;
    (a.rem_euclid(1.0) * 4294967296.0) as i64 as u32
}

/// Replace `map`'s segs, subsectors and nodes with the extended node data in
/// `lump` (signature included).
pub fn load_zdoom(map: &mut RawMap, lump: &[u8]) -> Result<(), ContentError> {
    let bad = |r: String| ContentError::malformed(format!("{} nodes", map.name), r);
    if lump.len() < 4 {
        return Err(bad("empty node lump".into()));
    }
    let sig = [lump[0], lump[1], lump[2], lump[3]];
    let (gl, version) = match &sig[1..] {
        b"NOD" => (false, 0),
        b"GLN" => (true, 1),
        b"GL2" => (true, 2),
        b"GL3" => (true, 3),
        _ => return Err(bad(format!("unknown signature {:?}", String::from_utf8_lossy(&sig)))),
    };
    let owned;
    let data: &[u8] = match sig[0] {
        b'X' => &lump[4..],
        b'Z' => {
            owned = zlib_decompress(&lump[4..], lump.len() * 3).map_err(|e| bad(e.to_string()))?;
            &owned
        }
        _ => return Err(bad("unknown signature".into())),
    };
    let mut r = Reader { d: data, pos: 0 };

    // Vertices: the map's first `orgverts`, then the node builder's.
    let orgverts = r.u32()? as usize;
    let newverts = r.count(8)?;
    if orgverts > map.vertexes.len() {
        return Err(bad(format!("expects {orgverts} map vertices, the map has {}", map.vertexes.len())));
    }
    if orgverts < map.vertexes.len() {
        map.warnings.push(format!("{}: nodes replace {} trailing vertices", map.name, map.vertexes.len() - orgverts));
        map.vertexes.truncate(orgverts);
    }
    for _ in 0..newverts {
        let (x, y) = (r.i32()?, r.i32()?);
        map.vertexes.push(MapVertex { x, y });
    }
    let nverts = map.vertexes.len() as u32;

    let nsubs = r.count(4)?;
    let mut sub_counts = Vec::with_capacity(nsubs);
    for _ in 0..nsubs {
        sub_counts.push(r.u32()? as usize);
    }

    let seg_size = match version {
        0 | 1 => 11,
        _ => 13,
    };
    let nsegs = r.count(seg_size)?;
    if sub_counts.iter().sum::<usize>() != nsegs {
        return Err(bad("subsector seg counts do not add up".into()));
    }
    // (v1, v2 or partner, line or u32::MAX, side)
    let mut raw = Vec::with_capacity(nsegs);
    for _ in 0..nsegs {
        let v1 = r.u32()?;
        let second = r.u32()?;
        let line = if version >= 2 {
            r.u32()?
        } else {
            match r.u16()? {
                0xffff => u32::MAX,
                l => l as u32,
            }
        };
        let side = r.u8()?;
        raw.push((v1, second, line, side));
    }

    let nnodes = r.count(if version == 3 { 32 } else { 24 })?;
    let mut nodes = Vec::with_capacity(nnodes);
    for _ in 0..nnodes {
        let (x, y, dx, dy) = if version == 3 {
            (r.i32()?, r.i32()?, r.i32()?, r.i32()?)
        } else {
            let f = |v: i16| (v as i32) << 16;
            (f(r.i16()?), f(r.i16()?), f(r.i16()?), f(r.i16()?))
        };
        let mut bbox = [[0i32; 4]; 2];
        for b in bbox.iter_mut() {
            for v in b.iter_mut() {
                *v = r.i16()? as i32;
            }
        }
        let children = [r.u32()?, r.u32()?];
        for &c in &children {
            let ok = if c & NF_SUBSECTOR != 0 { ((c & !NF_SUBSECTOR) as usize) < nsubs } else { (c as usize) < nnodes };
            if !ok {
                return Err(bad(format!("node child {c:#x} out of range")));
            }
        }
        nodes.push(MapNode { x, y, dx, dy, bbox, children });
    }

    // Resolve v2 (GL: the next seg's v1 in the same subsector).
    let mut seg_sub = vec![0usize; nsegs];
    let mut segs_v2 = vec![0u32; nsegs];
    let mut first = 0;
    for (s, &n) in sub_counts.iter().enumerate() {
        for k in 0..n {
            let i = first + k;
            seg_sub[i] = s;
            segs_v2[i] = if gl { raw[first + (k + 1) % n].0 } else { raw[i].1 };
        }
        first += n;
    }

    // Keep real segs; compute angle and offset.
    let mut segs = Vec::with_capacity(nsegs);
    let mut subsectors = Vec::with_capacity(nsubs);
    let mut first = 0;
    for &n in &sub_counts {
        let start = segs.len() as u32;
        for i in first..first + n {
            let (v1, _, line, side) = raw[i];
            let v2 = segs_v2[i];
            if line == u32::MAX {
                continue; // miniseg
            }
            if v1 >= nverts || v2 >= nverts {
                return Err(bad(format!("seg {i} references a missing vertex")));
            }
            let Some(ld) = map.linedefs.get(line as usize) else {
                return Err(bad(format!("seg {i} references missing linedef {line}")));
            };
            let (a, b) = (map.vertexes[v1 as usize], map.vertexes[v2 as usize]);
            let origin = map.vertexes.get(if side == 0 { ld.v1 } else { ld.v2 } as usize).copied().unwrap_or(a);
            let f = |v: i32| v as f64 / 65536.0;
            let offset = (f(a.x) - f(origin.x)).hypot(f(a.y) - f(origin.y));
            segs.push(MapSeg {
                v1,
                v2,
                angle: bam(f(b.x) - f(a.x), f(b.y) - f(a.y)),
                linedef: line,
                side: side as i16,
                offset: (offset * 65536.0).round() as i32,
            });
        }
        subsectors.push(MapSubsector { num_segs: segs.len() as u32 - start, first_seg: start, sector_side: None });
        first += n;
    }

    // Subsectors made only of minisegs: borrow a sector through partners.
    if gl && subsectors.iter().any(|s| s.num_segs == 0) {
        let side_of = |seg: usize| -> Option<u32> {
            let (_, _, line, side) = raw[seg];
            let ld = map.linedefs.get(line as usize)?;
            Some(ld.sidenum[(side & 1) as usize]).filter(|&s| s != crate::map::NO_SIDEDEF)
        };
        let mut sub_first = Vec::with_capacity(nsubs);
        let mut acc = 0;
        for &n in &sub_counts {
            sub_first.push(acc);
            acc += n;
        }
        let mut unresolved = 0;
        for s in 0..nsubs {
            if subsectors[s].num_segs != 0 {
                continue;
            }
            // Breadth-first over neighbouring subsectors.
            let mut queue = std::collections::VecDeque::from([s]);
            let mut seen = std::collections::HashSet::from([s]);
            let mut found = None;
            'search: while let Some(cur) = queue.pop_front() {
                for i in sub_first[cur]..sub_first[cur] + sub_counts[cur] {
                    if let Some(side) = side_of(i) {
                        found = Some(side);
                        break 'search;
                    }
                    let partner = raw[i].1;
                    if (partner as usize) < nsegs {
                        let next = seg_sub[partner as usize];
                        if seen.insert(next) {
                            queue.push_back(next);
                        }
                    }
                }
            }
            match found {
                Some(side) => subsectors[s].sector_side = Some(side),
                None => unresolved += 1,
            }
        }
        if unresolved > 0 {
            map.warnings.push(format!("{}: {unresolved} subsectors have no sector", map.name));
        }
    }

    map.segs = segs;
    map.subsectors = subsectors;
    map.nodes = nodes;
    Ok(())
}
