//! The perspective renderer's subsector polygons must cover every
//! subsector: an empty polygon is a hole in the floor and ceiling.

use std::path::Path;

use doom_content::map::list_maps;
use doom_content::texture::TextureSet;
use doom_content::ResourceStack;
use doom_render::perspective::subsector_polygons;
use doom_sim::level::Level;

#[test]
fn every_subsector_has_a_polygon() {
    let iwad = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad");
    if !iwad.exists() {
        eprintln!("skipped: no IWAD");
        return;
    }
    let stack = ResourceStack::load(&iwad, &[]).unwrap();
    let textures = TextureSet::load(&stack).unwrap();
    let mut bad = Vec::new();
    for name in list_maps(&stack) {
        let level = Level::load(&stack, &textures, &name.to_string()).unwrap();
        let polys = subsector_polygons(&level);
        for (i, p) in polys.iter().enumerate() {
            if p.len() < 3 {
                bad.push(format!("{name} subsector {i} ({} segs)", level.subsectors[i].num_lines));
            }
        }
    }
    assert!(bad.is_empty(), "{} empty polygons: {:?}", bad.len(), &bad[..bad.len().min(20)]);
}
