//! Geometric queries on the level (p_maputl.c, `R_PointInSubsector` /
//! `R_PointOnSide` from r_main.c). All bit-exact with the C code.

use doom_core::bbox::{BOXBOTTOM, BOXLEFT, BOXRIGHT, BOXTOP};
use doom_core::fixed::fixed_mul;
use doom_core::{BBox, Fixed, FRACBITS};

use crate::ids::*;
use crate::level::{Level, Line, Node, SlopeType, NF_SUBSECTOR};

/// `R_PointOnSide`: 0 = front (right), 1 = back (left) of a BSP node line.
pub fn point_on_node_side(x: Fixed, y: Fixed, node: &Node) -> usize {
    if node.dx.0 == 0 {
        return if x <= node.x { (node.dy.0 > 0) as usize } else { (node.dy.0 < 0) as usize };
    }
    if node.dy.0 == 0 {
        return if y <= node.y { (node.dx.0 < 0) as usize } else { (node.dx.0 > 0) as usize };
    }
    let dx = x - node.x;
    let dy = y - node.y;
    // Try to quickly decide by looking at sign bits.
    if (node.dy.0 ^ node.dx.0 ^ dx.0 ^ dy.0) < 0 {
        return if (node.dy.0 ^ dx.0) < 0 { 1 } else { 0 };
    }
    let left = fixed_mul(Fixed(node.dy.0 >> FRACBITS), dx);
    let right = fixed_mul(dy, Fixed(node.dx.0 >> FRACBITS));
    if right < left {
        0
    } else {
        1
    }
}

/// `R_PointInSubsector`: walk the BSP from the root.
pub fn point_in_subsector(level: &Level, x: Fixed, y: Fixed) -> SubsectorId {
    if level.nodes.is_empty() {
        return SubsectorId(0); // single subsector map
    }
    let mut nodenum = level.bsp_root();
    while nodenum & NF_SUBSECTOR == 0 {
        let node = &level.nodes[nodenum as usize];
        nodenum = node.children[point_on_node_side(x, y, node)];
    }
    SubsectorId((nodenum & !NF_SUBSECTOR) as u32)
}

/// Sector containing a point (via the BSP, as everything in Doom does).
#[inline]
pub fn point_in_sector(level: &Level, x: Fixed, y: Fixed) -> SectorId {
    level.subsectors[point_in_subsector(level, x, y).idx()].sector
}

/// `P_PointOnLineSide`: 0 = front, 1 = back. Note this version has *no*
/// sign-bit shortcut and uses `dy >> FRACBITS` precision, unlike
/// `P_PointOnDivlineSide`.
pub fn point_on_line_side(level: &Level, x: Fixed, y: Fixed, line: &Line) -> usize {
    let v1 = level.vertex(line.v1);
    if line.dx.0 == 0 {
        return if x <= v1.x { (line.dy.0 > 0) as usize } else { (line.dy.0 < 0) as usize };
    }
    if line.dy.0 == 0 {
        return if y <= v1.y { (line.dx.0 < 0) as usize } else { (line.dx.0 > 0) as usize };
    }
    let dx = x - v1.x;
    let dy = y - v1.y;
    let left = fixed_mul(Fixed(line.dy.0 >> FRACBITS), dx);
    let right = fixed_mul(dy, Fixed(line.dx.0 >> FRACBITS));
    if right < left {
        0
    } else {
        1
    }
}

/// `P_BoxOnLineSide`: `Some(side)` if the whole box is on one side,
/// `None` (C: -1) if the line crosses it.
pub fn box_on_line_side(level: &Level, tmbox: &BBox, ld: &Line) -> Option<usize> {
    let b = &tmbox.0;
    let v1 = level.vertex(ld.v1);
    let (p1, p2) = match ld.slope_type {
        SlopeType::Horizontal => {
            let (mut p1, mut p2) = ((b[BOXTOP] > v1.y) as usize, (b[BOXBOTTOM] > v1.y) as usize);
            if ld.dx.0 < 0 {
                p1 ^= 1;
                p2 ^= 1;
            }
            (p1, p2)
        }
        SlopeType::Vertical => {
            let (mut p1, mut p2) = ((b[BOXRIGHT] < v1.x) as usize, (b[BOXLEFT] < v1.x) as usize);
            if ld.dy.0 < 0 {
                p1 ^= 1;
                p2 ^= 1;
            }
            (p1, p2)
        }
        SlopeType::Positive => (
            point_on_line_side(level, b[BOXLEFT], b[BOXTOP], ld),
            point_on_line_side(level, b[BOXRIGHT], b[BOXBOTTOM], ld),
        ),
        SlopeType::Negative => (
            point_on_line_side(level, b[BOXRIGHT], b[BOXTOP], ld),
            point_on_line_side(level, b[BOXLEFT], b[BOXBOTTOM], ld),
        ),
    };
    (p1 == p2).then_some(p1)
}

/// `divline_t`: a line in point + delta form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Divline {
    pub x: Fixed,
    pub y: Fixed,
    pub dx: Fixed,
    pub dy: Fixed,
}

impl Divline {
    /// `P_MakeDivline`.
    pub fn from_line(level: &Level, li: &Line) -> Self {
        let v1 = level.vertex(li.v1);
        Divline { x: v1.x, y: v1.y, dx: li.dx, dy: li.dy }
    }
}

/// `P_PointOnDivlineSide`: has the sign-bit shortcut and works at `>> 8`
/// precision.
pub fn point_on_divline_side(x: Fixed, y: Fixed, line: &Divline) -> usize {
    if line.dx.0 == 0 {
        return if x <= line.x { (line.dy.0 > 0) as usize } else { (line.dy.0 < 0) as usize };
    }
    if line.dy.0 == 0 {
        return if y <= line.y { (line.dx.0 < 0) as usize } else { (line.dx.0 > 0) as usize };
    }
    let dx = x - line.x;
    let dy = y - line.y;
    if (line.dy.0 ^ line.dx.0 ^ dx.0 ^ dy.0) < 0 {
        return if (line.dy.0 ^ dx.0) < 0 { 1 } else { 0 };
    }
    let left = fixed_mul(line.dy >> 8, dx >> 8);
    let right = fixed_mul(dy >> 8, line.dx >> 8);
    if right < left {
        0
    } else {
        1
    }
}

/// `P_InterceptVector`: fractional position of the intersection along `v2`.
/// Returns 0 for parallel lines (the original's `I_Error` is commented out).
pub fn intercept_vector(v2: &Divline, v1: &Divline) -> Fixed {
    let den = fixed_mul(v1.dy >> 8, v2.dx) - fixed_mul(v1.dx >> 8, v2.dy);
    if den.0 == 0 {
        return Fixed(0);
    }
    let num = fixed_mul((v1.x - v2.x) >> 8, v1.dy) + fixed_mul((v2.y - v1.y) >> 8, v1.dx);
    num / den
}

/// Result of `P_LineOpening` (the C globals `opentop`, `openbottom`,
/// `openrange`, `lowfloor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opening {
    pub top: Fixed,
    pub bottom: Fixed,
    pub range: Fixed,
    pub low_floor: Fixed,
}

/// `P_LineOpening`. For one-sided lines vanilla only sets `openrange = 0` and
/// leaves the other globals untouched; callers that care pass the previous
/// opening in `prev` to reproduce that.
pub fn line_opening(level: &Level, line: &Line, prev: Opening) -> Opening {
    if line.sidenum[1].is_none() {
        return Opening { range: Fixed(0), ..prev };
    }
    // Two-sided lines always have both sectors (validated at load).
    let front = level.sector(line.front_sector.unwrap());
    let back = level.sector(line.back_sector.unwrap());
    let top = if front.ceiling_height < back.ceiling_height { front.ceiling_height } else { back.ceiling_height };
    let (bottom, low_floor) = if front.floor_height > back.floor_height {
        (front.floor_height, back.floor_height)
    } else {
        (back.floor_height, front.floor_height)
    };
    Opening { top, bottom, range: top - bottom, low_floor }
}

/// `P_BlockLinesIterator`: visit each line in block (x, y) not yet seen this
/// `validcount`, stopping when `f` returns false. Returns false if stopped.
///
/// Takes the level mutably only to stamp `validcount`; `f` receives a shared
/// borrow so it can inspect anything else.
pub fn block_lines_iterator(level: &mut Level, x: i32, y: i32, mut f: impl FnMut(&Level, LineId) -> bool) -> bool {
    if x < 0 || y < 0 || x >= level.blockmap.width || y >= level.blockmap.height {
        return true;
    }
    let start = level.blockmap.offsets[(y * level.blockmap.width + x) as usize];
    if start == u32::MAX {
        return true;
    }
    let mut i = start as usize;
    loop {
        let Some(&raw) = level.blockmap.words.get(i) else { break };
        i += 1;
        if raw == crate::level::Blockmap::END {
            break;
        }
        // Out-of-range entries in corrupt blockmaps are skipped instead of
        // reading garbage.
        if raw as usize >= level.lines.len() {
            continue;
        }
        let ld = raw as usize;
        if level.valid.lines[ld] == level.valid.current {
            continue; // already checked
        }
        level.valid.lines[ld] = level.valid.current;
        if !f(level, LineId(ld as u32)) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divline_side_basic() {
        let dl = Divline { x: Fixed(0), y: Fixed(0), dx: Fixed::from_int(10), dy: Fixed::from_int(10) };
        // Point to the right of a north-east line is the front side.
        assert_eq!(point_on_divline_side(Fixed::from_int(5), Fixed::from_int(0), &dl), 0);
        assert_eq!(point_on_divline_side(Fixed::from_int(0), Fixed::from_int(5), &dl), 1);
    }

    #[test]
    fn intercept_midpoint() {
        let trace = Divline { x: Fixed(0), y: Fixed::from_int(-10), dx: Fixed(0), dy: Fixed::from_int(20) };
        let wall = Divline { x: Fixed::from_int(-10), y: Fixed(0), dx: Fixed::from_int(20), dy: Fixed(0) };
        assert_eq!(intercept_vector(&trace, &wall), Fixed(doom_core::FRACUNIT / 2));
    }
}
