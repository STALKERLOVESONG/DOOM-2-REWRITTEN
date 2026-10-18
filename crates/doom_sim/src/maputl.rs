//! World-level map utilities (p_maputl.c): thing linking, blockmap thing
//! iteration and line-of-trace traversal.

use doom_core::fixed::{fixed_div, fixed_mul};
use doom_core::{Fixed, FRACUNIT};

use crate::geometry::{
    block_lines_iterator, intercept_vector, point_in_subsector, point_on_divline_side, point_on_line_side, Divline,
};
use crate::ids::*;
use crate::level::{MAPBLOCKSHIFT, MAPBLOCKSIZE, MAPBTOFRAC};
use crate::mobj::flags::*;
use crate::world::{World, MAXINTERCEPTS_ORIGINAL};

/// `P_UnsetThingPosition`: unlink from the sector and blockmap lists.
pub fn unset_thing_position(w: &mut World, id: MobjId) {
    let (flags, snext, sprev, bnext, bprev, ss, x, y) = {
        let m = w.mo(id);
        (m.flags, m.snext, m.sprev, m.bnext, m.bprev, m.subsector, m.x, m.y)
    };
    if flags & MF_NOSECTOR == 0 {
        if let Some(n) = snext {
            w.mo_mut(n).sprev = sprev;
        }
        match sprev {
            Some(p) => w.mo_mut(p).snext = snext,
            None => {
                let sec = w.level.subsectors[ss.idx()].sector;
                w.level.sector_mut(sec).thing_list = snext;
            }
        }
    }
    if flags & MF_NOBLOCKMAP == 0 {
        if let Some(n) = bnext {
            w.mo_mut(n).bprev = bprev;
        }
        match bprev {
            Some(p) => w.mo_mut(p).bnext = bnext,
            None => {
                let bm = &mut w.level.blockmap;
                let (bx, by) = (bm.block_x(x), bm.block_y(y));
                if bx >= 0 && bx < bm.width && by >= 0 && by < bm.height {
                    bm.links[(by * bm.width + bx) as usize] = bnext;
                }
            }
        }
    }
}

/// `P_SetThingPosition`: link into the subsector's sector list (at the head)
/// and the blockmap (at the head of the block's chain).
pub fn set_thing_position(w: &mut World, id: MobjId) {
    let (x, y, flags) = {
        let m = w.mo(id);
        (m.x, m.y, m.flags)
    };
    let ss = point_in_subsector(&w.level, x, y);
    w.mo_mut(id).subsector = ss;

    if flags & MF_NOSECTOR == 0 {
        let sec = w.level.subsectors[ss.idx()].sector;
        let head = w.level.sector(sec).thing_list;
        {
            let m = w.mo_mut(id);
            m.sprev = None;
            m.snext = head;
        }
        if let Some(h) = head {
            w.mo_mut(h).sprev = Some(id);
        }
        w.level.sector_mut(sec).thing_list = Some(id);
    }

    if flags & MF_NOBLOCKMAP == 0 {
        let bm = &w.level.blockmap;
        let (bx, by) = (bm.block_x(x), bm.block_y(y));
        if bx >= 0 && bx < bm.width && by >= 0 && by < bm.height {
            let slot = (by * bm.width + bx) as usize;
            let head = bm.links[slot];
            {
                let m = w.mo_mut(id);
                m.bprev = None;
                m.bnext = head;
            }
            if let Some(h) = head {
                w.mo_mut(h).bprev = Some(id);
            }
            w.level.blockmap.links[slot] = Some(id);
        } else {
            // Thing is off the map.
            let m = w.mo_mut(id);
            m.bnext = None;
            m.bprev = None;
        }
    }
}

/// `P_BlockThingsIterator`. The next link is read *after* `f` runs, so `f`
/// may unlink the current thing (as `PIT_ChangeSector` does).
pub fn block_things_iterator(w: &mut World, x: i32, y: i32, mut f: impl FnMut(&mut World, MobjId) -> bool) -> bool {
    let bm = &w.level.blockmap;
    if x < 0 || y < 0 || x >= bm.width || y >= bm.height {
        return true;
    }
    let mut cur = bm.links[(y * bm.width + x) as usize];
    while let Some(id) = cur {
        if !f(w, id) {
            return false;
        }
        cur = w.thinkers.get_mobj(id).and_then(|m| m.bnext);
    }
    true
}

/// `P_BlockLinesIterator` over the world (stamps `validcount`).
pub fn block_lines(w: &mut World, x: i32, y: i32, f: impl FnMut(&crate::level::Level, LineId) -> bool) -> bool {
    block_lines_iterator(&mut w.level, x, y, f)
}

// ---------------------------------------------------------------------------
// Intercepts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterceptKind {
    Line(LineId),
    Thing(MobjId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Intercept {
    /// Along the trace line.
    pub frac: Fixed,
    pub kind: InterceptKind,
}

pub mod pt {
    pub const ADDLINES: i32 = 1;
    pub const ADDTHINGS: i32 = 2;
    pub const EARLYOUT: i32 = 4;
    /// Doom 1.0-1.2 line intercepts (Chocolate's `PT_COMPATADDLINES`).
    pub const COMPATADDLINES: i32 = 8;
}

/// `PIT_AddLineIntercepts`. Returns false to stop (early out on a solid line).
fn add_line_intercept(w: &mut World, out: &mut Vec<Intercept>, ld: LineId) -> bool {
    let trace = w.tm.trace;
    let level = &w.level;
    let line = level.line(ld);
    let (v1, v2) = (level.vertex(line.v1), level.vertex(line.v2));
    // Avoid precision problems with two routines.
    let big = 16 * FRACUNIT;
    let (s1, s2) = if trace.dx.0 > big || trace.dy.0 > big || trace.dx.0 < -big || trace.dy.0 < -big {
        (point_on_divline_side(v1.x, v1.y, &trace), point_on_divline_side(v2.x, v2.y, &trace))
    } else {
        (
            point_on_line_side(level, trace.x, trace.y, line),
            point_on_line_side(level, trace.x + trace.dx, trace.y + trace.dy, line),
        )
    };
    if s1 == s2 {
        return true; // line isn't crossed
    }
    let dl = Divline::from_line(level, line);
    let frac = intercept_vector(&trace, &dl);
    if frac.0 < 0 {
        return true; // behind source
    }
    // Try to early out the check.
    if w.tm.earlyout && frac.0 < FRACUNIT && line.back_sector.is_none() {
        return false;
    }
    out.push(Intercept { frac, kind: InterceptKind::Line(ld) });
    if out.len() > MAXINTERCEPTS_ORIGINAL {
        w.note_overflow("intercepts overflow (InterceptsOverrun not emulated)");
    }
    true
}

/// `PIT_CompatAddLineIntercepts`: Doom 1.2 line intercepts (as used by its
/// sight check), reconstructed by PrBoom+/Chocolate Doom.
fn compat_add_line_intercept(w: &mut World, out: &mut Vec<Intercept>, ld: LineId) -> bool {
    let trace = w.tm.trace;
    let level = &w.level;
    let line = level.line(ld);
    let (v1, v2) = (level.vertex(line.v1), level.vertex(line.v2));
    if point_on_divline_side(v1.x, v1.y, &trace) == point_on_divline_side(v2.x, v2.y, &trace) {
        return true; // line isn't crossed
    }
    let dl = Divline::from_line(level, line);
    if point_on_divline_side(trace.x, trace.y, &dl) == point_on_divline_side(trace.x + trace.dx, trace.y + trace.dy, &dl) {
        return true; // line isn't crossed
    }
    let frac = intercept_vector(&trace, &dl);
    // Try to early out the check.
    if line.back_sector.is_none() {
        return false; // stop checking
    }
    out.push(Intercept { frac, kind: InterceptKind::Line(ld) });
    if out.len() > MAXINTERCEPTS_ORIGINAL {
        w.note_overflow("intercepts overflow (InterceptsOverrun not emulated)");
    }
    true
}

/// `PIT_AddThingIntercepts`.
fn add_thing_intercept(w: &mut World, out: &mut Vec<Intercept>, id: MobjId) -> bool {
    let trace = w.tm.trace;
    let m = w.mo(id);
    let tracepositive = (trace.dx.0 ^ trace.dy.0) > 0;
    // Check a corner to corner crossection for hit.
    let (x1, y1, x2, y2) = if tracepositive {
        (m.x - m.radius, m.y + m.radius, m.x + m.radius, m.y - m.radius)
    } else {
        (m.x - m.radius, m.y - m.radius, m.x + m.radius, m.y + m.radius)
    };
    let s1 = point_on_divline_side(x1, y1, &trace);
    let s2 = point_on_divline_side(x2, y2, &trace);
    if s1 == s2 {
        return true; // line isn't crossed
    }
    let dl = Divline { x: x1, y: y1, dx: x2 - x1, dy: y2 - y1 };
    let frac = intercept_vector(&trace, &dl);
    if frac.0 < 0 {
        return true; // behind source
    }
    out.push(Intercept { frac, kind: InterceptKind::Thing(id) });
    if out.len() > MAXINTERCEPTS_ORIGINAL {
        w.note_overflow("intercepts overflow (InterceptsOverrun not emulated)");
    }
    true
}

/// `P_TraverseIntercepts`: repeatedly pick the nearest remaining intercept
/// (first one in insertion order on ties) up to `maxfrac`.
fn traverse_intercepts(
    w: &mut World,
    intercepts: &mut [Intercept],
    maxfrac: Fixed,
    trav: &mut impl FnMut(&mut World, &Intercept) -> bool,
) -> bool {
    for _ in 0..intercepts.len() {
        let mut dist = i32::MAX;
        let mut best = 0;
        for (i, scan) in intercepts.iter().enumerate() {
            if scan.frac.0 < dist {
                dist = scan.frac.0;
                best = i;
            }
        }
        if dist > maxfrac.0 {
            return true; // checked everything in range
        }
        let inter = intercepts[best];
        if !trav(w, &inter) {
            return false; // don't bother going farther
        }
        intercepts[best].frac = Fixed(i32::MAX);
    }
    true
}

/// `P_PathTraverse`: trace from (x1,y1) to (x2,y2), calling `trav` for each
/// line/thing crossed in order. Returns true if `trav` never stopped it.
pub fn path_traverse(
    w: &mut World,
    mut x1: Fixed,
    mut y1: Fixed,
    mut x2: Fixed,
    mut y2: Fixed,
    flags: i32,
    mut trav: impl FnMut(&mut World, &Intercept) -> bool,
) -> bool {
    w.tm.earlyout = flags & pt::EARLYOUT != 0;
    w.level.valid.bump();
    let mut intercepts: Vec<Intercept> = Vec::new();

    let (org_x, org_y) = (w.level.blockmap.org_x, w.level.blockmap.org_y);
    if ((x1 - org_x).0 & (MAPBLOCKSIZE - 1)) == 0 {
        x1 += Fixed::ONE; // don't side exactly on a line
    }
    if ((y1 - org_y).0 & (MAPBLOCKSIZE - 1)) == 0 {
        y1 += Fixed::ONE;
    }
    w.tm.trace = Divline { x: x1, y: y1, dx: x2 - x1, dy: y2 - y1 };

    x1 -= org_x;
    y1 -= org_y;
    let xt1 = x1.0 >> MAPBLOCKSHIFT;
    let yt1 = y1.0 >> MAPBLOCKSHIFT;
    x2 -= org_x;
    y2 -= org_y;
    let xt2 = x2.0 >> MAPBLOCKSHIFT;
    let yt2 = y2.0 >> MAPBLOCKSHIFT;

    let (mapxstep, mut partial, ystep);
    if xt2 > xt1 {
        mapxstep = 1;
        partial = Fixed(FRACUNIT - ((x1.0 >> MAPBTOFRAC) & (FRACUNIT - 1)));
        ystep = fixed_div(y2 - y1, (x2 - x1).abs());
    } else if xt2 < xt1 {
        mapxstep = -1;
        partial = Fixed((x1.0 >> MAPBTOFRAC) & (FRACUNIT - 1));
        ystep = fixed_div(y2 - y1, (x2 - x1).abs());
    } else {
        mapxstep = 0;
        partial = Fixed::ONE;
        ystep = Fixed(256 * FRACUNIT);
    }
    let mut yintercept = Fixed(y1.0 >> MAPBTOFRAC) + fixed_mul(partial, ystep);

    let (mapystep, xstep);
    if yt2 > yt1 {
        mapystep = 1;
        partial = Fixed(FRACUNIT - ((y1.0 >> MAPBTOFRAC) & (FRACUNIT - 1)));
        xstep = fixed_div(x2 - x1, (y2 - y1).abs());
    } else if yt2 < yt1 {
        mapystep = -1;
        partial = Fixed((y1.0 >> MAPBTOFRAC) & (FRACUNIT - 1));
        xstep = fixed_div(x2 - x1, (y2 - y1).abs());
    } else {
        mapystep = 0;
        partial = Fixed::ONE;
        xstep = Fixed(256 * FRACUNIT);
    }
    let mut xintercept = Fixed(x1.0 >> MAPBTOFRAC) + fixed_mul(partial, xstep);

    // Step through map blocks. Count is present to prevent a round off error
    // from skipping the break.
    let (mut mapx, mut mapy) = (xt1, yt1);
    for _ in 0..64 {
        if flags & pt::ADDLINES != 0 {
            // Collect candidate lines first (stamping validcount), then test
            // them in the same order; `add_line_intercept` only reads.
            let mut lines = Vec::new();
            block_lines_iterator(&mut w.level, mapx, mapy, |_, l| {
                lines.push(l);
                true
            });
            for l in lines {
                if !add_line_intercept(w, &mut intercepts, l) {
                    return false; // early out
                }
            }
        }
        if flags & pt::ADDTHINGS != 0 {
            let mut things = Vec::new();
            block_things_iterator(w, mapx, mapy, |_, id| {
                things.push(id);
                true
            });
            for t in things {
                if !add_thing_intercept(w, &mut intercepts, t) {
                    return false;
                }
            }
        }
        if flags & pt::COMPATADDLINES != 0 {
            let mut lines = Vec::new();
            block_lines_iterator(&mut w.level, mapx, mapy, |_, l| {
                lines.push(l);
                true
            });
            for l in lines {
                if !compat_add_line_intercept(w, &mut intercepts, l) {
                    return false;
                }
            }
        }
        if mapx == xt2 && mapy == yt2 {
            break;
        }
        if yintercept.to_int() == mapy {
            yintercept += ystep;
            mapx += mapxstep;
        } else if xintercept.to_int() == mapx {
            xintercept += xstep;
            mapy += mapystep;
        }
    }
    // Go through the sorted list.
    traverse_intercepts(w, &mut intercepts, Fixed::ONE, &mut trav)
}
