//! ECS components shared by the systems.

use bevy::prelude::*;

/// Eye height above the floor, in DOOM map units.
pub const VIEW_HEIGHT: f32 = 41.0;

/// Authoritative player simulation state, in DOOM map units. Updated at 35 Hz;
/// the render `Transform` is interpolated from `prev_*` -> current.
#[derive(Component, Debug)]
pub struct DoomPlayer {
    pub pos: Vec2,
    pub prev_pos: Vec2,
    /// Smoothed feet height (follows `floor_z`).
    pub z: f32,
    pub prev_z: f32,
    /// Floor height of the sector the player stands in.
    pub floor_z: f32,
    /// Yaw in Bevy space (radians, 0 = facing DOOM north / Bevy -Z).
    pub yaw: f32,
}

/// Pitch lives on the camera child of the player.
#[derive(Component, Debug, Default)]
pub struct PlayerCamera {
    pub pitch: f32,
}

/// Velocity in DOOM map units per tick.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct Velocity(pub Vec2);

/// Index into `MapData::sectors` that this entity currently occupies.
#[derive(Component, Debug, Clone, Copy)]
pub struct SectorRef(pub usize);

/// Rotates around Y to face the active 3D camera (cylindrical billboard).
#[derive(Component, Debug, Default)]
pub struct BillboardSprite;

/// A THINGS-lump entity (monster, pickup, decoration).
#[derive(Component, Debug)]
pub struct DoomThing {
    pub kind: i16,
}
