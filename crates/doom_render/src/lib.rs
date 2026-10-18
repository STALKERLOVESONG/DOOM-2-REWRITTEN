//! Doom software renderer.
//!
//! A port of the vanilla renderer (r_main.c, r_bsp.c, r_segs.c, r_plane.c,
//! r_things.c, r_draw.c, r_sky.c) producing a palette-indexed framebuffer
//! with the same integer arithmetic, so frames match the original pixel for
//! pixel — including its quirks (tutti-frutti, medusa, slime trails,
//! visplane/drawseg/vissprite limits).
//!
//! The renderer only *reads* the simulation. Presentation (palette, scaling,
//! aspect correction, uploading) is the host's job.

pub mod data;
pub mod debug_draw;
pub mod perspective;
mod render;

pub use data::{RenderData, TexImage};
pub use debug_draw::{DebugLine, DebugScene, DebugTri, SceneSprite, SceneTexture, TexTri};
pub use perspective::{Camera, PerspectiveRenderer, ViewRect, PIXEL_ASPECT};
pub use render::{Limits, Renderer, SCREENHEIGHT, SCREENWIDTH};
