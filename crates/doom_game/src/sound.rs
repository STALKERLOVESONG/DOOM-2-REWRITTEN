//! Sound and music logic (s_sound.c): channel allocation by priority,
//! distance attenuation and stereo separation relative to the listener,
//! per-tic parameter updates, ambient sounds, and music selection.
//! Produces [`AudioCommand`]s for whatever mixer the host runs.
//!
//! The path from a gameplay event to the speaker:
//!
//! 1. an actor, weapon or script asks for a sound by name; the name became
//!    a [`SfxNum`] when definitions loaded (`doom_sim::sounds`);
//! 2. the playsim emits [`SimEvent::StartSound`] with the number and the
//!    source;
//! 3. here the number resolves (`$random`, Doom's linked sounds) to a
//!    resource: the one SNDINFO named, resolved at load, or Doom's `DS`
//!    lump;
//! 4. the resource decodes once, whatever its format (DMX, WAV, Ogg, FLAC,
//!    MP3: `doom_audio::decode`), and is cached;
//! 5. the sound is placed relative to the listener (distance, stereo
//!    separation) and sent to the mixer as [`AudioCommand::StartSound`].
//!
//! Nothing here knows a file format, and nothing in the playsim knows how
//! audio plays.

use std::collections::HashMap;
use std::sync::Arc;

use doom_audio::{AudioCommand, Genmidi, Song, Sound};
use doom_content::game::GameMode;
use doom_content::{LumpId, ResourceStack};
use doom_core::fixed::fixed_mul;
use doom_core::geom::point_to_angle;
use doom_core::tables::finesine;
use doom_core::{Fixed, FRACBITS, FRACUNIT};
use doom_sim::info::{Info, MusicNum, SfxNum, MUSIC_NAMES, SFX_ITEMUP, SFX_SAWHIT, SFX_SAWUP, SFX_TINK};
use doom_sim::sounds::{AmbientMode, AMBIENT_CUSTOM_DOOMEDNUM, AMBIENT_DOOMEDNUM};
use doom_sim::world::{SimEvent, SoundSource};
use doom_sim::{MobjId, World};

const S_CLIPPING_DIST: i32 = 1200 * FRACUNIT;
const S_CLOSE_DIST: i32 = 160 * FRACUNIT;
const S_ATTENUATOR: i32 = (S_CLIPPING_DIST - S_CLOSE_DIST) >> FRACBITS;
const S_STEREO_SWING: i32 = 96 * FRACUNIT;
const NORM_SEP: i32 = 128;
/// Sound channels. Doom had 8; mods layer many more sounds (ZDoom
/// defaults to 128). The host's mixer must have this many.
pub const NUM_CHANNELS: usize = 32;

#[derive(Clone, Copy, Debug)]
struct Channel {
    /// The sound as requested (limits count these).
    sfx: SfxNum,
    /// The source's slot (0: Doom's one-sound-per-source; 1..=7: ZDoom
    /// channels, one sound per source and slot).
    slot: u8,
    priority: i32,
    origin: Origin,
    /// Game tic at which the sample finishes playing (`u64::MAX`: loops).
    ends_at: u64,
    /// Volume scale from the definition (`$volume`, ambient volume).
    volume: f32,
    /// Distance scale (`$attenuation`; 0 = heard everywhere).
    attenuation: f32,
}

/// Where a sound comes from. Mobj origins remember their last position so
/// a sound keeps its place after the mobj is removed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    None,
    Mobj(MobjId, Fixed, Fixed),
    Point(Fixed, Fixed),
}

impl Origin {
    fn same_source(&self, other: &Origin) -> bool {
        match (self, other) {
            (Origin::Mobj(a, ..), Origin::Mobj(b, ..)) => a == b,
            (Origin::Point(ax, ay), Origin::Point(bx, by)) => ax == bx && ay == by,
            _ => false,
        }
    }
}

/// How a sound is asked to play.
#[derive(Clone, Copy, Debug)]
struct Play {
    /// 0..=1 on top of the definition's volume.
    volume: f32,
    /// Distance scale (1 = Doom's).
    atten: f32,
    looping: bool,
    /// The source's channel slot (0 = Doom's single slot).
    slot: u8,
    /// The listener's own sound: full volume, centred.
    local: bool,
}

impl Default for Play {
    fn default() -> Self {
        Play { volume: 1.0, atten: 1.0, looping: false, slot: 0, local: false }
    }
}

/// A map thing playing an `$ambient` sound.
#[derive(Clone, Copy, Debug)]
struct Ambient {
    mobj: MobjId,
    def: u32,
    /// Tic of the next start (random/periodic).
    next: u64,
}

pub struct SoundSystem {
    channels: [Option<Channel>; NUM_CHANNELS],
    /// Decoded sounds by resource (shared by every sound naming it).
    decoded: HashMap<LumpId, Option<Arc<Sound>>>,
    /// Resource of each sound number, once resolved.
    resources: HashMap<u16, Option<LumpId>>,
    songs: HashMap<String, Option<Arc<Song>>>,
    pub genmidi: Option<Arc<Genmidi>>,
    /// snd_SfxVolume, 0..=127.
    sfx_volume: i32,
    /// Music volume in menu units (0..=15) and a script's scale on it
    /// (`SetMusicVolume`).
    music_volume: i32,
    music_scale: f32,
    current_music: Option<String>,
    out: Vec<AudioCommand>,
    tic: u64,
    ambients: Vec<Ambient>,
    /// For `$random` and ambient timing: presentation only, so `M_Random`
    /// (used by vanilla's sound code, part of demo sync) is untouched.
    rng: u32,
    /// Sounds that could not be decoded, reported once each.
    warnings: Vec<String>,
}

impl SoundSystem {
    pub fn new(stack: &ResourceStack) -> Self {
        let genmidi = stack.lump("GENMIDI").and_then(|l| Genmidi::parse(l).ok()).map(Arc::new);
        SoundSystem {
            channels: [None; NUM_CHANNELS],
            decoded: HashMap::new(),
            resources: HashMap::new(),
            songs: HashMap::new(),
            genmidi,
            sfx_volume: 64,
            music_volume: 8,
            music_scale: 1.0,
            current_music: None,
            out: Vec::new(),
            tic: 0,
            ambients: Vec::new(),
            rng: 0x9e37_79b9,
            warnings: Vec::new(),
        }
    }

    /// Mixer commands produced since the last call.
    pub fn take_commands(&mut self) -> Vec<AudioCommand> {
        std::mem::take(&mut self.out)
    }

    /// Problems found decoding sounds since the last call.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Volumes in menu units (0..=15).
    pub fn set_volumes(&mut self, sfx: i32, music: i32) {
        self.sfx_volume = sfx.clamp(0, 15) * 8;
        self.music_volume = music.clamp(0, 15);
        self.push_music_volume();
    }

    /// Scale the music volume (scripts' `SetMusicVolume`, 0.0..=1.0).
    pub fn set_music_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.0, 1.0);
        if scale != self.music_scale {
            self.music_scale = scale;
            self.push_music_volume();
        }
    }

    fn push_music_volume(&mut self) {
        let v = (self.music_volume * 8) as f32 * self.music_scale;
        self.out.push(AudioCommand::SetMusicVolume(v.round() as u32));
    }

    fn random(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// The sound actually played for `sfx`: `$random` picks a member,
    /// Doom's linked sounds (chgun -> pistol) play the linked one.
    fn pick(&mut self, info: &Info, sfx: SfxNum) -> SfxNum {
        let mut s = sfx;
        for _ in 0..8 {
            match info.sound_defs.get(&s.0) {
                Some(d) if !d.random.is_empty() => {
                    let i = self.random() as usize % d.random.len();
                    s = d.random[i];
                }
                _ => break,
            }
        }
        if info.sound_defs.get(&s.0).is_none_or(|d| d.lump.is_none()) {
            if let Some(l) = info.sfx.get(s.0 as usize).and_then(|x| x.link) {
                return l;
            }
        }
        s
    }

    /// The resource `sfx` plays: its definition's, else Doom's `DS` lump.
    /// Resolved once per sound.
    fn resource(&mut self, stack: &ResourceStack, info: &Info, sfx: SfxNum) -> Option<LumpId> {
        *self.resources.entry(sfx.0).or_insert_with(|| match info.sound_defs.get(&sfx.0) {
            Some(d) if d.lump.is_some() => d.lump,
            // Defined but missing: silent (the load warned).
            Some(d) if !d.resource.is_empty() => None,
            _ => {
                let name = info.sfx.get(sfx.0 as usize)?.name;
                stack.find_sound(&format!("DS{}", name.to_ascii_uppercase()))
            }
        })
    }

    /// Decode (once) and return the sound for a resource.
    fn decode(&mut self, stack: &ResourceStack, lump: LumpId) -> Option<Arc<Sound>> {
        if let Some(s) = self.decoded.get(&lump) {
            return s.clone();
        }
        let s = match doom_audio::decode(stack.data(lump)) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                let info = stack.info(lump);
                let name = info.path.as_deref().map(str::to_string).unwrap_or_else(|| info.name.to_string());
                self.warnings.push(format!("sound {name}: {e}"));
                None
            }
        };
        self.decoded.insert(lump, s.clone());
        s
    }

    fn sound(&mut self, stack: &ResourceStack, info: &Info, sfx: SfxNum) -> Option<Arc<Sound>> {
        let lump = self.resource(stack, info, sfx)?;
        self.decode(stack, lump)
    }

    /// Decode these sounds now (in parallel), so they don't decode when
    /// they first play. Called at level start with the sounds the level's
    /// actors use.
    pub fn prefetch(&mut self, stack: &ResourceStack, info: &Info, sounds: impl IntoIterator<Item = SfxNum>) {
        let mut lumps: Vec<LumpId> = Vec::new();
        for s in sounds {
            if s.0 == 0 {
                continue;
            }
            let members: Vec<SfxNum> = match info.sound_defs.get(&s.0) {
                Some(d) if !d.random.is_empty() => d.random.clone(),
                _ => vec![s],
            };
            for m in members {
                if let Some(l) = self.resource(stack, info, m) {
                    if !self.decoded.contains_key(&l) && !lumps.contains(&l) {
                        lumps.push(l);
                    }
                }
            }
        }
        if lumps.is_empty() {
            return;
        }
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        let chunk = lumps.len().div_ceil(threads);
        let results: Vec<(LumpId, Result<Sound, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = lumps
                .chunks(chunk.max(1))
                .map(|part| scope.spawn(move || part.iter().map(|&l| (l, doom_audio::decode(stack.data(l)))).collect::<Vec<_>>()))
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        for (l, r) in results {
            let s = match r {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    let info = stack.info(l);
                    let name = info.path.as_deref().map(str::to_string).unwrap_or_else(|| info.name.to_string());
                    self.warnings.push(format!("sound {name}: {e}"));
                    None
                }
            };
            self.decoded.insert(l, s);
        }
    }

    /// Sounds decoded so far (for diagnostics and tests).
    pub fn decoded_count(&self) -> usize {
        self.decoded.values().filter(|s| s.is_some()).count()
    }

    fn stop_channel(&mut self, c: usize) {
        if self.channels[c].take().is_some() {
            self.out.push(AudioCommand::StopSound { channel: c });
        }
    }

    /// `S_Start`: stop all sounds at level start.
    pub fn stop_all(&mut self) {
        for c in 0..NUM_CHANNELS {
            self.stop_channel(c);
        }
        self.ambients.clear();
    }

    /// Set up a level's sounds: its ambient sound things, and decode the
    /// sounds its actors use.
    pub fn start_level(&mut self, stack: &ResourceStack, w: &World) {
        self.ambients.clear();
        let mut wanted: Vec<SfxNum> = Vec::new();
        let mut seen_types = std::collections::HashSet::new();
        for (id, m) in w.thinkers.mobjs() {
            let kind = m.spawnpoint.kind as i32;
            let def = if (AMBIENT_DOOMEDNUM + 1..AMBIENT_DOOMEDNUM + 65).contains(&kind) {
                Some((kind - AMBIENT_DOOMEDNUM) as u32)
            } else if kind == AMBIENT_CUSTOM_DOOMEDNUM {
                Some(m.args[0] as u32)
            } else {
                None
            };
            if let Some(d) = def.filter(|d| w.info.ambient_sounds.contains_key(d)) {
                self.ambients.push(Ambient { mobj: id, def: d, next: self.tic });
                wanted.push(w.info.ambient_sounds[&d].sound);
            }
            if seen_types.insert(m.mobj_type) {
                let i = w.info.mobj(m.mobj_type);
                wanted.extend([i.seesound, i.attacksound, i.painsound, i.deathsound, i.activesound]);
            }
        }
        self.prefetch(stack, &w.info, wanted);
    }

    /// `S_AdjustSoundParams`: volume and separation for a source, or None
    /// if out of earshot. `boss_map` disables clipping (vanilla map 8).
    /// `atten` scales the distance (1 = Doom's; 0 = full volume anywhere).
    fn adjust(&self, listener: (Fixed, Fixed, u32), x: Fixed, y: Fixed, boss_map: bool, atten: f32) -> Option<(i32, i32)> {
        let adx = (listener.0 - x).0.abs();
        let ady = (listener.1 - y).0.abs();
        // From _GG1_ p.428. Approx. Euclidean distance fast.
        let mut approx_dist = adx + ady - (adx.min(ady) >> 1);
        if atten != 1.0 {
            approx_dist = (approx_dist as f64 * atten as f64).min(i32::MAX as f64) as i32;
        }
        if !boss_map && approx_dist > S_CLIPPING_DIST {
            return None;
        }
        let mut angle = point_to_angle(x - listener.0, y - listener.1).0;
        if angle > listener.2 {
            angle = angle.wrapping_sub(listener.2);
        } else {
            angle = angle.wrapping_add(0xffff_ffff - listener.2);
        }
        let sep = 128 - (fixed_mul(Fixed(S_STEREO_SWING), finesine((angle >> 19) as usize)).0 >> FRACBITS);
        let vol = if approx_dist < S_CLOSE_DIST {
            self.sfx_volume
        } else if boss_map {
            approx_dist = approx_dist.min(S_CLIPPING_DIST);
            15 + ((self.sfx_volume - 15) * ((S_CLIPPING_DIST - approx_dist) >> FRACBITS)) / S_ATTENUATOR
        } else {
            (self.sfx_volume * ((S_CLIPPING_DIST - approx_dist) >> FRACBITS)) / S_ATTENUATOR
        };
        (vol > 0).then_some((vol, sep))
    }

    fn listener(w: &World) -> Option<(Fixed, Fixed, u32, MobjId)> {
        let id = w.console_player().mo?;
        let mo = w.thinkers.get_mobj(id)?;
        Some((mo.x, mo.y, mo.angle.0, id))
    }

    fn origin_of(w: &World, src: SoundSource) -> Origin {
        match src {
            SoundSource::None => Origin::None,
            SoundSource::Mobj(id) => match w.thinkers.get_mobj(id) {
                Some(m) => Origin::Mobj(id, m.x, m.y),
                None => Origin::None,
            },
            SoundSource::Sector(s) => {
                let o = w.level.sector(s).sound_org;
                Origin::Point(o.x, o.y)
            }
        }
    }

    /// `S_StartSound` for a sound with no world position (menus, HUD).
    pub fn start_local(&mut self, stack: &ResourceStack, info: &Info, sfx: SfxNum, rng: &mut doom_core::DoomRandom) {
        self.start(stack, info, sfx, Origin::None, None, false, rng, Play::default());
    }

    /// Handle the sim's sound events for this tic.
    pub fn handle_events(&mut self, stack: &ResourceStack, w: &mut World, events: &[SimEvent], boss_map: bool) {
        let listener = Self::listener(w);
        for ev in events {
            match *ev {
                SimEvent::StartSound { origin, sfx, slot, volume, atten, looping } => {
                    let o = Self::origin_of(w, origin);
                    // The console player's own sounds are not positioned
                    // (but still one per slot, as in Doom).
                    let local = matches!((o, listener), (Origin::Mobj(id, ..), Some((.., me))) if id == me);
                    let (info, rng) = (&w.info, &mut w.rng);
                    let play = Play {
                        volume: volume as f32 / 255.0,
                        atten: atten as f32 / doom_sim::world::ATTEN_NORMAL as f32,
                        looping,
                        slot,
                        local,
                    };
                    self.start(stack, info, sfx, o, listener.map(|l| (l.0, l.1, l.2)), boss_map, rng, play);
                }
                SimEvent::StopSound { origin, slot } => {
                    for c in 0..NUM_CHANNELS {
                        if self.channels[c].is_some_and(|ch| matches!(ch.origin, Origin::Mobj(id, ..) if id == origin) && (slot == 0 || ch.slot == slot)) {
                            self.stop_channel(c);
                            if slot == 0 {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        stack: &ResourceStack,
        info: &Info,
        sfx: SfxNum,
        origin: Origin,
        listener: Option<(Fixed, Fixed, u32)>,
        boss_map: bool,
        rng: &mut doom_core::DoomRandom,
        play: Play,
    ) {
        let Some(sinfo) = info.sfx.get(sfx.0 as usize).copied() else { return };
        let def = info.sound_defs.get(&sfx.0);
        let (def_volume, def_atten, limit, singular) = def.map_or((1.0, 1.0, 0, false), |d| (d.volume, d.attenuation, d.limit, d.singular));
        let atten = def_atten * play.atten;
        let looping = play.looping;
        let mut volume = self.sfx_volume;
        if sinfo.link.is_some() {
            volume += sinfo.volume;
            if volume < 1 {
                return;
            }
            volume = volume.min(self.sfx_volume);
        }
        let mut sep = NORM_SEP;
        if let (Origin::Mobj(_, x, y) | Origin::Point(x, y), Some(l), false) = (origin, listener, play.local) {
            match self.adjust(l, x, y, boss_map, atten) {
                Some((v, s)) => {
                    volume = v;
                    sep = if x == l.0 && y == l.1 { NORM_SEP } else { s };
                }
                None => return,
            }
        }
        // Pitch shifting is off (snd_pitchshift 0) but M_Random is still
        // consumed as in Chocolate Doom.
        if sfx.0 >= SFX_SAWUP.0 && sfx.0 <= SFX_SAWHIT.0 {
            rng.m_random();
        } else if sfx != SFX_ITEMUP && sfx != SFX_TINK {
            rng.m_random();
        }
        // `$singular` / `$limit`: enough of this sound already playing.
        let playing = self.channels.iter().flatten().filter(|ch| ch.sfx == sfx).count();
        if (singular && playing > 0) || (limit > 0 && playing >= limit as usize) {
            return;
        }
        // S_StopSound(origin): kill a sound from the same source (the same
        // slot, for mod sounds on separate channels).
        let same = |ch: &Channel| ch.origin.same_source(&origin) && (play.slot == 0 || ch.slot == play.slot);
        if origin != Origin::None {
            for c in 0..NUM_CHANNELS {
                if self.channels[c].is_some_and(|ch| same(&ch)) {
                    self.stop_channel(c);
                    break;
                }
            }
        }
        // S_GetChannel
        let mut slot = None;
        for c in 0..NUM_CHANNELS {
            match self.channels[c] {
                None => {
                    slot = Some(c);
                    break;
                }
                Some(ch) if origin != Origin::None && same(&ch) => {
                    self.stop_channel(c);
                    slot = Some(c);
                    break;
                }
                _ => {}
            }
        }
        if slot.is_none() {
            // Find a lower or equal priority (higher number) sound to drop.
            slot = (0..NUM_CHANNELS).find(|&c| self.channels[c].is_some_and(|ch| ch.priority >= sinfo.priority));
            match slot {
                Some(c) => self.stop_channel(c),
                None => return,
            }
        }
        let Some(c) = slot else { return };
        let played = self.pick(info, sfx);
        let Some(sound) = self.sound(stack, info, played) else { return };
        let scale = def_volume * play.volume;
        let len_tics = (sound.seconds() * 35.0).ceil() as u64 + 1;
        let ends_at = if looping { u64::MAX } else { self.tic + len_tics };
        let atten = if play.local { 0.0 } else { atten };
        self.channels[c] = Some(Channel { sfx, slot: play.slot, priority: sinfo.priority, origin, ends_at, volume: scale, attenuation: atten });
        let volume = ((volume as f32) * scale).round() as i32;
        self.out.push(AudioCommand::StartSound { channel: c, sound, volume, separation: sep, looping });
    }

    /// `S_UpdateSounds`: once per tic, follow moving sources, free finished
    /// channels and run ambient sounds.
    pub fn update(&mut self, stack: &ResourceStack, w: Option<&mut World>, boss_map: bool) {
        self.tic += 1;
        let w = match w {
            Some(w) => w,
            None => {
                for c in 0..NUM_CHANNELS {
                    if self.channels[c].is_some_and(|ch| self.tic >= ch.ends_at) {
                        self.channels[c] = None;
                    }
                }
                return;
            }
        };
        self.run_ambients(stack, w, boss_map);
        let listener = Self::listener(w);
        for c in 0..NUM_CHANNELS {
            let Some(mut ch) = self.channels[c] else { continue };
            if self.tic >= ch.ends_at {
                self.channels[c] = None;
                continue;
            }
            // Track the source mobj while it exists; a looping sound stops
            // with it.
            if let Origin::Mobj(id, ..) = ch.origin {
                match w.thinkers.get_mobj(id) {
                    Some(m) => {
                        ch.origin = Origin::Mobj(id, m.x, m.y);
                        self.channels[c] = Some(ch);
                    }
                    None if ch.ends_at == u64::MAX => {
                        self.stop_channel(c);
                        continue;
                    }
                    None => {}
                }
            }
            let (Origin::Mobj(_, x, y) | Origin::Point(x, y)) = ch.origin else { continue };
            let Some(l) = listener else { continue };
            if matches!(ch.origin, Origin::Mobj(id, ..) if id == l.3) {
                continue; // the listener's own sounds stay centred
            }
            let sinfo = w.info.sfx[ch.sfx.0 as usize];
            let mut vol_base = self.sfx_volume;
            if sinfo.link.is_some() {
                vol_base += sinfo.volume;
                if vol_base < 1 {
                    self.stop_channel(c);
                    continue;
                }
            }
            match self.adjust((l.0, l.1, l.2), x, y, boss_map, ch.attenuation) {
                Some((volume, separation)) => {
                    let volume = ((volume as f32) * ch.volume).round() as i32;
                    self.out.push(AudioCommand::UpdateSound { channel: c, volume, separation })
                }
                // A looping sound out of earshot keeps its channel, silent.
                None if ch.ends_at == u64::MAX => self.out.push(AudioCommand::UpdateSound { channel: c, volume: 0, separation: NORM_SEP }),
                None => self.stop_channel(c),
            }
        }
    }

    /// Start ambient sounds that are due.
    fn run_ambients(&mut self, stack: &ResourceStack, w: &mut World, boss_map: bool) {
        if self.ambients.is_empty() {
            return;
        }
        let listener = Self::listener(w).map(|l| (l.0, l.1, l.2));
        for k in 0..self.ambients.len() {
            let a = self.ambients[k];
            let Some(def) = w.info.ambient_sounds.get(&a.def).copied() else { continue };
            let Some(m) = w.thinkers.get_mobj(a.mobj) else { continue };
            let origin = if def.point { Origin::Mobj(a.mobj, m.x, m.y) } else { Origin::None };
            match def.mode {
                AmbientMode::Continuous => {
                    let playing = self.channels.iter().flatten().any(|ch| ch.ends_at == u64::MAX && ch.sfx == def.sound && (!def.point || ch.origin.same_source(&origin)));
                    if !playing && self.tic >= a.next {
                        // Ambient sounds are presentation: a private random
                        // number generator, never M_Random.
                        let mut r = doom_core::DoomRandom::new();
                        let play = Play { volume: def.volume, atten: def.attenuation, looping: true, slot: 1, local: false };
                        self.start(stack, &w.info, def.sound, origin, listener, boss_map, &mut r, play);
                        // Don't retry every tic if it can't start.
                        self.ambients[k].next = self.tic + 35;
                    }
                }
                AmbientMode::Random { .. } | AmbientMode::Periodic { .. } => {
                    let (min, max) = match def.mode {
                        AmbientMode::Random { min, max } => (min, max),
                        AmbientMode::Periodic { every } => (every, every),
                        AmbientMode::Continuous => unreachable!(),
                    };
                    if self.tic >= a.next {
                        let mut r = doom_core::DoomRandom::new();
                        let play = Play { volume: def.volume, atten: def.attenuation, looping: false, slot: 1, local: false };
                        self.start(stack, &w.info, def.sound, origin, listener, boss_map, &mut r, play);
                        let wait = if max > min { min + self.random() % (max - min + 1) } else { min };
                        self.ambients[k].next = self.tic + wait as u64;
                    }
                }
            }
        }
    }

    /// `S_ChangeMusic` by music number.
    pub fn change_music(&mut self, stack: &ResourceStack, music: MusicNum, looping: bool) {
        let Some(name) = MUSIC_NAMES.get(music.0 as usize) else { return };
        self.change_music_lump(stack, &format!("D_{}", name.to_ascii_uppercase()), looping);
    }

    pub fn change_music_lump(&mut self, stack: &ResourceStack, lump: &str, looping: bool) {
        if self.current_music.as_deref() == Some(lump) {
            return;
        }
        let song = self
            .songs
            .entry(lump.to_string())
            .or_insert_with(|| stack.find_sound(lump).map(|id| stack.data(id)).and_then(|d| Song::parse(d).ok()).map(Arc::new))
            .clone();
        self.current_music = Some(lump.to_string());
        match song {
            Some(song) => self.out.push(AudioCommand::PlayMusic { song, looping }),
            None => self.out.push(AudioCommand::StopMusic),
        }
    }

    pub fn stop_music(&mut self) {
        self.current_music = None;
        self.out.push(AudioCommand::StopMusic);
    }

    pub fn pause_music(&mut self, paused: bool) {
        self.out.push(AudioCommand::PauseMusic(paused));
    }

    /// `S_Start`'s music choice for a map.
    pub fn level_music(mode: GameMode, episode: i32, map: i32) -> MusicNum {
        use doom_sim::info::*;
        if mode == GameMode::Commercial {
            return MusicNum(MUS_RUNNIN.0 + (map - 1).clamp(0, 31) as u16);
        }
        let spmus = [MUS_E3M4, MUS_E3M2, MUS_E3M3, MUS_E1M5, MUS_E2M7, MUS_E2M4, MUS_E2M6, MUS_E2M5, MUS_E1M9];
        if episode < 4 {
            MusicNum(MUS_E1M1.0 + ((episode - 1) * 9 + map - 1).clamp(0, 26) as u16)
        } else {
            spmus[(map - 1).clamp(0, 8) as usize]
        }
    }
}
