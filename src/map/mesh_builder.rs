//! Turns map data into batched GPU meshes: one mesh per texture for walls and
//! one per flat for floors/ceilings. Sector light levels are baked into vertex
//! colours (the material is unlit, so vertex colour *is* the lighting).

use std::collections::HashMap;

use bevy::log::warn;
use bevy::prelude::{Mesh, Resource, Vec2, Vec3};
use bevy::render::mesh::Indices;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::PrimitiveTopology;

use super::data::{flags, MapData};
use super::shapes::SectorShape;
use super::to_world;
use crate::wad::textures::TextureLibrary;

#[derive(Default)]
pub struct MeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl MeshData {
    fn vertex(&mut self, p: Vec3, n: Vec3, uv: [f32; 2], color: [f32; 4]) -> u32 {
        let i = self.positions.len() as u32;
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.uvs.push(uv);
        self.colors.push(color);
        i
    }

    pub fn into_mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

#[derive(Resource, Default)]
pub struct LevelGeometry {
    /// Keyed by wall texture name.
    pub walls: HashMap<String, MeshData>,
    /// Keyed by flat name (floors and ceilings share batches).
    pub flats: HashMap<String, MeshData>,
}

pub fn build_level(map: &MapData, lib: &TextureLibrary, shapes: &[SectorShape]) -> LevelGeometry {
    let mut geo = LevelGeometry::default();
    build_flats(map, lib, shapes, &mut geo.flats);
    build_walls(map, lib, &mut geo.walls);
    geo
}

fn is_sky(flat: &str) -> bool {
    flat.starts_with("F_SKY")
}

// ---------------------------------------------------------------------------
// Floors & ceilings
// ---------------------------------------------------------------------------

fn triangulate(shape: &SectorShape) -> (Vec<Vec2>, Vec<usize>) {
    let mut pts = shape.outer.clone();
    let mut hole_starts = Vec::new();
    for h in &shape.holes {
        hole_starts.push(pts.len());
        pts.extend_from_slice(h);
    }
    let coords: Vec<f64> = pts.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
    match earcutr::earcut(&coords, &hole_starts, 2) {
        Ok(idx) => (pts, idx),
        Err(e) => {
            warn!("sector {}: triangulation failed ({e:?})", shape.sector);
            (pts, Vec::new())
        }
    }
}

fn push_flat(md: &mut MeshData, pts: &[Vec2], idx: &[usize], height: f32, color: [f32; 4], facing_up: bool) {
    let normal = if facing_up { Vec3::Y } else { Vec3::NEG_Y };
    let base = md.positions.len() as u32;
    for p in pts {
        // Flats are 64x64 and aligned to the world grid.
        md.vertex(to_world(p.x, p.y, height), normal, [p.x / 64.0, p.y / 64.0], color);
    }
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (pts[t[0]], pts[t[1]], pts[t[2]]);
        let area2 = (b - a).perp_dot(c - a);
        if area2 == 0.0 {
            continue;
        }
        // Earcut's output winding isn't guaranteed; force CCW (up) / CW (down).
        let ccw = area2 > 0.0;
        let (i0, i1, i2) = (base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
        if ccw == facing_up {
            md.indices.extend_from_slice(&[i0, i1, i2]);
        } else {
            md.indices.extend_from_slice(&[i0, i2, i1]);
        }
    }
}

fn build_flats(map: &MapData, lib: &TextureLibrary, shapes: &[SectorShape], out: &mut HashMap<String, MeshData>) {
    for shape in shapes {
        let sec = &map.sectors[shape.sector];
        let color = lib.light_color(sec.light);
        let (pts, idx) = triangulate(shape);
        if idx.is_empty() {
            continue;
        }
        push_flat(
            out.entry(sec.floor_tex.clone()).or_default(),
            &pts,
            &idx,
            sec.floor as f32,
            color,
            true,
        );
        if !is_sky(&sec.ceiling_tex) {
            push_flat(
                out.entry(sec.ceiling_tex.clone()).or_default(),
                &pts,
                &idx,
                sec.ceiling as f32,
                color,
                false,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Walls
// ---------------------------------------------------------------------------

fn has_tex(name: &str) -> bool {
    !name.is_empty() && name != "-"
}

/// One textured quad. `a`/`b` are the left/right endpoints as seen by the viewer.
/// `tex_top` is the world height of the texture's top row (offsets already applied).
#[allow(clippy::too_many_arguments)]
fn wall_quad(
    out: &mut HashMap<String, MeshData>,
    lib: &TextureLibrary,
    tex: &str,
    a: Vec2,
    b: Vec2,
    z_bottom: f32,
    z_top: f32,
    tex_top: f32,
    x_offset: f32,
    color: [f32; 4],
) {
    if z_top <= z_bottom || a.distance_squared(b) < 1e-6 {
        return;
    }
    let (tw, th) = lib.wall_size(tex).unwrap_or((64, 128));
    let (tw, th) = (tw as f32, th as f32);
    let len = a.distance(b);
    let (u0, u1) = (x_offset / tw, (x_offset + len) / tw);
    let (v_top, v_bot) = ((tex_top - z_top) / th, (tex_top - z_bottom) / th);

    let bl = to_world(a.x, a.y, z_bottom);
    let br = to_world(b.x, b.y, z_bottom);
    let tl = to_world(a.x, a.y, z_top);
    let normal = (br - bl).cross(tl - bl).normalize_or_zero();

    let md = out.entry(tex.to_string()).or_default();
    let i0 = md.vertex(bl, normal, [u0, v_bot], color);
    let i1 = md.vertex(br, normal, [u1, v_bot], color);
    let i2 = md.vertex(to_world(b.x, b.y, z_top), normal, [u1, v_top], color);
    let i3 = md.vertex(tl, normal, [u0, v_top], color);
    md.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
}

fn build_walls(map: &MapData, lib: &TextureLibrary, out: &mut HashMap<String, MeshData>) {
    for ld in &map.linedefs {
        let (v1, v2) = (map.vertex(ld.v1), map.vertex(ld.v2));
        // Front sidedef is on the right of v1->v2; a viewer there sees v1 on the left.
        if let Some(front) = ld.front {
            emit_side(map, lib, out, ld.flags, front, ld.back, v1, v2);
        }
        if let Some(back) = ld.back {
            emit_side(map, lib, out, ld.flags, back, ld.front, v2, v1);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_side(
    map: &MapData,
    lib: &TextureLibrary,
    out: &mut HashMap<String, MeshData>,
    ld_flags: u16,
    side_idx: u16,
    other_idx: Option<u16>,
    a: Vec2,
    b: Vec2,
) {
    let side = &map.sidedefs[side_idx as usize];
    let sec = &map.sectors[side.sector as usize];
    let color = lib.light_color(sec.light);
    let (xo, yo) = (side.x_offset as f32, side.y_offset as f32);
    let (floor, ceil) = (sec.floor as f32, sec.ceiling as f32);
    let upper_unpegged = ld_flags & flags::UPPER_UNPEGGED != 0;
    let lower_unpegged = ld_flags & flags::LOWER_UNPEGGED != 0;
    let tex_h = |name: &str| lib.wall_size(name).map_or(128.0, |s| s.1 as f32);

    let Some(other_idx) = other_idx else {
        // One-sided: a solid middle texture spanning floor..ceiling.
        if has_tex(&side.middle) {
            let th = tex_h(&side.middle);
            let top = if lower_unpegged { floor + th } else { ceil };
            wall_quad(out, lib, &side.middle, a, b, floor, ceil, top + yo, xo, color);
        }
        return;
    };

    let osec = &map.sectors[map.sidedefs[other_idx as usize].sector as usize];
    let (ofloor, oceil) = (osec.floor as f32, osec.ceiling as f32);

    // Upper: front ceiling is higher than back ceiling.
    if oceil < ceil && has_tex(&side.upper) && !(is_sky(&sec.ceiling_tex) && is_sky(&osec.ceiling_tex)) {
        let th = tex_h(&side.upper);
        let top = if upper_unpegged { ceil } else { oceil + th };
        wall_quad(out, lib, &side.upper, a, b, oceil, ceil, top + yo, xo, color);
    }
    // Lower: back floor is higher than front floor.
    if ofloor > floor && has_tex(&side.lower) {
        let top = if lower_unpegged { ceil } else { ofloor };
        wall_quad(out, lib, &side.lower, a, b, floor, ofloor, top + yo, xo, color);
    }
    // Middle on a two-sided line (gratings, fences): drawn once, clipped to the opening.
    if has_tex(&side.middle) {
        let (lo, hi) = (floor.max(ofloor), ceil.min(oceil));
        if hi > lo {
            let th = tex_h(&side.middle);
            let top = (if lower_unpegged { lo + th } else { hi }) + yo;
            wall_quad(out, lib, &side.middle, a, b, (top - th).max(lo), top.min(hi), top, xo, color);
        }
    }
}
