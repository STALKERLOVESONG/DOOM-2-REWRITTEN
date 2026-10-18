//! `P_Ticker` / `P_RunThinkers` (p_tick.c) and `P_RespawnSpecials`.

use doom_core::{Angle, Fixed};

use crate::info::*;
use crate::mobj::flags::MF_SPAWNCEILING;
use crate::mobj::{floor_at, mobj_thinker, spawn_mobj, ONCEILINGZ, ONFLOORZ};
use crate::player::MAXPLAYERS;
use crate::thinker::{ThinkFn, ThinkerData};
use crate::world::{SoundSource, World, ITEMQUESIZE};

const TICRATE: i32 = 35;

/// `P_RunThinkers`.
pub fn run_thinkers(w: &mut World) {
    let mut cur = w.thinkers.first();
    while let Some(id) = cur {
        match w.thinkers.func(id) {
            Some(ThinkFn::Removed) => {
                // Time to remove it.
                cur = w.thinkers.free(id);
                continue;
            }
            Some(ThinkFn::Active) => match w.thinkers.data(id) {
                Some(ThinkerData::Mobj(_)) => mobj_thinker(w, id),
                Some(ThinkerData::Special(_)) => crate::specials::run_special(w, id),
                _ => {}
            },
            _ => {}
        }
        cur = w.thinkers.next(id);
    }
}

/// `P_RespawnSpecials`: deathmatch 2 item respawn.
pub fn respawn_specials(w: &mut World) {
    // Only respawn items in deathmatch.
    if w.params.deathmatch != 2 {
        return;
    }
    // Nothing left to respawn?
    if w.iquehead == w.iquetail {
        return;
    }
    // Wait at least 30 seconds.
    if w.leveltime - w.itemrespawntime[w.iquetail] < 30 * TICRATE {
        return;
    }
    let mthing = w.itemrespawnque[w.iquetail];
    let (x, y) = (Fixed(mthing.x), Fixed(mthing.y));
    // Spawn a teleport fog at the new spot.
    let fz = floor_at(w, x, y);
    let fog = spawn_mobj(w, x, y, fz, MT_IFOG);
    w.start_sound(SoundSource::Mobj(fog), SFX_ITMBK);
    // Find which type to spawn.
    let Some(t) = w.info.type_for_doomednum(mthing.kind as i32) else {
        // Chocolate turns vanilla's buffer overrun into an error.
        panic!("P_RespawnSpecials: no mobj type with doomednum {}", mthing.kind);
    };
    let z = if w.info.mobj(t).flags & MF_SPAWNCEILING != 0 { ONCEILINGZ } else { ONFLOORZ };
    let mo = spawn_mobj(w, x, y, z, t);
    let m = w.mo_mut(mo);
    m.spawnpoint = mthing;
    m.angle = Angle::from_mapthing_degrees(mthing.angle);
    // Pull it from the queue.
    w.iquetail = (w.iquetail + 1) & (ITEMQUESIZE - 1);
}

/// `P_Ticker`. Menu pausing is the host's business; `paused` is handled by
/// the game layer.
pub fn p_ticker(w: &mut World) {
    for i in 0..MAXPLAYERS {
        if w.playeringame[i] {
            crate::user::player_think(w, i);
        }
    }
    run_thinkers(w);
    crate::acs_host::tick(w);
    crate::spec::update_specials(w);
    respawn_specials(w);
    // For par times.
    w.leveltime += 1;
}
