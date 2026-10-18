//! Fixed 35 Hz simulation clock (DOOM's TICRATE), decoupled from render rate.

use bevy::prelude::*;

pub const TICRATE: f64 = 35.0;

/// Number of simulation tics elapsed.
#[derive(Resource, Default, Debug)]
pub struct GameTick(pub u64);

/// Ordering inside each tic.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum TickSet {
    Input,
    Physics,
    Post,
}

pub struct TickPlugin;

impl Plugin for TickPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(TICRATE))
            .init_resource::<GameTick>()
            .configure_sets(
                FixedUpdate,
                (TickSet::Input, TickSet::Physics, TickSet::Post).chain(),
            )
            .add_systems(FixedFirst, |mut t: ResMut<GameTick>| t.0 += 1);
    }
}
