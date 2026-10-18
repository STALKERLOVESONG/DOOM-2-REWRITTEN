//! Typed map lumps: THINGS, VERTEXES, LINEDEFS, SIDEDEFS, SECTORS.

use bevy::log::info;
use bevy::prelude::{Resource, Vec2};
use byteorder::{ByteOrder, LittleEndian as LE};

use crate::wad::loader::{lump_name, WadDirectory, WadError};

pub mod flags {
    pub const IMPASSABLE: u16 = 0x0001;
    pub const UPPER_UNPEGGED: u16 = 0x0008;
    pub const LOWER_UNPEGGED: u16 = 0x0010;
}

#[derive(Debug, Clone)]
pub struct Thing {
    pub x: i16,
    pub y: i16,
    /// Degrees, 0 = east, 90 = north.
    pub angle: i16,
    pub kind: i16,
    pub flags: i16,
}

#[derive(Debug, Clone)]
pub struct LineDef {
    pub v1: u16,
    pub v2: u16,
    pub flags: u16,
    pub special: u16,
    pub tag: u16,
    /// Right-hand (front) sidedef.
    pub front: Option<u16>,
    /// Left-hand (back) sidedef.
    pub back: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct SideDef {
    pub x_offset: i16,
    pub y_offset: i16,
    pub upper: String,
    pub lower: String,
    pub middle: String,
    pub sector: u16,
}

#[derive(Debug, Clone)]
pub struct Sector {
    pub floor: i16,
    pub ceiling: i16,
    pub floor_tex: String,
    pub ceiling_tex: String,
    pub light: i16,
    pub special: i16,
    pub tag: i16,
}

#[derive(Resource)]
pub struct MapData {
    pub name: String,
    pub things: Vec<Thing>,
    pub vertexes: Vec<Vec2>,
    pub linedefs: Vec<LineDef>,
    pub sidedefs: Vec<SideDef>,
    pub sectors: Vec<Sector>,
}

fn records<'a>(
    lump: &str,
    data: &'a [u8],
    size: usize,
) -> Result<std::slice::ChunksExact<'a, u8>, WadError> {
    if data.len() % size != 0 {
        return Err(WadError::MalformedLump {
            lump: lump.to_string(),
            reason: format!("length {} is not a multiple of {size}", data.len()),
        });
    }
    Ok(data.chunks_exact(size))
}

fn bad(lump: &str, reason: String) -> WadError {
    WadError::MalformedLump {
        lump: lump.to_string(),
        reason,
    }
}

impl MapData {
    pub fn load(wad: &WadDirectory, marker: &str) -> Result<Self, WadError> {
        let name = marker.to_ascii_uppercase();
        let lumps = wad.map_lumps(&name)?;
        let get = |n: &str| {
            lumps
                .get(n)
                .copied()
                .ok_or_else(|| WadError::MissingLump(format!("{name}/{n}")))
        };

        let things: Vec<Thing> = records("THINGS", get("THINGS")?, 10)?
            .map(|c| Thing {
                x: LE::read_i16(&c[0..]),
                y: LE::read_i16(&c[2..]),
                angle: LE::read_i16(&c[4..]),
                kind: LE::read_i16(&c[6..]),
                flags: LE::read_i16(&c[8..]),
            })
            .collect();

        let vertexes: Vec<Vec2> = records("VERTEXES", get("VERTEXES")?, 4)?
            .map(|c| Vec2::new(LE::read_i16(&c[0..]) as f32, LE::read_i16(&c[2..]) as f32))
            .collect();

        let side = |v: u16| (v != 0xFFFF).then_some(v);
        let linedefs: Vec<LineDef> = records("LINEDEFS", get("LINEDEFS")?, 14)?
            .map(|c| LineDef {
                v1: LE::read_u16(&c[0..]),
                v2: LE::read_u16(&c[2..]),
                flags: LE::read_u16(&c[4..]),
                special: LE::read_u16(&c[6..]),
                tag: LE::read_u16(&c[8..]),
                front: side(LE::read_u16(&c[10..])),
                back: side(LE::read_u16(&c[12..])),
            })
            .collect();

        let sidedefs: Vec<SideDef> = records("SIDEDEFS", get("SIDEDEFS")?, 30)?
            .map(|c| SideDef {
                x_offset: LE::read_i16(&c[0..]),
                y_offset: LE::read_i16(&c[2..]),
                upper: lump_name(&c[4..12]),
                lower: lump_name(&c[12..20]),
                middle: lump_name(&c[20..28]),
                sector: LE::read_u16(&c[28..]),
            })
            .collect();

        let sectors: Vec<Sector> = records("SECTORS", get("SECTORS")?, 26)?
            .map(|c| Sector {
                floor: LE::read_i16(&c[0..]),
                ceiling: LE::read_i16(&c[2..]),
                floor_tex: lump_name(&c[4..12]),
                ceiling_tex: lump_name(&c[12..20]),
                light: LE::read_i16(&c[20..]),
                special: LE::read_i16(&c[22..]),
                tag: LE::read_i16(&c[24..]),
            })
            .collect();

        // Cross-reference validation so later stages can index without checks.
        for (i, l) in linedefs.iter().enumerate() {
            if l.v1 as usize >= vertexes.len() || l.v2 as usize >= vertexes.len() {
                return Err(bad("LINEDEFS", format!("linedef {i} references a missing vertex")));
            }
            for s in [l.front, l.back].into_iter().flatten() {
                if s as usize >= sidedefs.len() {
                    return Err(bad("LINEDEFS", format!("linedef {i} references missing sidedef {s}")));
                }
            }
        }
        for (i, s) in sidedefs.iter().enumerate() {
            if s.sector as usize >= sectors.len() {
                return Err(bad("SIDEDEFS", format!("sidedef {i} references missing sector {}", s.sector)));
            }
        }

        info!(
            "{name}: {} things, {} vertexes, {} linedefs, {} sidedefs, {} sectors",
            things.len(),
            vertexes.len(),
            linedefs.len(),
            sidedefs.len(),
            sectors.len()
        );
        Ok(Self {
            name,
            things,
            vertexes,
            linedefs,
            sidedefs,
            sectors,
        })
    }

    pub fn vertex(&self, i: u16) -> Vec2 {
        self.vertexes[i as usize]
    }

    pub fn sector_of_side(&self, side: u16) -> usize {
        self.sidedefs[side as usize].sector as usize
    }

    pub fn centroid(&self) -> Vec2 {
        if self.vertexes.is_empty() {
            return Vec2::ZERO;
        }
        self.vertexes.iter().copied().sum::<Vec2>() / self.vertexes.len() as f32
    }
}
