//! Spawning and removal (p_mobj.c).

use doom_content::map::{mtf, MapThing};
use doom_core::fixed::fixed_mul;
use doom_core::geom::{aprox_distance, point_to_angle2};
use doom_core::tables::{finecosine, finesine};
use doom_core::{Angle, Fixed, FRACUNIT};

use super::flags::*;
use super::{Mobj, ONCEILINGZ, ONFLOORZ};
use crate::geometry::point_in_subsector;
use crate::ids::MobjId;
use crate::info::*;
use crate::maputl::{set_thing_position, unset_thing_position};
use crate::player::{PlayerState, MAXPLAYERS, VIEWHEIGHT};
use crate::thinker::{ThinkFn, ThinkerData};
use crate::world::{Skill, SoundSource, World, ITEMQUESIZE};

/// `P_SpawnMobj`.
pub fn spawn_mobj(w: &mut World, x: Fixed, y: Fixed, z: Fixed, mobj_type: MobjType) -> MobjId {
    let info = *w.info.mobj(mobj_type);
    let mut mo = Mobj {
        mobj_type,
        x,
        y,
        radius: info.radius,
        height: info.height,
        flags: info.flags,
        health: info.spawnhealth,
        ..Default::default()
    };
    if w.params.skill != Skill::Nightmare {
        mo.reactiontime = info.reactiontime;
    }
    mo.lastlook = w.p_random() % MAXPLAYERS as i32;

    // Do not set the state with P_SetMobjState, because action routines can
    // not be called yet.
    let st = *w.info.state(info.spawnstate);
    mo.state = info.spawnstate;
    mo.tics = st.tics;
    mo.sprite = st.sprite;
    mo.frame = st.frame;

    // Vanilla links the mobj before adding the thinker; the order of these
    // two steps is not observable, so we add first to get a handle.
    let id = w.thinkers.add(ThinkerData::Mobj(Box::new(mo)), ThinkFn::Active);
    set_thing_position(w, id);

    let sec = w.level.subsectors[w.mo(id).subsector.idx()].sector;
    let (floor, ceil) = (w.level.sector(sec).floor_height, w.level.sector(sec).ceiling_height);
    let m = w.mo_mut(id);
    m.floorz = floor;
    m.ceilingz = ceil;
    m.z = if z == ONFLOORZ {
        floor
    } else if z == ONCEILINGZ {
        ceil - info.height
    } else {
        z
    };
    id
}

/// `P_RemoveMobj`.
pub fn remove_mobj(w: &mut World, id: MobjId) {
    let (flags, mobj_type, spawnpoint) = {
        let m = w.mo(id);
        (m.flags, m.mobj_type, m.spawnpoint)
    };
    if flags & MF_SPECIAL != 0 && flags & MF_DROPPED == 0 && mobj_type != MT_INV && mobj_type != MT_INS {
        w.itemrespawnque[w.iquehead] = spawnpoint;
        w.itemrespawntime[w.iquehead] = w.leveltime;
        w.iquehead = (w.iquehead + 1) & (ITEMQUESIZE - 1);
        // Lose one off the end?
        if w.iquehead == w.iquetail {
            w.iquetail = (w.iquetail + 1) & (ITEMQUESIZE - 1);
        }
    }
    unset_thing_position(w, id);
    w.events.push(crate::world::SimEvent::StopSound { origin: id, slot: 0 });
    w.thinkers.remove(id);
}

/// Guard against state cycles with zero-length tics (Chocolate's
/// `MOBJ_CYCLE_LIMIT`; vanilla would hang).
const MOBJ_CYCLE_LIMIT: u32 = 1_000_000;

/// `P_SetMobjState`. Returns false if the mobj was removed (state `S_NULL`).
pub fn set_mobj_state(w: &mut World, id: MobjId, mut state: StateNum) -> bool {
    let mut cycles = 0u32;
    loop {
        if state == S_NULL {
            w.mo_mut(id).state = S_NULL;
            remove_mobj(w, id);
            return false;
        }
        let st = *w.info.state(state);
        {
            let m = w.mo_mut(id);
            m.state = state;
            m.tics = st.tics;
            m.sprite = st.sprite;
            m.frame = st.frame;
        }
        // Mod states: random durations and action calls with arguments
        // (none in vanilla, so these lookups are skipped there).
        if !w.info.state_random_tics.is_empty() {
            if let Some(&(a, b)) = w.info.state_random_tics.get(&state.0) {
                let span = (b - a).abs() + 1;
                let r = crate::hexen::script_random(w) % span;
                w.mo_mut(id).tics = a.min(b) + r;
            }
        }
        // Call action functions when the state is set.
        if st.action != Action::None {
            crate::enemy::call_mobj_action(w, id, st.action);
        }
        if !w.info.state_calls.is_empty() {
            if let Some(call) = w.info.state_calls.get(&state.0).cloned() {
                if w.thinkers.get_mobj(id).is_some() {
                    if let Some(jump) = crate::actions::run(w, id, state, &call) {
                        // A jump replaces this state at once.
                        if w.thinkers.get_mobj(id).is_none() {
                            return false;
                        }
                        state = jump;
                        cycles += 1;
                        if cycles > MOBJ_CYCLE_LIMIT {
                            panic!("P_SetMobjState: infinite state cycle detected");
                        }
                        continue;
                    }
                }
            }
        }
        state = st.next;
        cycles += 1;
        if cycles > MOBJ_CYCLE_LIMIT {
            panic!("P_SetMobjState: infinite state cycle detected");
        }
        // The action may have removed the mobj or changed its tics.
        match w.thinkers.get_mobj(id) {
            Some(m) if m.tics == 0 => continue,
            _ => return true,
        }
    }
}

/// `P_ExplodeMissile`.
pub fn explode_missile(w: &mut World, id: MobjId) {
    {
        let m = w.mo_mut(id);
        m.momx = Fixed(0);
        m.momy = Fixed(0);
        m.momz = Fixed(0);
    }
    let t = w.mo(id).mobj_type;
    let death = w.info.mobj(t).deathstate;
    set_mobj_state(w, id, death);
    let r = w.p_random() & 3;
    let m = w.mo_mut(id);
    m.tics -= r;
    if m.tics < 1 {
        m.tics = 1;
    }
    m.flags &= !MF_MISSILE;
    let ds = w.info.mobj(t).deathsound;
    w.start_sound(SoundSource::Mobj(id), ds);
}

/// `P_SpawnPlayer`.
pub fn spawn_player(w: &mut World, mthing: &MapThing) {
    if mthing.kind == 0 {
        return;
    }
    let pn = (mthing.kind - 1) as usize;
    // Not playing?
    if !w.playeringame[pn] {
        return;
    }
    if w.players[pn].playerstate == PlayerState::Reborn {
        crate::game::player_reborn(w, pn);
    }
    let (x, y) = (Fixed(mthing.x), Fixed(mthing.y));
    let id = spawn_mobj(w, x, y, ONFLOORZ, MT_PLAYER);
    let health = w.players[pn].health;
    {
        let m = w.mo_mut(id);
        m.tid = mthing.tid;
        // Set color translations for player sprites.
        if mthing.kind > 1 {
            m.flags |= ((mthing.kind - 1) as u32) << MF_TRANSSHIFT;
        }
        m.angle = Angle::from_mapthing_degrees(mthing.angle);
        m.player = Some(pn);
        m.health = health;
    }
    let deathmatch = w.params.deathmatch != 0;
    let p = &mut w.players[pn];
    p.mo = Some(id);
    p.playerstate = PlayerState::Live;
    p.refire = 0;
    p.message = None;
    p.damagecount = 0;
    p.bonuscount = 0;
    p.extralight = 0;
    p.fixedcolormap = 0;
    p.viewheight = VIEWHEIGHT;
    // Give all cards in death match mode.
    if deathmatch {
        p.cards = [true; crate::player::NUMCARDS];
    }
    // Setup gun psprite.
    crate::pspr::setup_psprites(w, pn);
}

/// `P_SpawnMapThing`. `Err` carries vanilla's fatal `I_Error` cases.
pub fn spawn_map_thing(w: &mut World, mthing: &MapThing) -> Result<(), String> {
    // Count deathmatch start positions.
    if mthing.kind == 11 {
        if w.deathmatchstarts.len() < 10 {
            w.deathmatchstarts.push(*mthing);
        }
        return Ok(());
    }
    if mthing.kind <= 0 {
        // Thing type 0 is actually "player -1 start"; vanilla ignores it.
        return Ok(());
    }
    // Check for players specially.
    if mthing.kind <= 4 {
        // Save spots for respawning in network games.
        w.playerstarts[(mthing.kind - 1) as usize] = Some(*mthing);
        if w.params.deathmatch == 0 {
            spawn_player(w, mthing);
        }
        return Ok(());
    }
    // Check for appropriate skill level.
    if !w.params.netgame && mthing.options & mtf::NOTSINGLE != 0 {
        return Ok(());
    }
    let bit: i16 = match w.params.skill {
        Skill::Baby => 1,
        Skill::Nightmare => 4,
        s => (1u32 << ((s as i32 - 1) & 0x1F)) as i16,
    };
    if mthing.options & bit == 0 {
        return Ok(());
    }
    // Find which type to spawn.
    let Some(t) = w.info.type_for_doomednum(mthing.kind as i32) else {
        return Err(format!("P_SpawnMapThing: Unknown type {} at ({}, {})", mthing.kind, mthing.x, mthing.y));
    };
    let tflags = w.info.mobj(t).flags;
    // Don't spawn keycards and players in deathmatch.
    if w.params.deathmatch != 0 && tflags & MF_NOTDMATCH != 0 {
        return Ok(());
    }
    // Don't spawn any monsters if -nomonsters.
    if w.params.nomonsters && (t == MT_SKULL || tflags & MF_COUNTKILL != 0) {
        return Ok(());
    }
    let (x, y) = (Fixed(mthing.x), Fixed(mthing.y));
    let z = if tflags & MF_SPAWNCEILING != 0 { ONCEILINGZ } else { ONFLOORZ };
    let id = spawn_mobj(w, x, y, z, t);
    {
        let m = w.mo_mut(id);
        m.spawnpoint = *mthing;
        m.tid = mthing.tid;
        m.special = mthing.special;
        m.args = mthing.args;
    }
    // UDMF `height`: above the floor, or below the ceiling for hanging things.
    if mthing.z != 0 {
        let m = w.mo_mut(id);
        if tflags & MF_SPAWNCEILING != 0 {
            m.z -= Fixed(mthing.z);
        } else {
            m.z += Fixed(mthing.z);
        }
    }
    let tics = w.mo(id).tics;
    if tics > 0 {
        let r = w.p_random();
        w.mo_mut(id).tics = 1 + (r % tics);
    }
    let flags = w.mo(id).flags;
    if flags & MF_COUNTKILL != 0 {
        w.totalkills += 1;
    }
    if flags & MF_COUNTITEM != 0 {
        w.totalitems += 1;
    }
    let m = w.mo_mut(id);
    m.angle = Angle::from_mapthing_degrees(mthing.angle);
    if mthing.options & mtf::AMBUSH != 0 {
        m.flags |= MF_AMBUSH;
    }
    Ok(())
}

/// `P_SpawnPuff`.
pub fn spawn_puff(w: &mut World, x: Fixed, y: Fixed, mut z: Fixed) {
    z += Fixed(w.p_sub_random() << 10);
    let th = spawn_mobj(w, x, y, z, MT_PUFF);
    w.mo_mut(th).momz = Fixed::ONE;
    let r = w.p_random() & 3;
    let m = w.mo_mut(th);
    m.tics -= r;
    if m.tics < 1 {
        m.tics = 1;
    }
    // Don't make punches spark on the wall.
    if w.tm.attackrange == crate::map::MELEERANGE {
        set_mobj_state(w, th, S_PUFF3);
    }
}

/// What a custom puff hit (picks its state, as in ZDoom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PuffHit {
    /// A wall, floor or ceiling: `Crash` if the class has it.
    Wall,
    /// An actor that doesn't bleed: `Spawn`.
    Actor(MobjId),
    /// A bleeding actor (with +PUFFONACTORS): `XDeath` if it has it.
    Bleeder(MobjId),
    /// The end of a missed shot (+ALWAYSPUFF).
    Nothing,
}

/// `P_SpawnPuff` for a puff class from a mod: its states run like any
/// actor's, its `VSpeed` and puff flags apply. Randomness uses the script
/// RNG (mod attacks only), so vanilla demos are unaffected.
pub fn spawn_puff_class(w: &mut World, t: MobjType, x: Fixed, y: Fixed, mut z: Fixed, hit: PuffHit) -> MobjId {
    use crate::info::zflags;
    let r = crate::hexen::script_random(w) - crate::hexen::script_random(w);
    z += Fixed(r << 10);
    let th = spawn_mobj(w, x, y, z, t);
    let vspeed = w.info.class_vspeed[t.idx()].unwrap_or(0.0);
    let shooter = w.tm.shootthing;
    {
        let m = w.mo_mut(th);
        m.momz = Fixed((vspeed * FRACUNIT as f32) as i32);
    }
    if w.info.has_zflag(t, zflags::PUFFGETSOWNER) {
        w.mo_mut(th).target = shooter;
    }
    if let (true, PuffHit::Actor(a) | PuffHit::Bleeder(a)) = (w.info.has_zflag(t, zflags::HITTRACER), hit) {
        w.mo_mut(th).tracer = Some(a);
    }
    // The state for what was hit; melee attacks use `Melee`.
    let label = match hit {
        _ if w.tm.attackrange == crate::map::MELEERANGE => Some("melee"),
        PuffHit::Wall => Some("crash"),
        PuffHit::Bleeder(_) => Some("xdeath"),
        PuffHit::Actor(_) | PuffHit::Nothing => None,
    };
    if let Some(s) = label.and_then(|l| w.info.find_label(t, l)) {
        set_mobj_state(w, th, s);
    }
    if w.thinkers.get_mobj(th).is_some() && w.info.has_zflag(t, zflags::RANDOMIZE) {
        let r = crate::hexen::script_random(w) & 3;
        let m = w.mo_mut(th);
        if m.tics > 0 {
            m.tics = (m.tics - r).max(1);
        }
    }
    th
}

/// `P_SpawnBlood`.
pub fn spawn_blood(w: &mut World, x: Fixed, y: Fixed, mut z: Fixed, damage: i32) {
    z += Fixed(w.p_sub_random() << 10);
    let th = spawn_mobj(w, x, y, z, MT_BLOOD);
    w.mo_mut(th).momz = Fixed(FRACUNIT * 2);
    let r = w.p_random() & 3;
    let m = w.mo_mut(th);
    m.tics -= r;
    if m.tics < 1 {
        m.tics = 1;
    }
    if (9..=12).contains(&damage) {
        set_mobj_state(w, th, S_BLOOD2);
    } else if damage < 9 {
        set_mobj_state(w, th, S_BLOOD3);
    }
}

/// `P_CheckMissileSpawn`.
pub fn check_missile_spawn(w: &mut World, th: MobjId) {
    let r = w.p_random() & 3;
    let (x, y) = {
        let m = w.mo_mut(th);
        m.tics -= r;
        if m.tics < 1 {
            m.tics = 1;
        }
        // Move a little forward so an angle can be computed if it
        // immediately explodes.
        m.x += m.momx >> 1;
        m.y += m.momy >> 1;
        m.z += m.momz >> 1;
        (m.x, m.y)
    };
    if !crate::map::try_move(w, th, x, y) {
        explode_missile(w, th);
    }
}

/// `P_SpawnMissile`. A missing `dest` is replaced by a zeroed dummy as in
/// Chocolate Doom's `P_SubstNullMobj` (vanilla reads memory at NULL).
pub fn spawn_missile(w: &mut World, source: MobjId, dest: Option<MobjId>, mobj_type: MobjType) -> MobjId {
    let (sx, sy, sz) = {
        let s = w.mo(source);
        (s.x, s.y, s.z)
    };
    let (dx, dy, dz, dflags) = match w.try_mo(dest) {
        Some(d) => (d.x, d.y, d.z, d.flags),
        None => (Fixed(0), Fixed(0), Fixed(0), 0),
    };
    let th = spawn_mobj(w, sx, sy, sz + Fixed(4 * 8 * FRACUNIT), mobj_type);
    let info = *w.info.mobj(mobj_type);
    w.start_sound(SoundSource::Mobj(th), info.seesound);
    w.mo_mut(th).target = Some(source); // where it came from
    let mut an = point_to_angle2(sx, sy, dx, dy);
    // Fuzzy player.
    if dflags & MF_SHADOW != 0 {
        an += Angle((w.p_sub_random() << 20) as u32);
    }
    let speed = Fixed(info.speed);
    let dist = (aprox_distance(dx - sx, dy - sy).0 / info.speed).max(1);
    let m = w.mo_mut(th);
    m.angle = an;
    m.momx = fixed_mul(speed, finecosine(an.fine()));
    m.momy = fixed_mul(speed, finesine(an.fine()));
    m.momz = Fixed((dz - sz).0 / dist);
    check_missile_spawn(w, th);
    th
}

/// `P_SpawnPlayerMissile`.
pub fn spawn_player_missile(w: &mut World, source: MobjId, mobj_type: MobjType) {
    // See which target is to be aimed at.
    let sangle = w.mo(source).angle;
    let range = Fixed(16 * 64 * FRACUNIT);
    let mut an = sangle;
    let mut slope = crate::map::aim_line_attack(w, Some(source), an, range);
    if w.tm.linetarget.is_none() {
        an += Angle(1 << 26);
        slope = crate::map::aim_line_attack(w, Some(source), an, range);
        if w.tm.linetarget.is_none() {
            an -= Angle(2 << 26);
            slope = crate::map::aim_line_attack(w, Some(source), an, range);
        }
        if w.tm.linetarget.is_none() {
            an = sangle;
            slope = Fixed(0);
        }
    }
    let (x, y, z) = {
        let s = w.mo(source);
        (s.x, s.y, s.z + Fixed(4 * 8 * FRACUNIT))
    };
    let th = spawn_mobj(w, x, y, z, mobj_type);
    let info = *w.info.mobj(mobj_type);
    w.start_sound(SoundSource::Mobj(th), info.seesound);
    let speed = Fixed(info.speed);
    let m = w.mo_mut(th);
    m.target = Some(source);
    m.angle = an;
    m.momx = fixed_mul(speed, finecosine(an.fine()));
    m.momy = fixed_mul(speed, finesine(an.fine()));
    m.momz = fixed_mul(speed, slope);
    check_missile_spawn(w, th);
}

/// Spawn subsector lookup for teleport fog etc.
pub fn floor_at(w: &World, x: Fixed, y: Fixed) -> Fixed {
    let ss = point_in_subsector(&w.level, x, y);
    w.level.sector(w.level.subsectors[ss.idx()].sector).floor_height
}
