//! Movement, collision and line-of-fire (p_map.c).

use doom_core::bbox::{BOXBOTTOM, BOXLEFT, BOXRIGHT, BOXTOP};
use doom_core::fixed::{fixed_div, fixed_mul};
use doom_core::geom::{aprox_distance, point_to_angle2};
use doom_core::tables::{finecosine, finesine};
use doom_core::{Angle, BBox, Fixed, FRACUNIT};

use crate::geometry::{box_on_line_side, line_opening, point_in_subsector, point_on_line_side};
use crate::ids::*;
use crate::info::*;
use crate::level::{SlopeType, MAPBLOCKSHIFT, MAXRADIUS};
use crate::maputl::{block_things_iterator, path_traverse, pt, set_thing_position, unset_thing_position, Intercept, InterceptKind};
use crate::mobj::flags::*;
use crate::mobj::{remove_mobj, set_mobj_state, spawn_blood, spawn_mobj, spawn_puff};
use crate::world::{SoundSource, World, MAXSPECIALCROSS_ORIGINAL};
use doom_content::map::ml;

pub const MAXMOVE: Fixed = Fixed(30 * FRACUNIT);
pub const USERANGE: Fixed = Fixed(64 * FRACUNIT);
pub const MELEERANGE: Fixed = Fixed(64 * FRACUNIT);
pub const MISSILERANGE: Fixed = Fixed(32 * 64 * FRACUNIT);
/// Maximum step-up height.
const STEP: Fixed = Fixed(24 * FRACUNIT);

/// Chocolate Doom's default base address for spechit overrun emulation.
const DEFAULT_SPECHIT_MAGIC: u32 = 0x01C0_9C98;

fn thing_bbox(x: Fixed, y: Fixed, radius: Fixed) -> BBox {
    let mut b = BBox::cleared();
    b.0[BOXTOP] = y + radius;
    b.0[BOXBOTTOM] = y - radius;
    b.0[BOXRIGHT] = x + radius;
    b.0[BOXLEFT] = x - radius;
    b
}

/// Common start of `P_CheckPosition` / `P_TeleportMove`.
fn begin_position_check(w: &mut World, id: MobjId, x: Fixed, y: Fixed) {
    let (flags, radius) = {
        let m = w.mo(id);
        (m.flags, m.radius)
    };
    w.tm.tmthing = Some(id);
    w.tm.tmflags = flags;
    w.tm.tmx = x;
    w.tm.tmy = y;
    w.tm.tmbbox = thing_bbox(x, y, radius);
    let ss = point_in_subsector(&w.level, x, y);
    let sec = w.level.sector(w.level.subsectors[ss.idx()].sector);
    w.tm.ceilingline = None;
    w.tm.tmfloorz = sec.floor_height;
    w.tm.tmdropoffz = sec.floor_height;
    w.tm.tmceilingz = sec.ceiling_height;
    w.level.valid.bump();
    w.tm.spechit.clear();
}

/// Block range covered by the move box, optionally padded by `MAXRADIUS`.
fn block_range(w: &World, pad: Fixed) -> (i32, i32, i32, i32) {
    let b = &w.tm.tmbbox.0;
    let (ox, oy) = (w.level.blockmap.org_x, w.level.blockmap.org_y);
    (
        (b[BOXLEFT] - ox - pad).0 >> MAPBLOCKSHIFT,
        (b[BOXRIGHT] - ox + pad).0 >> MAPBLOCKSHIFT,
        (b[BOXBOTTOM] - oy - pad).0 >> MAPBLOCKSHIFT,
        (b[BOXTOP] - oy + pad).0 >> MAPBLOCKSHIFT,
    )
}

// ---------------------------------------------------------------------------
// Teleport move
// ---------------------------------------------------------------------------

/// `PIT_StompThing`.
fn pit_stomp_thing(w: &mut World, thing: MobjId) -> bool {
    let tm = w.tm.tmthing.unwrap();
    let (tx, ty) = (w.tm.tmx, w.tm.tmy);
    let t = w.mo(thing);
    if t.flags & MF_SHOOTABLE == 0 {
        return true;
    }
    let blockdist = t.radius + w.mo(tm).radius;
    if (t.x - tx).abs() >= blockdist || (t.y - ty).abs() >= blockdist {
        return true; // didn't hit it
    }
    // Don't clip against self.
    if thing == tm {
        return true;
    }
    // Monsters don't stomp things except on boss level.
    if w.mo(tm).player.is_none() && w.params.map != 30 {
        return false;
    }
    crate::inter::damage_mobj(w, thing, Some(tm), Some(tm), 10000);
    true
}

/// `P_TeleportMove`.
pub fn teleport_move(w: &mut World, id: MobjId, x: Fixed, y: Fixed) -> bool {
    begin_position_check(w, id, x, y);
    // Stomp on any things contacted.
    let (xl, xh, yl, yh) = block_range(w, MAXRADIUS);
    for bx in xl..=xh {
        for by in yl..=yh {
            if !block_things_iterator(w, bx, by, pit_stomp_thing) {
                return false;
            }
        }
    }
    // The move is ok, so link the thing into its new position.
    unset_thing_position(w, id);
    let (fz, cz) = (w.tm.tmfloorz, w.tm.tmceilingz);
    let m = w.mo_mut(id);
    m.floorz = fz;
    m.ceilingz = cz;
    m.x = x;
    m.y = y;
    set_thing_position(w, id);
    true
}

// ---------------------------------------------------------------------------
// Movement clipping
// ---------------------------------------------------------------------------

/// `SpechitOverrun`: vanilla's `spechit[8]` overflowed into the variables
/// that follow it in memory. Chocolate Doom's emulation is reproduced for
/// the cases it handles (9..=14 lines).
fn spechit_overrun(w: &mut World, ld: LineId) {
    let addr = DEFAULT_SPECHIT_MAGIC.wrapping_add(ld.0.wrapping_mul(0x3E));
    let n = w.tm.spechit.len();
    match n {
        9..=12 => w.tm.tmbbox.0[n - 9] = Fixed(addr as i32),
        13 => w.tm.crushchange = addr != 0,
        14 => w.tm.nofit = addr != 0,
        _ => w.note_overflow("spechit overrun beyond 14 lines (not emulated)"),
    }
}

/// `PIT_CheckLine`: adjusts `tmfloorz`/`tmceilingz` as lines are contacted.
fn pit_check_line(w: &mut World, ld: LineId) -> bool {
    let line = w.level.line(ld);
    let b = &w.tm.tmbbox.0;
    if b[BOXRIGHT] <= line.bbox.0[BOXLEFT]
        || b[BOXLEFT] >= line.bbox.0[BOXRIGHT]
        || b[BOXTOP] <= line.bbox.0[BOXBOTTOM]
        || b[BOXBOTTOM] >= line.bbox.0[BOXTOP]
    {
        return true;
    }
    if box_on_line_side(&w.level, &w.tm.tmbbox, line).is_some() {
        return true;
    }

    // A line has been hit.
    //
    // The moving thing's destination position will cross the given line. If
    // this should not be allowed, return false. If the line is special, keep
    // track of it to process later if the move is proven ok.
    // NOTE: specials are NOT sorted by order, so two special lines that are
    // only 8 pixels apart could be crossed in either order.
    if line.back_sector.is_none() {
        return false; // one sided line
    }
    let tm = w.tm.tmthing.unwrap();
    let (tflags, is_player) = {
        let t = w.mo(tm);
        (t.flags, t.player.is_some())
    };
    if tflags & MF_MISSILE == 0 {
        if line.flags & ml::BLOCKING != 0 {
            return false; // explicitly blocking everything
        }
        if !is_player && line.flags & ml::BLOCKMONSTERS != 0 {
            return false; // block monsters only
        }
    }

    // Set openrange, opentop, openbottom.
    let op = line_opening(&w.level, line, w.tm.opening);
    let special = line.special;
    w.tm.opening = op;
    // Adjust floor / ceiling heights.
    if op.top < w.tm.tmceilingz {
        w.tm.tmceilingz = op.top;
        w.tm.ceilingline = Some(ld);
    }
    if op.bottom > w.tm.tmfloorz {
        w.tm.tmfloorz = op.bottom;
    }
    if op.low_floor < w.tm.tmdropoffz {
        w.tm.tmdropoffz = op.low_floor;
    }
    // If contacted a special line, add it to the list.
    if special != 0 {
        w.tm.spechit.push(ld);
        if w.tm.spechit.len() > MAXSPECIALCROSS_ORIGINAL {
            spechit_overrun(w, ld);
        }
    }
    true
}

/// `PIT_CheckThing`.
fn pit_check_thing(w: &mut World, thing: MobjId) -> bool {
    let tm = w.tm.tmthing.unwrap();
    let (tx, ty) = (w.tm.tmx, w.tm.tmy);
    let t = w.mo(thing).clone();
    if t.flags & (MF_SOLID | MF_SPECIAL | MF_SHOOTABLE) == 0 {
        return true;
    }
    let me = w.mo(tm).clone();
    let blockdist = t.radius + me.radius;
    if (t.x - tx).abs() >= blockdist || (t.y - ty).abs() >= blockdist {
        return true; // didn't hit it
    }
    // Don't clip against self.
    if thing == tm {
        return true;
    }

    // Check for skulls slamming into things.
    if me.flags & MF_SKULLFLY != 0 {
        let damage = ((w.p_random() % 8) + 1) * w.info.mobj(me.mobj_type).damage;
        crate::inter::damage_mobj(w, thing, Some(tm), Some(tm), damage);
        let spawn = w.info.mobj(me.mobj_type).spawnstate;
        let m = w.mo_mut(tm);
        m.flags &= !MF_SKULLFLY;
        m.momx = Fixed(0);
        m.momy = Fixed(0);
        m.momz = Fixed(0);
        set_mobj_state(w, tm, spawn);
        return false; // stop moving
    }

    // Missiles can hit other things.
    if me.flags & MF_MISSILE != 0 {
        // See if it went over / under.
        if me.z > t.z + t.height {
            return true; // overhead
        }
        if me.z + me.height < t.z {
            return true; // underneath
        }
        if let Some(src) = w.try_mo(me.target).map(|s| s.mobj_type) {
            if src == t.mobj_type
                || (src == MT_KNIGHT && t.mobj_type == MT_BRUISER)
                || (src == MT_BRUISER && t.mobj_type == MT_KNIGHT)
            {
                // Don't hit same species as originator.
                if Some(thing) == me.target {
                    return true;
                }
                // Explode, but do no damage. Let players missile other
                // players. DEHACKED's `Monsters Infight` lifts this
                // (Chocolate's deh_species_infighting).
                if t.mobj_type != MT_PLAYER && !w.deh.species_infighting {
                    return false;
                }
            }
        }
        if t.flags & MF_SHOOTABLE == 0 {
            // Didn't do any damage.
            return t.flags & MF_SOLID == 0;
        }
        // Damage / explode.
        let damage = ((w.p_random() % 8) + 1) * w.info.mobj(me.mobj_type).damage;
        crate::inter::damage_mobj(w, thing, Some(tm), me.target, damage);
        // Don't traverse any more.
        return false;
    }

    // Check for special pickup.
    if t.flags & MF_SPECIAL != 0 {
        let solid = t.flags & MF_SOLID != 0;
        if w.tm.tmflags & MF_PICKUP != 0 {
            // Can remove thing.
            crate::inter::touch_special_thing(w, thing, tm);
        }
        return !solid;
    }
    t.flags & MF_SOLID == 0
}

/// `P_CheckPosition`.
///
/// This is purely informative, nothing is modified (except things picked up).
/// Sets `tmfloorz`, `tmceilingz`, `tmdropoffz`, `ceilingline` and `spechit`.
pub fn check_position(w: &mut World, id: MobjId, x: Fixed, y: Fixed) -> bool {
    begin_position_check(w, id, x, y);
    if w.tm.tmflags & MF_NOCLIP != 0 {
        return true;
    }

    // Check things first, possibly picking things up. The bounding box is
    // extended by MAXRADIUS because mobj_ts are grouped into mapblocks based
    // on their origin point, and can overlap into adjacent blocks by up to
    // MAXRADIUS units.
    let (xl, xh, yl, yh) = block_range(w, MAXRADIUS);
    for bx in xl..=xh {
        for by in yl..=yh {
            if !block_things_iterator(w, bx, by, pit_check_thing) {
                return false;
            }
        }
    }

    // Check lines.
    let (xl, xh, yl, yh) = block_range(w, Fixed(0));
    for bx in xl..=xh {
        for by in yl..=yh {
            // Collect first: `pit_check_line` needs the whole world, while
            // the iterator only stamps validcount. The iteration order and
            // stamping are identical to vanilla's because the callback's
            // early-out ends the whole check.
            let mut lines = Vec::new();
            crate::geometry::block_lines_iterator(&mut w.level, bx, by, |_, l| {
                lines.push(l);
                true
            });
            for l in lines {
                if !pit_check_line(w, l) {
                    return false;
                }
            }
        }
    }
    true
}

/// `P_TryMove`: attempt to move to a new position, crossing special lines
/// unless `MF_TELEPORT` is set.
pub fn try_move(w: &mut World, id: MobjId, x: Fixed, y: Fixed) -> bool {
    w.tm.floatok = false;
    if !check_position(w, id, x, y) {
        return false; // solid wall or thing
    }
    let m = w.mo(id);
    let (flags, z, height) = (m.flags, m.z, m.height);
    if flags & MF_NOCLIP == 0 {
        let tm = &w.tm;
        if tm.tmceilingz - tm.tmfloorz < height {
            return false; // doesn't fit
        }
        w.tm.floatok = true;
        let tm = &w.tm;
        if flags & MF_TELEPORT == 0 && tm.tmceilingz - z < height {
            return false; // mobj must lower itself to fit
        }
        if flags & MF_TELEPORT == 0 && tm.tmfloorz - z > STEP {
            return false; // too big a step up
        }
        if flags & (MF_DROPOFF | MF_FLOAT) == 0 && tm.tmfloorz - tm.tmdropoffz > STEP {
            return false; // don't stand over a dropoff
        }
    }

    // The move is ok, so link the thing into its new position.
    unset_thing_position(w, id);
    let (oldx, oldy) = (w.mo(id).x, w.mo(id).y);
    let (fz, cz) = (w.tm.tmfloorz, w.tm.tmceilingz);
    {
        let m = w.mo_mut(id);
        m.floorz = fz;
        m.ceilingz = cz;
        m.x = x;
        m.y = y;
    }
    set_thing_position(w, id);

    // If any special lines were hit, do the effect.
    if flags & (MF_TELEPORT | MF_NOCLIP) == 0 {
        // Processed from the last entry backwards (`while (numspechit--)`).
        // The list is re-read each step: a special can itself run a
        // position check and change it, exactly like the C global.
        while let Some(ld) = w.tm.spechit.pop() {
            let line = w.level.line(ld).clone();
            let (tx, ty) = (w.mo(id).x, w.mo(id).y);
            let side = point_on_line_side(&w.level, tx, ty, &line);
            let oldside = point_on_line_side(&w.level, oldx, oldy, &line);
            if side != oldside && line.special != 0 {
                crate::spec::cross_special_line(w, ld, oldside, id);
            }
        }
    }
    true
}

/// `P_ThingHeightClip`: takes a valid thing and adjusts `floorz`, `ceilingz`
/// and possibly `z`. Returns false if the thing no longer fits.
pub fn thing_height_clip(w: &mut World, id: MobjId) -> bool {
    let onfloor = w.mo(id).z == w.mo(id).floorz;
    let (x, y) = (w.mo(id).x, w.mo(id).y);
    check_position(w, id, x, y);
    // What about stranding a monster partially off an edge?
    let (fz, cz) = (w.tm.tmfloorz, w.tm.tmceilingz);
    let m = w.mo_mut(id);
    m.floorz = fz;
    m.ceilingz = cz;
    if onfloor {
        // Walking monsters rise and fall with the floor.
        m.z = m.floorz;
    } else if m.z + m.height > m.ceilingz {
        // Don't adjust a floating monster unless forced to.
        m.z = m.ceilingz - m.height;
    }
    m.ceilingz - m.floorz >= m.height
}

// ---------------------------------------------------------------------------
// Sliding
// ---------------------------------------------------------------------------

/// `P_HitSlideLine`: adjusts `tmxmove`/`tmymove` to run along the wall.
fn hit_slide_line(w: &mut World, ld: LineId) {
    let line = w.level.line(ld).clone();
    if line.slope_type == SlopeType::Horizontal {
        w.tm.tmymove = Fixed(0);
        return;
    }
    if line.slope_type == SlopeType::Vertical {
        w.tm.tmxmove = Fixed(0);
        return;
    }
    let slidemo = w.tm.slidemo.unwrap();
    let (sx, sy) = (w.mo(slidemo).x, w.mo(slidemo).y);
    let side = point_on_line_side(&w.level, sx, sy, &line);
    let mut lineangle = point_to_angle2(Fixed(0), Fixed(0), line.dx, line.dy);
    if side == 1 {
        lineangle += Angle::A180;
    }
    let moveangle = point_to_angle2(Fixed(0), Fixed(0), w.tm.tmxmove, w.tm.tmymove);
    let mut deltaangle = moveangle - lineangle;
    if deltaangle.0 > Angle::A180.0 {
        deltaangle += Angle::A180;
    }
    let (la, da) = (lineangle.fine(), deltaangle.fine());
    let movelen = aprox_distance(w.tm.tmxmove, w.tm.tmymove);
    let newlen = fixed_mul(movelen, finecosine(da));
    w.tm.tmxmove = fixed_mul(newlen, finecosine(la));
    w.tm.tmymove = fixed_mul(newlen, finesine(la));
}

/// `PTR_SlideTraverse`.
fn ptr_slide_traverse(w: &mut World, inter: &Intercept) -> bool {
    let InterceptKind::Line(ld) = inter.kind else {
        panic!("PTR_SlideTraverse: not a line?");
    };
    let line = w.level.line(ld).clone();
    let slidemo = w.tm.slidemo.unwrap();
    let (sx, sy, sz, sh) = {
        let m = w.mo(slidemo);
        (m.x, m.y, m.z, m.height)
    };
    let blocking = if line.flags & ml::TWOSIDED == 0 {
        if point_on_line_side(&w.level, sx, sy, &line) != 0 {
            return true; // don't hit the back side
        }
        true
    } else {
        // Set openrange, opentop, openbottom.
        let op = line_opening(&w.level, &line, w.tm.opening);
        w.tm.opening = op;
        // Doesn't fit / mobj is too high / too big a step up.
        op.range < sh || op.top - sz < sh || op.bottom - sz > STEP
    };
    if !blocking {
        return true; // this line doesn't block movement
    }
    // The line does block movement, see if it is closer than best so far.
    if inter.frac < w.tm.bestslidefrac {
        w.tm.secondslidefrac = w.tm.bestslidefrac;
        w.tm.secondslideline = w.tm.bestslideline;
        w.tm.bestslidefrac = inter.frac;
        w.tm.bestslideline = Some(ld);
    }
    false // stop
}

/// `P_SlideMove`: the momentum is reduced and redirected along the closest
/// blocking line. This is a kludgy mess (as the original comment says).
pub fn slide_move(w: &mut World, id: MobjId) {
    w.tm.slidemo = Some(id);
    let mut hitcount = 0;
    loop {
        // `retry:`
        hitcount += 1;
        if hitcount == 3 {
            stairstep(w, id);
            return; // don't loop forever
        }
        let m = w.mo(id).clone();
        // Trace along the three leading corners.
        let (leadx, trailx) = if m.momx.0 > 0 { (m.x + m.radius, m.x - m.radius) } else { (m.x - m.radius, m.x + m.radius) };
        let (leady, traily) = if m.momy.0 > 0 { (m.y + m.radius, m.y - m.radius) } else { (m.y - m.radius, m.y + m.radius) };

        w.tm.bestslidefrac = Fixed(FRACUNIT + 1);
        path_traverse(w, leadx, leady, leadx + m.momx, leady + m.momy, pt::ADDLINES, ptr_slide_traverse);
        path_traverse(w, trailx, leady, trailx + m.momx, leady + m.momy, pt::ADDLINES, ptr_slide_traverse);
        path_traverse(w, leadx, traily, leadx + m.momx, traily + m.momy, pt::ADDLINES, ptr_slide_traverse);

        // Move up to the wall.
        if w.tm.bestslidefrac.0 == FRACUNIT + 1 {
            // The move must have hit the middle, so stairstep.
            stairstep(w, id);
            return;
        }

        // Fudge a bit to make sure it doesn't hit.
        w.tm.bestslidefrac = Fixed(w.tm.bestslidefrac.0 - 0x800);
        if w.tm.bestslidefrac.0 > 0 {
            let (momx, momy) = (w.mo(id).momx, w.mo(id).momy);
            let newx = fixed_mul(momx, w.tm.bestslidefrac);
            let newy = fixed_mul(momy, w.tm.bestslidefrac);
            let (x, y) = (w.mo(id).x, w.mo(id).y);
            if !try_move(w, id, x + newx, y + newy) {
                stairstep(w, id);
                return;
            }
        }

        // Now continue along the wall. First calculate remainder.
        let mut frac = FRACUNIT - (w.tm.bestslidefrac.0 + 0x800);
        if frac > FRACUNIT {
            frac = FRACUNIT;
        }
        w.tm.bestslidefrac = Fixed(frac);
        if frac <= 0 {
            return;
        }
        let (momx, momy) = (w.mo(id).momx, w.mo(id).momy);
        w.tm.tmxmove = fixed_mul(momx, Fixed(frac));
        w.tm.tmymove = fixed_mul(momy, Fixed(frac));
        let best = w.tm.bestslideline.expect("slide line set when bestslidefrac < FRACUNIT+1");
        hit_slide_line(w, best); // clip the moves
        let (tx, ty) = (w.tm.tmxmove, w.tm.tmymove);
        {
            let m = w.mo_mut(id);
            m.momx = tx;
            m.momy = ty;
        }
        let (x, y) = (w.mo(id).x, w.mo(id).y);
        if try_move(w, id, x + tx, y + ty) {
            return;
        }
        // goto retry
    }
}

/// The `stairstep:` label of `P_SlideMove`.
fn stairstep(w: &mut World, id: MobjId) {
    let m = w.mo(id);
    let (x, y, momx, momy) = (m.x, m.y, m.momx, m.momy);
    if !try_move(w, id, x, y + momy) {
        try_move(w, id, x + momx, y);
    }
}

// ---------------------------------------------------------------------------
// Line attacks
// ---------------------------------------------------------------------------

/// Vanilla's 320x200 view: `topslope`/`bottomslope` limits for autoaim.
const SCREENWIDTH: i32 = 320;
const SCREENHEIGHT: i32 = 200;

/// `PTR_AimTraverse`. Sets `linetarget` and `aimslope` when a target is found.
fn ptr_aim_traverse(w: &mut World, inter: &Intercept) -> bool {
    match inter.kind {
        InterceptKind::Line(ld) => {
            let line = w.level.line(ld).clone();
            if line.flags & ml::TWOSIDED == 0 {
                return false; // stop
            }
            // Crosses a two sided line. A two sided line will restrict the
            // possible target ranges.
            let op = line_opening(&w.level, &line, w.tm.opening);
            w.tm.opening = op;
            if op.bottom >= op.top {
                return false; // stop
            }
            let dist = fixed_mul(w.tm.attackrange, inter.frac);
            let front = w.level.sector(line.front_sector.unwrap()).clone();
            let back = line.back_sector.map(|b| w.level.sector(b).clone());
            if back.as_ref().is_none_or(|b| front.floor_height != b.floor_height) {
                let slope = fixed_div(op.bottom - w.tm.shootz, dist);
                if slope > w.tm.bottomslope {
                    w.tm.bottomslope = slope;
                }
            }
            if back.as_ref().is_none_or(|b| front.ceiling_height != b.ceiling_height) {
                let slope = fixed_div(op.top - w.tm.shootz, dist);
                if slope < w.tm.topslope {
                    w.tm.topslope = slope;
                }
            }
            w.tm.topslope > w.tm.bottomslope // shot continues
        }
        InterceptKind::Thing(th) => {
            if Some(th) == w.tm.shootthing {
                return true; // can't shoot self
            }
            let t = w.mo(th);
            if t.flags & MF_SHOOTABLE == 0 {
                return true; // corpse or something
            }
            // Check angles to see if the thing can be aimed at.
            let dist = fixed_mul(w.tm.attackrange, inter.frac);
            let mut thingtopslope = fixed_div(t.z + t.height - w.tm.shootz, dist);
            if thingtopslope < w.tm.bottomslope {
                return true; // shot over the thing
            }
            let mut thingbottomslope = fixed_div(t.z - w.tm.shootz, dist);
            if thingbottomslope > w.tm.topslope {
                return true; // shot under the thing
            }
            // This thing can be hit!
            if thingtopslope > w.tm.topslope {
                thingtopslope = w.tm.topslope;
            }
            if thingbottomslope < w.tm.bottomslope {
                thingbottomslope = w.tm.bottomslope;
            }
            w.tm.aimslope = (thingtopslope + thingbottomslope) / 2;
            w.tm.linetarget = Some(th);
            false // don't go any farther
        }
    }
}

/// `P_AimLineAttack`. A `None` shooter uses Chocolate's zeroed dummy mobj.
pub fn aim_line_attack(w: &mut World, t1: Option<MobjId>, angle: Angle, distance: Fixed) -> Fixed {
    let (x, y, z, height) = match w.try_mo(t1) {
        Some(m) => (m.x, m.y, m.z, m.height),
        None => (Fixed(0), Fixed(0), Fixed(0), Fixed(0)),
    };
    let an = angle.fine();
    w.tm.shootthing = t1;
    let x2 = x + finecosine(an) * distance.to_int();
    let y2 = y + finesine(an) * distance.to_int();
    w.tm.shootz = z + (height >> 1) + Fixed(8 * FRACUNIT);
    // Can't shoot outside view angles.
    w.tm.topslope = Fixed((SCREENHEIGHT / 2) * FRACUNIT / (SCREENWIDTH / 2));
    w.tm.bottomslope = Fixed(-(SCREENHEIGHT / 2) * FRACUNIT / (SCREENWIDTH / 2));
    w.tm.attackrange = distance;
    w.tm.linetarget = None;
    path_traverse(w, x, y, x2, y2, pt::ADDLINES | pt::ADDTHINGS, ptr_aim_traverse);
    if w.tm.linetarget.is_some() {
        w.tm.aimslope
    } else {
        Fixed(0)
    }
}

/// `PTR_ShootTraverse`.
fn ptr_shoot_traverse(w: &mut World, inter: &Intercept) -> bool {
    let trace = w.tm.trace;
    match inter.kind {
        InterceptKind::Line(ld) => {
            let line = w.level.line(ld).clone();
            if line.special != 0 {
                let shooter = w.tm.shootthing;
                crate::spec::shoot_special_line(w, shooter, ld);
            }
            let hit = 'hit: {
                if line.flags & ml::TWOSIDED == 0 {
                    break 'hit true;
                }
                // Crosses a two sided line.
                let op = line_opening(&w.level, &line, w.tm.opening);
                w.tm.opening = op;
                let dist = fixed_mul(w.tm.attackrange, inter.frac);
                let front = w.level.sector(line.front_sector.unwrap()).clone();
                match line.back_sector.map(|b| w.level.sector(b).clone()) {
                    None => {
                        if fixed_div(op.bottom - w.tm.shootz, dist) > w.tm.aimslope {
                            break 'hit true;
                        }
                        if fixed_div(op.top - w.tm.shootz, dist) < w.tm.aimslope {
                            break 'hit true;
                        }
                    }
                    Some(back) => {
                        if front.floor_height != back.floor_height
                            && fixed_div(op.bottom - w.tm.shootz, dist) > w.tm.aimslope
                        {
                            break 'hit true;
                        }
                        if front.ceiling_height != back.ceiling_height
                            && fixed_div(op.top - w.tm.shootz, dist) < w.tm.aimslope
                        {
                            break 'hit true;
                        }
                    }
                }
                false
            };
            if !hit {
                return true; // shot continues
            }
            // Hit line: position a bit closer.
            let frac = inter.frac - fixed_div(Fixed(4 * FRACUNIT), w.tm.attackrange);
            let x = trace.x + fixed_mul(trace.dx, frac);
            let y = trace.y + fixed_mul(trace.dy, frac);
            let z = w.tm.shootz + fixed_mul(w.tm.aimslope, fixed_mul(frac, w.tm.attackrange));
            let front = w.level.sector(line.front_sector.unwrap());
            if front.ceiling_pic == w.sky_flat {
                // Don't shoot the sky!
                if z > front.ceiling_height {
                    return false;
                }
                // It's a sky hack wall.
                if line.back_sector.is_some_and(|b| w.level.sector(b).ceiling_pic == w.sky_flat) {
                    return false;
                }
            }
            // Spawn bullet puffs.
            match w.tm.puff_type {
                Some(t) => {
                    crate::mobj::spawn_puff_class(w, t, x, y, z, crate::mobj::PuffHit::Wall);
                }
                None => spawn_puff(w, x, y, z),
            }
            false // don't go any farther
        }
        InterceptKind::Thing(th) => {
            if Some(th) == w.tm.shootthing {
                return true; // can't shoot self
            }
            let t = w.mo(th).clone();
            if t.flags & MF_SHOOTABLE == 0 {
                return true; // corpse or something
            }
            // Check angles to see if the thing can be aimed at.
            let dist = fixed_mul(w.tm.attackrange, inter.frac);
            if fixed_div(t.z + t.height - w.tm.shootz, dist) < w.tm.aimslope {
                return true; // shot over the thing
            }
            if fixed_div(t.z - w.tm.shootz, dist) > w.tm.aimslope {
                return true; // shot under the thing
            }
            // Hit thing: position a bit closer.
            let frac = inter.frac - fixed_div(Fixed(10 * FRACUNIT), w.tm.attackrange);
            let x = trace.x + fixed_mul(trace.dx, frac);
            let y = trace.y + fixed_mul(trace.dy, frac);
            let z = w.tm.shootz + fixed_mul(w.tm.aimslope, fixed_mul(frac, w.tm.attackrange));
            // Spawn bullet puffs or blood spots, depending on target type.
            match w.tm.puff_type {
                Some(pt) => {
                    // A custom puff shows on actors that don't bleed, or
                    // with +PUFFONACTORS; bleeding actors also bleed.
                    let on_actors = w.info.has_zflag(pt, crate::info::zflags::PUFFONACTORS);
                    if on_actors || t.flags & MF_NOBLOOD != 0 {
                        let hit = if t.flags & MF_NOBLOOD != 0 { crate::mobj::PuffHit::Actor(th) } else { crate::mobj::PuffHit::Bleeder(th) };
                        crate::mobj::spawn_puff_class(w, pt, x, y, z, hit);
                    }
                    if t.flags & MF_NOBLOOD == 0 {
                        let d = w.tm.la_damage;
                        spawn_blood(w, x, y, z, d);
                    }
                }
                None if t.flags & MF_NOBLOOD != 0 => spawn_puff(w, x, y, z),
                None => {
                    let d = w.tm.la_damage;
                    spawn_blood(w, x, y, z, d);
                }
            }
            if w.tm.la_damage != 0 {
                let (s, d) = (w.tm.shootthing, w.tm.la_damage);
                crate::inter::damage_mobj(w, th, s, s, d);
            }
            false // don't go any farther
        }
    }
}

/// `P_LineAttack`. If `damage` is zero, it is just a test trace that will
/// leave `linetarget` set.
pub fn line_attack(w: &mut World, t1: MobjId, angle: Angle, distance: Fixed, slope: Fixed, damage: i32) {
    let an = angle.fine();
    let m = w.mo(t1);
    let (x, y, z, height) = (m.x, m.y, m.z, m.height);
    w.tm.shootthing = Some(t1);
    w.tm.la_damage = damage;
    let x2 = x + finecosine(an) * distance.to_int();
    let y2 = y + finesine(an) * distance.to_int();
    w.tm.shootz = z + (height >> 1) + Fixed(8 * FRACUNIT);
    w.tm.attackrange = distance;
    w.tm.aimslope = slope;
    let missed = path_traverse(w, x, y, x2, y2, pt::ADDLINES | pt::ADDTHINGS, ptr_shoot_traverse);
    // +ALWAYSPUFF puffs appear at the end of a shot that hit nothing.
    if let Some(t) = w.tm.puff_type {
        if missed && w.info.has_zflag(t, crate::info::zflags::ALWAYSPUFF) {
            let z2 = w.tm.shootz + fixed_mul(slope, distance);
            crate::mobj::spawn_puff_class(w, t, x2, y2, z2, crate::mobj::PuffHit::Nothing);
        }
    }
}

/// [`line_attack`] with a puff class (`A_FireBullets(..., "MyPuff")`);
/// `None` is the vanilla puff.
pub fn line_attack_puff(w: &mut World, t1: MobjId, angle: Angle, distance: Fixed, slope: Fixed, damage: i32, puff: Option<crate::info::MobjType>) {
    let saved = std::mem::replace(&mut w.tm.puff_type, puff);
    line_attack(w, t1, angle, distance, slope, damage);
    w.tm.puff_type = saved;
}

// ---------------------------------------------------------------------------
// Use lines
// ---------------------------------------------------------------------------

/// `P_UseLines`: looks for special lines in front of the player to activate.
pub fn use_lines(w: &mut World, player: usize) {
    let Some(mo) = w.players[player].mo else { return };
    let m = w.mo(mo);
    let an = m.angle.fine();
    let (x1, y1) = (m.x, m.y);
    let x2 = x1 + finecosine(an) * USERANGE.to_int();
    let y2 = y1 + finesine(an) * USERANGE.to_int();
    path_traverse(w, x1, y1, x2, y2, pt::ADDLINES, |w, inter| {
        // `PTR_UseTraverse`
        let InterceptKind::Line(ld) = inter.kind else { return true };
        let line = w.level.line(ld).clone();
        if line.special == 0 {
            let op = line_opening(&w.level, &line, w.tm.opening);
            w.tm.opening = op;
            if op.range.0 <= 0 {
                w.start_sound(SoundSource::Mobj(mo), SFX_NOWAY);
                return false; // can't use through a wall
            }
            return true; // not a special line, but keep checking
        }
        let (ux, uy) = (w.mo(mo).x, w.mo(mo).y);
        let side = (point_on_line_side(&w.level, ux, uy, &line) == 1) as usize;
        crate::spec::use_special_line(w, mo, ld, side);
        false // can't use for more than one special line in a row
    });
}

// ---------------------------------------------------------------------------
// Radius attack / sector change
// ---------------------------------------------------------------------------

/// `P_RadiusAttack`: source is the creature that caused the explosion at spot.
pub fn radius_attack(w: &mut World, spot: MobjId, source: Option<MobjId>, damage: i32) {
    // C: `(damage + MAXRADIUS) << FRACBITS` with MAXRADIUS already in fixed
    // point. The shift overflows and the MAXRADIUS term vanishes, leaving
    // effectively `damage << FRACBITS`. Reproduced as-is.
    let dist = Fixed(damage.wrapping_add(MAXRADIUS.0).wrapping_shl(16));
    let (sx, sy) = (w.mo(spot).x, w.mo(spot).y);
    let (ox, oy) = (w.level.blockmap.org_x, w.level.blockmap.org_y);
    let yh = (sy + dist - oy).0 >> MAPBLOCKSHIFT;
    let yl = (sy - dist - oy).0 >> MAPBLOCKSHIFT;
    let xh = (sx + dist - ox).0 >> MAPBLOCKSHIFT;
    let xl = (sx - dist - ox).0 >> MAPBLOCKSHIFT;
    for y in yl..=yh {
        for x in xl..=xh {
            block_things_iterator(w, x, y, |w, thing| {
                // `PIT_RadiusAttack`
                let t = w.mo(thing);
                if t.flags & MF_SHOOTABLE == 0 {
                    return true;
                }
                // Boss spider and cyborg take no damage from concussion.
                if t.mobj_type == MT_CYBORG || t.mobj_type == MT_SPIDER {
                    return true;
                }
                let dx = (t.x - sx).abs();
                let dy = (t.y - sy).abs();
                let d = if dx > dy { dx } else { dy };
                let d = ((d - t.radius).0 >> 16).max(0);
                if d >= damage {
                    return true; // out of range
                }
                if crate::sight::check_sight(w, thing, spot) {
                    // Must be in direct path.
                    crate::inter::damage_mobj(w, thing, Some(spot), source, damage - d);
                }
                true
            });
        }
    }
}

/// `PIT_ChangeSector`.
fn pit_change_sector(w: &mut World, thing: MobjId) -> bool {
    if thing_height_clip(w, thing) {
        return true; // keep checking
    }
    // Crunch bodies to giblets.
    if w.mo(thing).health <= 0 {
        set_mobj_state(w, thing, S_GIBS);
        let clear_solid = w.params.version > doom_content::game::GameVersion::Doom1_2;
        let m = w.mo_mut(thing);
        if clear_solid {
            m.flags &= !MF_SOLID;
        }
        m.height = Fixed(0);
        m.radius = Fixed(0);
        return true; // keep checking
    }
    // Crunch dropped items.
    if w.mo(thing).flags & MF_DROPPED != 0 {
        remove_mobj(w, thing);
        return true;
    }
    if w.mo(thing).flags & MF_SHOOTABLE == 0 {
        return true; // assume it is bloody gibs or something
    }
    w.tm.nofit = true;
    if w.tm.crushchange && w.leveltime & 3 == 0 {
        crate::inter::damage_mobj(w, thing, None, None, 10);
        // Spray blood in a random direction.
        let (x, y, z) = {
            let m = w.mo(thing);
            (m.x, m.y, m.z + m.height / 2)
        };
        let mo = spawn_mobj(w, x, y, z, MT_BLOOD);
        let mx = w.p_sub_random() << 12;
        let my = w.p_sub_random() << 12;
        let m = w.mo_mut(mo);
        m.momx = Fixed(mx);
        m.momy = Fixed(my);
    }
    true // keep checking (crush other things)
}

/// `P_ChangeSector`: returns true if any thing doesn't fit.
pub fn change_sector(w: &mut World, sector: SectorId, crunch: bool) -> bool {
    w.tm.nofit = false;
    w.tm.crushchange = crunch;
    let bb = w.level.sector(sector).blockbox;
    // Re-check heights for all things near the moving sector.
    for x in bb[BOXLEFT]..=bb[BOXRIGHT] {
        for y in bb[BOXBOTTOM]..=bb[BOXTOP] {
            block_things_iterator(w, x, y, pit_change_sector);
        }
    }
    w.tm.nofit
}
