//! Cylindrical billboards: sprites yaw towards the active 3D camera.

use bevy::prelude::*;
use bevy::transform::TransformSystem;

use crate::components::BillboardSprite;

pub struct BillboardPlugin;

impl Plugin for BillboardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            face_camera.before(TransformSystem::TransformPropagate),
        );
    }
}

fn face_camera(
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut sprites: Query<&mut Transform, With<BillboardSprite>>,
) {
    let Some((_, cam)) = cameras.iter().find(|(c, _)| c.is_active) else {
        return;
    };
    let cam_pos = cam.translation();
    for mut t in &mut sprites {
        let d = cam_pos - t.translation;
        if d.x.abs() + d.z.abs() < 1e-4 {
            continue;
        }
        // Quad faces +Z; rotating about Y by atan2(x, z) points +Z at the camera.
        t.rotation = Quat::from_rotation_y(d.x.atan2(d.z));
    }
}
