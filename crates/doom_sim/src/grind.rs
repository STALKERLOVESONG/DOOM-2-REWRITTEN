//! Grindable geometry found automatically in ordinary Doom maps.
//!
//! Doom maps carry no skateboarding data, so the skate controller
//! ([`crate::skate`]) grinds on edges this module derives from the level
//! when it loads:
//!
//! - **ledges and curbs**: two-sided lines between floors of different
//!   height (the rideable edge is the top of the step);
//! - **bars**: the same, where the raised side is a thin strip (a raised
//!   divider or kerb a few units wide);
//! - **rails**: two-sided lines with a masked middle texture (fences,
//!   railings) whose top sits a grindable height above the floor.
//!
//! Each candidate is filtered by length, height, headroom and how it can be
//! approached, then connected to its neighbours through the map vertices
//! they share, giving a graph ([`GrindGraph`]) of nodes, edges and
//! connections the controller follows around corners and along runs of
//! collinear lines. A uniform grid answers "which edges are near this
//! point" by looking at one cell, so detection never scans the map.
//!
//! Edge heights are read from the live sector heights, so ledges on lifts
//! and moving floors stay correct without rebuilding.

use std::collections::HashMap;

use doom_core::FRACUNIT;

use crate::ids::{LineId, SectorId, VertexId};
use crate::level::Level;

/// Build settings (the `skate_grind_*` cvars that shape the graph).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrindParams {
    /// Tallest ledge or rail (above the floor beside it) kept.
    pub max_height: f32,
    /// Lowest step kept (lower ones are bumps, not edges).
    pub min_height: f32,
    /// Shortest line kept.
    pub min_length: f32,
    /// Headroom needed above the edge.
    pub min_clearance: f32,
    /// Strips narrower than this are bars rather than ledges.
    pub bar_width: f32,
    /// Sharpest turn (degrees) two edges may meet at and stay connected.
    pub max_corner_angle: f32,
    /// Turns up to this (degrees) are continuous; sharper ones are corners.
    pub continuous_angle: f32,
}

impl Default for GrindParams {
    fn default() -> Self {
        GrindParams {
            max_height: 64.0,
            min_height: 4.0,
            min_length: 16.0,
            min_clearance: 48.0,
            bar_width: 24.0,
            max_corner_angle: 100.0,
            continuous_angle: 30.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GrindKind {
    /// The top edge of a step at least 16 units high.
    Ledge,
    /// A low step (under 16 units).
    Curb,
    /// The edge of a thin raised strip.
    Bar,
    /// The top of a masked middle texture (fence, railing).
    Rail,
}

impl GrindKind {
    pub fn name(self) -> &'static str {
        match self {
            GrindKind::Ledge => "ledge",
            GrindKind::Curb => "curb",
            GrindKind::Bar => "bar",
            GrindKind::Rail => "rail",
        }
    }
}

/// One grindable segment.
#[derive(Clone, Debug)]
pub struct GrindEdge {
    /// Endpoints (map units): `a` is the line's first vertex.
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Unit vector from `a` to `b`.
    pub tangent: [f32; 2],
    /// Unit vector across the edge pointing away from the raised side (to
    /// the low floor); for rails, the line's right-hand side.
    pub normal: [f32; 2],
    pub length: f32,
    pub kind: GrindKind,
    pub line: LineId,
    /// The edge's height is this sector's floor plus `top_offset`.
    pub top_sector: SectorId,
    pub top_offset: f32,
    /// The floor beside the edge (the low side of a ledge).
    pub low_sector: SectorId,
    /// Height above the low side when built.
    pub height: f32,
    /// Headroom above the edge when built.
    pub clearance: f32,
    /// Can be reached from the low side (not only by riding off the top).
    pub approach_low: bool,
    /// The node at each end ([`GrindGraph::nodes`]).
    pub nodes: [u32; 2],
}

impl GrindEdge {
    /// Current height of the grind surface.
    #[inline]
    pub fn top_z(&self, level: &Level) -> f32 {
        level.sectors[self.top_sector.idx()].floor_height.0 as f32 / FRACUNIT as f32 + self.top_offset
    }

    /// The point `s` units from `a`.
    #[inline]
    pub fn point(&self, s: f32) -> [f32; 2] {
        [self.a[0] + self.tangent[0] * s, self.a[1] + self.tangent[1] * s]
    }

    /// Distance along the edge of the point nearest `p` (unclamped) and the
    /// signed distance across it (positive on the `normal` side).
    #[inline]
    pub fn local(&self, p: [f32; 2]) -> (f32, f32) {
        let (dx, dy) = (p[0] - self.a[0], p[1] - self.a[1]);
        (dx * self.tangent[0] + dy * self.tangent[1], dx * self.normal[0] + dy * self.normal[1])
    }

    /// Direction of travel when riding toward `end` (0 = `a`, 1 = `b`).
    #[inline]
    pub fn dir_toward(&self, end: usize) -> [f32; 2] {
        if end == 1 {
            self.tangent
        } else {
            [-self.tangent[0], -self.tangent[1]]
        }
    }
}

/// Where edges meet (a shared map vertex).
#[derive(Clone, Debug)]
pub struct GrindNode {
    pub pos: [f32; 2],
    /// Range into [`GrindGraph::incidences`]: the (edge, end) pairs here.
    pub first: u32,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnKind {
    /// Nearly straight on: the grind carries on by itself.
    Continuous,
    /// A real corner: followed with grind assist or when steered into.
    Corner,
}

/// Leaving one edge through an end onto another edge.
#[derive(Clone, Copy, Debug)]
pub struct GrindConnection {
    pub edge: u32,
    /// The end of `edge` the grind enters at (it then rides toward the
    /// other end).
    pub enter_end: u8,
    /// Signed turn (radians, counter-clockwise positive).
    pub turn: f32,
    pub kind: ConnKind,
}

/// Uniform grid of edge lists (each edge is listed in every cell its
/// bounds, padded by the snap margin, overlap).
#[derive(Clone, Debug, Default)]
struct Grid {
    org: [f32; 2],
    cols: i32,
    rows: i32,
    /// Per cell: start into `items`; `cells[i + 1]` ends it.
    cells: Vec<u32>,
    items: Vec<u32>,
}

/// Cell size of the lookup grid.
pub const GRID_CELL: f32 = 128.0;
/// Edges are filed into every cell within this distance.
pub const GRID_MARGIN: f32 = 48.0;

#[derive(Clone, Debug, Default)]
pub struct GrindStats {
    pub lines_checked: usize,
    pub by_kind: [usize; 4],
    pub connections: usize,
    pub corners: usize,
    pub build_micros: u128,
}

/// The grind graph of one level.
#[derive(Clone, Debug)]
pub struct GrindGraph {
    pub params: GrindParams,
    /// Name of the level it was built for.
    pub level: String,
    pub edges: Vec<GrindEdge>,
    pub nodes: Vec<GrindNode>,
    /// (edge, end) pairs, grouped by node.
    pub incidences: Vec<(u32, u8)>,
    /// Connections out of each edge end: `conn_index[edge * 2 + end]` to
    /// `conn_index[edge * 2 + end + 1]` index `connections`, best (least
    /// turn) first.
    pub conn_index: Vec<u32>,
    pub connections: Vec<GrindConnection>,
    grid: Grid,
    pub stats: GrindStats,
}

#[inline]
fn fx(v: i32) -> f32 {
    v as f32 / FRACUNIT as f32
}

fn signed_angle(from: [f32; 2], to: [f32; 2]) -> f32 {
    let cross = from[0] * to[1] - from[1] * to[0];
    let dot = from[0] * to[0] + from[1] * to[1];
    cross.atan2(dot)
}

impl GrindGraph {
    /// Analyse `level` (texture heights from `texture_heights`, indexed by
    /// texture number).
    pub fn build(level: &Level, texture_heights: &[i32], params: GrindParams) -> GrindGraph {
        let start = std::time::Instant::now();
        let mut stats = GrindStats { lines_checked: level.lines.len(), ..Default::default() };
        let mut edges = Vec::new();
        let mut edge_vertices: Vec<[VertexId; 2]> = Vec::new();
        for (i, line) in level.lines.iter().enumerate() {
            let (Some(front), Some(back)) = (line.front_sector, line.back_sector) else { continue };
            if front == back || line.sidenum[1].is_none() {
                continue;
            }
            let (v1, v2) = (level.vertex(line.v1), level.vertex(line.v2));
            let a = [fx(v1.x.0), fx(v1.y.0)];
            let b = [fx(v2.x.0), fx(v2.y.0)];
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let length = (dx * dx + dy * dy).sqrt();
            if length < params.min_length {
                continue;
            }
            let tangent = [dx / length, dy / length];
            // Doom's front side is on the right of v1 -> v2.
            let right = [tangent[1], -tangent[0]];
            let (fs, bs) = (&level.sectors[front.idx()], &level.sectors[back.idx()]);
            let (ff, bf) = (fx(fs.floor_height.0), fx(bs.floor_height.0));
            let ceiling = fx(fs.ceiling_height.0.min(bs.ceiling_height.0));
            let dz = (ff - bf).abs();
            let candidate = if dz >= params.min_height {
                // A step: the edge is on the higher floor.
                let (top, low, normal) = if ff > bf { (front, back, [-right[0], -right[1]]) } else { (back, front, right) };
                let height = dz;
                let kind = if height < 16.0 { GrindKind::Curb } else { GrindKind::Ledge };
                Some((kind, top, low, 0.0, height, normal))
            } else {
                rail_top(level, line, texture_heights, ff.max(bf), ceiling)
                    .map(|off| (GrindKind::Rail, if ff >= bf { front } else { back }, if ff >= bf { back } else { front }, off, off, right))
            };
            let Some((mut kind, top, low, top_offset, height, normal)) = candidate else { continue };
            if height > params.max_height {
                continue;
            }
            let top_z = fx(level.sectors[top.idx()].floor_height.0) + top_offset;
            let clearance = ceiling - top_z;
            if clearance < params.min_clearance {
                continue;
            }
            if kind != GrindKind::Rail {
                let mid = [a[0] + dx * 0.5, a[1] + dy * 0.5];
                if strip_width(level, top, line_index(i), mid, [-normal[0], -normal[1]], params.bar_width) {
                    kind = GrindKind::Bar;
                }
            }
            stats.by_kind[kind as usize] += 1;
            edges.push(GrindEdge {
                a,
                b,
                tangent,
                normal,
                length,
                kind,
                line: line_index(i),
                top_sector: top,
                top_offset,
                low_sector: low,
                height,
                clearance,
                // An ollie clears about 48 units (more charged).
                approach_low: height <= 56.0,
                nodes: [0; 2],
            });
            edge_vertices.push([line.v1, line.v2]);
        }

        // Nodes: one per shared map vertex.
        let mut node_of: HashMap<VertexId, u32> = HashMap::new();
        let mut node_pos: Vec<[f32; 2]> = Vec::new();
        let mut per_node: Vec<Vec<(u32, u8)>> = Vec::new();
        for (e, verts) in edge_vertices.iter().enumerate() {
            for end in 0..2 {
                let v = verts[end];
                let n = *node_of.entry(v).or_insert_with(|| {
                    let p = level.vertex(v);
                    node_pos.push([fx(p.x.0), fx(p.y.0)]);
                    per_node.push(Vec::new());
                    (node_pos.len() - 1) as u32
                });
                per_node[n as usize].push((e as u32, end as u8));
                edges[e].nodes[end] = n;
            }
        }
        let mut nodes = Vec::with_capacity(per_node.len());
        let mut incidences = Vec::new();
        for (n, list) in per_node.iter().enumerate() {
            nodes.push(GrindNode { pos: node_pos[n], first: incidences.len() as u32, count: list.len() as u32 });
            incidences.extend_from_slice(list);
        }

        // Connections out of every edge end.
        let max_turn = params.max_corner_angle.to_radians();
        let cont = params.continuous_angle.to_radians();
        let mut conn_index = Vec::with_capacity(edges.len() * 2 + 1);
        let mut connections = Vec::new();
        let mut scratch: Vec<GrindConnection> = Vec::new();
        for e in 0..edges.len() {
            for end in 0..2 {
                conn_index.push(connections.len() as u32);
                let edge = &edges[e];
                let travel = edge.dir_toward(end);
                let node = &nodes[edge.nodes[end] as usize];
                let z = edge.top_z(level);
                scratch.clear();
                for &(f, fend) in &incidences[node.first as usize..(node.first + node.count) as usize] {
                    if f as usize == e {
                        continue;
                    }
                    let other = &edges[f as usize];
                    if (other.top_z(level) - z).abs() > 4.0 {
                        continue;
                    }
                    // Entering `other` at `fend`, riding toward the far end.
                    let next = other.dir_toward(1 - fend as usize);
                    let turn = signed_angle(travel, next);
                    if turn.abs() > max_turn {
                        continue;
                    }
                    // A ledge keeps its raised side on the same hand.
                    if edge.kind != GrindKind::Rail && other.kind != GrindKind::Rail {
                        let side = |d: [f32; 2], n: [f32; 2]| (d[0] * -n[1] - d[1] * -n[0]).signum();
                        if side(travel, edge.normal) != side(next, other.normal) {
                            continue;
                        }
                    }
                    let kind = if turn.abs() <= cont { ConnKind::Continuous } else { ConnKind::Corner };
                    scratch.push(GrindConnection { edge: f, enter_end: fend, turn, kind });
                }
                scratch.sort_by(|p, q| p.turn.abs().total_cmp(&q.turn.abs()));
                for c in &scratch {
                    stats.connections += 1;
                    if c.kind == ConnKind::Corner {
                        stats.corners += 1;
                    }
                }
                connections.extend_from_slice(&scratch);
            }
        }
        conn_index.push(connections.len() as u32);

        let grid = Grid::build(&edges);
        stats.build_micros = start.elapsed().as_micros();
        GrindGraph { params, level: level.name.clone(), edges, nodes, incidences, conn_index, connections, grid, stats }
    }

    /// Connections out of `edge` through `end`, best first.
    pub fn connections_at(&self, edge: u32, end: usize) -> &[GrindConnection] {
        let i = edge as usize * 2 + end;
        &self.connections[self.conn_index[i] as usize..self.conn_index[i + 1] as usize]
    }

    /// Edges filed near (x, y): every edge within [`GRID_MARGIN`] of the
    /// point is included (some farther ones too). No allocation.
    pub fn near(&self, x: f32, y: f32) -> impl Iterator<Item = u32> + '_ {
        self.grid.cell(x, y).iter().copied()
    }

    /// Edges in all cells overlapping a square (for drawing).
    pub fn for_each_in_box(&self, min: [f32; 2], max: [f32; 2], mut f: impl FnMut(u32)) {
        let g = &self.grid;
        if g.cols == 0 {
            return;
        }
        let c0 = (((min[0] - g.org[0]) / GRID_CELL).floor() as i32).clamp(0, g.cols - 1);
        let c1 = (((max[0] - g.org[0]) / GRID_CELL).floor() as i32).clamp(0, g.cols - 1);
        let r0 = (((min[1] - g.org[1]) / GRID_CELL).floor() as i32).clamp(0, g.rows - 1);
        let r1 = (((max[1] - g.org[1]) / GRID_CELL).floor() as i32).clamp(0, g.rows - 1);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let i = (r * g.cols + c) as usize;
                for &e in &g.items[g.cells[i] as usize..g.cells[i + 1] as usize] {
                    f(e);
                }
            }
        }
    }

    /// The edge nearest (x, y) within `radius` (2D), with its distance.
    pub fn nearest(&self, x: f32, y: f32, radius: f32) -> Option<(u32, f32)> {
        let mut best = None;
        for e in self.near(x, y) {
            let edge = &self.edges[e as usize];
            let (s, _) = edge.local([x, y]);
            let p = edge.point(s.clamp(0.0, edge.length));
            let d = ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt();
            if d <= radius && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((e, d));
            }
        }
        best
    }
}

#[inline]
fn line_index(i: usize) -> LineId {
    LineId(i as u32)
}

/// Top of a masked middle texture that makes a grindable rail, relative to
/// the higher floor; `None` if the line has no such texture or it reaches
/// the ceiling (a fence you cannot get over).
fn rail_top(level: &Level, line: &crate::level::Line, heights: &[i32], floor: f32, ceiling: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for side in line.sidenum.iter().flatten() {
        let sd = level.side(*side);
        let tex = sd.mid_texture.0 as usize;
        if tex == 0 {
            continue;
        }
        let h = *heights.get(tex)? as f32;
        let row = fx(sd.row_offset.0);
        // Masked middles hang from the ceiling unless lower-unpegged.
        let top = if line.flags & doom_content::map::ml::DONTPEGBOTTOM != 0 { floor + row + h } else { ceiling + row };
        let top = top.min(ceiling);
        if top >= ceiling - 1.0 {
            continue;
        }
        let off = top - floor;
        if off >= 8.0 {
            best = Some(best.map_or(off, |b: f32| b.max(off)));
        }
    }
    best
}

/// Whether `sector` is narrower than `width` going from `from` along
/// `dir` (a unit vector into the sector): a raised strip, not a platform.
fn strip_width(level: &Level, sector: SectorId, skip: LineId, from: [f32; 2], dir: [f32; 2], width: f32) -> bool {
    for &l in level.lines_of(sector) {
        if l == skip {
            continue;
        }
        let line = level.line(l);
        let (v1, v2) = (level.vertex(line.v1), level.vertex(line.v2));
        let (ax, ay, bx, by) = (fx(v1.x.0), fx(v1.y.0), fx(v2.x.0), fx(v2.y.0));
        // Ray from + t*dir against segment a + u*(b - a).
        let (ex, ey) = (bx - ax, by - ay);
        let den = dir[0] * ey - dir[1] * ex;
        if den.abs() < 1e-6 {
            continue;
        }
        let (wx, wy) = (ax - from[0], ay - from[1]);
        let t = (wx * ey - wy * ex) / den;
        let u = (wx * dir[1] - wy * dir[0]) / den;
        if t > 0.5 && t < width && (0.0..=1.0).contains(&u) {
            return true;
        }
    }
    false
}

impl Grid {
    fn build(edges: &[GrindEdge]) -> Grid {
        if edges.is_empty() {
            return Grid::default();
        }
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for e in edges {
            for p in [e.a, e.b] {
                for k in 0..2 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
        }
        let org = [min[0] - GRID_MARGIN - 1.0, min[1] - GRID_MARGIN - 1.0];
        let cols = (((max[0] + GRID_MARGIN + 1.0 - org[0]) / GRID_CELL).ceil() as i32).max(1);
        let rows = (((max[1] + GRID_MARGIN + 1.0 - org[1]) / GRID_CELL).ceil() as i32).max(1);
        let ncells = (cols * rows) as usize;
        let cells_of = |e: &GrindEdge, f: &mut dyn FnMut(usize)| {
            let lo = [e.a[0].min(e.b[0]) - GRID_MARGIN, e.a[1].min(e.b[1]) - GRID_MARGIN];
            let hi = [e.a[0].max(e.b[0]) + GRID_MARGIN, e.a[1].max(e.b[1]) + GRID_MARGIN];
            let c0 = (((lo[0] - org[0]) / GRID_CELL) as i32).clamp(0, cols - 1);
            let c1 = (((hi[0] - org[0]) / GRID_CELL) as i32).clamp(0, cols - 1);
            let r0 = (((lo[1] - org[1]) / GRID_CELL) as i32).clamp(0, rows - 1);
            let r1 = (((hi[1] - org[1]) / GRID_CELL) as i32).clamp(0, rows - 1);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    // Only cells the padded segment actually passes near.
                    let cx = org[0] + (c as f32 + 0.5) * GRID_CELL;
                    let cy = org[1] + (r as f32 + 0.5) * GRID_CELL;
                    let (s, n) = e.local([cx, cy]);
                    let along = if s < 0.0 { -s } else if s > e.length { s - e.length } else { 0.0 };
                    let reach = GRID_CELL * std::f32::consts::FRAC_1_SQRT_2 + GRID_MARGIN;
                    if n.abs() <= reach && along <= reach {
                        f((r * cols + c) as usize);
                    }
                }
            }
        };
        let mut counts = vec![0u32; ncells + 1];
        for e in edges {
            cells_of(e, &mut |i| counts[i] += 1);
        }
        let mut cells = vec![0u32; ncells + 1];
        for i in 0..ncells {
            cells[i + 1] = cells[i] + counts[i];
        }
        let mut fill = cells.clone();
        let mut items = vec![0u32; cells[ncells] as usize];
        for (k, e) in edges.iter().enumerate() {
            cells_of(e, &mut |i| {
                items[fill[i] as usize] = k as u32;
                fill[i] += 1;
            });
        }
        Grid { org, cols, rows, cells, items }
    }

    fn cell(&self, x: f32, y: f32) -> &[u32] {
        if self.cols == 0 {
            return &[];
        }
        let c = ((x - self.org[0]) / GRID_CELL).floor() as i32;
        let r = ((y - self.org[1]) / GRID_CELL).floor() as i32;
        if c < 0 || r < 0 || c >= self.cols || r >= self.rows {
            return &[];
        }
        let i = (r * self.cols + c) as usize;
        &self.items[self.cells[i] as usize..self.cells[i + 1] as usize]
    }
}
