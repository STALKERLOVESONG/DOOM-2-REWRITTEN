//! Extra world-space geometry for the perspective renderer: debug lines
//! (collision boxes, grind rails, velocity vectors, wireframe), flat shaded
//! and textured triangles (models such as the skateboard) and world
//! billboards (a player-controlled sprite such as the third-person
//! Doomguy).
//!
//! Callers fill a [`DebugScene`] each frame and pass it to
//! [`crate::PerspectiveRenderer::render_scene`]. The vectors keep their
//! capacity across [`DebugScene::clear`], so a scene that is rebuilt every
//! frame stops allocating once it has reached its working size.

/// A line segment in map coordinates (z up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugLine {
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// Palette index, drawn fullbright.
    pub color: u8,
    /// Drawn through walls (no depth test).
    pub xray: bool,
}

/// A triangle in map coordinates, lit like a sprite in its sector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugTri {
    pub p: [[f32; 3]; 3],
    /// Palette index of the unlit colour.
    pub color: u8,
    /// Colormap offset for face shading (positive = darker).
    pub shade: i8,
    /// Sector light level 0..=255; `None` = fullbright.
    pub light: Option<i16>,
}

/// A palettized texture with mip levels, for [`TexTri`]s.
#[derive(Clone, Debug, Default)]
pub struct SceneTexture {
    /// Level 0 first, each half the size of the previous: (width, height,
    /// palette indices row-major).
    pub mips: Vec<(u32, u32, Vec<u8>)>,
}

impl SceneTexture {
    /// From RGBA8 pixels: box-filtered down to at most `max_size`, then
    /// quantised to `palette` (nearest colour) with mip levels to 8x8.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8], palette: &[[u8; 3]; 256], max_size: u32) -> SceneTexture {
        let mut level: (u32, u32, Vec<[f32; 3]>) = (width, height, rgba.chunks_exact(4).map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect());
        while level.0 > max_size || level.1 > max_size {
            level = halve(&level);
        }
        let mut cache: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();
        let mut quantise = |c: [f32; 3]| -> u8 {
            // 5 bits per channel are plenty for picking a palette entry.
            let key = ((c[0] as u32 >> 3) << 10) | ((c[1] as u32 >> 3) << 5) | (c[2] as u32 >> 3);
            *cache.entry(key).or_insert_with(|| {
                let mut best = (f32::MAX, 0u8);
                for (i, p) in palette.iter().enumerate() {
                    let d = (p[0] as f32 - c[0]).powi(2) * 0.30 + (p[1] as f32 - c[1]).powi(2) * 0.59 + (p[2] as f32 - c[2]).powi(2) * 0.11;
                    if d < best.0 {
                        best = (d, i as u8);
                    }
                }
                best.1
            })
        };
        let mut mips = Vec::new();
        loop {
            mips.push((level.0, level.1, level.2.iter().map(|&c| quantise(c)).collect()));
            if level.0 <= 8 || level.1 <= 8 {
                break;
            }
            level = halve(&level);
        }
        SceneTexture { mips }
    }

    /// Texel at (u, v) in [0, 1) (wrapping) on a mip level.
    #[inline]
    pub fn sample(&self, level: usize, u: f32, v: f32) -> u8 {
        let (w, h, t) = &self.mips[level.min(self.mips.len() - 1)];
        let x = ((u - u.floor()) * *w as f32) as u32 % w;
        let y = ((v - v.floor()) * *h as f32) as u32 % h;
        t[(y * w + x) as usize]
    }
}

fn halve(l: &(u32, u32, Vec<[f32; 3]>)) -> (u32, u32, Vec<[f32; 3]>) {
    let (w, h, px) = l;
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = Vec::with_capacity((nw * nh) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                let p = px[(sy * w + sx) as usize];
                for k in 0..3 {
                    acc[k] += p[k] * 0.25;
                }
            }
            out.push(acc);
        }
    }
    (nw, nh, out)
}

/// A textured triangle (perspective-correct), lit like [`DebugTri`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexTri {
    pub p: [[f32; 3]; 3],
    /// Texture coordinates (v grows downwards in the image).
    pub uv: [[f32; 2]; 3],
    /// Index into [`DebugScene::textures`].
    pub tex: u16,
    pub shade: i8,
    pub light: Option<i16>,
}

/// A sprite standing in the world, facing the camera (a billboard), drawn
/// from the game's sprite frames with rotations chosen like an actor's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneSprite {
    /// Where the sprite stands, in map coordinates: the frame's lowest
    /// opaque row (its feet) is placed at this height, horizontally centred
    /// on the sprite's offset like an actor.
    pub pos: [f32; 3],
    /// Sprite number and frame (0 = `A`).
    pub sprite: u16,
    pub frame: u8,
    /// The direction the actor faces (radians, map angle).
    pub facing: f32,
    /// Horizontal and vertical scale (crouching squashes it).
    pub scale: [f32; 2],
    /// Sector light 0..=255; `None` = fullbright.
    pub light: Option<i16>,
    /// Player colour translation (0 = none).
    pub translation: u8,
    /// Depth bias in map units (positive: drawn in front of surfaces this
    /// close behind it, so feet don't sink into what they stand on).
    pub bias: f32,
}

/// Everything extra to draw in one frame.
#[derive(Clone, Debug, Default)]
pub struct DebugScene {
    pub lines: Vec<DebugLine>,
    pub tris: Vec<DebugTri>,
    pub tex_tris: Vec<TexTri>,
    /// Textures used by `tex_tris` (shared, so per frame this only copies
    /// pointers).
    pub textures: Vec<std::sync::Arc<SceneTexture>>,
    pub sprites: Vec<SceneSprite>,
    /// Replace the level with this colour before drawing the scene
    /// (`r_wireframe`).
    pub clear_view: Option<u8>,
}

impl DebugScene {
    /// Empty the scene, keeping its allocations.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.tris.clear();
        self.tex_tris.clear();
        self.textures.clear();
        self.sprites.clear();
        self.clear_view = None;
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.tris.is_empty() && self.tex_tris.is_empty() && self.sprites.is_empty() && self.clear_view.is_none()
    }

    /// Add a texture for this frame's [`TexTri`]s; returns its index.
    pub fn texture(&mut self, t: &std::sync::Arc<SceneTexture>) -> u16 {
        if let Some(i) = self.textures.iter().position(|x| std::sync::Arc::ptr_eq(x, t)) {
            return i as u16;
        }
        self.textures.push(t.clone());
        (self.textures.len() - 1) as u16
    }

    pub fn line(&mut self, a: [f32; 3], b: [f32; 3], color: u8, xray: bool) {
        self.lines.push(DebugLine { a, b, color, xray });
    }

    /// A three-axis cross marking a point.
    pub fn cross(&mut self, p: [f32; 3], size: f32, color: u8, xray: bool) {
        let [x, y, z] = p;
        self.line([x - size, y, z], [x + size, y, z], color, xray);
        self.line([x, y - size, z], [x, y + size, z], color, xray);
        self.line([x, y, z - size], [x, y, z + size], color, xray);
    }

    /// The edges of an axis-aligned box (an actor's collision box).
    pub fn aabb(&mut self, min: [f32; 3], max: [f32; 3], color: u8, xray: bool) {
        let c = |i: usize| [if i & 1 == 0 { min[0] } else { max[0] }, if i & 2 == 0 { min[1] } else { max[1] }, if i & 4 == 0 { min[2] } else { max[2] }];
        for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
            self.line(c(a), c(b), color, xray);
        }
    }

    pub fn tri(&mut self, p: [[f32; 3]; 3], color: u8, shade: i8, light: Option<i16>) {
        self.tris.push(DebugTri { p, color, shade, light });
    }
}
