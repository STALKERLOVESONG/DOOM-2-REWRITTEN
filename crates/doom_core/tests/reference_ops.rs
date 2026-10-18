//! Differential test against the original C implementations.
//!
//! `fixtures/core_ops.txt` is produced by `tools/refharness/gen_core_fixtures.sh`,
//! which compiles Chocolate Doom's m_fixed.c/tables.c plus verbatim copies of
//! R_PointToAngle and P_AproxDistance. Every result must match bit-for-bit.

use doom_core::fixed::{fixed_div, fixed_mul};
use doom_core::geom::{aprox_distance, point_to_angle};
use doom_core::Fixed;

#[test]
fn matches_reference_c() {
    let data = include_str!("fixtures/core_ops.txt");
    let mut counts = [0usize; 4];
    for (n, line) in data.lines().enumerate() {
        let f: Vec<&str> = line.split(' ').collect();
        let a = Fixed(f[1].parse().unwrap());
        let b = Fixed(f[2].parse().unwrap());
        let ctx = || format!("line {}: {line}", n + 1);
        match f[0] {
            "mul" => {
                assert_eq!(fixed_mul(a, b).0, f[3].parse::<i32>().unwrap(), "{}", ctx());
                counts[0] += 1;
            }
            "div" => {
                let expected = if f[3] == "CRASH" { i32::MIN } else { f[3].parse().unwrap() };
                assert_eq!(fixed_div(a, b).0, expected, "{}", ctx());
                counts[1] += 1;
            }
            "ang" => {
                assert_eq!(point_to_angle(a, b).0, f[3].parse::<u32>().unwrap(), "{}", ctx());
                counts[2] += 1;
            }
            "dist" => {
                assert_eq!(aprox_distance(a, b).0, f[3].parse::<i32>().unwrap(), "{}", ctx());
                counts[3] += 1;
            }
            op => panic!("unknown op {op}"),
        }
    }
    assert!(counts.iter().all(|&c| c > 1000), "fixture too small: {counts:?}");
}
