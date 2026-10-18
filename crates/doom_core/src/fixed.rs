//! 16.16 fixed point (`fixed_t`, m_fixed.c).

use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Shl, Shr, Sub, SubAssign};

pub const FRACBITS: i32 = 16;
pub const FRACUNIT: i32 = 1 << FRACBITS;

/// Doom's `fixed_t`: a signed 32-bit integer with 16 fractional bits.
///
/// Operator semantics mirror the C source:
/// - `+`, `-`, unary `-`, `* i32`: wrapping integer arithmetic.
/// - `Fixed * Fixed`: `FixedMul`.
/// - `Fixed / Fixed`: `FixedDiv` (saturating overflow check, integer divide).
/// - `Fixed / i32`: plain C integer division (truncates toward zero).
/// - `>>` / `<<`: arithmetic shifts on the raw value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Fixed(pub i32);

impl Fixed {
    pub const ZERO: Fixed = Fixed(0);
    pub const ONE: Fixed = Fixed(FRACUNIT);
    pub const MAX: Fixed = Fixed(i32::MAX);
    pub const MIN: Fixed = Fixed(i32::MIN);

    /// `n << FRACBITS` (wrapping, as in C).
    #[inline]
    pub const fn from_int(n: i32) -> Self {
        Fixed(n.wrapping_shl(FRACBITS as u32))
    }

    /// `x >> FRACBITS`: arithmetic shift, i.e. floor toward negative infinity.
    #[inline]
    pub const fn to_int(self) -> i32 {
        self.0 >> FRACBITS
    }

    #[inline]
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// C `abs()` on the raw value. `abs(INT_MIN)` stays `INT_MIN`, as on x86.
    #[inline]
    pub const fn abs(self) -> Self {
        Fixed(self.0.wrapping_abs())
    }

    /// Lossy conversion for presentation only. Never feed this back into the simulation.
    #[inline]
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / FRACUNIT as f32
    }

    #[inline]
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / FRACUNIT as f64
    }
}

/// `FixedMul`: 64-bit product shifted down. The result is truncated to 32 bits
/// exactly like the C cast from `int64_t` to `fixed_t`.
#[inline]
pub const fn fixed_mul(a: Fixed, b: Fixed) -> Fixed {
    Fixed(((a.0 as i64 * b.0 as i64) >> FRACBITS) as i32)
}

/// `FixedDiv`: saturates to `INT_MIN`/`INT_MAX` when the quotient would not fit
/// (the `abs(a) >> 14 >= abs(b)` test, which also covers division by zero),
/// otherwise performs an exact 64-bit integer division truncated toward zero.
///
/// linuxdoom-1.10's `FixedDiv2` used `double`; the DOS executable used an
/// integer `idiv`, which is what we reproduce (same as Chocolate Doom).
#[inline]
pub const fn fixed_div(a: Fixed, b: Fixed) -> Fixed {
    if (a.0.wrapping_abs() >> 14) >= b.0.wrapping_abs() {
        if (a.0 ^ b.0) < 0 {
            Fixed(i32::MIN)
        } else {
            Fixed(i32::MAX)
        }
    } else if b.0 == 0 {
        // Only reachable for a == INT_MIN: abs(INT_MIN) is negative, so the
        // overflow test above does not catch the division by zero. The
        // original traps (DOS divide error / linuxdoom I_Error). Deliberate
        // deviation: saturate like the overflow path instead of aborting.
        Fixed(i32::MIN)
    } else {
        Fixed((((a.0 as i64) << FRACBITS) / b.0 as i64) as i32)
    }
}

impl Add for Fixed {
    type Output = Fixed;
    #[inline]
    fn add(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.wrapping_add(rhs.0))
    }
}
impl Sub for Fixed {
    type Output = Fixed;
    #[inline]
    fn sub(self, rhs: Fixed) -> Fixed {
        Fixed(self.0.wrapping_sub(rhs.0))
    }
}
impl AddAssign for Fixed {
    #[inline]
    fn add_assign(&mut self, rhs: Fixed) {
        *self = *self + rhs;
    }
}
impl SubAssign for Fixed {
    #[inline]
    fn sub_assign(&mut self, rhs: Fixed) {
        *self = *self - rhs;
    }
}
impl Neg for Fixed {
    type Output = Fixed;
    #[inline]
    fn neg(self) -> Fixed {
        Fixed(self.0.wrapping_neg())
    }
}
impl Mul for Fixed {
    type Output = Fixed;
    #[inline]
    fn mul(self, rhs: Fixed) -> Fixed {
        fixed_mul(self, rhs)
    }
}
impl Mul<i32> for Fixed {
    type Output = Fixed;
    #[inline]
    fn mul(self, rhs: i32) -> Fixed {
        Fixed(self.0.wrapping_mul(rhs))
    }
}
impl Div for Fixed {
    type Output = Fixed;
    #[inline]
    fn div(self, rhs: Fixed) -> Fixed {
        fixed_div(self, rhs)
    }
}
impl Div<i32> for Fixed {
    type Output = Fixed;
    /// Plain C integer division of the raw value (truncates toward zero).
    #[inline]
    fn div(self, rhs: i32) -> Fixed {
        Fixed(self.0.wrapping_div(rhs))
    }
}
impl Shr<u32> for Fixed {
    type Output = Fixed;
    #[inline]
    fn shr(self, rhs: u32) -> Fixed {
        Fixed(self.0 >> rhs)
    }
}
impl Shl<u32> for Fixed {
    type Output = Fixed;
    #[inline]
    fn shl(self, rhs: u32) -> Fixed {
        Fixed(self.0.wrapping_shl(rhs))
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fixed({:#x} ≈ {:.4})", self.0, self.to_f64())
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.4}", self.to_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_basic() {
        assert_eq!(Fixed::from_int(3) * Fixed::from_int(4), Fixed::from_int(12));
        assert_eq!(Fixed(FRACUNIT / 2) * Fixed::from_int(-3), Fixed(-3 * FRACUNIT / 2));
        // Truncation of the 64-bit product back to 32 bits wraps.
        assert_eq!(Fixed::from_int(30000) * Fixed::from_int(30000), Fixed((900_000_000i64 << 16) as i32));
    }

    #[test]
    fn div_saturates() {
        assert_eq!(Fixed::ONE / Fixed::ZERO, Fixed::MAX);
        assert_eq!(-Fixed::ONE / Fixed::ZERO, Fixed::MIN);
        // INT_MIN: abs() wraps to INT_MIN, so the overflow test does not fire.
        assert_eq!(Fixed::MIN / Fixed::from_int(-2), Fixed((((i32::MIN as i64) << 16) / (-2i64 << 16)) as i32));
    }

    #[test]
    fn to_int_floors() {
        assert_eq!(Fixed(-1).to_int(), -1);
        assert_eq!(Fixed(FRACUNIT - 1).to_int(), 0);
    }
}
