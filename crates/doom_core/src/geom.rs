//! Small geometric primitives shared by the playsim and renderer.

use crate::angle::{Angle, ANG180, ANG270, ANG90, SLOPERANGE};
use crate::fixed::Fixed;
use crate::tables::TANTOANGLE;

/// `SlopeDiv` (tables.c). Operates on *unsigned* values: callers pass `int`s
/// that C converts implicitly, so `INT_MIN` becomes `0x8000_0000`.
#[inline]
pub fn slope_div(num: u32, den: u32) -> usize {
    if den < 512 {
        return SLOPERANGE as usize;
    }
    let ans = (num << 3) / (den >> 8);
    if ans <= SLOPERANGE {
        ans as usize
    } else {
        SLOPERANGE as usize
    }
}

/// `R_PointToAngle` with the view origin already subtracted: the angle of the
/// vector (`x`, `y`). Reproduces the octant folding, including the
/// `ANG90 - 1 - ...` asymmetries and the behaviour for `INT_MIN` components.
pub fn point_to_angle(x: Fixed, y: Fixed) -> Angle {
    let (mut x, mut y) = (x.0, y.0);
    if x == 0 && y == 0 {
        return Angle(0);
    }
    let t = |n: i32, d: i32| TANTOANGLE[slope_div(n as u32, d as u32)];
    let a = if x >= 0 {
        if y >= 0 {
            if x > y {
                t(y, x) // octant 0
            } else {
                (ANG90 - 1).wrapping_sub(t(x, y)) // octant 1
            }
        } else {
            y = y.wrapping_neg();
            if x > y {
                t(y, x).wrapping_neg() // octant 8
            } else {
                ANG270.wrapping_add(t(x, y)) // octant 7
            }
        }
    } else {
        x = x.wrapping_neg();
        if y >= 0 {
            if x > y {
                (ANG180 - 1).wrapping_sub(t(y, x)) // octant 3
            } else {
                ANG90.wrapping_add(t(x, y)) // octant 2
            }
        } else {
            y = y.wrapping_neg();
            if x > y {
                ANG180.wrapping_add(t(y, x)) // octant 4
            } else {
                (ANG270 - 1).wrapping_sub(t(x, y)) // octant 5
            }
        }
    };
    Angle(a)
}

/// `R_PointToAngle2(x1, y1, x2, y2)`: angle from point 1 to point 2.
///
/// The original also overwrites the renderer's global `viewx`/`viewy` as a
/// side effect. That only matters if gameplay code runs between
/// `R_SetupFrame` and drawing, which our architecture never does, so the side
/// effect is intentionally not reproduced.
#[inline]
pub fn point_to_angle2(x1: Fixed, y1: Fixed, x2: Fixed, y2: Fixed) -> Angle {
    point_to_angle(x2 - x1, y2 - y1)
}

/// `P_AproxDistance`: the octagonal distance approximation
/// `max + min/2` (on absolute values, using C `abs`).
#[inline]
pub fn aprox_distance(dx: Fixed, dy: Fixed) -> Fixed {
    let dx = dx.abs();
    let dy = dy.abs();
    if dx < dy {
        dx + dy - (dx >> 1)
    } else {
        dx + dy - (dy >> 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinal_angles() {
        let one = Fixed::ONE;
        assert_eq!(point_to_angle(one, Fixed::ZERO), Angle(0));
        assert_eq!(point_to_angle(Fixed::ZERO, one), Angle(ANG90 - 1 - 0)); // octant 1, tantoangle[0] = 0
        assert_eq!(point_to_angle(-one, Fixed::ZERO).0, ANG180 - 1);
        assert_eq!(point_to_angle(one, one).0, ANG90 - 1 - 0x2000_0000);
    }
}
