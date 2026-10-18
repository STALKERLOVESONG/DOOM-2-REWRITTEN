//! The screen melt (f_wipe.c `wipe_Melt`): the old frame slides down in
//! jittered two-pixel columns to reveal the new one.
//!
//! D_Display starts a wipe whenever the game state changes (and on every
//! level load), with the last displayed frame as the start screen and the
//! first frame of the new state as the end screen. Vanilla then blocks the
//! game loop until the melt is done; [`Session`](crate::Session) does the
//! same by not running game tics while a wipe is active.
//!
//! The melt is tic-driven: [`Melt::tick`] is one `wipe_doMelt` step at
//! 35 Hz. Composition is stateless, so a frame can be drawn any number of
//! times between tics, and at any resolution: columns and rows are mapped
//! onto vanilla's 160x200 melt grid, so a truecolor or high-resolution
//! framebuffer melts with the same shape and timing.

use doom_core::DoomRandom;

/// Vanilla melts columns in pairs of pixels: 320 / 2.
pub const MELT_COLUMNS: usize = 160;
/// The height the melt offsets are measured in (`SCREENHEIGHT`).
pub const MELT_HEIGHT: i32 = 200;

#[derive(Clone, Debug)]
pub struct Melt {
    /// Per column pair: < 0 not started yet (counts up to 0), otherwise how
    /// many rows of the end screen are uncovered.
    y: [i32; MELT_COLUMNS],
}

impl Melt {
    /// `wipe_initMelt`. Consumes `M_Random` exactly as vanilla does: once
    /// for the first column, then once per remaining *pixel* column (vanilla
    /// sets up 320 offsets but only melts the first 160).
    pub fn new(rng: &mut DoomRandom) -> Melt {
        let mut y = [0; MELT_COLUMNS];
        y[0] = -(rng.m_random() % 16);
        let mut prev = y[0];
        for i in 1..MELT_COLUMNS * 2 {
            let r = rng.m_random() % 3 - 1;
            let mut v = prev + r;
            if v > 0 {
                v = 0;
            } else if v == -16 {
                v = -15;
            }
            if i < MELT_COLUMNS {
                y[i] = v;
            }
            prev = v;
        }
        Melt { y }
    }

    /// Offsets for a melt built elsewhere (tests, replays).
    pub fn from_offsets(y: [i32; MELT_COLUMNS]) -> Melt {
        Melt { y }
    }

    pub fn offsets(&self) -> &[i32; MELT_COLUMNS] {
        &self.y
    }

    /// One tic of `wipe_doMelt`. Returns true when every column has left the
    /// screen (the wipe is over).
    pub fn tick(&mut self) -> bool {
        let mut done = true;
        for y in &mut self.y {
            if *y < 0 {
                *y += 1;
                done = false;
            } else if *y < MELT_HEIGHT {
                // Accelerate over the first 16 rows, then fall 8 rows a tic.
                let dy = if *y < 16 { *y + 1 } else { 8 };
                *y = (*y + dy).min(MELT_HEIGHT);
                done = false;
            }
        }
        done
    }

    pub fn is_done(&self) -> bool {
        self.y.iter().all(|&y| y >= MELT_HEIGHT)
    }

    /// Draw the wipe into `out`: in each column the top `y` rows show `end`,
    /// below them `start` is drawn shifted down by `y`. Buffers are
    /// row-major `width * height`, any pixel type (palette indices or RGBA
    /// words) and any size.
    pub fn composite<T: Copy>(&self, start: &[T], end: &[T], out: &mut [T], width: usize, height: usize) {
        assert!(start.len() >= width * height && end.len() >= width * height && out.len() >= width * height);
        for x in 0..width {
            let col = (x * MELT_COLUMNS / width).min(MELT_COLUMNS - 1);
            // Melt rows are 1/200 of the screen whatever its height.
            let shift = (self.y[col].clamp(0, MELT_HEIGHT) as usize * height) / MELT_HEIGHT as usize;
            for row in 0..shift {
                out[row * width + x] = end[row * width + x];
            }
            for row in shift..height {
                out[row * width + x] = start[(row - shift) * width + x];
            }
        }
    }
}

/// A running wipe: the two captured frames and the melt state.
#[derive(Clone, Debug)]
pub struct Wipe {
    pub melt: Melt,
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

impl Wipe {
    pub fn new(start: Vec<u8>, end: Vec<u8>, rng: &mut DoomRandom) -> Wipe {
        Wipe { melt: Melt::new(rng), start, end }
    }

    /// Advance one tic; true when finished.
    pub fn tick(&mut self) -> bool {
        self.melt.tick()
    }

    pub fn draw(&self, out: &mut [u8], width: usize, height: usize) {
        self.melt.composite(&self.start, &self.end, out, width, height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_matches_vanilla_rules() {
        let mut rng = DoomRandom::new();
        let m = Melt::new(&mut rng);
        // 1 + 319 M_Random calls, as in wipe_initMelt with width 320.
        assert_eq!(rng.menu_index, (MELT_COLUMNS * 2) as u8);
        let y = m.offsets();
        // rndtable[1] = 8: the first column waits 8 tics.
        assert_eq!(y[0], -8);
        for w in y.windows(2) {
            assert!((w[1] - w[0]).abs() <= 1 || w[1] == -15, "jitter is at most one tic: {w:?}");
        }
        assert!(y.iter().all(|&v| (-15..=0).contains(&v)), "{y:?}");
    }

    #[test]
    fn column_speed_follows_wipe_do_melt() {
        let mut m = Melt::from_offsets([0; MELT_COLUMNS]);
        let mut seen = vec![];
        while !m.tick() {
            seen.push(m.offsets()[0]);
        }
        // 0 -> 1 -> 3 -> 7 -> 15 -> 31, then 8 rows a tic, clamped at 200.
        assert_eq!(&seen[..6], &[1, 3, 7, 15, 31, 39]);
        assert_eq!(*seen.last().unwrap(), MELT_HEIGHT);
        assert!(m.is_done());
    }

    #[test]
    fn whole_melt_takes_about_a_second() {
        let mut m = Melt::new(&mut DoomRandom::new());
        let tics = (1..).find(|_| m.tick()).unwrap();
        assert!((25..=45).contains(&tics), "{tics} tics");
    }

    #[test]
    fn composite_slides_start_over_end() {
        let (w, h) = (320, 200);
        let start = vec![1u8; w * h];
        let end = vec![2u8; w * h];
        let mut out = vec![0u8; w * h];
        let mut y = [0; MELT_COLUMNS];
        y[0] = 50;
        y[1] = -3;
        let m = Melt::from_offsets(y);
        m.composite(&start, &end, &mut out, w, h);
        // Pixel columns 0 and 1 show 50 rows of the new frame.
        for x in 0..2 {
            assert!((0..50).all(|r| out[r * w + x] == 2));
            assert!((50..200).all(|r| out[r * w + x] == 1));
        }
        // Columns not yet started show the old frame unmoved.
        assert!((0..200).all(|r| out[r * w + 2] == 1 && out[r * w + 4] == 1));
    }

    #[test]
    fn composite_scales_to_high_resolution() {
        let (w, h) = (1280, 800);
        let start: Vec<u32> = (0..w * h).map(|i| (i / w) as u32).collect();
        let end = vec![u32::MAX; w * h];
        let mut out = vec![0u32; w * h];
        let mut y = [0; MELT_COLUMNS];
        y[10] = 100; // half the screen
        Melt::from_offsets(y).composite(&start, &end, &mut out, w, h);
        // Column pair 10 covers pixels 80..88 at 4x.
        assert_eq!(out[399 * w + 80], u32::MAX);
        assert_eq!(out[400 * w + 87], 0, "old frame's row 0 now at mid-screen");
        assert_eq!(out[400 * w + 88], 400, "the next pair has not moved");
    }
}
