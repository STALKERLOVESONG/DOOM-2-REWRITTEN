//! MUS music played through the OPL chip with the GENMIDI instrument bank,
//! following DMX as reconstructed by Chocolate Doom's `i_oplmusic.c`
//! (Doom 1.9 driver behaviour, OPL2 mode, 9 voices).
//!
//! DMX works on MUS events directly (Chocolate converts to MIDI and maps
//! back); this player reads MUS natively, so channel 15 is percussion.

use std::sync::Arc;

use crate::opl::Opl;

/// MUS timing: 140 ticks per second.
pub const MUS_TICK_RATE: u32 = 140;
const NUM_VOICES: usize = 9;
const NUM_CHANNELS: usize = 16;
const PERCUSSION_CHANNEL: usize = 15;
const FLAG_FIXED: u16 = 0x0001;
const FLAG_2VOICE: u16 = 0x0004;

const REG_TREMOLO: u16 = 0x20;
const REG_LEVEL: u16 = 0x40;
const REG_ATTACK: u16 = 0x60;
const REG_SUSTAIN: u16 = 0x80;
const REG_WAVEFORM: u16 = 0xe0;
const REG_FREQ_1: u16 = 0xa0;
const REG_FREQ_2: u16 = 0xb0;
const REG_FEEDBACK: u16 = 0xc0;

const VOICE_OPERATORS: [[u16; NUM_VOICES]; 2] =
    [[0x00, 0x01, 0x02, 0x08, 0x09, 0x0a, 0x10, 0x11, 0x12], [0x03, 0x04, 0x05, 0x0b, 0x0c, 0x0d, 0x13, 0x14, 0x15]];

/// Generated from `frequency_curve` in i_oplmusic.c (the last entry is a
/// DMX table overrun, kept as is).
#[rustfmt::skip]
static FREQUENCY_CURVE: [u16; 668] = [
    0x133, 0x133, 0x134, 0x134, 0x135, 0x136, 0x136, 0x137, 0x137, 0x138, 0x138, 0x139, 0x139, 0x13a, 0x13b, 0x13b,
    0x13c, 0x13c, 0x13d, 0x13d, 0x13e, 0x13f, 0x13f, 0x140, 0x140, 0x141, 0x142, 0x142, 0x143, 0x143, 0x144, 0x144,
    0x145, 0x146, 0x146, 0x147, 0x147, 0x148, 0x149, 0x149, 0x14a, 0x14a, 0x14b, 0x14c, 0x14c, 0x14d, 0x14d, 0x14e,
    0x14f, 0x14f, 0x150, 0x150, 0x151, 0x152, 0x152, 0x153, 0x153, 0x154, 0x155, 0x155, 0x156, 0x157, 0x157, 0x158,
    0x158, 0x159, 0x15a, 0x15a, 0x15b, 0x15b, 0x15c, 0x15d, 0x15d, 0x15e, 0x15f, 0x15f, 0x160, 0x161, 0x161, 0x162,
    0x162, 0x163, 0x164, 0x164, 0x165, 0x166, 0x166, 0x167, 0x168, 0x168, 0x169, 0x16a, 0x16a, 0x16b, 0x16c, 0x16c,
    0x16d, 0x16e, 0x16e, 0x16f, 0x170, 0x170, 0x171, 0x172, 0x172, 0x173, 0x174, 0x174, 0x175, 0x176, 0x176, 0x177,
    0x178, 0x178, 0x179, 0x17a, 0x17a, 0x17b, 0x17c, 0x17c, 0x17d, 0x17e, 0x17e, 0x17f, 0x180, 0x181, 0x181, 0x182,
    0x183, 0x183, 0x184, 0x185, 0x185, 0x186, 0x187, 0x188, 0x188, 0x189, 0x18a, 0x18a, 0x18b, 0x18c, 0x18d, 0x18d,
    0x18e, 0x18f, 0x18f, 0x190, 0x191, 0x192, 0x192, 0x193, 0x194, 0x194, 0x195, 0x196, 0x197, 0x197, 0x198, 0x199,
    0x19a, 0x19a, 0x19b, 0x19c, 0x19d, 0x19d, 0x19e, 0x19f, 0x1a0, 0x1a0, 0x1a1, 0x1a2, 0x1a3, 0x1a3, 0x1a4, 0x1a5,
    0x1a6, 0x1a6, 0x1a7, 0x1a8, 0x1a9, 0x1a9, 0x1aa, 0x1ab, 0x1ac, 0x1ad, 0x1ad, 0x1ae, 0x1af, 0x1b0, 0x1b0, 0x1b1,
    0x1b2, 0x1b3, 0x1b4, 0x1b4, 0x1b5, 0x1b6, 0x1b7, 0x1b8, 0x1b8, 0x1b9, 0x1ba, 0x1bb, 0x1bc, 0x1bc, 0x1bd, 0x1be,
    0x1bf, 0x1c0, 0x1c0, 0x1c1, 0x1c2, 0x1c3, 0x1c4, 0x1c4, 0x1c5, 0x1c6, 0x1c7, 0x1c8, 0x1c9, 0x1c9, 0x1ca, 0x1cb,
    0x1cc, 0x1cd, 0x1ce, 0x1ce, 0x1cf, 0x1d0, 0x1d1, 0x1d2, 0x1d3, 0x1d3, 0x1d4, 0x1d5, 0x1d6, 0x1d7, 0x1d8, 0x1d8,
    0x1d9, 0x1da, 0x1db, 0x1dc, 0x1dd, 0x1de, 0x1de, 0x1df, 0x1e0, 0x1e1, 0x1e2, 0x1e3, 0x1e4, 0x1e5, 0x1e5, 0x1e6,
    0x1e7, 0x1e8, 0x1e9, 0x1ea, 0x1eb, 0x1ec, 0x1ed, 0x1ed, 0x1ee, 0x1ef, 0x1f0, 0x1f1, 0x1f2, 0x1f3, 0x1f4, 0x1f5,
    0x1f6, 0x1f6, 0x1f7, 0x1f8, 0x1f9, 0x1fa, 0x1fb, 0x1fc, 0x1fd, 0x1fe, 0x1ff, 0x200, 0x201, 0x201, 0x202, 0x203,
    0x204, 0x205, 0x206, 0x207, 0x208, 0x209, 0x20a, 0x20b, 0x20c, 0x20d, 0x20e, 0x20f, 0x210, 0x210, 0x211, 0x212,
    0x213, 0x214, 0x215, 0x216, 0x217, 0x218, 0x219, 0x21a, 0x21b, 0x21c, 0x21d, 0x21e, 0x21f, 0x220, 0x221, 0x222,
    0x223, 0x224, 0x225, 0x226, 0x227, 0x228, 0x229, 0x22a, 0x22b, 0x22c, 0x22d, 0x22e, 0x22f, 0x230, 0x231, 0x232,
    0x233, 0x234, 0x235, 0x236, 0x237, 0x238, 0x239, 0x23a, 0x23b, 0x23c, 0x23d, 0x23e, 0x23f, 0x240, 0x241, 0x242,
    0x244, 0x245, 0x246, 0x247, 0x248, 0x249, 0x24a, 0x24b, 0x24c, 0x24d, 0x24e, 0x24f, 0x250, 0x251, 0x252, 0x253,
    0x254, 0x256, 0x257, 0x258, 0x259, 0x25a, 0x25b, 0x25c, 0x25d, 0x25e, 0x25f, 0x260, 0x262, 0x263, 0x264, 0x265,
    0x266, 0x267, 0x268, 0x269, 0x26a, 0x26c, 0x26d, 0x26e, 0x26f, 0x270, 0x271, 0x272, 0x273, 0x275, 0x276, 0x277,
    0x278, 0x279, 0x27a, 0x27b, 0x27d, 0x27e, 0x27f, 0x280, 0x281, 0x282, 0x284, 0x285, 0x286, 0x287, 0x288, 0x289,
    0x28b, 0x28c, 0x28d, 0x28e, 0x28f, 0x290, 0x292, 0x293, 0x294, 0x295, 0x296, 0x298, 0x299, 0x29a, 0x29b, 0x29c,
    0x29e, 0x29f, 0x2a0, 0x2a1, 0x2a2, 0x2a4, 0x2a5, 0x2a6, 0x2a7, 0x2a9, 0x2aa, 0x2ab, 0x2ac, 0x2ae, 0x2af, 0x2b0,
    0x2b1, 0x2b2, 0x2b4, 0x2b5, 0x2b6, 0x2b7, 0x2b9, 0x2ba, 0x2bb, 0x2bd, 0x2be, 0x2bf, 0x2c0, 0x2c2, 0x2c3, 0x2c4,
    0x2c5, 0x2c7, 0x2c8, 0x2c9, 0x2cb, 0x2cc, 0x2cd, 0x2ce, 0x2d0, 0x2d1, 0x2d2, 0x2d4, 0x2d5, 0x2d6, 0x2d8, 0x2d9,
    0x2da, 0x2dc, 0x2dd, 0x2de, 0x2e0, 0x2e1, 0x2e2, 0x2e4, 0x2e5, 0x2e6, 0x2e8, 0x2e9, 0x2ea, 0x2ec, 0x2ed, 0x2ee,
    0x2f0, 0x2f1, 0x2f2, 0x2f4, 0x2f5, 0x2f6, 0x2f8, 0x2f9, 0x2fb, 0x2fc, 0x2fd, 0x2ff, 0x300, 0x302, 0x303, 0x304,
    0x306, 0x307, 0x309, 0x30a, 0x30b, 0x30d, 0x30e, 0x310, 0x311, 0x312, 0x314, 0x315, 0x317, 0x318, 0x31a, 0x31b,
    0x31c, 0x31e, 0x31f, 0x321, 0x322, 0x324, 0x325, 0x327, 0x328, 0x329, 0x32b, 0x32c, 0x32e, 0x32f, 0x331, 0x332,
    0x334, 0x335, 0x337, 0x338, 0x33a, 0x33b, 0x33d, 0x33e, 0x340, 0x341, 0x343, 0x344, 0x346, 0x347, 0x349, 0x34a,
    0x34c, 0x34d, 0x34f, 0x350, 0x352, 0x353, 0x355, 0x357, 0x358, 0x35a, 0x35b, 0x35d, 0x35e, 0x360, 0x361, 0x363,
    0x365, 0x366, 0x368, 0x369, 0x36b, 0x36c, 0x36e, 0x370, 0x371, 0x373, 0x374, 0x376, 0x378, 0x379, 0x37b, 0x37c,
    0x37e, 0x380, 0x381, 0x383, 0x384, 0x386, 0x388, 0x389, 0x38b, 0x38d, 0x38e, 0x390, 0x392, 0x393, 0x395, 0x397,
    0x398, 0x39a, 0x39c, 0x39d, 0x39f, 0x3a1, 0x3a2, 0x3a4, 0x3a6, 0x3a7, 0x3a9, 0x3ab, 0x3ac, 0x3ae, 0x3b0, 0x3b1,
    0x3b3, 0x3b5, 0x3b7, 0x3b8, 0x3ba, 0x3bc, 0x3bd, 0x3bf, 0x3c1, 0x3c3, 0x3c4, 0x3c6, 0x3c8, 0x3ca, 0x3cb, 0x3cd,
    0x3cf, 0x3d1, 0x3d2, 0x3d4, 0x3d6, 0x3d8, 0x3da, 0x3db, 0x3dd, 0x3df, 0x3e1, 0x3e3, 0x3e4, 0x3e6, 0x3e8, 0x3ea,
    0x3ec, 0x3ed, 0x3ef, 0x3f1, 0x3f3, 0x3f5, 0x3f6, 0x3f8, 0x3fa, 0x3fc, 0x3fe, 0x36c,
];

/// MIDI volume (0-127) to OPL level.
#[rustfmt::skip]
static VOLUME_MAPPING: [u32; 128] = [
    0, 1, 3, 5, 6, 8, 10, 11, 13, 14, 16, 17, 19, 20, 22, 23,
    25, 26, 27, 29, 30, 32, 33, 34, 36, 37, 39, 41, 43, 45, 47, 49,
    50, 52, 54, 55, 57, 59, 60, 61, 63, 64, 66, 67, 68, 69, 71, 72,
    73, 74, 75, 76, 77, 79, 80, 81, 82, 83, 84, 84, 85, 86, 87, 88,
    89, 90, 91, 92, 92, 93, 94, 95, 96, 96, 97, 98, 99, 99, 100, 101,
    101, 102, 103, 103, 104, 105, 105, 106, 107, 107, 108, 109, 109, 110, 110, 111,
    112, 112, 113, 113, 114, 114, 115, 115, 116, 117, 117, 118, 118, 119, 119, 120,
    120, 121, 121, 122, 122, 123, 123, 123, 124, 124, 125, 125, 126, 126, 127, 127,
];

// ---------------------------------------------------------------------------
// GENMIDI
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct GenmidiOp {
    pub tremolo: u8,
    pub attack: u8,
    pub sustain: u8,
    pub waveform: u8,
    pub scale: u8,
    pub level: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GenmidiVoice {
    pub modulator: GenmidiOp,
    pub feedback: u8,
    pub carrier: GenmidiOp,
    pub base_note_offset: i16,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GenmidiInstr {
    pub flags: u16,
    pub fine_tuning: u8,
    pub fixed_note: u8,
    pub voices: [GenmidiVoice; 2],
}

/// The `GENMIDI` lump: 128 melodic instruments and 47 percussion
/// instruments (keys 35-81).
#[derive(Clone, Debug)]
pub struct Genmidi {
    pub main: Vec<GenmidiInstr>,
    pub percussion: Vec<GenmidiInstr>,
}

impl Genmidi {
    pub fn parse(lump: &[u8]) -> Result<Genmidi, String> {
        const INSTR_SIZE: usize = 36;
        // DMX does not check the "#OPL_II#" header; neither do we.
        let need = 8 + (128 + 47) * INSTR_SIZE;
        if lump.len() < need {
            return Err(format!("GENMIDI lump too short ({} bytes)", lump.len()));
        }
        let op = |b: &[u8]| GenmidiOp { tremolo: b[0], attack: b[1], sustain: b[2], waveform: b[3], scale: b[4], level: b[5] };
        let voice = |b: &[u8]| GenmidiVoice {
            modulator: op(&b[0..6]),
            feedback: b[6],
            carrier: op(&b[7..13]),
            base_note_offset: i16::from_le_bytes([b[14], b[15]]),
        };
        let instr = |i: usize| {
            let b = &lump[8 + i * INSTR_SIZE..8 + (i + 1) * INSTR_SIZE];
            GenmidiInstr {
                flags: u16::from_le_bytes([b[0], b[1]]),
                fine_tuning: b[2],
                fixed_note: b[3],
                voices: [voice(&b[4..20]), voice(&b[20..36])],
            }
        };
        Ok(Genmidi { main: (0..128).map(instr).collect(), percussion: (128..175).map(instr).collect() })
    }
}

// ---------------------------------------------------------------------------
// MUS
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MusEvent {
    ReleaseNote { channel: u8, note: u8 },
    /// `volume` is `None` when the event reuses the channel's last volume.
    PlayNote { channel: u8, note: u8, volume: Option<u8> },
    /// 0..255, 128 = centre.
    PitchBend { channel: u8, bend: u8 },
    /// MUS system events 10-14 (sounds off, notes off, mono, poly, reset).
    System { channel: u8, controller: u8 },
    Controller { channel: u8, controller: u8, value: u8 },
    ScoreEnd,
    /// Measure end (5) and the unused type 7: no effect, but may carry a delay.
    Nop,
}

/// A parsed MUS lump: events with the delay (in 140 Hz ticks) that follows
/// each one.
#[derive(Clone, Debug)]
pub struct Song {
    pub events: Vec<(MusEvent, u32)>,
}

impl Song {
    pub fn parse(lump: &[u8]) -> Result<Song, String> {
        if lump.len() < 16 || &lump[0..4] != b"MUS\x1a" {
            return Err("not a MUS lump".into());
        }
        let score_len = u16::from_le_bytes([lump[4], lump[5]]) as usize;
        let start = u16::from_le_bytes([lump[6], lump[7]]) as usize;
        let end = (start + score_len).min(lump.len());
        let mut p = start;
        let mut byte = || -> Option<u8> {
            let b = *lump.get(p).filter(|_| p < end)?;
            p += 1;
            Some(b)
        };
        let mut events = Vec::new();
        while let Some(desc) = byte() {
            let channel = desc & 0x0f;
            let ev = match (desc >> 4) & 0x07 {
                0 => MusEvent::ReleaseNote { channel, note: byte().unwrap_or(0) & 0x7f },
                1 => {
                    let n = byte().unwrap_or(0);
                    let volume = if n & 0x80 != 0 { Some(byte().unwrap_or(0) & 0x7f) } else { None };
                    MusEvent::PlayNote { channel, note: n & 0x7f, volume }
                }
                2 => MusEvent::PitchBend { channel, bend: byte().unwrap_or(128) },
                3 => MusEvent::System { channel, controller: byte().unwrap_or(0) & 0x7f },
                4 => {
                    let controller = byte().unwrap_or(0);
                    // mus2mid clamps (not masks) out-of-range values.
                    let value = byte().unwrap_or(0).min(0x7f);
                    MusEvent::Controller { channel, controller, value }
                }
                6 => MusEvent::ScoreEnd,
                // 5 = end of measure, 7 = unused: no data bytes.
                _ => MusEvent::Nop,
            };
            let mut delay = 0u32;
            if desc & 0x80 != 0 {
                while let Some(b) = byte() {
                    delay = (delay << 7) | (b & 0x7f) as u32;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
            }
            let end_of_score = ev == MusEvent::ScoreEnd;
            events.push((ev, delay));
            if end_of_score {
                break;
            }
        }
        if !matches!(events.last(), Some((MusEvent::ScoreEnd, _))) {
            events.push((MusEvent::ScoreEnd, 0));
        }
        Ok(Song { events })
    }
}

// ---------------------------------------------------------------------------
// Player (i_oplmusic.c, Doom 1.9 driver)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct ChannelState {
    instrument: usize,
    volume: u32,
    volume_base: u32,
    bend: i32,
    /// Last note-on volume (MUS reuses it when a play event omits it).
    last_velocity: u8,
}

#[derive(Clone, Copy, Debug)]
enum InstrRef {
    Main(usize),
    Percussion(usize),
}

#[derive(Clone, Copy, Debug)]
struct Voice {
    index: usize,
    op1: u16,
    op2: u16,
    current: Option<(InstrRef, usize)>,
    channel: Option<usize>,
    key: u32,
    note: u32,
    freq: u32,
    note_volume: u32,
    car_volume: u32,
    mod_volume: u32,
}

/// Plays one song at a time on its own OPL chip.
pub struct MusicPlayer {
    opl: Opl,
    bank: Arc<Genmidi>,
    voices: [Voice; NUM_VOICES],
    free: Vec<usize>,
    alloced: Vec<usize>,
    channels: [ChannelState; NUM_CHANNELS],
    song: Option<Arc<Song>>,
    looping: bool,
    pos: usize,
    /// Output samples until the next event, in 1/140 s ticks scaled by the
    /// output rate (fixed point: samples * 140).
    wait: u64,
    sample_rate: u32,
    current_volume: u32,
    start_volume: u32,
    paused: bool,
    finished: bool,
    /// Output samples rendered so far (for traces).
    position: u64,
    /// Register writes `(sample, register, value)` while tracing.
    trace: Option<Vec<(u64, u16, u8)>>,
}

impl MusicPlayer {
    pub fn new(bank: Arc<Genmidi>, sample_rate: u32) -> Self {
        let mut p = MusicPlayer {
            opl: Opl::new(sample_rate),
            bank,
            voices: std::array::from_fn(|i| Voice {
                index: i,
                op1: VOICE_OPERATORS[0][i],
                op2: VOICE_OPERATORS[1][i],
                current: None,
                channel: None,
                key: 0,
                note: 0,
                freq: 0,
                note_volume: 0,
                car_volume: 0,
                mod_volume: 0,
            }),
            free: (0..NUM_VOICES).collect(),
            alloced: Vec::new(),
            channels: [ChannelState { instrument: 0, volume: 100, volume_base: 100, bend: 0, last_velocity: 127 };
                NUM_CHANNELS],
            song: None,
            looping: false,
            pos: 0,
            wait: 0,
            sample_rate,
            current_volume: 64,
            start_volume: 64,
            paused: false,
            finished: true,
            position: 0,
            trace: None,
        };
        p.init_registers();
        p
    }

    /// Write an OPL register (recorded while tracing).
    fn reg(&mut self, r: u16, v: u8) {
        if let Some(t) = &mut self.trace {
            t.push((self.position, r, v));
        }
        self.opl.write_buffered(r, v);
    }

    /// Start recording register writes (diagnostics, reference comparisons).
    pub fn start_trace(&mut self) {
        self.trace = Some(Vec::new());
    }

    /// The register writes recorded since [`MusicPlayer::start_trace`], as
    /// `(output sample index, register, value)`.
    pub fn take_trace(&mut self) -> Vec<(u64, u16, u8)> {
        self.trace.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// `OPL_InitRegisters` (OPL2).
    fn init_registers(&mut self) {
        for r in REG_LEVEL..=REG_LEVEL + 0x15 {
            self.reg(r, 0x3f);
        }
        for r in REG_ATTACK..=REG_WAVEFORM + 0x15 {
            self.reg(r, 0x00);
        }
        for r in 1..REG_LEVEL {
            self.reg(r, 0x00);
        }
        self.reg(0x04, 0x60);
        self.reg(0x04, 0x80);
        self.reg(0x01, 0x20); // waveform select enable
        self.reg(0x08, 0x40); // keyboard split
    }

    fn instr(&self, r: InstrRef) -> &GenmidiInstr {
        match r {
            InstrRef::Main(i) => &self.bank.main[i],
            InstrRef::Percussion(i) => &self.bank.percussion[i],
        }
    }

    /// Music volume, 0-127 (`I_OPL_SetMusicVolume`).
    pub fn set_volume(&mut self, volume: u32) {
        let volume = volume.min(127);
        if self.current_volume == volume {
            return;
        }
        self.current_volume = volume;
        for ch in 0..NUM_CHANNELS {
            // Percussion takes the music volume directly (a DMX quirk).
            let v = if ch == PERCUSSION_CHANNEL { volume } else { self.channels[ch].volume_base };
            self.set_channel_volume(ch, v, false);
        }
    }

    pub fn play(&mut self, song: Arc<Song>, looping: bool) {
        self.stop();
        self.song = Some(song);
        self.looping = looping;
        self.pos = 0;
        self.wait = 0;
        self.finished = false;
        self.paused = false;
        self.start_volume = self.current_volume;
        for ch in 0..NUM_CHANNELS {
            self.init_channel(ch);
        }
    }

    pub fn stop(&mut self) {
        while !self.alloced.is_empty() {
            self.release_voice(0);
        }
        self.song = None;
        self.finished = true;
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused && !self.paused {
            // DMX keys off all voices while paused (I_OPL_PauseSong).
            for i in 0..self.alloced.len() {
                let v = self.voices[self.alloced[i]];
                self.reg(REG_FREQ_2 + v.index as u16, (v.freq >> 8) as u8);
            }
        } else if !paused && self.paused {
            for i in 0..self.alloced.len() {
                let v = self.voices[self.alloced[i]];
                self.reg(REG_FREQ_2 + v.index as u16, ((v.freq >> 8) | 0x20) as u8);
            }
        }
        self.paused = paused;
    }

    pub fn is_playing(&self) -> bool {
        !self.finished
    }

    fn init_channel(&mut self, ch: usize) {
        let c = &mut self.channels[ch];
        c.instrument = 0;
        c.volume_base = 100;
        c.volume = self.current_volume.min(c.volume_base);
        c.bend = 0;
        c.last_velocity = 127;
    }

    // ---- voices -------------------------------------------------------------

    fn get_free_voice(&mut self) -> Option<usize> {
        if self.free.is_empty() {
            return None;
        }
        let v = self.free.remove(0);
        self.alloced.push(v);
        Some(v)
    }

    fn voice_key_off(&mut self, v: usize) {
        let voice = self.voices[v];
        self.reg(REG_FREQ_2 + voice.index as u16, (voice.freq >> 8) as u8);
    }

    fn release_voice(&mut self, index: usize) {
        if index >= self.alloced.len() {
            return;
        }
        let v = self.alloced.remove(index);
        self.voice_key_off(v);
        self.voices[v].channel = None;
        self.voices[v].note = 0;
        // DMX appends to the end of the free list.
        self.free.push(v);
    }

    fn load_operator(&mut self, op: u16, data: GenmidiOp, max_level: bool) -> u32 {
        let level = data.scale as u32 | if max_level { 0x3f } else { data.level as u32 };
        self.reg(REG_LEVEL + op, level as u8);
        self.reg(REG_TREMOLO + op, data.tremolo);
        self.reg(REG_ATTACK + op, data.attack);
        self.reg(REG_SUSTAIN + op, data.sustain);
        self.reg(REG_WAVEFORM + op, data.waveform);
        level
    }

    fn set_voice_instrument(&mut self, v: usize, instr: InstrRef, instr_voice: usize) {
        if let Some((cur, cv)) = self.voices[v].current {
            let same = match (cur, instr) {
                (InstrRef::Main(a), InstrRef::Main(b)) | (InstrRef::Percussion(a), InstrRef::Percussion(b)) => a == b,
                _ => false,
            };
            if same && cv == instr_voice {
                return;
            }
        }
        self.voices[v].current = Some((instr, instr_voice));
        let data = self.instr(instr).voices[instr_voice];
        let modulating = data.feedback & 0x01 == 0;
        let (op1, op2, index) = (self.voices[v].op1, self.voices[v].op2, self.voices[v].index);
        // Doom loads the carrier first, at minimum volume until set below.
        self.voices[v].car_volume = self.load_operator(op2, data.carrier, true);
        self.voices[v].mod_volume = self.load_operator(op1, data.modulator, !modulating);
        // Upper nybble: OPL3 output enables (both channels).
        self.reg(REG_FEEDBACK + index as u16, data.feedback | 0x30);
    }

    fn set_voice_volume(&mut self, v: usize, volume: u32) {
        self.voices[v].note_volume = volume;
        let Some((instr, iv)) = self.voices[v].current else { return };
        let Some(ch) = self.voices[v].channel else { return };
        let data = self.instr(instr).voices[iv];
        let midi_volume = 2 * (VOLUME_MAPPING[self.channels[ch].volume.min(127) as usize] + 1);
        let full_volume = (VOLUME_MAPPING[volume.min(127) as usize] * midi_volume) >> 9;
        let car_volume = 0x3f - full_volume.min(0x3f);
        let voice = &mut self.voices[v];
        if car_volume != voice.car_volume & 0x3f {
            voice.car_volume = car_volume | (voice.car_volume & 0xc0);
            let (op1, op2) = (voice.op1, voice.op2);
            let cv = voice.car_volume;
            self.reg(REG_LEVEL + op2, cv as u8);
            // Non-modulated feedback mode: both operators carry volume.
            if data.feedback & 0x01 != 0 && data.modulator.level != 0x3f {
                let mut mod_volume = (data.modulator.level as u32).max(car_volume);
                mod_volume |= self.voices[v].mod_volume & 0xc0;
                if mod_volume != self.voices[v].mod_volume {
                    self.voices[v].mod_volume = mod_volume;
                    self.reg(REG_LEVEL + op1, (mod_volume | (data.modulator.scale as u32 & 0xc0)) as u8);
                }
            }
        }
    }

    fn frequency_for_voice(&self, v: usize) -> u32 {
        let voice = &self.voices[v];
        let Some((instr, iv)) = voice.current else { return 0 };
        let ins = self.instr(instr);
        let gm = ins.voices[iv];
        let mut note = voice.note as i32;
        if ins.flags & FLAG_FIXED == 0 {
            note += gm.base_note_offset as i32;
        }
        while note < 0 {
            note += 12;
        }
        while note > 95 {
            note -= 12;
        }
        let bend = voice.channel.map_or(0, |c| self.channels[c].bend);
        let mut freq_index = 64 + 32 * note + bend;
        if iv != 0 {
            freq_index += (ins.fine_tuning as i32 / 2) - 64;
        }
        let freq_index = freq_index.max(0) as usize;
        if freq_index < 284 {
            return FREQUENCY_CURVE[freq_index] as u32;
        }
        let sub_index = (freq_index - 284) % (12 * 32);
        let octave = ((freq_index - 284) / (12 * 32)).min(7) as u32;
        FREQUENCY_CURVE[sub_index + 284] as u32 | (octave << 10)
    }

    fn update_voice_frequency(&mut self, v: usize) {
        let freq = self.frequency_for_voice(v);
        if self.voices[v].freq != freq {
            let index = self.voices[v].index as u16;
            self.reg(REG_FREQ_1 + index, (freq & 0xff) as u8);
            self.reg(REG_FREQ_2 + index, ((freq >> 8) | 0x20) as u8);
            self.voices[v].freq = freq;
        }
    }

    fn voice_key_on(&mut self, ch: usize, instr: InstrRef, instr_voice: usize, note: u32, key: u32, volume: u32) {
        let Some(v) = self.get_free_voice() else { return };
        self.voices[v].channel = Some(ch);
        self.voices[v].key = key;
        let ins = *self.instr(instr);
        self.voices[v].note = if ins.flags & FLAG_FIXED != 0 { ins.fixed_note as u32 } else { note };
        self.set_voice_instrument(v, instr, instr_voice);
        self.set_voice_volume(v, volume);
        self.voices[v].freq = 0;
        self.update_voice_frequency(v);
    }

    /// `ReplaceExistingVoice` (Doom 1.9): drop the second voice of a
    /// double-voice instrument, else the highest-numbered channel.
    fn replace_existing_voice(&mut self) {
        let mut result = 0;
        for i in 0..self.alloced.len() {
            let a = &self.voices[self.alloced[i]];
            let r = &self.voices[self.alloced[result]];
            if a.current.is_some_and(|(_, iv)| iv != 0) || a.channel >= r.channel {
                result = i;
            }
        }
        self.release_voice(result);
    }

    fn key_off(&mut self, ch: usize, key: u32) {
        let mut i = 0;
        while i < self.alloced.len() {
            let v = &self.voices[self.alloced[i]];
            if v.channel == Some(ch) && v.key == key {
                self.release_voice(i);
            } else {
                i += 1;
            }
        }
    }

    fn key_on(&mut self, ch: usize, key: u32, volume: u32) {
        if volume == 0 {
            self.key_off(ch, key);
            return;
        }
        let (instr, note) = if ch == PERCUSSION_CHANNEL {
            if !(35..=81).contains(&key) {
                return;
            }
            (InstrRef::Percussion(key as usize - 35), 60)
        } else {
            (InstrRef::Main(self.channels[ch].instrument), key)
        };
        let double_voice = self.instr(instr).flags & FLAG_2VOICE != 0;
        if self.free.is_empty() {
            self.replace_existing_voice();
        }
        self.voice_key_on(ch, instr, 0, note, key, volume);
        if double_voice {
            self.voice_key_on(ch, instr, 1, note, key, volume);
        }
    }

    fn set_channel_volume(&mut self, ch: usize, volume: u32, clip_start: bool) {
        self.channels[ch].volume_base = volume;
        let mut volume = volume.min(self.current_volume);
        if clip_start {
            volume = volume.min(self.start_volume);
        }
        self.channels[ch].volume = volume;
        for v in 0..NUM_VOICES {
            if self.voices[v].channel == Some(ch) {
                let nv = self.voices[v].note_volume;
                self.set_voice_volume(v, nv);
            }
        }
    }

    fn all_notes_off(&mut self, ch: usize) {
        let mut i = 0;
        while i < self.alloced.len() {
            if self.voices[self.alloced[i]].channel == Some(ch) {
                self.release_voice(i);
            } else {
                i += 1;
            }
        }
    }

    fn pitch_bend(&mut self, ch: usize, bend: u8) {
        // DMX only uses the MSB: MUS 0..255 -> MIDI MSB 0..127.
        self.channels[ch].bend = (bend >> 1) as i32 - 64;
        let mut updated = Vec::new();
        let mut others = Vec::new();
        for &v in &self.alloced.clone() {
            if self.voices[v].channel == Some(ch) {
                self.update_voice_frequency(v);
                updated.push(v);
            } else {
                others.push(v);
            }
        }
        others.extend(updated);
        self.alloced = others;
    }

    fn process(&mut self, ev: MusEvent) {
        match ev {
            MusEvent::ReleaseNote { channel, note } => self.key_off(channel as usize, note as u32),
            MusEvent::PlayNote { channel, note, volume } => {
                let ch = channel as usize;
                if let Some(v) = volume {
                    self.channels[ch].last_velocity = v;
                }
                let vel = self.channels[ch].last_velocity as u32;
                self.key_on(ch, note as u32, vel);
            }
            MusEvent::PitchBend { channel, bend } => self.pitch_bend(channel as usize, bend),
            // Only 11 (all notes off) has an effect in DMX; 10 (all sounds
            // off), mono/poly and reset are ignored.
            MusEvent::System { channel, controller } => {
                if controller == 11 {
                    self.all_notes_off(channel as usize);
                }
            }
            MusEvent::Controller { channel, controller, value } => {
                let ch = channel as usize;
                match controller {
                    0 => self.channels[ch].instrument = value as usize & 0x7f,
                    3 => self.set_channel_volume(ch, value as u32, true),
                    // Pan only matters in OPL3 mode; the rest are ignored by DMX.
                    _ => {}
                }
            }
            MusEvent::ScoreEnd | MusEvent::Nop => {}
        }
    }

    /// Fill `out` (interleaved stereo i16) with music, advancing the song.
    pub fn render(&mut self, out: &mut [i16]) {
        for frame in out.chunks_exact_mut(2) {
            if !self.paused {
                self.advance();
            }
            let s = self.opl.generate();
            frame[0] = s[0];
            frame[1] = s[1];
            self.position += 1;
        }
    }

    /// Process every event due at the current sample.
    fn advance(&mut self) {
        let Some(song) = self.song.clone() else { return };
        // `wait` counts in units of 1/(rate*140) s: each sample consumes 140.
        while self.wait < MUS_TICK_RATE as u64 {
            let Some(&(ev, delay)) = song.events.get(self.pos) else {
                self.finished = true;
                self.song = None;
                return;
            };
            self.pos += 1;
            if ev == MusEvent::ScoreEnd {
                if self.looping {
                    self.pos = 0;
                    self.start_volume = self.current_volume;
                    for ch in 0..NUM_CHANNELS {
                        self.init_channel(ch);
                    }
                    // Guard against empty songs looping forever in one sample.
                    self.wait += self.sample_rate as u64 * MUS_TICK_RATE as u64 / 200;
                } else {
                    self.stop();
                }
                return;
            }
            self.process(ev);
            self.wait += delay as u64 * self.sample_rate as u64;
        }
        self.wait -= MUS_TICK_RATE as u64;
    }
}
