//! YMF262 (OPL3) emulation, ported from Nuked OPL3 1.8 by Nuke.YKT
//! (LGPL-2.1-or-later), as vendored in Chocolate Doom.
//!
//! Scope: the two-operator FM voices DMX music uses — OPL2 compatibility
//! mode and OPL3 stereo routing for 2-op channels. Envelope, phase,
//! waveform, feedback, tremolo and vibrato follow Nuked sample for sample.
//! Not implemented: rhythm (percussion) mode, 4-op channels, timers, and
//! Nuked's one-sample channel output delay quirk (inaudible). Writes to
//! those registers are accepted and ignored.

use crate::opl_tables::{EXPROM, LOGSINROM};

/// Native sample rate of the chip (14.318 MHz / 288).
pub const OPL_RATE: u32 = 49716;
const RSM_FRAC: u32 = 10;

const MT: [u32; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 20, 24, 24, 30, 30];
const KSLROM: [u8; 16] = [0, 32, 40, 45, 48, 51, 53, 55, 56, 58, 59, 60, 61, 62, 63, 64];
const KSLSHIFT: [u8; 4] = [8, 1, 2, 0];
const EG_INCSTEP: [[u8; 4]; 4] = [[0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 1, 0], [1, 1, 1, 0]];
const AD_SLOT: [i8; 0x20] = [
    0, 1, 2, 3, 4, 5, -1, -1, 6, 7, 8, 9, 10, 11, -1, -1, 12, 13, 14, 15, 16, 17, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    -1,
];
const CH_SLOT: [usize; 18] = [0, 1, 2, 6, 7, 8, 12, 13, 14, 18, 19, 20, 24, 25, 26, 30, 31, 32];

const EG_ATTACK: u8 = 0;
const EG_DECAY: u8 = 1;
const EG_SUSTAIN: u8 = 2;
const EG_RELEASE: u8 = 3;

/// What feeds an operator's phase modulation input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModSrc {
    Zero,
    /// The slot's own feedback.
    Feedback,
    /// Another slot's output (always processed earlier in the sample).
    Slot(usize),
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    channel: usize,
    modulation: ModSrc,
    trem: bool,
    pg_reset: bool,
    pg_phase: u32,
    pg_phase_out: u16,
    out: i16,
    fbmod: i16,
    prout: i16,
    eg_rout: u16,
    eg_out: u16,
    key: u8,
    eg_gen: u8,
    eg_ksl: u8,
    reg_vib: bool,
    reg_type: bool,
    reg_ksr: u8,
    reg_mult: u8,
    reg_ksl: u8,
    reg_tl: u8,
    reg_sl: u8,
    reg_rr: u8,
    reg_wf: u8,
    eg_rates: [u8; 4],
}

impl Slot {
    fn new(channel: usize) -> Self {
        Slot {
            channel,
            modulation: ModSrc::Zero,
            trem: false,
            pg_reset: false,
            pg_phase: 0,
            pg_phase_out: 0,
            out: 0,
            fbmod: 0,
            prout: 0,
            eg_rout: 0x1ff,
            eg_out: 0x1ff,
            key: 0,
            eg_gen: EG_RELEASE,
            eg_ksl: 0,
            reg_vib: false,
            reg_type: false,
            reg_ksr: 0,
            reg_mult: 0,
            reg_ksl: 0,
            reg_tl: 0,
            reg_sl: 0,
            reg_rr: 0,
            reg_wf: 0,
            eg_rates: [0; 4],
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Channel {
    slots: [usize; 2],
    f_num: u16,
    block: u8,
    fb: u8,
    con: u8,
    ksv: u8,
    /// Output enables for the left/right DAC (all bits set = on).
    cha: u16,
    chb: u16,
}

/// One emulated chip.
pub struct Opl {
    slots: [Slot; 36],
    channels: [Channel; 18],
    wf: Box<[[u16; 1024]; 8]>,
    timer: u16,
    eg_timer: u64,
    eg_timerrem: u8,
    eg_state: u8,
    eg_add: u8,
    eg_timer_lo: u8,
    newm: u8,
    nts: u8,
    vibpos: u8,
    vibshift: u8,
    tremolo: u8,
    tremolopos: u8,
    tremoloshift: u8,
    // Linear resampler to the output rate (OPL3_GenerateResampled).
    rateratio: i32,
    samplecnt: i32,
    oldsamples: [i16; 2],
    samples: [i16; 2],
    // Buffered register writes (OPL3_WriteRegBuffered): (native sample
    // time, register, value).
    writebuf: std::collections::VecDeque<(u64, u16, u8)>,
    writebuf_samplecnt: u64,
    writebuf_lasttime: u64,
    /// The right mix of the previous sample (output one sample late).
    mix_right: i32,
}

/// `OPL_WRITEBUF_SIZE` / `OPL_WRITEBUF_DELAY`: queued writes are applied at
/// least 2 native samples apart, like the bus delays real hardware needs.
const WRITEBUF_SIZE: usize = 1024;
const WRITEBUF_DELAY: u64 = 2;

/// `OPL3_BuildWfTable`: the eight waveforms from the quarter-wave ROM.
fn build_waveforms() -> Box<[[u16; 1024]; 8]> {
    let mut wf = Box::new([[0u16; 1024]; 8]);
    let rom = |i: usize| LOGSINROM[i & 0xff];
    for p in 0..1024usize {
        let c2 = if p & 0x100 != 0 { rom((p & 0xff) ^ 0xff) } else { rom(p & 0xff) };
        let c6: u16 = if p & 0x200 != 0 { 0x8000 } else { 0 };
        let c5 = if c6 != 0 {
            0x1000
        } else if p & 0x80 != 0 {
            rom(((p ^ 0xff) << 1) & 0xff)
        } else {
            rom((p << 1) & 0xff)
        };
        wf[0][p] = c6 | c2;
        wf[1][p] = if c6 != 0 { 0x1000 } else { c2 };
        wf[2][p] = c2;
        wf[3][p] = if p & 0x100 != 0 { 0x1000 } else { rom(p & 0xff) };
        wf[4][p] = (if p & 0x300 == 0x100 { 0x8000 } else { 0 }) | c5;
        wf[5][p] = c5;
        wf[6][p] = c6;
        wf[7][p] = c6 | ((if c6 != 0 { ((p & 0x1ff) ^ 0x1ff) as u16 } else { p as u16 }) << 3);
    }
    wf
}

impl Opl {
    /// `OPL3_Reset`: a silent chip producing output at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        let mut slots = [Slot::new(0); 36];
        let mut channels = [Channel { slots: [0, 0], f_num: 0, block: 0, fb: 0, con: 0, ksv: 0, cha: 0xffff, chb: 0xffff }; 18];
        for (ch, c) in channels.iter_mut().enumerate() {
            let s = CH_SLOT[ch];
            c.slots = [s, s + 3];
            slots[s].channel = ch;
            slots[s + 3].channel = ch;
        }
        let mut opl = Opl {
            slots,
            channels,
            wf: build_waveforms(),
            timer: 0,
            eg_timer: 0,
            eg_timerrem: 0,
            eg_state: 0,
            eg_add: 0,
            eg_timer_lo: 0,
            newm: 0,
            nts: 0,
            vibpos: 0,
            vibshift: 1,
            tremolo: 0,
            tremolopos: 0,
            tremoloshift: 4,
            rateratio: ((sample_rate << RSM_FRAC) / OPL_RATE) as i32,
            samplecnt: 0,
            oldsamples: [0; 2],
            samples: [0; 2],
            writebuf: std::collections::VecDeque::with_capacity(WRITEBUF_SIZE),
            writebuf_samplecnt: 0,
            writebuf_lasttime: 0,
            mix_right: 0,
        };
        for ch in 0..18 {
            opl.setup_alg(ch);
        }
        opl
    }

    fn setup_alg(&mut self, ch: usize) {
        let [s0, s1] = self.channels[ch].slots;
        self.slots[s0].modulation = ModSrc::Feedback;
        self.slots[s1].modulation = if self.channels[ch].con & 1 == 0 { ModSrc::Slot(s0) } else { ModSrc::Zero };
    }

    // ---- envelope -----------------------------------------------------------

    fn update_ksl(&mut self, s: usize) {
        let c = &self.channels[self.slots[s].channel];
        let ksl = ((KSLROM[(c.f_num >> 6) as usize] as i16) << 2) - (((0x08 - c.block as i16) << 5) as i16);
        self.slots[s].eg_ksl = ksl.max(0) as u8;
    }

    fn envelope_calc(&mut self, s: usize) {
        let ksv = self.channels[self.slots[s].channel].ksv;
        let (eg_state, eg_add, eg_timer_lo, tremolo) = (self.eg_state, self.eg_add, self.eg_timer_lo, self.tremolo);
        let slot = &mut self.slots[s];
        // The C code shifts after integer promotion: a shift of 8 yields 0.
        let tl_ksl = ((slot.reg_tl as u16) << 2) + ((slot.eg_ksl as u16) >> KSLSHIFT[slot.reg_ksl as usize]);
        let trem = if slot.trem { tremolo as u16 } else { 0 };
        slot.eg_out = slot.eg_rout + tl_ksl + trem;

        let reset = slot.key != 0 && slot.eg_gen == EG_RELEASE;
        let reg_rate = if reset { slot.eg_rates[0] } else { slot.eg_rates[slot.eg_gen as usize] };
        slot.pg_reset = reset;
        let nonzero = reg_rate != 0;
        let ks = ksv >> ((slot.reg_ksr ^ 1) << 1);
        let rate = ks + (reg_rate << 2);
        let mut rate_hi = rate >> 2;
        let rate_lo = rate & 0x03;
        if rate_hi & 0x10 != 0 {
            rate_hi = 0x0f;
        }
        let eg_shift = rate_hi + eg_add;
        let mut shift = 0u8;
        if nonzero {
            if rate_hi < 12 {
                if eg_state != 0 {
                    shift = match eg_shift {
                        12 => 1,
                        13 => (rate_lo >> 1) & 0x01,
                        14 => rate_lo & 0x01,
                        _ => 0,
                    };
                }
            } else {
                shift = (rate_hi & 0x03) + EG_INCSTEP[rate_lo as usize][eg_timer_lo as usize];
                if shift & 0x04 != 0 {
                    shift = 0x03;
                }
                if shift == 0 {
                    shift = eg_state;
                }
            }
        }
        let mut eg_rout = slot.eg_rout;
        let mut eg_inc: i16 = 0;
        let mut eg_off = false;
        // Instant attack.
        if reset && rate_hi == 0x0f {
            eg_rout = 0x00;
        }
        // Envelope off.
        if slot.eg_rout & 0x1f8 == 0x1f8 {
            eg_off = true;
        }
        if slot.eg_gen != EG_ATTACK && !reset && eg_off {
            eg_rout = 0x1ff;
        }
        match slot.eg_gen {
            EG_ATTACK => {
                if slot.eg_rout == 0 {
                    slot.eg_gen = EG_DECAY;
                } else if slot.key != 0 && shift > 0 && rate_hi != 0x0f {
                    eg_inc = (!(slot.eg_rout as i16)) >> (4 - shift);
                }
            }
            EG_DECAY => {
                if (slot.eg_rout >> 4) as u8 == slot.reg_sl {
                    slot.eg_gen = EG_SUSTAIN;
                } else if !eg_off && !reset && shift > 0 {
                    eg_inc = 1 << (shift - 1);
                }
            }
            _ => {
                if !eg_off && !reset && shift > 0 {
                    eg_inc = 1 << (shift - 1);
                }
            }
        }
        slot.eg_rout = ((eg_rout as i16).wrapping_add(eg_inc) as u16) & 0x1ff;
        // Key off.
        if reset {
            slot.eg_gen = EG_ATTACK;
        }
        if slot.key == 0 {
            slot.eg_gen = EG_RELEASE;
        }
    }

    // ---- phase --------------------------------------------------------------

    fn phase_generate(&mut self, s: usize) {
        let c = self.channels[self.slots[s].channel];
        let slot = &mut self.slots[s];
        let mut f_num = c.f_num;
        if slot.reg_vib {
            let mut range = ((f_num >> 7) & 7) as i8;
            let vibpos = self.vibpos;
            if vibpos & 3 == 0 {
                range = 0;
            } else if vibpos & 1 != 0 {
                range >>= 1;
            }
            range >>= self.vibshift;
            if vibpos & 4 != 0 {
                range = -range;
            }
            f_num = f_num.wrapping_add(range as i16 as u16);
        }
        let basefreq = ((f_num as u32) << c.block) >> 1;
        let phaseinc = (basefreq * MT[slot.reg_mult as usize]) >> 1;
        let phase = (slot.pg_phase >> 9) as u16;
        if slot.pg_reset {
            slot.pg_phase = 0;
        }
        slot.pg_phase = slot.pg_phase.wrapping_add(phaseinc);
        slot.pg_phase_out = phase;
    }

    // ---- operator output ----------------------------------------------------

    fn slot_generate(&mut self, s: usize) {
        let m = match self.slots[s].modulation {
            ModSrc::Zero => 0,
            ModSrc::Feedback => self.slots[s].fbmod,
            ModSrc::Slot(o) => self.slots[o].out,
        };
        let slot = &mut self.slots[s];
        let phase = slot.pg_phase_out.wrapping_add(m as u16);
        let wf_data = self.wf[slot.reg_wf as usize][(phase & 0x3ff) as usize];
        let neg = ((wf_data as i16) >> 15) as u16;
        let mut level = (wf_data & 0x7fff) as u32 + ((slot.eg_out as u32) << 3);
        if level > 0x1fff {
            level = 0x1fff;
        }
        // As in C, shift after promotion: shifts past 15 must yield 0.
        slot.out = (((EXPROM[(level & 0xff) as usize] as u32) >> (level >> 8)) as u16 ^ neg) as i16;
    }

    fn calc_fb(&mut self, s: usize, fb: u8) {
        let slot = &mut self.slots[s];
        slot.fbmod = if fb != 0 { ((slot.prout as i32 + slot.out as i32) >> (0x09 - fb)) as i16 } else { 0 };
        slot.prout = slot.out;
    }

    fn process_slot(&mut self, s: usize, fb: u8) {
        self.calc_fb(s, fb);
        self.envelope_calc(s);
        self.phase_generate(s);
        self.slot_generate(s);
    }

    /// One native-rate sample (left, right).
    fn generate_native(&mut self) -> [i16; 2] {
        let mut mix = [0i32; 2];
        for ch in 0..18 {
            let c = self.channels[ch];
            self.process_slot(c.slots[0], c.fb);
            self.process_slot(c.slots[1], c.fb);
            let [s0, s1] = c.slots;
            // The chip's pipeline (Nuked OPL_QUIRK_CHANNELSAMPLEDELAY): the
            // left mix sees slots 15 and up one sample late; the right mix
            // (slots 33 and up late) comes out one sample later as a whole.
            let left = |s: usize| if s >= 15 { self.slots[s].prout } else { self.slots[s].out };
            let right = |s: usize| if s >= 33 { self.slots[s].prout } else { self.slots[s].out };
            // accm is an int16_t in the C code: the sum wraps.
            let (accl, accr): (i16, i16) = if c.con & 1 == 0 {
                (left(s1), right(s1))
            } else {
                (left(s0).wrapping_add(left(s1)), right(s0).wrapping_add(right(s1)))
            };
            mix[0] += (accl as u16 & c.cha) as i16 as i32;
            mix[1] += (accr as u16 & c.chb) as i16 as i32;
        }
        let out = [mix[0].clamp(-32768, 32767) as i16, self.mix_right.clamp(-32768, 32767) as i16];
        self.mix_right = mix[1];

        if self.timer & 0x3f == 0x3f {
            self.tremolopos += 1;
            if self.tremolopos == 210 {
                self.tremolopos = 0;
            }
        }
        self.tremolo = if self.tremolopos < 105 {
            self.tremolopos >> self.tremoloshift
        } else {
            (210 - self.tremolopos) >> self.tremoloshift
        };
        if self.timer & 0x3ff == 0x3ff {
            self.vibpos = (self.vibpos + 1) & 7;
        }
        self.timer = self.timer.wrapping_add(1);

        if self.eg_state != 0 {
            let low = (self.eg_timer & 0x1fff) as u32;
            self.eg_add = if low == 0 { 0 } else { low.trailing_zeros() as u8 + 1 };
            self.eg_timer_lo = (self.eg_timer & 0x3) as u8;
        }
        if self.eg_timerrem != 0 || self.eg_state != 0 {
            if self.eg_timer == 0xf_ffff_ffff {
                self.eg_timer = 0;
                self.eg_timerrem = 1;
            } else {
                self.eg_timer += 1;
                self.eg_timerrem = 0;
            }
        }
        self.eg_state ^= 1;

        while let Some(&(time, reg, v)) = self.writebuf.front() {
            if time > self.writebuf_samplecnt {
                break;
            }
            self.writebuf.pop_front();
            self.write(reg, v);
        }
        self.writebuf_samplecnt += 1;
        out
    }

    /// `OPL3_WriteRegBuffered`: queue a write to land at least
    /// `WRITEBUF_DELAY` native samples after the previous queued one (how
    /// Chocolate Doom drives the emulator).
    pub fn write_buffered(&mut self, reg: u16, v: u8) {
        if self.writebuf.len() == WRITEBUF_SIZE {
            // Ring full: the oldest write is applied now.
            let (time, r, old) = self.writebuf.pop_front().unwrap();
            self.write(r, old);
            self.writebuf_samplecnt = time;
        }
        let time = (self.writebuf_lasttime + WRITEBUF_DELAY).max(self.writebuf_samplecnt);
        self.writebuf.push_back((time, reg, v));
        self.writebuf_lasttime = time;
    }

    /// One sample at the rate given to [`Opl::new`] (`OPL3_GenerateResampled`).
    pub fn generate(&mut self) -> [i16; 2] {
        while self.samplecnt >= self.rateratio {
            self.oldsamples = self.samples;
            self.samples = self.generate_native();
            self.samplecnt -= self.rateratio;
        }
        let (r, c) = (self.rateratio, self.samplecnt);
        let lerp = |o: i16, n: i16| ((o as i32 * (r - c) + n as i32 * c) / r) as i16;
        let out = [lerp(self.oldsamples[0], self.samples[0]), lerp(self.oldsamples[1], self.samples[1])];
        self.samplecnt += 1 << RSM_FRAC;
        out
    }

    /// `OPL3_WriteReg`. Register 0x1xx addresses the second register array.
    pub fn write(&mut self, reg: u16, v: u8) {
        let high = ((reg >> 8) & 0x01) as usize;
        let regm = (reg & 0xff) as u8;
        let slot_of = |r: u8| -> Option<usize> {
            let a = AD_SLOT[(r & 0x1f) as usize];
            (a >= 0).then(|| 18 * high + a as usize)
        };
        match regm & 0xf0 {
            0x00 => {
                if high != 0 {
                    if regm & 0x0f == 0x05 {
                        self.newm = v & 0x01;
                    }
                } else if regm & 0x0f == 0x08 {
                    self.nts = (v >> 6) & 0x01;
                }
            }
            0x20 | 0x30 => {
                if let Some(s) = slot_of(regm) {
                    let slot = &mut self.slots[s];
                    slot.trem = (v >> 7) & 1 != 0;
                    slot.reg_vib = (v >> 6) & 1 != 0;
                    slot.reg_type = (v >> 5) & 1 != 0;
                    slot.eg_rates[2] = if slot.reg_type { 0 } else { slot.reg_rr };
                    slot.reg_ksr = (v >> 4) & 1;
                    slot.reg_mult = v & 0x0f;
                }
            }
            0x40 | 0x50 => {
                if let Some(s) = slot_of(regm) {
                    self.slots[s].reg_ksl = (v >> 6) & 0x03;
                    self.slots[s].reg_tl = v & 0x3f;
                    self.update_ksl(s);
                }
            }
            0x60 | 0x70 => {
                if let Some(s) = slot_of(regm) {
                    self.slots[s].eg_rates[0] = (v >> 4) & 0x0f;
                    self.slots[s].eg_rates[1] = v & 0x0f;
                }
            }
            0x80 | 0x90 => {
                if let Some(s) = slot_of(regm) {
                    let slot = &mut self.slots[s];
                    slot.reg_sl = (v >> 4) & 0x0f;
                    if slot.reg_sl == 0x0f {
                        slot.reg_sl = 0x1f;
                    }
                    slot.reg_rr = v & 0x0f;
                    slot.eg_rates[2] = if slot.reg_type { 0 } else { slot.reg_rr };
                    slot.eg_rates[3] = slot.reg_rr;
                }
            }
            0xe0 | 0xf0 => {
                if let Some(s) = slot_of(regm) {
                    self.slots[s].reg_wf = v & if self.newm == 0 { 0x03 } else { 0x07 };
                }
            }
            0xa0 => {
                if regm & 0x0f < 9 {
                    let ch = 9 * high + (regm & 0x0f) as usize;
                    let c = &mut self.channels[ch];
                    c.f_num = (c.f_num & 0x300) | v as u16;
                    c.ksv = (c.block << 1) | ((c.f_num >> (0x09 - self.nts)) & 0x01) as u8;
                    let [a, b] = c.slots;
                    self.update_ksl(a);
                    self.update_ksl(b);
                }
            }
            0xb0 => {
                if regm == 0xbd && high == 0 {
                    self.tremoloshift = (((v >> 7) ^ 1) << 1) + 2;
                    self.vibshift = ((v >> 6) & 0x01) ^ 1;
                    // Rhythm mode (bit 5) is not emulated.
                } else if regm & 0x0f < 9 {
                    let ch = 9 * high + (regm & 0x0f) as usize;
                    let c = &mut self.channels[ch];
                    c.f_num = (c.f_num & 0xff) | (((v & 0x03) as u16) << 8);
                    c.block = (v >> 2) & 0x07;
                    c.ksv = (c.block << 1) | ((c.f_num >> (0x09 - self.nts)) & 0x01) as u8;
                    let [a, b] = c.slots;
                    self.update_ksl(a);
                    self.update_ksl(b);
                    for s in [a, b] {
                        if v & 0x20 != 0 {
                            self.slots[s].key |= 1;
                        } else {
                            self.slots[s].key &= !1;
                        }
                    }
                }
            }
            0xc0 => {
                if regm & 0x0f < 9 {
                    let ch = 9 * high + (regm & 0x0f) as usize;
                    let newm = self.newm != 0;
                    let c = &mut self.channels[ch];
                    c.fb = (v & 0x0e) >> 1;
                    c.con = v & 0x01;
                    if newm {
                        c.cha = if (v >> 4) & 1 != 0 { 0xffff } else { 0 };
                        c.chb = if (v >> 5) & 1 != 0 { 0xffff } else { 0 };
                    } else {
                        c.cha = 0xffff;
                        c.chb = 0xffff;
                    }
                    self.setup_alg(ch);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain sine voice (modulator silent) produces a tone near the
    /// programmed frequency and decays to silence after key off.
    #[test]
    fn sine_voice() {
        let rate = 44100;
        let mut opl = Opl::new(rate);
        // Channel 0: modulator (op 0x00) silent, carrier (op 0x03) full.
        opl.write(0x20, 0x21); // sustained envelope, mult 1
        opl.write(0x23, 0x21);
        opl.write(0x40, 0x3f); // modulator total level max attenuation
        opl.write(0x43, 0x00); // carrier loud
        opl.write(0x60, 0xf0); // fast attack
        opl.write(0x63, 0xf0);
        opl.write(0x80, 0x0f); // sustain 0, release fast
        opl.write(0x83, 0x0f);
        opl.write(0xc0, 0x00);
        // f_num 0x241, block 4: f = 0x241 * 49716 / 2^(20-4) ~= 440 Hz
        opl.write(0xa0, 0x41);
        opl.write(0xb0, 0x20 | (4 << 2) | 0x02);
        let n = rate as usize / 2;
        let samples: Vec<i16> = (0..n).map(|_| opl.generate()[0]).collect();
        let crossings = samples.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        let hz = crossings as f64 * 2.0;
        assert!((hz - 440.0).abs() < 10.0, "frequency {hz}");
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 2000, "peak {peak}");
        // Key off: release to silence.
        opl.write(0xb0, (4 << 2) | 0x02);
        for _ in 0..rate {
            opl.generate();
        }
        let tail = (0..1000).map(|_| opl.generate()[0].unsigned_abs()).max().unwrap();
        assert!(tail < 10, "tail {tail}");
    }
}
