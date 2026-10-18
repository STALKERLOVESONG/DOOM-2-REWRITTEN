//! Integration tests against a real IWAD.
//!
//! Set `DOOM_IWAD` to the IWAD path, or place it at `WADS/doom1.wad` in the
//! workspace root. Tests are skipped (with a message) when it is absent, so
//! the suite stays runnable without commercial data.

use std::collections::HashMap;
use std::path::PathBuf;

use doom_content::flat::parse_flat;
use doom_content::game::{identify, GameMode};
use doom_content::map::{list_maps, RawMap};
use doom_content::palette::{Colormaps, Playpal};
use doom_content::picture::Picture;
use doom_content::texture::{compose_simple, TextureSet};
use doom_content::{Namespace, ResourceStack};

fn iwad() -> Option<ResourceStack> {
    let path = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    if !path.exists() {
        eprintln!("skipping: no IWAD at {}", path.display());
        return None;
    }
    Some(ResourceStack::load(&path, &[]).expect("IWAD loads"))
}

#[test]
fn identifies_and_parses_everything() {
    let Some(stack) = iwad() else { return };
    assert!(stack.warnings.is_empty(), "{:?}", stack.warnings);

    let info = identify(&stack, None).expect("identified");
    eprintln!("game: {info:?}");

    let pal = Playpal::parse(stack.require("PLAYPAL").unwrap()).unwrap();
    assert_eq!(pal.palettes.len(), 14);
    let cm = Colormaps::parse("COLORMAP", stack.require("COLORMAP").unwrap()).unwrap();
    assert_eq!(cm.maps.len(), 34);

    // Every flat and sprite decodes.
    let flats = stack.namespace(Namespace::Flats);
    assert!(!flats.is_empty());
    for &id in flats {
        // F1_START etc. occupy flat numbers (vanilla numbering) but are empty.
        if stack.data(id).is_empty() {
            continue;
        }
        parse_flat(&stack.info(id).name.to_string(), stack.data(id)).unwrap();
    }
    let sprites = stack.namespace(Namespace::Sprites);
    assert!(!sprites.is_empty());
    for &id in sprites {
        Picture::parse(&stack.info(id).name.to_string(), stack.data(id)).unwrap();
    }

    // Textures resolve all their patches and compose.
    let tex = TextureSet::load(&stack).unwrap();
    assert!(tex.warnings.is_empty(), "{:?}", tex.warnings);
    let mut cache = HashMap::new();
    for t in &tex.textures {
        let img = compose_simple(t, &stack, &mut cache);
        assert_eq!(img.width as i32, t.width);
    }
    assert_eq!(tex.check_num(doom_content::LumpName::new("-")), Some(0));

    // All maps load as Doom format with vanilla nodes.
    let maps = list_maps(&stack);
    let expected = match info.mode {
        GameMode::Shareware => 9,
        GameMode::Registered => 27,
        GameMode::Retail => 36,
        GameMode::Commercial => 32,
    };
    assert_eq!(maps.len(), expected);
    for m in maps {
        let raw = RawMap::load(&stack, &m.to_string()).unwrap();
        assert!(raw.warnings.is_empty(), "{m}: {:?}", raw.warnings);
        assert!(!raw.blockmap.is_empty(), "{m}: no blockmap");
    }
}
