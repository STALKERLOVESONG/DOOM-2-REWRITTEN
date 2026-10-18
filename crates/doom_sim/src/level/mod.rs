//! Runtime level data (r_defs.h): the map as the simulation and renderer see
//! it after `P_SetupLevel`.

mod setup;

use doom_core::{Angle, BBox, Fixed};

use crate::ids::*;

pub use setup::{LevelLoadError, LevelResources};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub x: Fixed,
    pub y: Fixed,
}

/// `degenmobj_t`: a sector's sound origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundOrigin {
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
}

#[derive(Clone, Debug)]
pub struct Sector {
    pub floor_height: Fixed,
    pub ceiling_height: Fixed,
    pub floor_pic: FlatNum,
    pub ceiling_pic: FlatNum,
    pub light_level: i16,
    pub special: i16,
    pub tag: i16,
    /// 0 = untraversed, 1, 2 = sound lines - 1 (`P_RecursiveSound`).
    pub sound_traversed: i32,
    /// Thing that made a sound (monsters wake up on it).
    pub sound_target: Option<MobjId>,
    /// Blockmap bounding box (block coords) used for height changes.
    pub blockbox: [i32; 4],
    pub sound_org: SoundOrigin,
    /// Head of the sector's thing list (`thinglist`, linked via mobj `snext`).
    pub thing_list: Option<MobjId>,
    /// Active floor/ceiling mover (`specialdata`).
    pub special_data: Option<ThinkerId>,
    /// A ceiling mover started by a Hexen-style special. ZDoom moves floors
    /// and ceilings independently; vanilla specials only use
    /// `special_data`, so their behaviour is unchanged.
    pub ceiling_data: Option<ThinkerId>,
    /// Range into [`Level::sector_lines`] (`lines`/`linecount`).
    pub lines_start: u32,
    pub line_count: u32,
    /// Presentation extras from UDMF (defaults for Doom-format maps):
    /// flat placement and per-plane light.
    pub floor_xform: doom_content::map::PlaneXform,
    pub ceiling_xform: doom_content::map::PlaneXform,
    /// (light, absolute): `absolute` replaces the sector light, otherwise
    /// it is added.
    pub floor_light: (i16, bool),
    pub ceiling_light: (i16, bool),
}

#[derive(Clone, Debug)]
pub struct Side {
    pub texture_offset: Fixed,
    pub row_offset: Fixed,
    pub top_texture: TexNum,
    pub bottom_texture: TexNum,
    pub mid_texture: TexNum,
    pub sector: SectorId,
    /// UDMF per-part offsets added to the base ones (map units), [top, mid,
    /// bottom] x (x, y). The classic renderer ignores them.
    pub part_offsets: doom_content::map::PartOffsets,
    /// UDMF side light: (light, absolute).
    pub light: (i16, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlopeType {
    Horizontal,
    Vertical,
    Positive,
    Negative,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub v1: VertexId,
    pub v2: VertexId,
    /// Precalculated `v2 - v1`.
    pub dx: Fixed,
    pub dy: Fixed,
    pub flags: u16,
    pub special: i16,
    pub tag: i16,
    /// Front and back sidedefs (`sidenum[2]`, `-1` -> `None`).
    pub sidenum: [Option<SideId>; 2],
    pub bbox: BBox,
    pub slope_type: SlopeType,
    pub front_sector: Option<SectorId>,
    pub back_sector: Option<SectorId>,
    /// Active special thinker (`specialdata`); used by some line actions.
    pub special_data: Option<ThinkerId>,
    /// Hexen-style special (Hexen-format and UDMF maps). Such lines have
    /// `special == HEXEN_LINE` so the vanilla trigger checks still find
    /// them; the trigger then runs `crate::hexen`.
    pub action: Option<doom_content::map::LineAction>,
    /// Line ID (UDMF `id`, `Line_SetIdentification`); -1 = none.
    pub id: i32,
}

/// `Line::special` of a line whose special is a Hexen-style `action`.
pub const HEXEN_LINE: i16 = -1;

impl Line {
    #[inline]
    pub fn is_two_sided(&self) -> bool {
        self.sidenum[1].is_some()
    }
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub v1: VertexId,
    pub v2: VertexId,
    pub offset: Fixed,
    pub angle: Angle,
    pub sidedef: SideId,
    pub linedef: LineId,
    pub front_sector: SectorId,
    /// `None` for one-sided lines. For the "glass hack" (two-sided flag with
    /// a missing back sidedef) this points at [`Level::null_sector`].
    pub back_sector: Option<SectorId>,
}

#[derive(Clone, Debug)]
pub struct Subsector {
    pub sector: SectorId,
    pub num_lines: u32,
    pub first_line: u32,
}

/// Child index flag: the child is a subsector (32-bit, as in ZDoom's
/// extended nodes; vanilla's 16-bit flag is widened on load).
pub const NF_SUBSECTOR: u32 = doom_content::map::NF_SUBSECTOR;

#[derive(Clone, Debug)]
pub struct Node {
    pub x: Fixed,
    pub y: Fixed,
    pub dx: Fixed,
    pub dy: Fixed,
    /// Bounding box per child (right = 0, left = 1).
    pub bbox: [BBox; 2],
    /// Child node index, or subsector index with [`NF_SUBSECTOR`] set.
    pub children: [u32; 2],
}

impl Level {
    /// The root of the BSP tree: the last node, or subsector 0 for a map
    /// with a single subsector (vanilla passes `numnodes - 1 == -1`).
    pub fn bsp_root(&self) -> u32 {
        if self.nodes.is_empty() {
            NF_SUBSECTOR
        } else {
            (self.nodes.len() - 1) as u32
        }
    }
}

pub const MAPBLOCKUNITS: i32 = 128;
pub const MAPBLOCKSIZE: i32 = MAPBLOCKUNITS * doom_core::FRACUNIT;
pub const MAPBLOCKSHIFT: u32 = doom_core::FRACBITS as u32 + 7;
pub const MAPBMASK: i32 = MAPBLOCKSIZE - 1;
pub const MAPBTOFRAC: u32 = MAPBLOCKSHIFT - doom_core::FRACBITS as u32;
/// Largest thing radius, used to pad sector block boxes.
pub const MAXRADIUS: Fixed = Fixed(32 * doom_core::FRACUNIT);

/// The block map: per-block line lists plus the per-block mobj chains
/// (`blocklinks`).
#[derive(Clone, Debug)]
pub struct Blockmap {
    pub org_x: Fixed,
    pub org_y: Fixed,
    pub width: i32,
    pub height: i32,
    /// Start of each block's list in `words` (`u32::MAX`: no list).
    pub offsets: Vec<u32>,
    /// Line numbers; each list ends with [`Blockmap::END`].
    pub words: Vec<u32>,
    /// Head of each block's mobj chain (linked via mobj `bnext`).
    pub links: Vec<Option<MobjId>>,
}

impl Blockmap {
    pub const END: u32 = u32::MAX;

    /// From a BLOCKMAP lump (16-bit words). Offsets are read as signed
    /// shorts like vanilla (`int offset = *(short*)...`), so a negative one
    /// gives an empty list; line numbers are unsigned.
    pub fn from_lump(lump: &[i16]) -> Option<Blockmap> {
        if lump.len() < 4 {
            return None;
        }
        let (w, h) = (lump[2] as i32, lump[3] as i32);
        if w <= 0 || h <= 0 || lump.len() < 4 + (w * h) as usize {
            return None;
        }
        let offsets = lump[4..4 + (w * h) as usize]
            .iter()
            .map(|&o| if o < 0 || o as usize >= lump.len() { u32::MAX } else { o as u32 })
            .collect();
        let words = lump.iter().map(|&v| if v == -1 { Self::END } else { v as u16 as u32 }).collect();
        Some(Blockmap {
            org_x: Fixed::from_int(lump[0] as i32),
            org_y: Fixed::from_int(lump[1] as i32),
            width: w,
            height: h,
            offsets,
            words,
            links: vec![None; (w * h) as usize],
        })
    }

    /// Line-list words for block (x, y), ending before the terminator.
    ///
    /// Vanilla quirk preserved: every list begins with a `0` delimiter that
    /// `P_BlockLinesIterator` does *not* skip, so linedef 0 is visited from
    /// every block (built blockmaps include it too, like Boom's).
    pub fn block_list(&self, x: i32, y: i32) -> &[u32] {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return &[];
        }
        let start = self.offsets[(y * self.width + x) as usize];
        if start == u32::MAX {
            return &[];
        }
        let list = &self.words[start as usize..];
        let end = list.iter().position(|&w| w == Self::END).unwrap_or(list.len());
        &list[..end]
    }

    /// Block coordinate for a map coordinate: `(v - org) >> MAPBLOCKSHIFT`.
    #[inline]
    pub fn block_x(&self, x: Fixed) -> i32 {
        (x - self.org_x).0 >> MAPBLOCKSHIFT
    }
    #[inline]
    pub fn block_y(&self, y: Fixed) -> i32 {
        (y - self.org_y).0 >> MAPBLOCKSHIFT
    }
}

/// Level-wide `validcount` bookkeeping. Kept apart from the geometry so that
/// iteration helpers can mutate it while callbacks read the level.
#[derive(Clone, Debug, Default)]
pub struct ValidCount {
    pub current: i32,
    pub lines: Vec<i32>,
    pub sectors: Vec<i32>,
}

impl ValidCount {
    pub fn new(num_lines: usize, num_sectors: usize) -> Self {
        Self { current: 0, lines: vec![0; num_lines], sectors: vec![0; num_sectors] }
    }

    /// `validcount++`.
    #[inline]
    pub fn bump(&mut self) {
        self.current = self.current.wrapping_add(1);
    }
}

#[derive(Clone, Debug)]
pub struct Level {
    pub name: String,
    pub vertexes: Vec<Vertex>,
    pub sectors: Vec<Sector>,
    pub sides: Vec<Side>,
    pub lines: Vec<Line>,
    pub segs: Vec<Seg>,
    pub subsectors: Vec<Subsector>,
    pub nodes: Vec<Node>,
    pub blockmap: Blockmap,
    /// REJECT matrix, padded to `(numsectors^2 + 7) / 8` bytes exactly like
    /// Chocolate Doom's `PadRejectArray` (emulating the memory vanilla read
    /// past a short lump).
    pub reject: Vec<u8>,
    /// Backing store for every sector's line list.
    pub sector_lines: Vec<LineId>,
    /// Sector used as the back sector of "glass hack" segs; see [`Seg`].
    /// Index `sectors.len() - 1` when present (it is appended last so it
    /// never collides with map sector numbers).
    pub null_sector: Option<SectorId>,
    pub valid: ValidCount,
    /// `skyflatnum`: flat number of `F_SKY1`.
    pub sky_flat: FlatNum,
    /// Raw THINGS, kept for spawning (the playsim consumes them).
    pub things: Vec<doom_content::map::MapThing>,
    /// Non-fatal problems found while building the level.
    pub warnings: Vec<String>,
    /// The map's compiled ACS and the libraries it runs with (taken by the
    /// world when the level starts).
    pub acs_map: Option<doom_script::acs::Module>,
    pub acs_libs: Vec<doom_script::acs::Module>,
}

impl Level {
    /// Placeholder with no geometry, used while swapping levels.
    pub fn empty() -> Self {
        Level {
            name: String::new(),
            vertexes: Vec::new(),
            sectors: Vec::new(),
            sides: Vec::new(),
            lines: Vec::new(),
            segs: Vec::new(),
            subsectors: Vec::new(),
            nodes: Vec::new(),
            blockmap: Blockmap {
                org_x: Fixed(0),
                org_y: Fixed(0),
                width: 0,
                height: 0,
                offsets: Vec::new(),
                words: Vec::new(),
                links: Vec::new(),
            },
            reject: Vec::new(),
            sector_lines: Vec::new(),
            null_sector: None,
            valid: ValidCount::default(),
            sky_flat: FlatNum::MISSING,
            things: Vec::new(),
            warnings: Vec::new(),
            acs_map: None,
            acs_libs: Vec::new(),
        }
    }

    #[inline]
    pub fn vertex(&self, v: VertexId) -> Vertex {
        self.vertexes[v.idx()]
    }
    #[inline]
    pub fn sector(&self, s: SectorId) -> &Sector {
        &self.sectors[s.idx()]
    }
    #[inline]
    pub fn sector_mut(&mut self, s: SectorId) -> &mut Sector {
        &mut self.sectors[s.idx()]
    }
    #[inline]
    pub fn line(&self, l: LineId) -> &Line {
        &self.lines[l.idx()]
    }
    #[inline]
    pub fn side(&self, s: SideId) -> &Side {
        &self.sides[s.idx()]
    }

    /// Lines bordering a sector (`sector->lines`).
    pub fn lines_of(&self, s: SectorId) -> &[LineId] {
        let sec = &self.sectors[s.idx()];
        &self.sector_lines[sec.lines_start as usize..(sec.lines_start + sec.line_count) as usize]
    }

    /// Number of real map sectors (excluding the synthetic null sector).
    pub fn num_map_sectors(&self) -> usize {
        self.sectors.len() - self.null_sector.is_some() as usize
    }

    /// REJECT lookup as in `P_CheckSight`: true if `s1` can never see `s2`.
    pub fn rejected(&self, s1: SectorId, s2: SectorId) -> bool {
        let n = self.num_map_sectors();
        let pnum = s1.idx() * n + s2.idx();
        let byte = pnum >> 3;
        self.reject.get(byte).is_some_and(|b| b & (1 << (pnum & 7)) != 0)
    }
}
