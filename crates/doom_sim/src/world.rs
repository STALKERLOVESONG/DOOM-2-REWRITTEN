//! The simulation state of one running level ("doomstat" for the playsim).
//!
//! Vanilla keeps this in C globals; here it is one struct the host owns and
//! advances. A number of C globals are *semantic* — values left behind by one
//! routine and read by a later one (`ceilingline` from `P_CheckPosition`,
//! `linetarget` from `P_AimLineAttack`, `onground` from `P_MovePlayer`) — so
//! they are kept as explicit fields with their original names rather than
//! being turned into return values that would change behaviour.

use std::collections::BTreeMap;

use doom_content::game::{GameMode, GameVersion};
use doom_core::{BBox, DoomRandom, Fixed};

use crate::geometry::{Divline, Opening};
use crate::ids::*;
use crate::info::{Info, SfxNum};
use crate::level::Level;
use crate::mobj::Mobj;
use crate::player::{Player, MAXPLAYERS};
use crate::thinker::Thinkers;

/// `skill_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Skill {
    Baby = 0,
    Easy = 1,
    #[default]
    Medium = 2,
    Hard = 3,
    Nightmare = 4,
}

/// Session-wide game settings that gameplay code consults.
#[derive(Clone, Copy, Debug)]
pub struct GameParams {
    pub mode: GameMode,
    pub version: GameVersion,
    pub skill: Skill,
    pub episode: i32,
    pub map: i32,
    /// 0 = cooperative/single, 1 = deathmatch, 2 = altdeath.
    pub deathmatch: i32,
    pub netgame: bool,
    pub respawnmonsters: bool,
    pub nomonsters: bool,
    pub fastparm: bool,
    pub consoleplayer: usize,
}

/// Something the presentation layer should react to. The playsim never
/// depends on how (or whether) these are handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimEvent {
    /// `S_StartSound(origin, sfx)`. Vanilla sounds use slot 0 (one sound
    /// per source); mod actions name a ZDoom channel (`CHAN_WEAPON` = 1 ...
    /// `CHAN_7`), so a source can play one sound per slot at once.
    StartSound { origin: SoundSource, sfx: SfxNum, slot: u8, volume: u8, atten: SoundAtten, looping: bool },
    /// `S_StopSound(origin)` (slot 0: every slot).
    StopSound { origin: MobjId, slot: u8 },
}

/// A sound's distance attenuation, in 1/32 (32 = Doom's; 0 = heard
/// everywhere at full volume).
pub type SoundAtten = u8;
/// Doom's attenuation.
pub const ATTEN_NORMAL: SoundAtten = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundSource {
    /// Not positioned (menu/HUD sounds, `S_StartSound(NULL, ...)`).
    None,
    Mobj(MobjId),
    /// A sector's `soundorg`.
    Sector(SectorId),
}

/// p_map.c / p_maputl.c globals.
#[derive(Clone, Debug, Default)]
pub struct MapGlobals {
    pub tmbbox: BBox,
    pub tmthing: Option<MobjId>,
    pub tmflags: u32,
    pub tmx: Fixed,
    pub tmy: Fixed,
    /// If true, move would be ok if within `tmfloorz - tmceilingz`.
    pub floatok: bool,
    pub tmfloorz: Fixed,
    pub tmceilingz: Fixed,
    pub tmdropoffz: Fixed,
    /// Keep track of the line that lowers the ceiling, so missiles don't
    /// explode against sky hack walls.
    pub ceilingline: Option<LineId>,
    /// Special lines crossed during the last position check.
    pub spechit: Vec<LineId>,
    /// `crushchange` / `nofit` (P_ChangeSector).
    pub crushchange: bool,
    pub nofit: bool,
    pub opening: Opening,
    pub trace: Divline,
    pub earlyout: bool,
    // P_SlideMove
    pub bestslidefrac: Fixed,
    pub secondslidefrac: Fixed,
    pub bestslideline: Option<LineId>,
    pub secondslideline: Option<LineId>,
    pub slidemo: Option<MobjId>,
    pub tmxmove: Fixed,
    pub tmymove: Fixed,
    // Attacks (P_AimLineAttack / P_LineAttack)
    pub linetarget: Option<MobjId>,
    pub shootthing: Option<MobjId>,
    pub shootz: Fixed,
    pub la_damage: i32,
    /// Puff class of the hitscan attack in progress (`None`: vanilla's
    /// `MT_PUFF`, spawned exactly as `P_SpawnPuff`).
    pub puff_type: Option<crate::info::MobjType>,
    pub attackrange: Fixed,
    pub aimslope: Fixed,
    pub topslope: Fixed,
    pub bottomslope: Fixed,
}

/// Original `MAXSPECIALCROSS` before which `spechit` overflows memory.
pub const MAXSPECIALCROSS_ORIGINAL: usize = 8;
pub const ITEMQUESIZE: usize = 128;
/// Original `MAXINTERCEPTS`.
pub const MAXINTERCEPTS_ORIGINAL: usize = 128;

/// Something a script asked the presentation layer to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptEvent {
    /// `Print`/`HudMessage` text for a player (`None`: everyone).
    Print { player: Option<usize>, text: String },
    /// `SetMusic`: change the music (lump name; "" stops it).
    Music { name: String },
    /// `SetMusicVolume` (0.0 ..= 1.0).
    MusicVolume(f32),
}

/// Where a scripted exit goes (`Teleport_NewMap`, `ChangeLevel`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitTarget {
    /// A level number (MAPINFO `levelnum`, or the map number).
    Number(i32),
    /// A map lump name.
    Name(String),
    /// `Teleport_EndGame`.
    EndGame,
}

/// Level exit requested by a special (`G_ExitLevel`/`G_SecretExitLevel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameExit {
    Normal,
    Secret,
}

pub struct World {
    pub info: Info,
    /// Content-derived numbering (textures, flats, heights).
    pub content: crate::content::ContentInfo,
    /// Animation and switch tables built from `content` (`P_Init`).
    pub spec: crate::spec::SpecTables,
    /// `texturetranslation[]` / `flattranslation[]` (animated surfaces); read
    /// by the renderer.
    pub texture_translation: Vec<u16>,
    pub flat_translation: Vec<u16>,
    pub buttons: [crate::spec::Button; crate::spec::MAXBUTTONS],
    pub activeceilings: [Option<ThinkerId>; crate::specials::ceilings::MAXCEILINGS],
    pub activeplats: [Option<ThinkerId>; crate::specials::plats::MAXPLATS],
    /// Scrolling walls (`linespeciallist`).
    pub line_specials: Vec<LineId>,
    /// `-timer` deathmatch limit in minutes (0 = none).
    pub timelimit: i32,
    pub level_timer: bool,
    pub level_time_count: i32,
    /// Pending level exit, consumed by the next `tick`.
    pub gameexit: Option<GameExit>,
    /// `skytexture` (renderer).
    pub sky_texture: TexNum,
    /// `weaponinfo[]` (DEHACKED-modifiable).
    pub weaponinfo: [crate::items::WeaponInfo; 9],
    /// DEHACKED misc values.
    pub deh: crate::items::DehMisc,
    /// `gametic`: tics since the game (not level) started; `A_Tracer` keys
    /// off it.
    pub gametic: i32,
    /// p_pspr.c `bulletslope`.
    pub bulletslope: Fixed,
    /// p_enemy.c globals.
    pub corpsehit: Option<MobjId>,
    pub braintargets: Vec<MobjId>,
    pub braintargeton: usize,
    /// `A_BrainSpit`'s `static int easy`.
    pub brain_easy: bool,
    pub params: GameParams,
    pub level: Level,
    pub thinkers: Thinkers,
    pub rng: DoomRandom,
    pub players: [Player; MAXPLAYERS],
    pub playeringame: [bool; MAXPLAYERS],
    /// `playerstarts[]` / `deathmatchstarts[]`.
    pub playerstarts: [Option<doom_content::map::MapThing>; MAXPLAYERS],
    pub deathmatchstarts: Vec<doom_content::map::MapThing>,
    pub leveltime: i32,
    pub sky_flat: FlatNum,
    pub totalkills: i32,
    pub totalitems: i32,
    pub totalsecret: i32,
    /// p_user.c `onground`.
    pub onground: bool,
    pub tm: MapGlobals,
    /// Deathmatch item respawn queue (`itemrespawnque` etc., `ITEMQUESIZE` = 128).
    pub itemrespawnque: [doom_content::map::MapThing; ITEMQUESIZE],
    pub itemrespawntime: [i32; ITEMQUESIZE],
    pub iquehead: usize,
    pub iquetail: usize,
    pub events: Vec<SimEvent>,
    /// Behaviour the simulation reached but does not implement yet, with hit
    /// counts. Surfaced to the host so gaps are visible, never silent.
    pub unimplemented: BTreeMap<&'static str, u64>,
    /// Vanilla-overflow conditions encountered whose memory-corruption side
    /// effects are not emulated.
    pub overflows: BTreeMap<&'static str, u64>,
    /// Problems met while setting up the current level that vanilla would
    /// treat as fatal but we skip (unknown thing types in mods, ...).
    pub load_warnings: Vec<String>,
    /// The map's ACS scripts (`None` when the level has none).
    pub acs: Option<Box<doom_script::acs::AcsRuntime>>,
    /// ACS world/global variables, kept across levels.
    pub acs_globals: doom_script::acs::AcsGlobals,
    /// Script starts asked for by lines and things while scripts ran.
    pub acs_requests: Vec<doom_script::acs::ScriptRequest>,
    /// Messages and music changes from scripts for the presentation layer.
    pub script_events: Vec<ScriptEvent>,
    /// Destination of a scripted level exit (else the normal next map).
    pub exit_target: Option<ExitTarget>,
    /// RNG for scripts and Hexen specials (separate from `P_Random`, so
    /// mod scripts never disturb vanilla demo sync).
    pub script_rng: u32,
    /// Skateboard settings and the level's grind graph ([`crate::skate`]).
    pub skate: crate::skate::SkateWorld,
}

impl World {
    #[inline]
    pub fn mo(&self, id: MobjId) -> &Mobj {
        self.thinkers.get_mobj(id).expect("dangling mobj handle")
    }

    #[inline]
    pub fn mo_mut(&mut self, id: MobjId) -> &mut Mobj {
        self.thinkers.get_mobj_mut(id).expect("dangling mobj handle")
    }

    /// Resolve an optional reference (`mo->target` etc.). Removed-but-not-yet
    /// freed mobjs still resolve, as in vanilla.
    #[inline]
    pub fn try_mo(&self, id: Option<MobjId>) -> Option<&Mobj> {
        id.and_then(|i| self.thinkers.get_mobj(i))
    }

    pub fn start_sound(&mut self, origin: SoundSource, sfx: SfxNum) {
        if sfx.0 != 0 {
            self.events.push(SimEvent::StartSound { origin, sfx, slot: 0, volume: 255, atten: ATTEN_NORMAL, looping: false });
        }
    }

    /// A mod's `A_StartSound`: a channel slot (1..=7), volume (0..=1),
    /// attenuation (1 = Doom's, 0 = everywhere) and looping.
    pub fn start_sound_ex(&mut self, origin: SoundSource, sfx: SfxNum, slot: u8, volume: f32, atten: f32, looping: bool) {
        if sfx.0 != 0 {
            let volume = (volume.clamp(0.0, 1.0) * 255.0).round() as u8;
            let atten = (atten.clamp(0.0, 7.9) * ATTEN_NORMAL as f32).round() as u8;
            self.events.push(SimEvent::StartSound { origin, sfx, slot: slot & 7, volume, atten, looping });
        }
    }

    pub fn note_unimplemented(&mut self, what: &'static str) {
        *self.unimplemented.entry(what).or_default() += 1;
    }

    pub fn note_overflow(&mut self, what: &'static str) {
        *self.overflows.entry(what).or_default() += 1;
    }

    /// `P_Random`.
    #[inline]
    pub fn p_random(&mut self) -> i32 {
        self.rng.p_random()
    }

    /// `P_SubRandom`.
    #[inline]
    pub fn p_sub_random(&mut self) -> i32 {
        self.rng.p_sub_random()
    }
}
