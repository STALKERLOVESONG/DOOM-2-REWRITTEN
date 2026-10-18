//! Deterministic Doom simulation.
//!
//! This crate is the playsim: everything that must be bit-identical for demo
//! playback and netgame sync. It has no engine/renderer dependencies and is
//! driven one tic at a time by a host (the Bevy app, a test, a demo player).
//!
//! Why not Bevy ECS? Vanilla behaviour depends on the order thinkers run and
//! the order mobjs are linked into sector/blockmap lists (that order decides
//! which object is hit first and the order `P_Random` is consumed). Bevy does
//! not guarantee query iteration order, so the simulation keeps explicit,
//! ordered data structures and the host mirrors its state for presentation.

pub mod actions;
pub mod actors;
pub mod acs_host;
pub mod content;
pub mod decorate;
pub mod dehacked;
pub mod demo;
pub mod enemy;
pub mod game;
pub mod geometry;
pub mod grind;
pub mod hexen;
pub mod ids;
pub mod info;
pub mod inventory;
pub mod inter;
pub mod items;
pub mod level;
pub mod map;
pub mod maputl;
pub mod mobj;
pub mod player;
pub mod pspr;
pub mod sight;
pub mod skate;
pub mod sounds;
pub mod spec;
pub mod specials;
pub mod thinker;
pub mod tick;
pub mod ticcmd;
pub mod user;
pub mod weapons;
pub mod world;

pub use game::GameAction;
pub use ids::*;
pub use level::Level;
pub use world::{GameParams, Skill, World};
