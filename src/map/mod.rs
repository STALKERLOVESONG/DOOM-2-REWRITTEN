pub mod collision;
pub mod data;
pub mod mesh_builder;
pub mod shapes;

use bevy::math::Vec3;

/// DOOM map units -> Bevy metres (player height 56 units = 1.75 m).
pub const DOOM_SCALE: f32 = 1.0 / 32.0;

/// DOOM (x east, y north, z up) -> Bevy (x east, y up, -z north). A proper
/// rotation, so triangle winding is preserved.
pub fn to_world(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x * DOOM_SCALE, z * DOOM_SCALE, -y * DOOM_SCALE)
}
