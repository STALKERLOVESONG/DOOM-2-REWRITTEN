//! Raw map lumps (doomdata.h) and map-format detection.
//!
//! These are the on-disk records, decoded but not interpreted, in a form
//! shared by every map format: coordinates are 16.16 fixed point and
//! indices are `u32` (Doom-format values are widened without changing
//! them; see [`crate::udmf`] and [`crate::nodes`] for UDMF and ZDoom
//! extended nodes). Building the runtime level (resolving references,
//! grouping lines) is the simulation's job (`doom_sim::level`).

use crate::le;
use crate::name::LumpName;
use crate::resources::{LumpId, ResourceStack};
use crate::ContentError;

/// Doom-format lump order after the map marker (`ML_THINGS`..`ML_BLOCKMAP`).
pub const DOOM_MAP_LUMPS: [&str; 10] =
    ["THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT", "BLOCKMAP"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapFormat {
    /// Vanilla Doom binary format.
    Doom,
    /// Hexen binary format (BEHAVIOR lump; ZDoom "Doom-in-Hexen" maps).
    Hexen,
    /// Universal Doom Map Format (TEXTMAP ... ENDMAP).
    Udmf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeFormat {
    /// Vanilla SEGS/SSECTORS/NODES.
    Vanilla,
    /// DeePBSP "xNd4" extended nodes.
    DeepBsp,
    /// ZDoom extended/compressed nodes ("XNOD", "ZNOD", GL variants).
    Zdoom([u8; 4]),
    /// No nodes present: a node builder would be required.
    Missing,
}

/// 16.16 fixed point from map units.
#[inline]
pub fn fixed(units: i32) -> i32 {
    units << 16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapVertex {
    /// 16.16 fixed point.
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MapThing {
    /// 16.16 fixed point.
    pub x: i32,
    pub y: i32,
    /// UDMF `height`: offset from the floor (fixed); 0 in Doom-format maps.
    pub z: i32,
    /// Degrees (0 = east, 90 = north).
    pub angle: i16,
    pub kind: i16,
    /// Vanilla `MTF_*` bits (UDMF flags are translated into them).
    pub options: i16,
    /// Thing ID (Hexen/UDMF), 0 = none.
    pub tid: i32,
    /// Hexen-style special run when the thing dies or is picked up, and
    /// its arguments.
    pub special: i32,
    pub args: [i32; 5],
}

/// Linedef flags (`ML_*`).
pub mod ml {
    pub const BLOCKING: u16 = 1;
    pub const BLOCKMONSTERS: u16 = 2;
    pub const TWOSIDED: u16 = 4;
    pub const DONTPEGTOP: u16 = 8;
    pub const DONTPEGBOTTOM: u16 = 16;
    pub const SECRET: u16 = 32;
    pub const SOUNDBLOCK: u16 = 64;
    pub const DONTDRAW: u16 = 128;
    pub const MAPPED: u16 = 256;
}

/// Thing option flags (`MTF_*`).
pub mod mtf {
    pub const EASY: i16 = 1;
    pub const NORMAL: i16 = 2;
    pub const HARD: i16 = 4;
    pub const AMBUSH: i16 = 8;
    /// "Not in single player" (vanilla checks bit 16 directly in P_SpawnMapThing).
    pub const NOTSINGLE: i16 = 16;
}

/// How a Hexen-style line special is activated (`SPAC_*`, plus ZDoom's
/// UDMF extensions).
pub mod spac {
    /// A player walks across.
    pub const CROSS: u32 = 1;
    /// A player presses use.
    pub const USE: u32 = 2;
    /// A monster walks across.
    pub const MCROSS: u32 = 4;
    /// A hitscan or projectile hits it.
    pub const IMPACT: u32 = 8;
    /// A player bumps into it.
    pub const PUSH: u32 = 16;
    /// A projectile crosses it.
    pub const PCROSS: u32 = 32;
    /// Use passes through to lines behind (`passuse`).
    pub const USETHROUGH: u32 = 64;
    /// Anything crosses it.
    pub const ANYCROSS: u32 = 128;
    /// A monster uses it.
    pub const MUSE: u32 = 256;
    /// A monster bumps into it.
    pub const MPUSH: u32 = 512;
    /// Usable from the back side too.
    pub const USEBACK: u32 = 1024;
    /// Only from the front side (`firstsideonly`).
    pub const FIRSTSIDEONLY: u32 = 2048;
}

/// A Hexen-style line special (Hexen-format and most UDMF maps): what it
/// does, with what arguments, and what triggers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LineAction {
    pub special: i32,
    pub args: [i32; 5],
    /// `spac::*` bits.
    pub activation: u32,
    pub repeat: bool,
    /// ZDoom lock number required (0 = none).
    pub lock: i32,
}

/// Sentinel for an absent sidedef in `MapLinedef::sidenum`.
pub const NO_SIDEDEF: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapLinedef {
    pub v1: u32,
    pub v2: u32,
    /// Vanilla `ML_*` bits (UDMF flags are translated into them).
    pub flags: u16,
    /// Doom-format special (0 when the line has a Hexen-style `action`).
    pub special: i16,
    pub tag: i16,
    /// Front (right) and back (left) sidedef; `NO_SIDEDEF` if absent.
    pub sidenum: [u32; 2],
    /// UDMF translucency (1.0 = opaque).
    pub alpha: f32,
    /// Hexen-style special (Hexen-format and UDMF `zdoom`/`hexen` maps).
    pub action: Option<LineAction>,
    /// Line ID for specials that address lines (UDMF `id`,
    /// `Line_SetIdentification`); -1 = none.
    pub id: i32,
}

/// Per-part texture offsets added to the sidedef's base offsets (UDMF
/// `offsetx_top` ...), in map units: [top, mid, bottom] x (x, y).
pub type PartOffsets = [[f32; 2]; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapSidedef {
    /// Map units (UDMF allows fractions).
    pub texture_offset: f32,
    pub row_offset: f32,
    pub top: LumpName,
    pub bottom: LumpName,
    pub mid: LumpName,
    pub sector: u32,
    pub part_offsets: PartOffsets,
    /// UDMF `light` / `lightabsolute`.
    pub light: i16,
    pub light_absolute: bool,
}

/// Flat placement (UDMF `xpanningfloor` ...).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneXform {
    pub x_offset: f32,
    pub y_offset: f32,
    pub x_scale: f32,
    pub y_scale: f32,
    /// Degrees.
    pub rotation: f32,
}

impl Default for PlaneXform {
    fn default() -> Self {
        PlaneXform { x_offset: 0.0, y_offset: 0.0, x_scale: 1.0, y_scale: 1.0, rotation: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapSector {
    /// 16.16 fixed point.
    pub floor_height: i32,
    pub ceiling_height: i32,
    pub floor_pic: LumpName,
    pub ceiling_pic: LumpName,
    pub light_level: i16,
    pub special: i16,
    pub tag: i16,
    pub floor_xform: PlaneXform,
    pub ceiling_xform: PlaneXform,
    /// UDMF `lightfloor`/`lightceiling` and their `...absolute` flags.
    pub floor_light: (i16, bool),
    pub ceiling_light: (i16, bool),
}

impl MapSector {
    pub fn doom(floor: i32, ceiling: i32, floor_pic: LumpName, ceiling_pic: LumpName, light: i16, special: i16, tag: i16) -> Self {
        MapSector {
            floor_height: floor,
            ceiling_height: ceiling,
            floor_pic,
            ceiling_pic,
            light_level: light,
            special,
            tag,
            floor_xform: PlaneXform::default(),
            ceiling_xform: PlaneXform::default(),
            floor_light: (0, false),
            ceiling_light: (0, false),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapSubsector {
    pub num_segs: u32,
    pub first_seg: u32,
    /// For GL-node subsectors left without segs (only minisegs): a sidedef
    /// of a neighbouring subsector that gives its sector.
    pub sector_side: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapSeg {
    pub v1: u32,
    pub v2: u32,
    /// BAM.
    pub angle: u32,
    pub linedef: u32,
    /// 0 = front side of the linedef, 1 = back. Kept signed: vanilla
    /// indexes with whatever is stored.
    pub side: i16,
    /// Distance along the linedef to the seg start (fixed).
    pub offset: i32,
}

/// Child flag: the child index refers to a subsector (`NF_SUBSECTOR`,
/// widened to 32 bits as in ZDoom's extended nodes).
pub const NF_SUBSECTOR: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapNode {
    /// Partition line, 16.16 fixed point.
    pub x: i32,
    pub y: i32,
    pub dx: i32,
    pub dy: i32,
    /// [child][BOXTOP, BOXBOTTOM, BOXLEFT, BOXRIGHT], map units.
    pub bbox: [[i32; 4]; 2],
    /// Right (0) and left (1) children.
    pub children: [u32; 2],
}

/// All lumps of one map, decoded into records.
#[derive(Clone, Debug)]
pub struct RawMap {
    pub name: LumpName,
    pub format: MapFormat,
    pub things: Vec<MapThing>,
    pub linedefs: Vec<MapLinedef>,
    pub sidedefs: Vec<MapSidedef>,
    pub vertexes: Vec<MapVertex>,
    pub segs: Vec<MapSeg>,
    pub subsectors: Vec<MapSubsector>,
    pub nodes: Vec<MapNode>,
    pub sectors: Vec<MapSector>,
    /// Raw REJECT bytes (may be short or empty; see `doom_sim::level`).
    pub reject: Vec<u8>,
    /// Raw BLOCKMAP as 16-bit words (empty: the level builds one).
    pub blockmap: Vec<i16>,
    /// The map's compiled ACS (`BEHAVIOR`), Hexen-format and UDMF maps.
    pub behavior: Option<Vec<u8>>,
    /// Non-fatal oddities (lump names out of place, trailing bytes, ...).
    pub warnings: Vec<String>,
}

fn records<'a>(lump: &str, data: &'a [u8], size: usize, warnings: &mut Vec<String>) -> std::slice::ChunksExact<'a, u8> {
    if data.len() % size != 0 {
        // Vanilla computes the count as len / size and ignores the remainder.
        warnings.push(format!("{lump}: {} trailing bytes ignored", data.len() % size));
    }
    data.chunks_exact(size)
}

/// Locate a map's lumps and identify its format.
pub fn detect_format(stack: &ResourceStack, marker: LumpId) -> MapFormat {
    let next = stack.next_in_source(marker);
    let name_at = |id: Option<LumpId>| id.map(|i| stack.info(i).name);
    if name_at(next) == Some(LumpName::new("TEXTMAP")) {
        return MapFormat::Udmf;
    }
    // BEHAVIOR directly follows BLOCKMAP in Hexen-format maps.
    let mut id = next;
    for _ in 0..DOOM_MAP_LUMPS.len() {
        id = id.and_then(|i| stack.next_in_source(i));
    }
    if name_at(id) == Some(LumpName::new("BEHAVIOR")) {
        MapFormat::Hexen
    } else {
        MapFormat::Doom
    }
}

pub fn detect_node_format(nodes: &[u8], segs_len: usize, ssectors_len: usize) -> NodeFormat {
    if nodes.len() >= 4 {
        let sig = [nodes[0], nodes[1], nodes[2], nodes[3]];
        match &sig {
            b"xNd4" => return NodeFormat::DeepBsp,
            b"XNOD" | b"ZNOD" | b"XGLN" | b"ZGLN" | b"XGL2" | b"ZGL2" | b"XGL3" | b"ZGL3" => {
                return NodeFormat::Zdoom(sig)
            }
            _ => {}
        }
    }
    if nodes.is_empty() || segs_len == 0 || ssectors_len == 0 {
        NodeFormat::Missing
    } else {
        NodeFormat::Vanilla
    }
}

impl RawMap {
    /// Load a Doom-format map by marker name (`E1M1`, `MAP01`, ...).
    ///
    /// Like vanilla `P_SetupLevel`, lumps are taken **by position** after the
    /// marker (marker+1 is THINGS, ...), not by name. Names that do not match
    /// the expected order are reported as warnings.
    pub fn load(stack: &ResourceStack, map: &str) -> Result<Self, ContentError> {
        let mut m = Self::load_unrepaired(stack, map)?;
        m.repair();
        Ok(m)
    }

    /// Fix authoring mistakes that ZDoom-family ports load anyway, with a
    /// warning each:
    /// - a linedef with only a back sidedef is flipped (vertices and sides
    ///   swapped; segs on it change side), so its one side becomes the front.
    pub fn repair(&mut self) {
        let mut flipped = vec![false; self.linedefs.len()];
        for (i, l) in self.linedefs.iter_mut().enumerate() {
            if l.sidenum[0] == NO_SIDEDEF && l.sidenum[1] != NO_SIDEDEF {
                std::mem::swap(&mut l.v1, &mut l.v2);
                l.sidenum.swap(0, 1);
                flipped[i] = true;
                self.warnings.push(format!("linedef {i} has only a back side; flipped"));
            }
        }
        for seg in &mut self.segs {
            if flipped.get(seg.linedef as usize).copied().unwrap_or(false) {
                seg.side ^= 1;
            }
        }
        // A linedef with no sidedefs at all (an invisible blocking line)
        // gets an untextured front side in the sector on its right.
        for i in 0..self.linedefs.len() {
            let l = self.linedefs[i];
            if l.sidenum != [NO_SIDEDEF; 2] {
                continue;
            }
            let (Some(a), Some(b)) = (self.vertexes.get(l.v1 as usize), self.vertexes.get(l.v2 as usize)) else { continue };
            let f = |v: i32| v as f64 / 65536.0;
            let (dx, dy) = (f(b.x) - f(a.x), f(b.y) - f(a.y));
            let len = dx.hypot(dy).max(1e-9);
            let (mx, my) = ((f(a.x) + f(b.x)) / 2.0, (f(a.y) + f(b.y)) / 2.0);
            let Some(sector) = self.point_sector(mx + dy / len, my - dx / len) else { continue };
            self.linedefs[i].sidenum[0] = self.sidedefs.len() as u32;
            self.sidedefs.push(MapSidedef {
                texture_offset: 0.0,
                row_offset: 0.0,
                top: LumpName::new("-"),
                bottom: LumpName::new("-"),
                mid: LumpName::new("-"),
                sector,
                part_offsets: [[0.0; 2]; 3],
                light: 0,
                light_absolute: false,
            });
            self.warnings.push(format!("linedef {i} has no sidedefs; given an empty front side"));
        }
    }

    /// The sector containing a point (map units), found through the map's
    /// nodes as `R_PointInSubsector` does.
    pub fn point_sector(&self, x: f64, y: f64) -> Option<u32> {
        let mut child = if self.nodes.is_empty() { NF_SUBSECTOR } else { self.nodes.len() as u32 - 1 };
        while child & NF_SUBSECTOR == 0 {
            let n = self.nodes.get(child as usize)?;
            let f = |v: i32| v as f64 / 65536.0;
            let (ddx, ddy) = (x - f(n.x), y - f(n.y));
            // R_PointOnSide: front (0) when the point is right of the line.
            let side = (f(n.dy) * ddx - f(n.dx) * ddy <= 0.0) as usize;
            child = n.children[side];
        }
        let ss = self.subsectors.get((child & !NF_SUBSECTOR) as usize)?;
        let segs = self.segs.get(ss.first_seg as usize..(ss.first_seg + ss.num_segs) as usize).unwrap_or(&[]);
        let side_of = |seg: &MapSeg| {
            let l = self.linedefs.get(seg.linedef as usize)?;
            let sd = *l.sidenum.get((seg.side & 1) as usize)?;
            (sd != NO_SIDEDEF).then_some(sd)
        };
        let sd = segs.iter().find_map(side_of).or(ss.sector_side)?;
        self.sidedefs.get(sd as usize).map(|s| s.sector)
    }

    fn load_unrepaired(stack: &ResourceStack, map: &str) -> Result<Self, ContentError> {
        let name = LumpName::new(map);
        let marker = stack.check_num(name).ok_or_else(|| ContentError::MissingLump(name.to_string()))?;
        let format = detect_format(stack, marker);
        match format {
            MapFormat::Doom | MapFormat::Hexen => {}
            MapFormat::Udmf => return crate::udmf::load(stack, marker, name),
        }

        let mut warnings = Vec::new();
        let mut lumps: [&[u8]; 10] = [&[]; 10];
        let mut id = marker;
        for (i, expected) in DOOM_MAP_LUMPS.iter().enumerate() {
            id = stack.next_in_source(id).ok_or_else(|| ContentError::MissingLump(format!("{name}/{expected}")))?;
            let actual = stack.info(id).name;
            if actual != LumpName::new(expected) {
                warnings.push(format!("{name}: lump {} is {actual}, expected {expected} (used positionally)", i + 1));
            }
            lumps[i] = stack.data(id);
        }
        let [things, linedefs, sidedefs, vertexes, segs, ssectors, nodes, sectors, reject, blockmap] = lumps;
        let hexen = format == MapFormat::Hexen;
        let behavior = if hexen { stack.next_in_source(id).map(|b| stack.data(b).to_vec()) } else { None };

        let things = if hexen {
            records("THINGS", things, 20, &mut warnings).map(hexen_thing).collect()
        } else {
            records("THINGS", things, 10, &mut warnings)
                .map(|c| MapThing {
                    x: fixed(le::i16(c, 0) as i32),
                    y: fixed(le::i16(c, 2) as i32),
                    z: 0,
                    angle: le::i16(c, 4),
                    kind: le::i16(c, 6),
                    options: le::i16(c, 8),
                    ..MapThing::default()
                })
                .collect()
        };
        // Index fields are unsigned shorts; 0xFFFF means "none" for sides.
        let side = |v: u16| if v == 0xFFFF { NO_SIDEDEF } else { v as u32 };
        let linedefs = if hexen {
            records("LINEDEFS", linedefs, 16, &mut warnings)
                .map(|c| hexen_linedef(c, [side(le::u16(c, 12)), side(le::u16(c, 14))]))
                .collect()
        } else {
            records("LINEDEFS", linedefs, 14, &mut warnings)
                .map(|c| MapLinedef {
                    v1: le::u16(c, 0) as u32,
                    v2: le::u16(c, 2) as u32,
                    flags: le::u16(c, 4),
                    special: le::i16(c, 6),
                    tag: le::i16(c, 8),
                    sidenum: [side(le::u16(c, 10)), side(le::u16(c, 12))],
                    alpha: 1.0,
                    action: None,
                    id: -1,
                })
                .collect()
        };
        let sidedefs = records("SIDEDEFS", sidedefs, 30, &mut warnings)
            .map(|c| MapSidedef {
                texture_offset: le::i16(c, 0) as f32,
                row_offset: le::i16(c, 2) as f32,
                top: LumpName::from_raw(&c[4..12]),
                bottom: LumpName::from_raw(&c[12..20]),
                mid: LumpName::from_raw(&c[20..28]),
                sector: le::u16(c, 28) as u32,
                part_offsets: [[0.0; 2]; 3],
                light: 0,
                light_absolute: false,
            })
            .collect();
        let vertexes: Vec<MapVertex> = records("VERTEXES", vertexes, 4, &mut warnings)
            .map(|c| MapVertex { x: fixed(le::i16(c, 0) as i32), y: fixed(le::i16(c, 2) as i32) })
            .collect();
        let sectors = records("SECTORS", sectors, 26, &mut warnings)
            .map(|c| {
                MapSector::doom(
                    fixed(le::i16(c, 0) as i32),
                    fixed(le::i16(c, 2) as i32),
                    LumpName::from_raw(&c[4..12]),
                    LumpName::from_raw(&c[12..20]),
                    le::i16(c, 20),
                    le::i16(c, 22),
                    le::i16(c, 24),
                )
            })
            .collect();
        let blockmap = records("BLOCKMAP", blockmap, 2, &mut warnings).map(|c| le::i16(c, 0)).collect();

        let mut map = RawMap {
            name,
            format: MapFormat::Doom,
            things,
            linedefs,
            sidedefs,
            vertexes,
            segs: Vec::new(),
            subsectors: Vec::new(),
            nodes: Vec::new(),
            sectors,
            reject: reject.to_vec(),
            blockmap,
            behavior,
            warnings,
        };
        match detect_node_format(nodes, segs.len(), ssectors.len()) {
            NodeFormat::Vanilla => {}
            NodeFormat::Zdoom(_) => {
                crate::nodes::load_zdoom(&mut map, nodes)?;
                return Ok(map);
            }
            other => {
                // Missing or unsupported nodes: build our own.
                if other != NodeFormat::Missing {
                    map.warnings.push(format!("{name}: {other:?} nodes not supported; rebuilt"));
                }
                crate::nodebuild::build_nodes(&mut map);
                return Ok(map);
            }
        }
        let mut warnings = std::mem::take(&mut map.warnings);
        let segs = records("SEGS", segs, 12, &mut warnings)
            .map(|c| MapSeg {
                v1: le::u16(c, 0) as u32,
                v2: le::u16(c, 2) as u32,
                // `SHORT(angle) << FRACBITS` into an unsigned angle_t.
                angle: ((le::i16(c, 4) as i32) << 16) as u32,
                linedef: le::u16(c, 6) as u32,
                side: le::i16(c, 8),
                offset: fixed(le::i16(c, 10) as i32),
            })
            .collect();
        let subsectors = records("SSECTORS", ssectors, 4, &mut warnings)
            .map(|c| MapSubsector { num_segs: le::u16(c, 0) as u32, first_seg: le::u16(c, 2) as u32, sector_side: None })
            .collect();
        let nodes = records("NODES", nodes, 28, &mut warnings)
            .map(|c| {
                let bb = |o: usize| {
                    [le::i16(c, o) as i32, le::i16(c, o + 2) as i32, le::i16(c, o + 4) as i32, le::i16(c, o + 6) as i32]
                };
                // Widen the 16-bit child flag to NF_SUBSECTOR.
                let child = |v: u16| if v & 0x8000 != 0 { (v & 0x7fff) as u32 | NF_SUBSECTOR } else { v as u32 };
                MapNode {
                    x: fixed(le::i16(c, 0) as i32),
                    y: fixed(le::i16(c, 2) as i32),
                    dx: fixed(le::i16(c, 4) as i32),
                    dy: fixed(le::i16(c, 6) as i32),
                    bbox: [bb(8), bb(16)],
                    children: [child(le::u16(c, 24)), child(le::u16(c, 26))],
                }
            })
            .collect();
        map.segs = segs;
        map.subsectors = subsectors;
        map.nodes = nodes;
        map.warnings = warnings;
        Ok(map)
    }
}

/// Map marker names present in the stack, in load order of their *last*
/// definition: `ExMy` and `MAPxx`.
pub fn list_maps(stack: &ResourceStack) -> Vec<LumpName> {
    let mut out: Vec<LumpName> = Vec::new();
    for i in 0..stack.num_lumps() {
        let n = stack.info(LumpId(i as u32)).name;
        if is_map_name(&n) && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

fn is_map_name(n: &LumpName) -> bool {
    let b = n.as_bytes();
    (b.len() == 4 && b[0] == b'E' && b[1].is_ascii_digit() && b[2] == b'M' && b[3].is_ascii_digit())
        || (b.len() == 5 && b.starts_with(b"MAP") && b[3].is_ascii_digit() && b[4].is_ascii_digit())
}

/// Hexen-format THINGS record (20 bytes): tid, x, y, height, angle, type,
/// flags, special, args.
fn hexen_thing(c: &[u8]) -> MapThing {
    let flags = le::u16(c, 12);
    // Hexen flags: 1/2/4 skills, 8 ambush, 256 single player. Our options
    // use vanilla's bits.
    let mut options = (flags & 0xf) as i16;
    if flags & 256 == 0 {
        options |= mtf::NOTSINGLE;
    }
    MapThing {
        tid: le::i16(c, 0) as i32,
        x: fixed(le::i16(c, 2) as i32),
        y: fixed(le::i16(c, 4) as i32),
        z: fixed(le::i16(c, 6) as i32),
        angle: le::i16(c, 8),
        kind: le::i16(c, 10),
        options,
        special: c[14] as i32,
        args: [c[15] as i32, c[16] as i32, c[17] as i32, c[18] as i32, c[19] as i32],
    }
}

/// Hexen-format LINEDEFS record (16 bytes): v1, v2, flags, special, args,
/// sides. The activation type lives in flag bits 10-12, "repeat" in 0x200.
fn hexen_linedef(c: &[u8], sidenum: [u32; 2]) -> MapLinedef {
    let raw = le::u16(c, 4);
    let special = c[6] as i32;
    let mut args = [c[7] as i32, c[8] as i32, c[9] as i32, c[10] as i32, c[11] as i32];
    let mut activation = match (raw >> 10) & 7 {
        0 => spac::CROSS,
        1 => spac::USE,
        2 => spac::MCROSS,
        3 => spac::IMPACT,
        4 => spac::PUSH,
        5 => spac::PCROSS,
        _ => 0,
    };
    // ZDoom: 0x2000 lets monsters activate it too.
    if raw & 0x2000 != 0 {
        activation |= match activation {
            spac::USE => spac::MUSE,
            spac::PUSH => spac::MPUSH,
            spac::CROSS => spac::MCROSS,
            _ => 0,
        };
    }
    let mut flags = raw & 0x1ff;
    if raw & 0x8000 != 0 {
        // "Block everything": closest vanilla behaviour.
        flags |= ml::BLOCKING;
    }
    // Line_SetIdentification(id, flags, , , id_high) only names the line.
    let mut id = -1;
    let mut action = Some(LineAction { special, args, activation, repeat: raw & 0x200 != 0, lock: 0 });
    if special == 121 {
        id = args[0] + (args[4] << 8);
        action = None;
    }
    if special == 0 {
        action = None;
    }
    if let Some(a) = &mut action {
        // TranslucentLine and friends name lines in arg 0 too; nothing to do here.
        a.args = std::mem::take(&mut args);
    }
    MapLinedef {
        v1: le::u16(c, 0) as u32,
        v2: le::u16(c, 2) as u32,
        flags,
        special: 0,
        tag: 0,
        sidenum,
        alpha: 1.0,
        action,
        id,
    }
}
