//! Host-independent game layer.
//!
//! Everything a front end needs that is not windowing, input devices, audio
//! output or GPU upload: loading content, starting games, running tics,
//! level progression, menus, status bar, automap, intermission, finale,
//! sound and music logic, palette effects and developer cheats. The Bevy
//! host is a thin shell over [`Session`]; tools and tests use it headless.

pub mod cheats;
pub mod console;
pub mod cvar;
pub mod gamma;
pub mod maps;
pub mod png;
pub mod session;
pub mod skate_model;
pub mod sound;
pub mod ui;
pub mod wipe;

pub use doom_sim::ticcmd::InputState;
pub use cheats::{Cheat, CheatMatcher};
pub use cvar::{CVar, CVarRegistry, CVarSpec, CVarValue};
pub use maps::MapRef;
pub use session::{GameState, HostView, Hud, RenderMode, Session, SessionConfig, ViewPath, MAX_PITCH, TICRATE};
pub use ui::Key;
