//! Sound-effect lumps and the software mixer that plays them together with
//! music.

use std::sync::Arc;

use crate::music::{Genmidi, MusicPlayer, Song};

/// A decoded sound effect: signed 16-bit PCM, mono or interleaved
/// stereo, at its own rate (the mixer resamples). Every format decodes to
/// this ([`crate::decode`]).
#[derive(Clone, Debug)]
pub struct Sound {
    pub rate: u32,
    /// 1 or 2.
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Sound {
    /// Decode a DMX sound lump (format 3). Like DMX, the 16 padding bytes at
    /// each end of the sample data are skipped.
    pub fn parse_dmx(lump: &[u8]) -> Option<Sound> {
        if lump.len() < 8 || lump[0] != 0x03 || lump[1] != 0x00 {
            return None;
        }
        let rate = u16::from_le_bytes([lump[2], lump[3]]) as u32;
        let length = u32::from_le_bytes([lump[4], lump[5], lump[6], lump[7]]) as usize;
        if length > lump.len() - 8 || length <= 48 || rate == 0 {
            return None;
        }
        let pcm = &lump[8 + 16..8 + 16 + length - 32];
        Some(Sound { rate, channels: 1, samples: pcm.iter().map(|&b| ((b as i16) - 128) << 8).collect() })
    }

    /// Sample frames (one sample per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// Duration in seconds.
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate as f64
    }
}

/// A request from the game thread to the audio thread.
#[derive(Clone, Debug)]
pub enum AudioCommand {
    /// Start `sound` on mixer channel `channel` (replacing what it plays).
    /// `volume` 0..=127, `separation` 0..=255 (128 = centre), as computed by
    /// `S_StartSound`.
    StartSound { channel: usize, sound: Arc<Sound>, volume: i32, separation: i32, looping: bool },
    UpdateSound { channel: usize, volume: i32, separation: i32 },
    StopSound { channel: usize },
    PlayMusic { song: Arc<Song>, looping: bool },
    StopMusic,
    PauseMusic(bool),
    /// 0..=127.
    SetMusicVolume(u32),
}

#[derive(Clone, Debug)]
struct Voice {
    sound: Arc<Sound>,
    /// Position in source samples, 32.32 fixed point.
    pos: u64,
    step: u64,
    left: f32,
    right: f32,
    looping: bool,
}

/// Digital music being played (`Song::Stream`).
#[derive(Clone, Debug)]
struct Stream {
    sound: Arc<Sound>,
    /// 32.32 fixed point source frames, as [`Voice`].
    pos: u64,
    step: u64,
    looping: bool,
}

/// Mixes sound-effect channels and music into interleaved stereo `f32`.
pub struct Mixer {
    rate: u32,
    voices: Vec<Option<Voice>>,
    music: Option<MusicPlayer>,
    stream: Option<Stream>,
    /// Music volume 0..=127 and pause state, for digital music (the OPL
    /// player keeps its own).
    music_volume: u32,
    music_paused: bool,
    music_buf: Vec<i16>,
    /// DC blocker state per channel (x[n-1], y[n-1]).
    dc: [(f32, f32); 2],
    dc_r: f32,
}

impl Mixer {
    /// `genmidi`: the instrument bank; without one, music is silent.
    pub fn new(rate: u32, channels: usize, genmidi: Option<Arc<Genmidi>>) -> Self {
        Mixer {
            rate,
            voices: vec![None; channels],
            music: genmidi.map(|g| MusicPlayer::new(g, rate)),
            stream: None,
            music_volume: 64,
            music_paused: false,
            music_buf: Vec::new(),
            dc: [(0.0, 0.0); 2],
            // One-pole high-pass at ~3 Hz.
            dc_r: 1.0 - (2.0 * std::f32::consts::PI * 3.0 / rate as f32),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// `I_UpdateSoundParams` (i_sdlsound.c): DMX separation to SDL_mixer
    /// panning gains.
    fn gains(volume: i32, separation: i32) -> (f32, f32) {
        let left = ((254 - separation) * volume / 127).clamp(0, 255);
        let right = (separation * volume / 127).clamp(0, 255);
        (left as f32 / 255.0, right as f32 / 255.0)
    }

    pub fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::StartSound { channel, sound, volume, separation, looping } => {
                if channel >= self.voices.len() || sound.frames() == 0 {
                    return;
                }
                let (left, right) = Self::gains(volume, separation);
                let step = ((sound.rate as u64) << 32) / self.rate as u64;
                self.voices[channel] = Some(Voice { sound, pos: 0, step, left, right, looping });
            }
            AudioCommand::UpdateSound { channel, volume, separation } => {
                if let Some(Some(v)) = self.voices.get_mut(channel) {
                    (v.left, v.right) = Self::gains(volume, separation);
                }
            }
            AudioCommand::StopSound { channel } => {
                if let Some(v) = self.voices.get_mut(channel) {
                    *v = None;
                }
            }
            AudioCommand::PlayMusic { song, looping } => {
                self.stream = match &*song {
                    Song::Stream(sound) if sound.frames() > 0 => Some(Stream {
                        sound: sound.clone(),
                        pos: 0,
                        step: ((sound.rate as u64) << 32) / self.rate as u64,
                        looping,
                    }),
                    _ => None,
                };
                if let Some(m) = &mut self.music {
                    // A digital song stops whatever the OPL was playing.
                    m.play(song, looping);
                }
            }
            AudioCommand::StopMusic => {
                self.stream = None;
                if let Some(m) = &mut self.music {
                    m.stop();
                }
            }
            AudioCommand::PauseMusic(p) => {
                self.music_paused = p;
                if let Some(m) = &mut self.music {
                    m.set_paused(p);
                }
            }
            AudioCommand::SetMusicVolume(v) => {
                self.music_volume = v.min(127);
                if let Some(m) = &mut self.music {
                    m.set_volume(v);
                }
            }
        }
    }

    /// Fill `out` (interleaved stereo) with the next samples.
    pub fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        out.fill(0.0);
        if let Some(m) = &mut self.music {
            self.music_buf.resize(frames * 2, 0);
            m.render(&mut self.music_buf);
            // The OPL's half/absolute-sine waveforms carry DC; sound cards
            // AC-couple their output, so block it here.
            for (i, (o, &s)) in out.iter_mut().zip(&self.music_buf).enumerate() {
                let x = s as f32 / 32768.0;
                let (x1, y1) = &mut self.dc[i & 1];
                let y = x - *x1 + self.dc_r * *y1;
                *x1 = x;
                *y1 = y;
                *o = y;
            }
        }
        if !self.music_paused {
            let gain = self.music_volume as f32 / 127.0;
            if let Some(s) = &mut self.stream {
                if !mix_stream(s, out, gain) {
                    self.stream = None;
                }
            }
        }
        for slot in &mut self.voices {
            let Some(v) = slot else { continue };
            let ch = v.sound.channels.max(1) as usize;
            let data = &v.sound.samples;
            let n = (data.len() / ch) as u64;
            for frame in out.chunks_exact_mut(2) {
                let mut i = (v.pos >> 32) as usize;
                if i as u64 >= n {
                    if !v.looping {
                        *slot = None;
                        break;
                    }
                    v.pos -= n << 32;
                    i = (v.pos >> 32) as usize;
                }
                // Linear interpolation between neighbouring frames.
                let t = (v.pos & 0xffff_ffff) as f32 / 4294967296.0;
                let at = |k: usize, c: usize| data[k * ch + c] as f32 / 32768.0;
                let next = (i + 1).min(n as usize - 1);
                let l = at(i, 0) + (at(next, 0) - at(i, 0)) * t;
                let r = if ch == 2 { at(i, 1) + (at(next, 1) - at(i, 1)) * t } else { l };
                // Stereo sounds keep their channels; panning gains apply on
                // top (positioned sounds are mostly mono).
                frame[0] += l * v.left;
                frame[1] += r * v.right;
                v.pos += v.step;
            }
        }
        for o in out.iter_mut() {
            *o = o.clamp(-1.0, 1.0);
        }
    }
}

/// Mix digital music into `out`; false once a non-looping song has ended.
fn mix_stream(s: &mut Stream, out: &mut [f32], gain: f32) -> bool {
    let ch = s.sound.channels.max(1) as usize;
    let data = &s.sound.samples;
    let n = (data.len() / ch) as u64;
    for frame in out.chunks_exact_mut(2) {
        let mut i = (s.pos >> 32) as usize;
        if i as u64 >= n {
            if !s.looping {
                return false;
            }
            s.pos %= n << 32;
            i = (s.pos >> 32) as usize;
        }
        let t = (s.pos & 0xffff_ffff) as f32 / 4294967296.0;
        let at = |k: usize, c: usize| data[k * ch + c] as f32 / 32768.0;
        // Interpolate towards the start of the song when looping.
        let next = if i + 1 < n as usize { i + 1 } else if s.looping { 0 } else { i };
        let l = at(i, 0) + (at(next, 0) - at(i, 0)) * t;
        let r = if ch >= 2 { at(i, 1) + (at(next, 1) - at(i, 1)) * t } else { l };
        frame[0] += l * gain;
        frame[1] += r * gain;
        s.pos += s.step;
    }
    true
}
