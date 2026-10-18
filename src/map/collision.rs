//! Sector lookup and circle-vs-linedef collision for the player.

use bevy::prelude::{Resource, Vec2};

use super::data::{flags, MapData};
use super::shapes::SectorShape;

pub const PLAYER_RADIUS: f32 = 16.0;
pub const PLAYER_HEIGHT: f32 = 56.0;
pub const MAX_STEP: f32 = 24.0;

pub struct CollisionLine {
    pub a: Vec2,
    pub b: Vec2,
    pub front_sector: usize,
    pub back_sector: Option<usize>,
    pub impassable: bool,
}

#[derive(Resource)]
pub struct CollisionMap {
    pub lines: Vec<CollisionLine>,
    /// (floor, ceiling) per sector.
    pub heights: Vec<(f32, f32)>,
    shapes: Vec<SectorShape>,
}

impl CollisionMap {
    pub fn new(map: &MapData, shapes: Vec<SectorShape>) -> Self {
        let lines = map
            .linedefs
            .iter()
            .filter_map(|ld| {
                let front = ld.front?;
                Some(CollisionLine {
                    a: map.vertex(ld.v1),
                    b: map.vertex(ld.v2),
                    front_sector: map.sector_of_side(front),
                    back_sector: ld.back.map(|b| map.sector_of_side(b)),
                    impassable: ld.flags & flags::IMPASSABLE != 0,
                })
            })
            .collect();
        let heights = map
            .sectors
            .iter()
            .map(|s| (s.floor as f32, s.ceiling as f32))
            .collect();
        Self { lines, heights, shapes }
    }

    pub fn sector_at(&self, p: Vec2) -> Option<usize> {
        self.shapes.iter().find(|s| s.contains(p)).map(|s| s.sector)
    }

    fn blocks(&self, l: &CollisionLine, player_on_back_side: bool, floor_z: f32) -> bool {
        let Some(back) = l.back_sector else { return true };
        if l.impassable {
            return true;
        }
        let far = if player_on_back_side { l.front_sector } else { back };
        let near = if player_on_back_side { back } else { l.front_sector };
        let (far_floor, far_ceil) = self.heights[far];
        let near_ceil = self.heights[near].1;
        far_floor - floor_z > MAX_STEP || far_ceil.min(near_ceil) - far_floor.max(floor_z) < PLAYER_HEIGHT
    }

    /// Move a circle from `from` towards `to`, sliding along blocking lines.
    pub fn slide_move(&self, from: Vec2, to: Vec2, floor_z: f32) -> Vec2 {
        let mut p = to;
        let (lo, hi) = (
            from.min(to) - Vec2::splat(PLAYER_RADIUS + 1.0),
            from.max(to) + Vec2::splat(PLAYER_RADIUS + 1.0),
        );
        for _ in 0..4 {
            let mut pushed = false;
            for l in &self.lines {
                // Cheap bounding-box reject.
                if l.a.x.max(l.b.x) < lo.x || l.a.x.min(l.b.x) > hi.x || l.a.y.max(l.b.y) < lo.y || l.a.y.min(l.b.y) > hi.y {
                    continue;
                }
                let ab = l.b - l.a;
                let len2 = ab.length_squared();
                if len2 < 1e-6 {
                    continue;
                }
                // Left of a->b is the back side (front sidedef is on the right).
                let n_left = Vec2::new(-ab.y, ab.x) / len2.sqrt();
                let from_side = n_left.dot(from - l.a);
                let on_back = from_side >= 0.0;
                if !self.blocks(l, on_back, floor_z) {
                    continue;
                }
                let n = if on_back { n_left } else { -n_left };
                let t = (p - l.a).dot(ab) / len2;
                if (0.0..=1.0).contains(&t) {
                    let sd = n.dot(p - l.a); // signed distance on the side we started from
                    if sd < PLAYER_RADIUS {
                        p += n * (PLAYER_RADIUS - sd);
                        pushed = true;
                    }
                } else {
                    let cp = l.a + ab * t.clamp(0.0, 1.0);
                    let d = p.distance(cp);
                    if d < PLAYER_RADIUS && d > 1e-4 {
                        p = cp + (p - cp) / d * PLAYER_RADIUS;
                        pushed = true;
                    }
                }
            }
            if !pushed {
                break;
            }
        }
        p
    }
}
