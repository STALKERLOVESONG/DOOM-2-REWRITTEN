//! Reconstruct sector outlines (outer rings + holes) from linedefs.

use std::collections::HashMap;

use bevy::log::warn;
use bevy::prelude::Vec2;

use super::data::MapData;

pub struct SectorShape {
    pub sector: usize,
    /// Clockwise ring (DOOM coords, y up).
    pub outer: Vec<Vec2>,
    /// Counter-clockwise rings fully inside `outer`.
    pub holes: Vec<Vec<Vec2>>,
    pub min: Vec2,
    pub max: Vec2,
}

impl SectorShape {
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && point_in_polygon(p, &self.outer)
            && !self.holes.iter().any(|h| point_in_polygon(p, h))
    }
}

/// Even-odd ray cast.
pub fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Shoelace; positive = counter-clockwise.
pub fn signed_area(poly: &[Vec2]) -> f32 {
    let mut s = 0.0;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        s += a.x * b.y - b.x * a.y;
    }
    0.5 * s
}

/// Chain directed edges end-to-start into closed rings of vertex indices.
fn chain_loops(edges: &[(u16, u16)]) -> Vec<Vec<u16>> {
    let mut outgoing: HashMap<u16, Vec<usize>> = HashMap::new();
    for (i, (a, _)) in edges.iter().enumerate() {
        outgoing.entry(*a).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();

    for start in 0..edges.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let first = edges[start].0;
        let mut ring = vec![first];
        let mut end = edges[start].1;
        let mut closed = end == first;
        while !closed {
            let next = outgoing
                .get(&end)
                .and_then(|v| v.iter().copied().find(|&i| !used[i]));
            let Some(i) = next else { break };
            used[i] = true;
            ring.push(edges[i].0);
            end = edges[i].1;
            closed = end == first;
        }
        if closed && ring.len() >= 3 {
            loops.push(ring);
        }
    }
    loops
}

pub fn build_sector_shapes(map: &MapData) -> Vec<SectorShape> {
    // Directed edges per sector, oriented so the sector lies on the RIGHT.
    let mut edges: Vec<Vec<(u16, u16)>> = vec![Vec::new(); map.sectors.len()];
    for ld in &map.linedefs {
        let fs = ld.front.map(|s| map.sector_of_side(s));
        let bs = ld.back.map(|s| map.sector_of_side(s));
        if fs == bs {
            continue; // interior line of one sector: not a boundary
        }
        if let Some(s) = fs {
            edges[s].push((ld.v1, ld.v2));
        }
        if let Some(s) = bs {
            edges[s].push((ld.v2, ld.v1));
        }
    }

    let mut shapes = Vec::new();
    for (sector, sector_edges) in edges.iter().enumerate() {
        if sector_edges.is_empty() {
            continue;
        }
        let rings: Vec<Vec<Vec2>> = chain_loops(sector_edges)
            .into_iter()
            .map(|r| r.into_iter().map(|v| map.vertex(v)).collect())
            .collect();

        let (mut outers, holes): (Vec<_>, Vec<_>) = rings
            .into_iter()
            .partition(|r| signed_area(r) < 0.0); // clockwise = outer boundary
        if outers.is_empty() {
            warn!("sector {sector}: no closed outer boundary; not rendered");
            continue;
        }

        let mut assigned: Vec<Vec<Vec<Vec2>>> = vec![Vec::new(); outers.len()];
        for hole in holes {
            // Smallest outer ring that contains the hole's first vertex.
            let owner = (0..outers.len())
                .filter(|&i| point_in_polygon(hole[0], &outers[i]))
                .min_by(|&a, &b| {
                    signed_area(&outers[a])
                        .abs()
                        .total_cmp(&signed_area(&outers[b]).abs())
                });
            match owner {
                Some(i) => assigned[i].push(hole),
                None => warn!("sector {sector}: orphan hole ring ignored"),
            }
        }

        for (outer, holes) in outers.drain(..).zip(assigned) {
            let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in &outer {
                min = min.min(*p);
                max = max.max(*p);
            }
            shapes.push(SectorShape {
                sector,
                outer,
                holes,
                min,
                max,
            });
        }
    }
    shapes
}
