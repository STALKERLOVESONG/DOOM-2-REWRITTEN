//! Loads the WAD stack + map, uploads meshes/textures, spawns player and THINGS.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;

use crate::components::*;
use crate::map::collision::CollisionMap;
use crate::map::data::MapData;
use crate::map::mesh_builder::{build_level, LevelGeometry};
use crate::map::shapes::build_sector_shapes;
use crate::map::{to_world, DOOM_SCALE};
use crate::wad::loader::{WadDirectory, WadError};
use crate::wad::textures::{decode_picture, RgbaImage, TextureLibrary};

#[derive(Resource, Clone)]
pub struct LaunchConfig {
    pub iwad: PathBuf,
    pub pwads: Vec<PathBuf>,
    pub map: Option<String>,
    pub skill: u8,
}

pub struct LevelPlugin;

impl Plugin for LevelPlugin {
    fn build(&self, app: &mut App) {
        // Loading happens inside the app so `warn!`/`info!` reach the log plugin.
        app.add_systems(
            Startup,
            (load_level, spawn_level.run_if(resource_exists::<MapData>)).chain(),
        );
    }
}

type Loaded = (WadDirectory, TextureLibrary, MapData, LevelGeometry, CollisionMap);

fn try_load(cfg: &LaunchConfig) -> Result<Loaded, WadError> {
    let wad = WadDirectory::load(&cfg.iwad, &cfg.pwads)?;
    let map_name = cfg
        .map
        .clone()
        .or_else(|| wad.default_map())
        .ok_or_else(|| WadError::MissingLump("E1M1 or MAP01".into()))?;
    let lib = TextureLibrary::load(&wad)?;
    let map = MapData::load(&wad, &map_name)?;
    let shapes = build_sector_shapes(&map);
    let geometry = build_level(&map, &lib, &shapes);
    let collision = CollisionMap::new(&map, shapes);
    Ok((wad, lib, map, geometry, collision))
}

fn load_level(mut commands: Commands, cfg: Res<LaunchConfig>, mut exit: EventWriter<AppExit>) {
    match try_load(&cfg) {
        Ok((wad, lib, map, geometry, collision)) => {
            commands.insert_resource(wad);
            commands.insert_resource(lib);
            commands.insert_resource(map);
            commands.insert_resource(geometry);
            commands.insert_resource(collision);
        }
        Err(e) => {
            error!("failed to load level: {e}");
            exit.write(AppExit::error());
        }
    }
}

// ---------------------------------------------------------------------------

/// Hands out one GPU image per texture name, with a checkerboard for missing ones.
#[derive(Default)]
struct ImageCache {
    walls: HashMap<String, Handle<Image>>,
    flats: HashMap<String, Handle<Image>>,
    fallback: Option<Handle<Image>>,
}

impl ImageCache {
    fn fallback(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        self.fallback
            .get_or_insert_with(|| {
                let mut img = RgbaImage::new(64, 64);
                for y in 0..64usize {
                    for x in 0..64usize {
                        let on = (x / 8 + y / 8) % 2 == 0;
                        let c: [u8; 4] = if on { [255, 0, 255, 255] } else { [20, 20, 20, 255] };
                        img.data[(y * 64 + x) * 4..][..4].copy_from_slice(&c);
                    }
                }
                images.add(img.to_bevy_image(true))
            })
            .clone()
    }

    fn get(&mut self, name: &str, flat: bool, lib: &TextureLibrary, images: &mut Assets<Image>) -> Handle<Image> {
        let (cache, source) = if flat {
            (&mut self.flats, &lib.flats)
        } else {
            (&mut self.walls, &lib.walls)
        };
        if let Some(h) = cache.get(name) {
            return h.clone();
        }
        match source.get(name) {
            Some(img) => {
                let h = images.add(img.to_bevy_image(true));
                cache.insert(name.to_string(), h.clone());
                h
            }
            None => {
                warn!("texture/flat `{name}` not found; using placeholder");
                self.fallback(images)
            }
        }
    }
}

struct SpriteAsset {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    /// Quad centre relative to the thing's feet.
    offset: Vec3,
}

#[allow(clippy::too_many_arguments)]
fn spawn_level(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut geometry: ResMut<LevelGeometry>,
    lib: Res<TextureLibrary>,
    map: Res<MapData>,
    wad: Res<WadDirectory>,
    collision: Res<CollisionMap>,
    cfg: Res<LaunchConfig>,
) {
    let mut cache = ImageCache::default();

    // --- static geometry -------------------------------------------------
    for (name, md) in std::mem::take(&mut geometry.walls) {
        let tex = cache.get(&name, false, &lib, &mut images);
        commands.spawn((
            Name::new(format!("walls:{name}")),
            Mesh3d(meshes.add(md.into_mesh())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color_texture: Some(tex),
                unlit: true, // lighting comes from baked vertex colours
                alpha_mode: AlphaMode::Mask(0.5),
                ..default()
            })),
            Transform::default(),
        ));
    }
    for (name, md) in std::mem::take(&mut geometry.flats) {
        let tex = cache.get(&name, true, &lib, &mut images);
        commands.spawn((
            Name::new(format!("flat:{name}")),
            Mesh3d(meshes.add(md.into_mesh())),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color_texture: Some(tex),
                unlit: true,
                ..default()
            })),
            Transform::default(),
        ));
    }

    // --- player ------------------------------------------------------------
    let (start, angle) = match map.things.iter().find(|t| t.kind == 1) {
        Some(t) => (Vec2::new(t.x as f32, t.y as f32), t.angle as f32),
        None => {
            warn!("{} has no player 1 start; spawning at map centroid", map.name);
            (map.centroid(), 0.0)
        }
    };
    let sector = collision.sector_at(start);
    let floor = sector.map_or(0.0, |s| collision.heights[s].0);
    // DOOM angle 0 = east; our yaw 0 = north.
    let yaw = (angle - 90.0).to_radians();

    commands
        .spawn((
            Name::new("Player"),
            DoomPlayer {
                pos: start,
                prev_pos: start,
                z: floor,
                prev_z: floor,
                floor_z: floor,
                yaw,
            },
            Velocity::default(),
            SectorRef(sector.unwrap_or(0)),
            Transform::from_translation(to_world(start.x, start.y, floor + VIEW_HEIGHT))
                .with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
        ))
        .with_children(|p| {
            p.spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: 60f32.to_radians(),
                    near: 0.02,
                    far: 2000.0,
                    ..default()
                }),
                Tonemapping::None, // keep unlit palette colours faithful
                PlayerCamera::default(),
                Transform::default(),
            ));
        });

    // --- things ----------------------------------------------------------
    let mut prefix_cache: HashMap<&'static str, Option<String>> = HashMap::new();
    let mut sprite_cache: HashMap<String, Option<SpriteAsset>> = HashMap::new();
    let mut spawned = 0;

    for t in &map.things {
        if t.kind == 1 || !passes_skill(t.flags, cfg.skill) {
            continue;
        }
        let Some(prefix) = sprite_prefix(t.kind) else { continue };
        let Some(lump) = prefix_cache
            .entry(prefix)
            .or_insert_with(|| find_sprite_lump(&wad, prefix))
            .clone()
        else {
            continue;
        };
        let asset = sprite_cache.entry(lump.clone()).or_insert_with(|| {
            load_sprite(&wad, &lib, &lump, &mut meshes, &mut images, &mut materials)
        });
        let Some(asset) = asset else { continue };

        let pos = Vec2::new(t.x as f32, t.y as f32);
        let sector = collision.sector_at(pos);
        let floor = sector.map_or(0.0, |s| collision.heights[s].0);
        commands
            .spawn((
                Name::new(format!("thing:{}", t.kind)),
                DoomThing { kind: t.kind },
                BillboardSprite,
                SectorRef(sector.unwrap_or(0)),
                Transform::from_translation(to_world(pos.x, pos.y, floor)),
                Visibility::default(),
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(asset.mesh.clone()),
                    MeshMaterial3d(asset.material.clone()),
                    Transform::from_translation(asset.offset),
                ));
            });
        spawned += 1;
    }
    info!("spawned {spawned} things with sprites");
}

fn passes_skill(flags: i16, skill: u8) -> bool {
    if flags & 0x0010 != 0 {
        return false; // multiplayer only
    }
    let bit = match skill {
        1 | 2 => 0x0001,
        3 => 0x0002,
        _ => 0x0004,
    };
    flags & bit != 0
}

/// Pick a front-facing frame-A lump for a 4-letter sprite prefix.
fn find_sprite_lump(wad: &WadDirectory, prefix: &str) -> Option<String> {
    wad.lump_names()
        .filter(|n| n.starts_with(prefix) && (n.len() == 6 || n.len() == 8))
        .map(|n| {
            let b = n.as_bytes();
            let front_a = (b[4] == b'A' && matches!(b[5], b'0' | b'1'))
                || (n.len() == 8 && b[6] == b'A' && matches!(b[7], b'0' | b'1'));
            let score = if front_a { 0 } else if b[4] == b'A' { 1 } else { 2 };
            (score, n)
        })
        .min()
        .map(|(_, n)| n.to_string())
}

fn load_sprite(
    wad: &WadDirectory,
    lib: &TextureLibrary,
    lump: &str,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<SpriteAsset> {
    let pic = match decode_picture(lump, wad.try_lump(lump)?, &lib.palette) {
        Ok(p) => p,
        Err(e) => {
            warn!("{e}");
            return None;
        }
    };
    let (w, h) = (pic.image.width as f32, pic.image.height as f32);
    let tex = images.add(pic.image.to_bevy_image(false));
    Some(SpriteAsset {
        mesh: meshes.add(Rectangle::new(w * DOOM_SCALE, h * DOOM_SCALE)),
        material: materials.add(StandardMaterial {
            base_color_texture: Some(tex),
            unlit: true,
            alpha_mode: AlphaMode::Mask(0.5),
            cull_mode: None,
            ..default()
        }),
        // Patch origin is at the feet; `left`/`top` locate the picture around it.
        offset: Vec3::new(
            (w * 0.5 - pic.left as f32) * DOOM_SCALE,
            (pic.top as f32 - h * 0.5) * DOOM_SCALE,
            0.0,
        ),
    })
}

/// DOOM editor number -> 4-letter sprite prefix.
fn sprite_prefix(kind: i16) -> Option<&'static str> {
    Some(match kind {
        // Monsters
        3004 => "POSS", 9 => "SPOS", 3001 => "TROO", 3002 | 58 => "SARG",
        3006 => "SKUL", 3005 => "HEAD", 3003 => "BOSS", 69 => "BOS2",
        64 => "VILE", 66 => "SKEL", 67 => "FATT", 68 => "BSPI", 71 => "PAIN",
        65 => "CPOS", 84 => "SSWV", 7 => "SPID", 16 => "CYBR", 72 => "KEEN",
        // Weapons & ammo
        2001 => "SHOT", 2002 => "MGUN", 2003 => "LAUN", 2004 => "PLAS",
        2005 => "CSAW", 2006 => "BFUG", 82 => "SGN2", 2007 => "CLIP",
        2008 => "SHEL", 2010 => "ROCK", 2047 => "CELL", 2048 => "AMMO",
        2049 => "SBOX", 2046 => "BROK", 17 => "CELP", 8 => "BPAK",
        // Health, armour, powerups
        2011 => "STIM", 2012 => "MEDI", 2014 => "BON1", 2015 => "BON2",
        2018 => "ARM1", 2019 => "ARM2", 2013 => "SOUL", 2022 => "PINV",
        2023 => "PSTR", 2024 => "PINS", 2025 => "SUIT", 2026 => "PMAP",
        2045 => "PVIS", 83 => "MEGA",
        // Keys
        5 => "BKEY", 6 => "YKEY", 13 => "RKEY", 38 => "RSKU", 39 => "YSKU", 40 => "BSKU",
        // Decorations
        2035 => "BAR1", 2028 => "COLU", 30 => "COL1", 31 => "COL2", 32 => "COL3",
        33 => "COL4", 34 => "CAND", 35 => "CBRA", 36 => "COL5", 37 => "COL6",
        41 => "CEYE", 42 => "FSKU", 43 => "TRE1", 44 => "TBLU", 45 => "TGRN",
        46 => "TRED", 47 => "SMIT", 48 => "ELEC", 54 => "TRE2", 55 => "SMBT",
        56 => "SMGT", 57 => "SMRT", 70 => "FCAN", 85 => "TLMP", 86 => "TLP2",
        25 => "POL1", 26 => "POL6", 27 => "POL4", 28 => "POL2", 29 => "POL3",
        24 => "POL5", 49 | 63 => "GOR1", 50 | 59 => "GOR2", 51 | 61 => "GOR3",
        52 | 60 => "GOR4", 53 | 62 => "GOR5", 73 => "HDB1", 74 => "HDB2",
        75 => "HDB3", 76 => "HDB4", 77 => "HDB5", 78 => "HDB6", 79 => "POB1",
        80 => "POB2", 81 => "BRS1", 10 | 12 | 15 => "PLAY",
        _ => return None,
    })
}
