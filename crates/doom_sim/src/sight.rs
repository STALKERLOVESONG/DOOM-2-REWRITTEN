//! Line of sight (p_sight.c): REJECT test, then a BSP walk along the sight
//! line checking two-sided openings.

use doom_content::game::GameVersion;
use doom_core::fixed::{fixed_div, fixed_mul};
use doom_core::{Fixed, FRACBITS};

use crate::geometry::{line_opening, Divline};
use crate::ids::*;
use crate::level::NF_SUBSECTOR;
use crate::maputl::{path_traverse, pt, InterceptKind};
use crate::world::World;
use doom_content::map::ml;

struct SightTrace {
    sightzstart: Fixed,
    strace: Divline,
    t2x: Fixed,
    t2y: Fixed,
}

/// `P_DivlineSide`: 0 front, 1 back, 2 on the line.
///
/// Vanilla bug preserved: the horizontal-line case compares `x` against
/// `node->y`.
fn divline_side(x: Fixed, y: Fixed, node: &Divline) -> i32 {
    if node.dx.0 == 0 {
        if x == node.x {
            return 2;
        }
        if x <= node.x {
            return (node.dy.0 > 0) as i32;
        }
        return (node.dy.0 < 0) as i32;
    }
    if node.dy.0 == 0 {
        if x == node.y {
            return 2;
        }
        if y <= node.y {
            return (node.dx.0 < 0) as i32;
        }
        return (node.dx.0 > 0) as i32;
    }
    let dx = x - node.x;
    let dy = y - node.y;
    let left = (node.dy.0 >> FRACBITS).wrapping_mul(dx.0 >> FRACBITS);
    let right = (dy.0 >> FRACBITS).wrapping_mul(node.dx.0 >> FRACBITS);
    if right < left {
        0 // front side
    } else if left == right {
        2
    } else {
        1 // back side
    }
}

/// `P_InterceptVector2` (identical arithmetic to `P_InterceptVector`).
fn intercept_vector2(v2: &Divline, v1: &Divline) -> Fixed {
    let den = fixed_mul(v1.dy >> 8, v2.dx) - fixed_mul(v1.dx >> 8, v2.dy);
    if den.0 == 0 {
        return Fixed(0);
    }
    let num = fixed_mul((v1.x - v2.x) >> 8, v1.dy) + fixed_mul((v2.y - v1.y) >> 8, v1.dx);
    fixed_div(num, den)
}

/// `P_CrossSubsector`: returns true if the strace crosses the given
/// subsector successfully.
fn cross_subsector(w: &mut World, st: &SightTrace, num: usize) -> bool {
    let (first, count) = {
        let ss = &w.level.subsectors[num];
        (ss.first_line as usize, ss.num_lines as usize)
    };
    for segi in first..first + count {
        let seg = w.level.segs[segi].clone();
        let li = seg.linedef.idx();
        // Allready checked other side?
        if w.level.valid.lines[li] == w.level.valid.current {
            continue;
        }
        w.level.valid.lines[li] = w.level.valid.current;
        let line = &w.level.lines[li];
        let (v1, v2) = (w.level.vertex(line.v1), w.level.vertex(line.v2));
        let s1 = divline_side(v1.x, v1.y, &st.strace);
        let s2 = divline_side(v2.x, v2.y, &st.strace);
        // Line isn't crossed?
        if s1 == s2 {
            continue;
        }
        let divl = Divline { x: v1.x, y: v1.y, dx: v2.x - v1.x, dy: v2.y - v1.y };
        let s1 = divline_side(st.strace.x, st.strace.y, &divl);
        let s2 = divline_side(st.t2x, st.t2y, &divl);
        // Line isn't crossed?
        if s1 == s2 {
            continue;
        }
        // Backsector may be NULL if this is an "impassible glass" hack line.
        if line.back_sector.is_none() {
            return false;
        }
        // Stop because it is not two sided anyway; might do this after
        // updating validcount?
        if line.flags & ml::TWOSIDED == 0 {
            return false;
        }
        // Crosses a two sided line.
        let front = w.level.sector(seg.front_sector);
        let back = w.level.sector(seg.back_sector.expect("two-sided seg has a back sector"));
        // No wall to block sight with?
        if front.floor_height == back.floor_height && front.ceiling_height == back.ceiling_height {
            continue;
        }
        // Possible occluder because of ceiling height differences.
        let opentop = if front.ceiling_height < back.ceiling_height { front.ceiling_height } else { back.ceiling_height };
        // Because of ceiling height differences.
        let openbottom = if front.floor_height > back.floor_height { front.floor_height } else { back.floor_height };
        // Quick test for totally closed doors.
        if openbottom >= opentop {
            return false; // stop
        }
        let frac = intercept_vector2(&st.strace, &divl);
        if front.floor_height != back.floor_height {
            let slope = fixed_div(openbottom - st.sightzstart, frac);
            if slope > w.tm.bottomslope {
                w.tm.bottomslope = slope;
            }
        }
        if front.ceiling_height != back.ceiling_height {
            let slope = fixed_div(opentop - st.sightzstart, frac);
            if slope < w.tm.topslope {
                w.tm.topslope = slope;
            }
        }
        if w.tm.topslope <= w.tm.bottomslope {
            return false; // stop
        }
    }
    // Passed the subsector ok.
    true
}

/// `P_CrossBSPNode`: returns true if the strace crosses the given node
/// successfully.
fn cross_bsp_node(w: &mut World, st: &SightTrace, bspnum: u32) -> bool {
    // Vanilla's `bspnum == -1` (no nodes) is `bsp_root()` = subsector 0.
    if bspnum & NF_SUBSECTOR != 0 {
        return cross_subsector(w, st, (bspnum & !NF_SUBSECTOR) as usize);
    }
    let node = &w.level.nodes[bspnum as usize];
    let dl = Divline { x: node.x, y: node.y, dx: node.dx, dy: node.dy };
    let children = node.children;
    // Decide which side the start point is on.
    let mut side = divline_side(st.strace.x, st.strace.y, &dl);
    if side == 2 {
        side = 0; // an "on" should cross both sides
    }
    // Cross the starting side.
    if !cross_bsp_node(w, st, children[side as usize]) {
        return false;
    }
    // The partition plane is crossed here.
    if side == divline_side(st.t2x, st.t2y, &dl) {
        return true; // the line doesn't touch the other side
    }
    // Cross the ending side.
    cross_bsp_node(w, st, children[(side ^ 1) as usize])
}

/// `P_CheckSight`: returns true if a straight line between t1 and t2 is
/// unobstructed. Uses REJECT.
pub fn check_sight(w: &mut World, t1: MobjId, t2: MobjId) -> bool {
    let (a, b) = (w.mo(t1).clone(), w.mo(t2).clone());
    let s1 = w.level.subsectors[a.subsector.idx()].sector;
    let s2 = w.level.subsectors[b.subsector.idx()].sector;
    // Check for trivial rejection.
    if w.level.rejected(s1, s2) {
        return false; // can't possibly be connected
    }
    // An unobstructed LOS is possible. Now look from eyes of t1 to any part
    // of t2.
    w.level.valid.bump();
    let sightzstart = a.z + a.height - (a.height >> 2);
    w.tm.topslope = (b.z + b.height) - sightzstart;
    w.tm.bottomslope = b.z - sightzstart;

    if w.params.version <= GameVersion::Doom1_2 {
        return path_traverse(w, a.x, a.y, b.x, b.y, pt::EARLYOUT | pt::COMPATADDLINES, move |w, inter| {
            // `PTR_SightTraverse`
            let InterceptKind::Line(ld) = inter.kind else { return true };
            let line = w.level.line(ld).clone();
            let op = line_opening(&w.level, &line, w.tm.opening);
            w.tm.opening = op;
            if op.bottom >= op.top {
                return false; // quick test for totally closed doors
            }
            let front = w.level.sector(line.front_sector.unwrap()).clone();
            let back = w.level.sector(line.back_sector.unwrap()).clone();
            if front.floor_height != back.floor_height {
                let slope = fixed_div(op.bottom - sightzstart, inter.frac);
                if slope > w.tm.bottomslope {
                    w.tm.bottomslope = slope;
                }
            }
            if front.ceiling_height != back.ceiling_height {
                let slope = fixed_div(op.top - sightzstart, inter.frac);
                if slope < w.tm.topslope {
                    w.tm.topslope = slope;
                }
            }
            w.tm.topslope > w.tm.bottomslope
        });
    }

    let st = SightTrace {
        sightzstart,
        strace: Divline { x: a.x, y: a.y, dx: b.x - a.x, dy: b.y - a.y },
        t2x: b.x,
        t2y: b.y,
    };
    // The head node is the last node output.
    cross_bsp_node(w, &st, w.level.bsp_root())
}
