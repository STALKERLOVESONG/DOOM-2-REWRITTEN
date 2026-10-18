//! First-person controller: mouse look per frame, WASD physics per 35 Hz tic.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, PrimaryWindow};

use super::tick::TickSet;
use crate::components::*;
use crate::map::collision::CollisionMap;
use crate::map::to_world;

const MOUSE_SENSITIVITY: f32 = 0.0025;
/// DOOM's player friction per tic.
const FRICTION: f32 = 0.90625;
/// Doom thrust: move(50) * 2048 / 65536 per tic^2 at full forward.
const FORWARD_THRUST: f32 = 50.0 / 32.0;
const STRAFE_THRUST: f32 = 40.0 / 32.0;
const STOP_SPEED: f32 = 0.0625;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (cursor_grab, mouse_look, interpolate_player).chain(),
        )
        .add_systems(
            FixedUpdate,
            (
                apply_input_thrust.in_set(TickSet::Input),
                move_player.in_set(TickSet::Physics),
            ),
        );
    }
}

fn cursor_grab(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        window.cursor_options.grab_mode = CursorGrabMode::None;
        window.cursor_options.visible = true;
    }
}

fn mouse_look(
    motion: Res<AccumulatedMouseMotion>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut player: Single<&mut DoomPlayer>,
    camera: Single<(&mut Transform, &mut PlayerCamera)>,
) {
    if window.cursor_options.grab_mode == CursorGrabMode::None {
        return;
    }
    let d = motion.delta;
    player.yaw -= d.x * MOUSE_SENSITIVITY;
    let (mut tf, mut cam) = camera.into_inner();
    cam.pitch = (cam.pitch - d.y * MOUSE_SENSITIVITY).clamp(-1.5, 1.5);
    tf.rotation = Quat::from_rotation_x(cam.pitch);
}

/// Tic input: WASD adds thrust in DOOM map space. Shift = walk.
fn apply_input_thrust(
    keys: Res<ButtonInput<KeyCode>>,
    mut q: Query<(&DoomPlayer, &mut Velocity)>,
) {
    let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i8 as f32 - keys.pressed(neg) as i8 as f32;
    let fwd = axis(KeyCode::KeyW, KeyCode::KeyS);
    let strafe = axis(KeyCode::KeyD, KeyCode::KeyA);
    let speed = if keys.pressed(KeyCode::ShiftLeft) { 0.5 } else { 1.0 };

    for (p, mut vel) in &mut q {
        // See derivation: yaw 0 faces DOOM north (+y); right-hand vector is east.
        let forward = Vec2::new(-p.yaw.sin(), p.yaw.cos());
        let right = Vec2::new(p.yaw.cos(), p.yaw.sin());
        vel.0 += (forward * fwd * FORWARD_THRUST + right * strafe * STRAFE_THRUST) * speed;
    }
}

fn move_player(
    collision: Res<CollisionMap>,
    mut q: Query<(&mut DoomPlayer, &mut Velocity, &mut SectorRef)>,
) {
    for (mut p, mut vel, mut sector) in &mut q {
        p.prev_pos = p.pos;
        p.prev_z = p.z;

        let target = p.pos + vel.0;
        let new_pos = collision.slide_move(p.pos, target, p.floor_z);
        vel.0 = new_pos - p.pos; // walls eat the momentum they stop
        p.pos = new_pos;

        if let Some(s) = collision.sector_at(new_pos) {
            sector.0 = s;
            p.floor_z = collision.heights[s].0;
        }
        // Smooth stair-stepping / lifts for the view.
        let dz = p.floor_z - p.z;
        p.z = if dz.abs() < 0.1 { p.floor_z } else { p.z + dz * 0.5 };

        vel.0 *= FRICTION;
        if vel.0.length() < STOP_SPEED {
            vel.0 = Vec2::ZERO;
        }
    }
}

/// Render-rate smoothing between the last two tics.
fn interpolate_player(fixed: Res<Time<Fixed>>, mut q: Query<(&DoomPlayer, &mut Transform)>) {
    let alpha = fixed.overstep_fraction();
    for (p, mut tf) in &mut q {
        let pos = p.prev_pos.lerp(p.pos, alpha);
        let z = p.prev_z + (p.z - p.prev_z) * alpha;
        tf.translation = to_world(pos.x, pos.y, z + VIEW_HEIGHT);
        tf.rotation = Quat::from_rotation_y(p.yaw);
    }
}
