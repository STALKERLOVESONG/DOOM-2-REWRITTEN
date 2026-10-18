//! The perspective renderer must light surfaces like vanilla: at pitch 0,
//! wherever it samples the same texel as the classic renderer (which matches
//! Chocolate Doom), it must pick the same colormap (light level).
//!
//! Both renderers read light through `RenderData::colormaps`: rendering with
//! identity rows yields texels, rendering with row `c` filled with `c`
//! yields the colormap index per pixel. See also `examples/light_compare.rs`.

use std::path::PathBuf;

use doom_game::{InputState, Session, SessionConfig};
use doom_render::{Camera, ViewRect, SCREENHEIGHT, SCREENWIDTH};

fn iwad() -> Option<PathBuf> {
    let p = std::env::var_os("DOOM_IWAD")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../WADS/doom1.wad"));
    p.exists().then_some(p).or_else(|| {
        eprintln!("skipping: no IWAD");
        None
    })
}

/// (texel, colormap) buffers for (classic, perspective).
fn buffers(s: &mut Session) -> [(Vec<u8>, Vec<u8>); 2] {
    let real = s.render_data.colormaps.clone();
    let rows = real.len() / 256;
    let identity: Vec<u8> = (0..rows).flat_map(|_| 0..=255u8).collect();
    let index: Vec<u8> = (0..rows).flat_map(|r| std::iter::repeat(r as u8).take(256)).collect();
    let player = s.world.params.consoleplayer;
    let r = &s.renderer;
    let rect = ViewRect { x: r.viewwindowx, y: r.viewwindowy, width: r.scaledviewwidth, height: r.viewheight, low_detail: s.settings.low_detail };
    let cam = Camera::from_player(&s.world, player, 0.0).unwrap();
    let mut render = |classic: bool, table: &Vec<u8>| {
        s.render_data.colormaps = table.clone();
        let mut screen = vec![0u8; SCREENWIDTH * SCREENHEIGHT];
        if classic {
            s.renderer.render_player_view(&s.render_data, &s.world, player, &mut screen);
        } else {
            s.perspective.render(&s.render_data, &s.world, player, &cam, rect, &mut screen);
        }
        screen
    };
    let out = [
        (render(true, &identity), render(true, &index)),
        (render(false, &identity), render(false, &index)),
    ];
    s.render_data.colormaps = real;
    out
}

fn check(low_detail: bool) {
    let Some(iwad) = iwad() else { return };
    let mut cfg = SessionConfig::new(iwad);
    cfg.demo = Some("DEMO1".into());
    let mut s = Session::new(&cfg).unwrap();
    s.set_view(10, low_detail);
    let (mut same_texel, mut same_light, mut total) = (0usize, 0usize, 0usize);
    for _ in 0..4 {
        for _ in 0..200 {
            s.tick(&InputState::default());
        }
        let [(ct, cc), (pt, pc)] = buffers(&mut s);
        let r = &s.renderer;
        // Above the weapon sprite, which only the classic pass draws here.
        for y in r.viewwindowy as usize..(r.viewwindowy + r.viewheight / 2) as usize {
            for x in r.viewwindowx as usize..(r.viewwindowx + r.scaledviewwidth) as usize {
                let k = y * SCREENWIDTH + x;
                total += 1;
                if ct[k] == pt[k] {
                    same_texel += 1;
                    same_light += (cc[k] == pc[k]) as usize;
                }
            }
        }
    }
    let texel_rate = same_texel as f64 / total as f64;
    let light_rate = same_light as f64 / same_texel as f64;
    eprintln!("low_detail {low_detail}: same texel {:.1}%, same light {:.2}%", texel_rate * 100.0, light_rate * 100.0);
    assert!(texel_rate > 0.6, "texture sampling drifted: {:.1}% same texels", texel_rate * 100.0);
    assert!(light_rate > 0.97, "lighting differs from vanilla: {:.2}% same colormap", light_rate * 100.0);
}

#[test]
fn perspective_lighting_matches_vanilla() {
    check(false);
}

#[test]
fn perspective_lighting_matches_vanilla_low_detail() {
    check(true);
}
