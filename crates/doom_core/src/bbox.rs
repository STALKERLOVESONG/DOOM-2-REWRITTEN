//! Axis-aligned bounding boxes in fixed point (m_bbox.c).

use crate::fixed::Fixed;

pub const BOXTOP: usize = 0;
pub const BOXBOTTOM: usize = 1;
pub const BOXLEFT: usize = 2;
pub const BOXRIGHT: usize = 3;

/// `fixed_t box[4]` indexed by `BOXTOP`, `BOXBOTTOM`, `BOXLEFT`, `BOXRIGHT`.
/// Kept as an array (not named fields) because several algorithms index it
/// with computed values (e.g. `R_CheckBBox`'s `checkcoord` table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BBox(pub [Fixed; 4]);

impl Default for BBox {
    fn default() -> Self {
        Self::cleared()
    }
}

impl BBox {
    /// `M_ClearBox`: an inverted box that any `add_point` will replace.
    pub const fn cleared() -> Self {
        let mut b = [Fixed::ZERO; 4];
        b[BOXTOP] = Fixed::MIN;
        b[BOXRIGHT] = Fixed::MIN;
        b[BOXBOTTOM] = Fixed::MAX;
        b[BOXLEFT] = Fixed::MAX;
        BBox(b)
    }

    /// `M_AddToBox`. Note the `else if`: as in the original, a point that sets
    /// the left edge is not tested against the right edge (and likewise for
    /// bottom/top). With a freshly cleared box this means the first point only
    /// sets left/bottom; the original has the same behaviour.
    pub fn add_point(&mut self, x: Fixed, y: Fixed) {
        if x < self.0[BOXLEFT] {
            self.0[BOXLEFT] = x;
        } else if x > self.0[BOXRIGHT] {
            self.0[BOXRIGHT] = x;
        }
        if y < self.0[BOXBOTTOM] {
            self.0[BOXBOTTOM] = y;
        } else if y > self.0[BOXTOP] {
            self.0[BOXTOP] = y;
        }
    }

    #[inline]
    pub fn top(&self) -> Fixed {
        self.0[BOXTOP]
    }
    #[inline]
    pub fn bottom(&self) -> Fixed {
        self.0[BOXBOTTOM]
    }
    #[inline]
    pub fn left(&self) -> Fixed {
        self.0[BOXLEFT]
    }
    #[inline]
    pub fn right(&self) -> Fixed {
        self.0[BOXRIGHT]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_point_quirk() {
        let mut b = BBox::cleared();
        b.add_point(Fixed(5), Fixed(7));
        assert_eq!(b.left(), Fixed(5));
        assert_eq!(b.bottom(), Fixed(7));
        // The `else if` means right/top are still at their cleared values.
        assert_eq!(b.right(), Fixed::MIN);
        assert_eq!(b.top(), Fixed::MIN);
    }
}
