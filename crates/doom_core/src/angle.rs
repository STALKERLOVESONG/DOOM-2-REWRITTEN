//! Binary angle measurement (`angle_t`, tables.h).

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

pub const FINEANGLES: usize = 8192;
pub const FINEMASK: usize = FINEANGLES - 1;
/// `angle_t >> ANGLETOFINESHIFT` gives a fine-table index (0..8192).
pub const ANGLETOFINESHIFT: u32 = 19;

pub const ANG45: u32 = 0x2000_0000;
pub const ANG90: u32 = 0x4000_0000;
pub const ANG180: u32 = 0x8000_0000;
pub const ANG270: u32 = 0xC000_0000;
pub const ANG_MAX: u32 = 0xFFFF_FFFF;
pub const ANG1: u32 = ANG45 / 45;
pub const ANG60: u32 = ANG180 / 3;

pub const SLOPERANGE: u32 = 2048;
pub const SLOPEBITS: u32 = 11;
pub const DBITS: u32 = 16 - SLOPEBITS;

/// A full circle is 2^32. All arithmetic wraps, exactly as `unsigned` in C.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Angle(pub u32);

impl Angle {
    pub const ZERO: Angle = Angle(0);
    pub const A45: Angle = Angle(ANG45);
    pub const A90: Angle = Angle(ANG90);
    pub const A180: Angle = Angle(ANG180);
    pub const A270: Angle = Angle(ANG270);

    /// Fine-table index: `angle >> ANGLETOFINESHIFT`.
    #[inline]
    pub const fn fine(self) -> usize {
        (self.0 >> ANGLETOFINESHIFT) as usize
    }

    /// Map-format angle in whole degrees (THINGS lump) -> BAM, as in `P_SpawnMapThing`:
    /// `ANG45 * (mthing->angle / 45)`. Note the integer division: vanilla snaps
    /// thing angles to multiples of 45 degrees.
    #[inline]
    pub const fn from_mapthing_degrees(deg: i16) -> Angle {
        Angle(ANG45.wrapping_mul((deg as i32 / 45) as u32))
    }

    /// Lossy conversion for presentation only.
    #[inline]
    pub fn to_radians_f32(self) -> f32 {
        (self.0 as f64 * (std::f64::consts::TAU / 4_294_967_296.0)) as f32
    }
}

impl Add for Angle {
    type Output = Angle;
    #[inline]
    fn add(self, rhs: Angle) -> Angle {
        Angle(self.0.wrapping_add(rhs.0))
    }
}
impl Sub for Angle {
    type Output = Angle;
    #[inline]
    fn sub(self, rhs: Angle) -> Angle {
        Angle(self.0.wrapping_sub(rhs.0))
    }
}
impl AddAssign for Angle {
    #[inline]
    fn add_assign(&mut self, rhs: Angle) {
        self.0 = self.0.wrapping_add(rhs.0);
    }
}
impl SubAssign for Angle {
    #[inline]
    fn sub_assign(&mut self, rhs: Angle) {
        self.0 = self.0.wrapping_sub(rhs.0);
    }
}
impl Neg for Angle {
    type Output = Angle;
    #[inline]
    fn neg(self) -> Angle {
        Angle(self.0.wrapping_neg())
    }
}

impl fmt::Debug for Angle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Angle({:#010x} ≈ {:.3}°)", self.0, self.0 as f64 * 360.0 / 4_294_967_296.0)
    }
}
