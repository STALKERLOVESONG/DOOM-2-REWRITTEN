//! `P_SetupLevel` data loading: `P_LoadBlockMap`, `P_LoadVertexes`,
//! `P_LoadSectors`, `P_LoadSideDefs`, `P_LoadLineDefs`, `P_LoadSubsectors`,
//! `P_LoadNodes`, `P_LoadSegs`, `P_GroupLines`, `P_LoadReject`.
//!
//! Thing spawning (`P_LoadThings`) and `P_SpawnSpecials` belong to the
//! playsim and run after this.
//!
//! Where vanilla would dereference an out-of-range index (crash or garbage)
//! we return an error instead; this never rejects a map vanilla can run.

use std::fmt;

use doom_content::map::{RawMap, NO_SIDEDEF};
use doom_content::texture::TextureSet;
use doom_content::{ContentError, LumpName, Namespace, ResourceStack};
use doom_core::bbox::{BOXBOTTOM, BOXLEFT, BOXRIGHT, BOXTOP};
use doom_core::{Angle, BBox, Fixed};

use super::*;

#[derive(Debug)]
pub enum LevelLoadError {
    Content(ContentError),
    Invalid { map: String, reason: String },
}

impl fmt::Display for LevelLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Content(e) => e.fmt(f),
            Self::Invalid { map, reason } => write!(f, "map {map} is invalid: {reason}"),
        }
    }
}

impl std::error::Error for LevelLoadError {}

impl From<ContentError> for LevelLoadError {
    fn from(e: ContentError) -> Self {
        Self::Content(e)
    }
}

/// Content needed to resolve texture and flat names to numbers.
pub struct LevelResources<'a> {
    pub stack: &'a ResourceStack,
    pub textures: &'a TextureSet,
}

impl LevelResources<'_> {
    /// `R_FlatNumForName`, resolved in the merged flat namespace. ZDoom
    /// also allows wall textures on floors and ceilings: those get flat
    /// numbers past the namespace, `num_flats + texture number`.
    fn flat_num(&self, name: LumpName, warnings: &mut Vec<String>) -> FlatNum {
        match self.stack.namespace_index(Namespace::Flats, name) {
            Some(i) => FlatNum(i as u16),
            None => match self.textures.check_num(name).filter(|&t| t != 0) {
                Some(t) => FlatNum((self.stack.namespace(Namespace::Flats).len() + t) as u16),
                None => {
                    warnings.push(format!("flat {name} not found"));
                    FlatNum::MISSING
                }
            },
        }
    }

    /// `R_TextureNumForName`. Missing textures become 0 ("no texture") with a
    /// warning; vanilla aborts.
    fn tex_num(&self, name: LumpName, warnings: &mut Vec<String>) -> TexNum {
        match self.textures.check_num(name) {
            Some(i) => TexNum(i as u16),
            None => {
                warnings.push(format!("texture {name} not found"));
                TexNum(0)
            }
        }
    }
}

/// Floor/ceiling of the "null sector" read through a NULL pointer by the
/// glass-hack (two-sided line without a back sidedef). These are the bytes
/// Chocolate Doom's default DOS 7.1 memory emulation returns for offsets 0
/// and 4.
const NULL_SECTOR_FLOOR: Fixed = Fixed(0x00C9_0F9E);
const NULL_SECTOR_CEILING: Fixed = Fixed(0x0070_0465);

impl Level {
    pub fn load(stack: &ResourceStack, textures: &TextureSet, map: &str) -> Result<Self, LevelLoadError> {
        let raw = RawMap::load(stack, map)?;
        let mut level = Self::from_raw(&raw, &LevelResources { stack, textures })?;
        let (acs_map, acs_libs) = crate::acs_host::load_modules(stack, map, raw.behavior.clone(), &mut level.warnings);
        level.acs_map = acs_map;
        level.acs_libs = acs_libs;
        Ok(level)
    }

    pub fn from_raw(raw: &RawMap, res: &LevelResources) -> Result<Self, LevelLoadError> {
        let map = raw.name.to_string();
        let invalid = |reason: String| LevelLoadError::Invalid { map: map.clone(), reason };
        let mut warnings = raw.warnings.clone();

        // --- P_LoadVertexes -------------------------------------------------
        let vertexes: Vec<Vertex> = raw.vertexes.iter().map(|v| Vertex { x: Fixed(v.x), y: Fixed(v.y) }).collect();

        // --- P_LoadBlockMap (built when missing, as Boom and ZDoom do) -------
        let blockmap = if raw.blockmap.is_empty() {
            build_blockmap(&vertexes, &raw.linedefs)
        } else {
            Blockmap::from_lump(&raw.blockmap).ok_or_else(|| invalid("BLOCKMAP header does not fit the lump".into()))?
        };
        let (bw, bh) = (blockmap.width, blockmap.height);

        // --- P_LoadSectors --------------------------------------------------
        let mut sectors: Vec<Sector> = raw
            .sectors
            .iter()
            .map(|s| Sector {
                floor_height: Fixed(s.floor_height),
                ceiling_height: Fixed(s.ceiling_height),
                floor_pic: res.flat_num(s.floor_pic, &mut warnings),
                ceiling_pic: res.flat_num(s.ceiling_pic, &mut warnings),
                light_level: s.light_level,
                special: s.special,
                tag: s.tag,
                sound_traversed: 0,
                sound_target: None,
                blockbox: [0; 4],
                sound_org: SoundOrigin::default(),
                thing_list: None,
                special_data: None,
                ceiling_data: None,
                lines_start: 0,
                line_count: 0,
                floor_xform: s.floor_xform,
                ceiling_xform: s.ceiling_xform,
                floor_light: s.floor_light,
                ceiling_light: s.ceiling_light,
            })
            .collect();
        let num_map_sectors = sectors.len();

        // --- P_LoadSideDefs -------------------------------------------------
        let mut sides = Vec::with_capacity(raw.sidedefs.len());
        for (i, s) in raw.sidedefs.iter().enumerate() {
            if s.sector as usize >= num_map_sectors {
                return Err(invalid(format!("sidedef {i} references missing sector {}", s.sector)));
            }
            let units = |v: f32| Fixed((v as f64 * 65536.0).round() as i32);
            sides.push(Side {
                texture_offset: units(s.texture_offset),
                row_offset: units(s.row_offset),
                top_texture: res.tex_num(s.top, &mut warnings),
                bottom_texture: res.tex_num(s.bottom, &mut warnings),
                mid_texture: res.tex_num(s.mid, &mut warnings),
                sector: SectorId(s.sector),
                part_offsets: s.part_offsets,
                light: (s.light, s.light_absolute),
            });
        }

        // --- P_LoadLineDefs -------------------------------------------------
        let side_ref = |n: u32| (n != NO_SIDEDEF).then_some(n as usize);
        let mut lines = Vec::with_capacity(raw.linedefs.len());
        for (i, l) in raw.linedefs.iter().enumerate() {
            let (Some(&a), Some(&b)) = (vertexes.get(l.v1 as usize), vertexes.get(l.v2 as usize)) else {
                return Err(invalid(format!("linedef {i} references a missing vertex")));
            };
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let slope_type = if dx.0 == 0 {
                SlopeType::Vertical
            } else if dy.0 == 0 {
                SlopeType::Horizontal
            } else if (dy / dx).0 > 0 {
                SlopeType::Positive
            } else {
                SlopeType::Negative
            };
            let mut bbox = BBox::cleared();
            (bbox.0[BOXLEFT], bbox.0[BOXRIGHT]) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
            (bbox.0[BOXBOTTOM], bbox.0[BOXTOP]) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };

            let mut sidenum = [None; 2];
            for k in 0..2 {
                if let Some(s) = side_ref(l.sidenum[k]) {
                    if s >= sides.len() {
                        return Err(invalid(format!("linedef {i} references missing sidedef {s}")));
                    }
                    sidenum[k] = Some(SideId(s as u32));
                }
            }
            let Some(front) = sidenum[0] else {
                // Vanilla dereferences NULL in P_GroupLines.
                return Err(invalid(format!("linedef {i} has no front sidedef")));
            };
            lines.push(Line {
                v1: VertexId(l.v1),
                v2: VertexId(l.v2),
                dx,
                dy,
                flags: l.flags,
                special: if l.action.is_some() { HEXEN_LINE } else { l.special },
                tag: l.tag,
                sidenum,
                bbox,
                slope_type,
                front_sector: Some(sides[front.idx()].sector),
                back_sector: sidenum[1].map(|s| sides[s.idx()].sector),
                special_data: None,
                action: l.action,
                id: l.id,
            });
        }

        // --- P_LoadSubsectors / P_LoadNodes ----------------------------------
        let mut subsectors = Vec::with_capacity(raw.subsectors.len());
        for (i, s) in raw.subsectors.iter().enumerate() {
            let empty_ok = s.num_segs == 0 && s.sector_side.is_some_and(|sd| (sd as usize) < sides.len());
            if (s.num_segs == 0 && !empty_ok) || s.first_seg as usize + s.num_segs as usize > raw.segs.len() {
                return Err(invalid(format!("subsector {i} has an invalid seg range")));
            }
            subsectors.push(Subsector { sector: SectorId(0), num_lines: s.num_segs, first_line: s.first_seg });
        }
        let mut nodes = Vec::with_capacity(raw.nodes.len());
        for (i, n) in raw.nodes.iter().enumerate() {
            for &c in &n.children {
                let ok = if c & NF_SUBSECTOR != 0 {
                    ((c & !NF_SUBSECTOR) as usize) < subsectors.len()
                } else {
                    (c as usize) < raw.nodes.len()
                };
                if !ok {
                    return Err(invalid(format!("node {i} has an out-of-range child {c:#06x}")));
                }
            }
            let bb = |b: [i32; 4]| BBox(b.map(Fixed::from_int));
            nodes.push(Node {
                x: Fixed(n.x),
                y: Fixed(n.y),
                dx: Fixed(n.dx),
                dy: Fixed(n.dy),
                bbox: [bb(n.bbox[0]), bb(n.bbox[1])],
                children: n.children,
            });
        }

        // --- P_LoadSegs ------------------------------------------------------
        let mut null_sector = None;
        let mut segs = Vec::with_capacity(raw.segs.len());
        for (i, s) in raw.segs.iter().enumerate() {
            if s.v1 as usize >= vertexes.len() || s.v2 as usize >= vertexes.len() {
                return Err(invalid(format!("seg {i} references a missing vertex")));
            }
            let Some(ldef) = lines.get(s.linedef as usize) else {
                return Err(invalid(format!("seg {i} references missing linedef {}", s.linedef)));
            };
            // Vanilla indexes sidenum[side] with whatever `side` holds; only
            // 0 and 1 are meaningful.
            let side = (s.side & 1) as usize;
            if s.side != side as i16 {
                warnings.push(format!("seg {i} has side value {}", s.side));
            }
            let Some(sidedef) = ldef.sidenum[side] else {
                return Err(invalid(format!(
                    "linedef {} for seg {i} references a non-existent sidedef",
                    s.linedef
                )));
            };
            let back_sector = if ldef.flags & doom_content::map::ml::TWOSIDED != 0 {
                match ldef.sidenum[side ^ 1] {
                    Some(b) => Some(sides[b.idx()].sector),
                    None => {
                        // "Glass hack": see Seg::back_sector.
                        Some(*null_sector.get_or_insert(SectorId(num_map_sectors as u32)))
                    }
                }
            } else {
                None
            };
            segs.push(Seg {
                v1: VertexId(s.v1),
                v2: VertexId(s.v2),
                angle: Angle(s.angle),
                offset: Fixed(s.offset),
                sidedef,
                linedef: LineId(s.linedef),
                front_sector: sides[sidedef.idx()].sector,
                back_sector,
            });
        }
        if let Some(id) = null_sector {
            warnings.push("two-sided line without back sidedef (glass hack); using emulated null sector".into());
            debug_assert_eq!(id.idx(), sectors.len());
            sectors.push(Sector {
                floor_height: NULL_SECTOR_FLOOR,
                ceiling_height: NULL_SECTOR_CEILING,
                floor_pic: FlatNum(0),
                ceiling_pic: FlatNum(0),
                light_level: 0,
                special: 0,
                tag: 0,
                sound_traversed: 0,
                sound_target: None,
                blockbox: [0; 4],
                sound_org: SoundOrigin::default(),
                thing_list: None,
                special_data: None,
                ceiling_data: None,
                lines_start: 0,
                line_count: 0,
                floor_xform: Default::default(),
                ceiling_xform: Default::default(),
                floor_light: (0, false),
                ceiling_light: (0, false),
            });
        }

        // --- P_GroupLines ----------------------------------------------------
        for (ss, r) in subsectors.iter_mut().zip(&raw.subsectors) {
            ss.sector = match r.sector_side {
                Some(sd) if ss.num_lines == 0 => sides[sd as usize].sector,
                _ => sides[segs[ss.first_line as usize].sidedef.idx()].sector,
            };
        }
        let mut counts = vec![0u32; sectors.len()];
        for l in &lines {
            counts[l.front_sector.unwrap().idx()] += 1;
            if let Some(b) = l.back_sector {
                if Some(b) != l.front_sector {
                    counts[b.idx()] += 1;
                }
            }
        }
        let mut start = 0;
        for (s, &c) in sectors.iter_mut().zip(&counts) {
            s.lines_start = start;
            start += c;
        }
        let mut sector_lines = vec![LineId(0); start as usize];
        for (i, l) in lines.iter().enumerate() {
            let mut push = |s: SectorId| {
                let sec = &mut sectors[s.idx()];
                sector_lines[(sec.lines_start + sec.line_count) as usize] = LineId(i as u32);
                sec.line_count += 1;
            };
            let front = l.front_sector.unwrap();
            push(front);
            if let Some(b) = l.back_sector {
                if b != front {
                    push(b);
                }
            }
        }
        for s in sectors.iter_mut() {
            let mut bbox = BBox::cleared();
            for &li in &sector_lines[s.lines_start as usize..(s.lines_start + s.line_count) as usize] {
                let l = &lines[li.idx()];
                let (a, b) = (vertexes[l.v1.idx()], vertexes[l.v2.idx()]);
                bbox.add_point(a.x, a.y);
                bbox.add_point(b.x, b.y);
            }
            s.sound_org.x = (bbox.right() + bbox.left()) / 2;
            s.sound_org.y = (bbox.top() + bbox.bottom()) / 2;

            let (ox, oy) = (blockmap.org_x, blockmap.org_y);
            let block = |v: Fixed| v.0 >> MAPBLOCKSHIFT;
            s.blockbox[BOXTOP] = block(bbox.top() - oy + MAXRADIUS).min(bh - 1);
            s.blockbox[BOXBOTTOM] = block(bbox.bottom() - oy - MAXRADIUS).max(0);
            s.blockbox[BOXRIGHT] = block(bbox.right() - ox + MAXRADIUS).min(bw - 1);
            s.blockbox[BOXLEFT] = block(bbox.left() - ox - MAXRADIUS).max(0);
        }

        // --- P_LoadReject ----------------------------------------------------
        let total_lines = lines
            .iter()
            .map(|l| 1 + (l.back_sector.is_some() && l.back_sector != l.front_sector) as usize)
            .sum::<usize>();
        let reject = if raw.reject.is_empty() && raw.format != doom_content::map::MapFormat::Doom {
            // UDMF and other modern maps: no REJECT means nothing is rejected.
            vec![0; (num_map_sectors * num_map_sectors).div_ceil(8)]
        } else {
            pad_reject(&raw.reject, num_map_sectors, total_lines, &mut warnings)
        };

        let valid = ValidCount::new(lines.len(), sectors.len());
        let sky_flat = res.stack.namespace_index(Namespace::Flats, LumpName::new("F_SKY1")).map_or(FlatNum::MISSING, |i| FlatNum(i as u16));
        Ok(Level {
            name: map,
            vertexes,
            sectors,
            sides,
            lines,
            segs,
            subsectors,
            nodes,
            blockmap,
            reject,
            sector_lines,
            null_sector,
            valid,
            sky_flat,
            things: raw.things.clone(),
            warnings,
            acs_map: None,
            acs_libs: Vec::new(),
        })
    }
}

/// Build a blockmap for maps that ship without one (UDMF): 128-unit blocks
/// over the vertex bounds, each list starting with the vanilla `0`
/// delimiter like Boom's builder.
fn build_blockmap(vertexes: &[Vertex], lines: &[doom_content::map::MapLinedef]) -> Blockmap {
    let (mut minx, mut miny, mut maxx, mut maxy) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for v in vertexes {
        minx = minx.min(v.x.0 >> 16);
        miny = miny.min(v.y.0 >> 16);
        maxx = maxx.max(v.x.0 >> 16);
        maxy = maxy.max(v.y.0 >> 16);
    }
    if minx > maxx {
        (minx, miny, maxx, maxy) = (0, 0, 0, 0);
    }
    let (ox, oy) = (minx - 8, miny - 8);
    let width = ((maxx - ox) >> 7) + 1;
    let height = ((maxy - oy) >> 7) + 1;
    let mut lists: Vec<Vec<u32>> = vec![Vec::new(); (width * height) as usize];
    for (i, l) in lines.iter().enumerate() {
        let (Some(a), Some(b)) = (vertexes.get(l.v1 as usize), vertexes.get(l.v2 as usize)) else { continue };
        let f = |v: Fixed| v.0 as f64 / 65536.0;
        let (x1, y1, x2, y2) = (f(a.x) - ox as f64, f(a.y) - oy as f64, f(b.x) - ox as f64, f(b.y) - oy as f64);
        let bx0 = ((x1.min(x2) / 128.0).floor() as i32).max(0);
        let bx1 = ((x1.max(x2) / 128.0).floor() as i32).min(width - 1);
        let by0 = ((y1.min(y2) / 128.0).floor() as i32).max(0);
        let by1 = ((y1.max(y2) / 128.0).floor() as i32).min(height - 1);
        for by in by0..=by1 {
            for bx in bx0..=bx1 {
                // Does the segment touch this block's square? Test the four
                // corners against the line, plus the bbox overlap above.
                let (cx0, cy0) = (bx as f64 * 128.0, by as f64 * 128.0);
                let side = |px: f64, py: f64| (x2 - x1) * (py - y1) - (y2 - y1) * (px - x1);
                let s = [side(cx0, cy0), side(cx0 + 128.0, cy0), side(cx0, cy0 + 128.0), side(cx0 + 128.0, cy0 + 128.0)];
                let all_pos = s.iter().all(|&v| v > 0.0);
                let all_neg = s.iter().all(|&v| v < 0.0);
                if !all_pos && !all_neg {
                    lists[(by * width + bx) as usize].push(i as u32);
                }
            }
        }
    }
    let mut offsets = Vec::with_capacity(lists.len());
    let mut words = Vec::new();
    for list in &lists {
        offsets.push(words.len() as u32);
        words.push(0);
        words.extend_from_slice(list);
        words.push(Blockmap::END);
    }
    Blockmap {
        org_x: Fixed::from_int(ox),
        org_y: Fixed::from_int(oy),
        width,
        height,
        offsets,
        words,
        links: vec![None; (width * height) as usize],
    }
}

/// `P_LoadReject` + `PadRejectArray`: a short REJECT lump is extended with the
/// bytes vanilla would have read from the zone memory that follows it.
fn pad_reject(lump: &[u8], num_sectors: usize, total_lines: usize, warnings: &mut Vec<String>) -> Vec<u8> {
    let min_len = (num_sectors * num_sectors).div_ceil(8);
    let mut out = lump.to_vec();
    if out.len() >= min_len {
        return out;
    }
    warnings.push(format!("REJECT is {} bytes, expected {min_len}; padded", lump.len()));
    let pad: [u32; 4] = [
        (((total_lines * 4 + 3) & !3) + 24) as u32, // zone block size
        0,                                          // part of the zone block header
        50,                                         // PU_LEVEL
        0x1d4a11,                                   // ZONEID
    ];
    let len = min_len - out.len();
    for i in 0..len {
        if i < 16 {
            out.push((pad[i / 4] >> ((i % 4) * 8)) as u8);
        } else {
            out.push(0); // Chocolate's default beyond the emulated header
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_padding_matches_chocolate() {
        let mut w = Vec::new();
        let r = pad_reject(&[], 9, 10, &mut w); // 81 bits -> 11 bytes
        assert_eq!(r.len(), 11);
        // size = ((10*4+3)&~3)+24 = 64
        assert_eq!(&r[..8], &[64, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&r[8..11], &[50, 0, 0]);
    }
}
