//! Trigonometric lookup tables (tables.c). Values are verbatim; see
//! `tools/gen_tables.py`.

use crate::angle::{Angle, FINEANGLES};
use crate::fixed::Fixed;
pub use crate::tables_data::{FINESINE, FINETANGENT, TANTOANGLE};

/// `finesine[i]` for a fine index (0..10240; the table covers 5/4 of a circle so
/// that `finecosine` can alias into it).
#[inline]
pub fn finesine(i: usize) -> Fixed {
    Fixed(FINESINE[i])
}

/// `finecosine[i]`, which in C is `&finesine[FINEANGLES/4]`.
#[inline]
pub fn finecosine(i: usize) -> Fixed {
    Fixed(FINESINE[i + FINEANGLES / 4])
}

/// `finetangent[i]` for 0..4096 (covers -90..+90 degrees).
#[inline]
pub fn finetangent(i: usize) -> Fixed {
    Fixed(FINETANGENT[i])
}

#[inline]
pub fn sine(a: Angle) -> Fixed {
    finesine(a.fine())
}

#[inline]
pub fn cosine(a: Angle) -> Fixed {
    finecosine(a.fine())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_shapes() {
        assert_eq!(FINESINE.len(), 10240);
        assert_eq!(FINETANGENT.len(), 4096);
        assert_eq!(TANTOANGLE.len(), 2049);
        // Known anchors from the original table.
        assert_eq!(FINESINE[0], 25);
        assert_eq!(FINESINE[2048], 65535); // sin(90°) is not quite FRACUNIT
        assert_eq!(TANTOANGLE[2048], 0x2000_0000);
        assert_eq!(finecosine(0), finesine(2048));
    }
}
