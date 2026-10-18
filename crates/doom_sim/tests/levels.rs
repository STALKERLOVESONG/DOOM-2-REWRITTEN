//! Level loading against real content. Skipped without an IWAD
//! (`DOOM_IWAD` or `WADS/doom1.wad`).

use std::path::PathBuf;

use doom_content::map::list_maps;
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_core::Fixed;
use doom_sim::geometry::point_in_subsector;
use doom_sim::level::Level;
use doom_sim::SubsectorId;

fn wad_path(name: &str, env: &str) -> Option<PathBuf> {
    let path = std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS").join(name));
    if path.exists() {
        Some(path)
    } else {
        eprintln!("skipping: no {name} at {}", path.display());
        None
    }
}

#[test]
fn all_maps_load_and_bsp_is_consistent() {
    let Some(iwad) = wad_path("doom1.wad", "DOOM_IWAD") else { return };
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();

    for name in list_maps(&stack) {
        let level = Level::load(&stack, &textures, &name.to_string()).unwrap();
        assert!(level.warnings.is_empty(), "{name}: {:?}", level.warnings);

        // The centroid of a subsector's seg endpoints lies inside the convex
        // subsector, so the BSP walk must find that same subsector (or, when
        // the centroid sits on a partition line, one of the same sector).
        // Subsectors whose segs are all collinear (common: one seg) have
        // their centroid on a boundary, so they are skipped.
        let (mut exact, mut checked) = (0, 0);
        for (i, ss) in level.subsectors.iter().enumerate() {
            let (mut sx, mut sy, mut n) = (0i64, 0i64, 0i64);
            let mut pts = Vec::new();
            for seg in &level.segs[ss.first_line as usize..(ss.first_line + ss.num_lines) as usize] {
                for v in [seg.v1, seg.v2] {
                    let p = level.vertex(v);
                    sx += p.x.0 as i64;
                    sy += p.y.0 as i64;
                    n += 1;
                    pts.push((p.x.to_int() as i64, p.y.to_int() as i64));
                }
            }
            let (ax, ay) = pts[0];
            let degenerate = pts.iter().all(|&(bx, by)| {
                pts.iter().all(|&(cx, cy)| (bx - ax) * (cy - ay) - (by - ay) * (cx - ax) == 0)
            });
            if degenerate {
                continue;
            }
            checked += 1;
            let (cx, cy) = (Fixed((sx / n) as i32), Fixed((sy / n) as i32));
            let found = point_in_subsector(&level, cx, cy);
            if found == SubsectorId(i as u32) {
                exact += 1;
            } else {
                assert_eq!(
                    level.subsectors[found.idx()].sector,
                    ss.sector,
                    "{name}: subsector {i} centroid resolved to {found:?} in another sector"
                );
            }
        }
        assert!(exact * 100 >= checked * 95, "{name}: only {exact}/{checked} exact");

        // Every sector has lines. (Block boxes are deliberately not checked:
        // degenerate sectors, e.g. E3M2 sector 21 with a single line, get an
        // inverted box from M_AddToBox's `else if`, exactly as in vanilla.)
        for (i, s) in level.sectors.iter().enumerate() {
            assert!(s.line_count > 0, "{name}: sector {i} has no lines");
        }
        assert_eq!(level.reject.len(), (level.sectors.len().pow(2) + 7) / 8);
    }
}

#[test]
fn udmf_map_with_gl_nodes_loads() {
    // MyHouse.wad: a UDMF (zdoom namespace) map with XGL3 nodes, PNG
    // textures in TX_START and DECORATE things, for Doom II.
    let (Some(iwad), Some(pwad)) = (wad_path("doom2.wad", "DOOM2_IWAD"), wad_path("myhouse.wad", "MYHOUSE_WAD")) else {
        return;
    };
    let stack = ResourceStack::load(&iwad, &[pwad]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let level = Level::load(&stack, &textures, "MAP01").unwrap();
    assert_eq!(level.lines.len(), 7892);
    assert_eq!(level.sides.len(), 14591);
    assert_eq!(level.num_map_sectors(), 1688);
    assert_eq!(level.things.len(), 81);
    assert!(level.vertexes.len() >= 6374);
    // Every seg belongs to a real linedef; every subsector has a sector.
    assert!(!level.segs.is_empty());
    assert!(level.subsectors.iter().all(|s| s.sector.idx() < level.num_map_sectors()));
    // The player start is inside the map and on a floor below its ceiling.
    let start = level.things.iter().find(|t| t.kind == 1).unwrap();
    let ss = doom_sim::geometry::point_in_subsector(&level, doom_core::Fixed(start.x), doom_core::Fixed(start.y));
    let sec = level.sector(level.subsectors[ss.idx()].sector);
    assert!(sec.ceiling_height > sec.floor_height);
    // A blockmap was built (UDMF maps have none).
    assert!(level.blockmap.width > 0 && level.blockmap.height > 0);
    // TILE01 is referenced by the map but exists in neither WAD (GZDoom
    // shows its missing-texture pattern too). Everything else resolves,
    // including wall textures used on floors and flats used on walls.
    let unknown: Vec<_> = level.warnings.iter().filter(|w| w.contains("not found") && !w.contains("TILE01")).collect();
    assert!(unknown.is_empty(), "missing resources: {unknown:?}");
}

#[test]
fn rebuilt_nodes_agree_with_shipped_nodes() {
    let Some(iwad) = wad_path("doom1.wad", "DOOM_IWAD") else { return };
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let res = doom_sim::level::LevelResources { stack: &stack, textures: &textures };
    for name in list_maps(&stack) {
        let raw = doom_content::map::RawMap::load(&stack, &name.to_string()).unwrap();
        let shipped = Level::from_raw(&raw, &res).unwrap();
        let mut rebuilt_raw = raw.clone();
        let t = std::time::Instant::now();
        doom_content::nodebuild::build_nodes(&mut rebuilt_raw);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let rebuilt = Level::from_raw(&rebuilt_raw, &res).unwrap();
        // Ground truth: just in front of each line side is that side's
        // sector. Both BSPs must agree with it (sectors thinner than the
        // probe distance may not; allow a few).
        let correct = |l: &Level| {
            let (mut ok, mut total) = (0, 0);
            for line in &l.lines {
                let (a, b) = (l.vertex(line.v1), l.vertex(line.v2));
                let (dx, dy) = ((b.x.0 - a.x.0) as f64, (b.y.0 - a.y.0) as f64);
                let len = dx.hypot(dy);
                if len < 4.0 * 65536.0 {
                    continue;
                }
                let (mx, my) = ((a.x.0 as f64 + b.x.0 as f64) / 2.0, (a.y.0 as f64 + b.y.0 as f64) / 2.0);
                for (k, side) in line.sidenum.iter().enumerate() {
                    let Some(side) = side else { continue };
                    // Right of v1->v2 for the front, left for the back.
                    let sgn = if k == 0 { 1.0 } else { -1.0 };
                    let (px, py) = (mx + sgn * dy / len * 32768.0, my - sgn * dx / len * 32768.0);
                    let got = l.subsectors[point_in_subsector(l, Fixed(px as i32), Fixed(py as i32)).idx()].sector;
                    total += 1;
                    ok += (got == l.side(*side).sector) as i32;
                }
            }
            ok as f64 / total as f64
        };
        let (rate_shipped, rate) = (correct(&shipped), correct(&rebuilt));
        eprintln!("{name}: {} nodes rebuilt in {ms:.1} ms; sides found: shipped {:.2}%, rebuilt {:.2}%", rebuilt.nodes.len(), rate_shipped * 100.0, rate * 100.0);
        assert!(rate >= rate_shipped - 0.001, "{name}: rebuilt nodes place {:.2}% of probes right", rate * 100.0);
    }
}
