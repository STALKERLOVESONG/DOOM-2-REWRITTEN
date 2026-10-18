//! A node builder: BSP nodes, subsectors and segs for maps shipped without
//! usable nodes (common for UDMF maps made for GZDoom, which builds its own).
//!
//! Classic BSP: every linedef side becomes a seg; a partition line is
//! chosen among the segs (fewest splits, best balance), segs crossing it
//! are split at a new vertex, and each half is processed the same way until
//! the remaining segs bound a convex region (a subsector). Output follows
//! the vanilla layout ([`RawMap::segs`]/`subsectors`/`nodes`, root last),
//! so every renderer and the playsim use it unchanged.
//!
//! Geometry is computed in `f64` and new vertices are rounded to 16.16
//! fixed point.

use crate::map::{MapNode, MapSeg, MapSubsector, MapVertex, RawMap, NF_SUBSECTOR, NO_SIDEDEF};

/// Partition candidates tried per node before falling back to all segs.
const MAX_CANDIDATES: usize = 48;
/// Cost of one split relative to one seg of imbalance.
const SPLIT_COST: i64 = 8;
/// Distances below this (map units) count as "on the line".
const EPS: f64 = 1.0 / 64.0;

#[derive(Clone, Copy, Debug)]
struct BSeg {
    v1: usize,
    v2: usize,
    linedef: u32,
    side: i16,
    /// Distance from the start of the linedef side to `v1` (map units).
    offset: f64,
}

struct Builder {
    verts: Vec<[f64; 2]>,
    segs: Vec<MapSeg>,
    subsectors: Vec<MapSubsector>,
    nodes: Vec<MapNode>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Front,
    Back,
    Split,
}

/// Which side of the partition through `p` with direction `d` a point is
/// on, as a signed distance (negative = front/right, as `R_PointOnSide`).
fn dist(p: [f64; 2], d: [f64; 2], len: f64, q: [f64; 2]) -> f64 {
    (d[0] * (q[1] - p[1]) - d[1] * (q[0] - p[0])) / len
}

impl Builder {
    fn classify(&self, part: &BSeg, s: &BSeg) -> (Side, f64, f64) {
        let (p, q) = (self.verts[part.v1], self.verts[part.v2]);
        let d = [q[0] - p[0], q[1] - p[1]];
        let len = d[0].hypot(d[1]);
        let (a, b) = (self.verts[s.v1], self.verts[s.v2]);
        let (d1, d2) = (dist(p, d, len, a), dist(p, d, len, b));
        let side = if d1.abs() < EPS && d2.abs() < EPS {
            // On the partition: front if it faces the same way.
            let sd = [b[0] - a[0], b[1] - a[1]];
            if sd[0] * d[0] + sd[1] * d[1] > 0.0 {
                Side::Front
            } else {
                Side::Back
            }
        } else if d1 < EPS && d2 < EPS {
            Side::Front
        } else if d1 > -EPS && d2 > -EPS {
            Side::Back
        } else {
            Side::Split
        };
        (side, d1, d2)
    }

    /// Score a partition: `None` if it doesn't divide the set.
    fn score(&self, part: &BSeg, segs: &[BSeg]) -> Option<i64> {
        let (mut front, mut back, mut splits) = (0i64, 0i64, 0i64);
        for s in segs {
            match self.classify(part, s).0 {
                Side::Front => front += 1,
                Side::Back => back += 1,
                Side::Split => {
                    splits += 1;
                    front += 1;
                    back += 1;
                }
            }
        }
        (front > 0 && back > 0).then_some(splits * SPLIT_COST + (front - back).abs())
    }

    fn choose(&self, segs: &[BSeg]) -> Option<usize> {
        let best = |cands: &mut dyn Iterator<Item = usize>| -> Option<usize> {
            let mut best: Option<(i64, usize)> = None;
            for i in cands {
                if let Some(sc) = self.score(&segs[i], segs) {
                    if best.is_none_or(|(b, _)| sc < b) {
                        best = Some((sc, i));
                    }
                }
            }
            best.map(|(_, i)| i)
        };
        let stride = (segs.len() / MAX_CANDIDATES).max(1);
        best(&mut (0..segs.len()).step_by(stride)).or_else(|| if stride > 1 { best(&mut (0..segs.len())) } else { None })
    }

    fn split(&mut self, part: &BSeg, segs: Vec<BSeg>) -> (Vec<BSeg>, Vec<BSeg>) {
        let (mut front, mut back) = (Vec::new(), Vec::new());
        for s in segs {
            let (side, d1, d2) = self.classify(part, &s);
            match side {
                Side::Front => front.push(s),
                Side::Back => back.push(s),
                Side::Split => {
                    let (a, b) = (self.verts[s.v1], self.verts[s.v2]);
                    let t = d1 / (d1 - d2);
                    let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                    // Rounded like a stored vertex, so later tests see the
                    // point where it really is.
                    let p = [(p[0] * 65536.0).round() / 65536.0, (p[1] * 65536.0).round() / 65536.0];
                    let vi = self.verts.len();
                    self.verts.push(p);
                    let first = BSeg { v2: vi, ..s };
                    let second = BSeg { v1: vi, offset: s.offset + (p[0] - a[0]).hypot(p[1] - a[1]), ..s };
                    // v1's side decides which half goes where.
                    if d1 < 0.0 {
                        front.push(first);
                        back.push(second);
                    } else {
                        back.push(first);
                        front.push(second);
                    }
                }
            }
        }
        (front, back)
    }

    fn bbox(&self, segs: &[BSeg]) -> [i32; 4] {
        let (mut top, mut bottom, mut left, mut right) = (f64::MIN, f64::MAX, f64::MAX, f64::MIN);
        for s in segs {
            for v in [s.v1, s.v2] {
                let [x, y] = self.verts[v];
                top = top.max(y);
                bottom = bottom.min(y);
                left = left.min(x);
                right = right.max(x);
            }
        }
        [top.ceil() as i32, bottom.floor() as i32, left.floor() as i32, right.ceil() as i32]
    }

    fn subsector(&mut self, segs: Vec<BSeg>) -> u32 {
        let first = self.segs.len() as u32;
        for s in &segs {
            let (a, b) = (self.verts[s.v1], self.verts[s.v2]);
            let ang = (b[1] - a[1]).atan2(b[0] - a[0]);
            let bam = (ang.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU * 4294967296.0) as u64 as u32;
            self.segs.push(MapSeg {
                v1: s.v1 as u32,
                v2: s.v2 as u32,
                angle: bam,
                linedef: s.linedef,
                side: s.side,
                offset: (s.offset * 65536.0).round() as i32,
            });
        }
        self.subsectors.push(MapSubsector { num_segs: segs.len() as u32, first_seg: first, sector_side: None });
        (self.subsectors.len() as u32 - 1) | NF_SUBSECTOR
    }
}

/// A child while building: a finished subsector, or a node not yet
/// numbered.
#[derive(Clone, Copy)]
enum Child {
    Subsector(u32),
    Node(usize),
}

/// A node while building (children fixed up when it is numbered).
struct Pending {
    part: Option<BSeg>,
    bbox: [[i32; 4]; 2],
    children: [Child; 2],
}

/// Work items for the explicit stack (deep BSPs would overflow recursion).
enum Task {
    Build { segs: Vec<BSeg>, parent: Option<(usize, usize)> },
    Finish { node: usize },
}

/// Build segs, subsectors and nodes for `map`, replacing any it has.
/// New vertices are appended to `map.vertexes`.
pub fn build_nodes(map: &mut RawMap) {
    let mut b = Builder {
        verts: map.vertexes.iter().map(|v| [v.x as f64 / 65536.0, v.y as f64 / 65536.0]).collect(),
        segs: Vec::new(),
        subsectors: Vec::new(),
        nodes: Vec::new(),
    };
    let mut segs = Vec::with_capacity(map.linedefs.len() * 2);
    for (i, l) in map.linedefs.iter().enumerate() {
        let (v1, v2) = (l.v1 as usize, l.v2 as usize);
        if v1 >= b.verts.len() || v2 >= b.verts.len() {
            continue;
        }
        let (a, c) = (b.verts[v1], b.verts[v2]);
        if (c[0] - a[0]).hypot(c[1] - a[1]) < EPS {
            continue;
        }
        if l.sidenum[0] != NO_SIDEDEF {
            segs.push(BSeg { v1, v2, linedef: i as u32, side: 0, offset: 0.0 });
        }
        if l.sidenum[1] != NO_SIDEDEF {
            segs.push(BSeg { v1: v2, v2: v1, linedef: i as u32, side: 1, offset: 0.0 });
        }
    }
    // Nodes get their numbers when finished (both children done), so the
    // root is last and children always precede their parent, as in vanilla.
    let mut pending: Vec<Pending> = Vec::new();
    let mut number: Vec<u32> = Vec::new();
    let mut finished: Vec<usize> = Vec::new();
    let mut stack = vec![Task::Build { segs, parent: None }];
    while let Some(task) = stack.pop() {
        match task {
            Task::Build { segs, parent } => {
                let child = match b.choose(&segs) {
                    None => Child::Subsector(b.subsector(segs)),
                    Some(pi) => {
                        let part = segs[pi];
                        let (front, back) = b.split(&part, segs);
                        let node = pending.len();
                        pending.push(Pending {
                            part: Some(part),
                            bbox: [b.bbox(&front), b.bbox(&back)],
                            children: [Child::Subsector(0); 2],
                        });
                        stack.push(Task::Finish { node });
                        stack.push(Task::Build { segs: back, parent: Some((node, 1)) });
                        stack.push(Task::Build { segs: front, parent: Some((node, 0)) });
                        Child::Node(node)
                    }
                };
                if let Some((node, side)) = parent {
                    pending[node].children[side] = child;
                }
            }
            Task::Finish { node } => finished.push(node),
        }
    }
    number.resize(pending.len(), 0);
    for (i, &n) in finished.iter().enumerate() {
        number[n] = i as u32;
    }
    let fx = |v: f64| (v * 65536.0).round() as i32;
    for &n in &finished {
        let p = &pending[n];
        let part = p.part.expect("a split node has a partition");
        let (a, c) = (b.verts[part.v1], b.verts[part.v2]);
        let child = |c: Child| match c {
            Child::Subsector(s) => s,
            Child::Node(i) => number[i],
        };
        b.nodes.push(MapNode {
            x: fx(a[0]),
            y: fx(a[1]),
            dx: fx(c[0] - a[0]),
            dy: fx(c[1] - a[1]),
            bbox: p.bbox,
            children: [child(p.children[0]), child(p.children[1])],
        });
    }
    map.vertexes = b.verts.iter().map(|&[x, y]| MapVertex { x: fx(x), y: fx(y) }).collect();
    map.segs = b.segs;
    map.subsectors = b.subsectors;
    map.nodes = b.nodes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{MapFormat, MapLinedef, MapSector, MapSidedef};
    use crate::name::LumpName;

    /// A square room with a square pillar: not convex, needs nodes.
    fn room() -> RawMap {
        let v = |x: i32, y: i32| MapVertex { x: x << 16, y: y << 16 };
        let vertexes = vec![v(0, 0), v(256, 0), v(256, 256), v(0, 256), v(96, 96), v(160, 96), v(160, 160), v(96, 160)];
        let side = |sector: u32| MapSidedef {
            texture_offset: 0.0,
            row_offset: 0.0,
            top: LumpName::new("-"),
            bottom: LumpName::new("-"),
            mid: LumpName::new("STARTAN3"),
            sector,
            part_offsets: [[0.0; 2]; 3],
            light: 0,
            light_absolute: false,
        };
        let line = |a: u32, c: u32, s: u32| MapLinedef { v1: a, v2: c, flags: 1, special: 0, tag: 0, sidenum: [s, NO_SIDEDEF], alpha: 1.0, action: None, id: -1 };
        // Outer walls face in (clockwise), pillar walls face out.
        let linedefs = vec![line(0, 3, 0), line(3, 2, 1), line(2, 1, 2), line(1, 0, 3), line(4, 5, 4), line(5, 6, 5), line(6, 7, 6), line(7, 4, 7)];
        RawMap {
            name: LumpName::new("TEST"),
            format: MapFormat::Udmf,
            things: vec![],
            linedefs,
            sidedefs: (0..8).map(|_| side(0)).collect(),
            vertexes,
            segs: vec![],
            subsectors: vec![],
            nodes: vec![],
            sectors: vec![MapSector::doom(0, 128 << 16, LumpName::new("FLOOR4_8"), LumpName::new("CEIL3_5"), 160, 0, 0)],
            reject: vec![],
            blockmap: vec![],
            behavior: None,
            warnings: vec![],
        }
    }

    fn point_subsector(m: &RawMap, x: f64, y: f64) -> usize {
        let mut child = m.nodes.len() as u32 - 1;
        while child & NF_SUBSECTOR == 0 {
            let n = &m.nodes[child as usize];
            let f = |v: i32| v as f64 / 65536.0;
            let side = (f(n.dy) * (x - f(n.x)) - f(n.dx) * (y - f(n.y)) <= 0.0) as usize;
            child = n.children[side];
        }
        (child & !NF_SUBSECTOR) as usize
    }

    #[test]
    fn builds_a_valid_bsp_for_a_non_convex_room() {
        let mut m = room();
        build_nodes(&mut m);
        assert!(m.nodes.len() >= 2, "{} nodes", m.nodes.len());
        // Every seg is used once and every original side is covered.
        let total: u32 = m.subsectors.iter().map(|s| s.num_segs).sum();
        assert_eq!(total as usize, m.segs.len());
        for l in 0..8 {
            assert!(m.segs.iter().any(|s| s.linedef == l));
        }
        // Each subsector is convex: every seg's vertices are on or in front
        // of every other seg of the same subsector.
        for ss in &m.subsectors {
            let segs = &m.segs[ss.first_seg as usize..(ss.first_seg + ss.num_segs) as usize];
            for a in segs {
                let (p, q) = (m.vertexes[a.v1 as usize], m.vertexes[a.v2 as usize]);
                let d = [(q.x - p.x) as f64, (q.y - p.y) as f64];
                for b in segs {
                    for v in [b.v1, b.v2] {
                        let r = m.vertexes[v as usize];
                        let cross = d[0] * (r.y - p.y) as f64 - d[1] * (r.x - p.x) as f64;
                        assert!(cross / d[0].hypot(d[1]) < 65536.0 / 16.0, "subsector not convex");
                    }
                }
            }
        }
        // Points in the room find a subsector; the root is the last node.
        for (x, y) in [(10.0, 10.0), (200.0, 200.0), (128.0, 40.0), (40.0, 128.0)] {
            assert!(point_subsector(&m, x, y) < m.subsectors.len());
        }
        // Seg offsets are distances along their linedef side.
        for s in &m.segs {
            let l = &m.linedefs[s.linedef as usize];
            let start = if s.side == 0 { l.v1 } else { l.v2 };
            let (a, b) = (m.vertexes[start as usize], m.vertexes[s.v1 as usize]);
            let expect = ((b.x - a.x) as f64).hypot((b.y - a.y) as f64);
            assert!((s.offset as f64 - expect).abs() <= 2.0, "offset {} vs {expect}", s.offset);
        }
    }
}
