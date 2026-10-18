//! Lighting diagnosis: compare the colormap (light level) the classic and
//! perspective renderers pick for each pixel, at pitch 0, over demo frames.
//!
//! Both renderers look light up through `RenderData::colormaps`. Rendering
//! once with every colormap row set to identity gives the texel per pixel;
//! rendering with row `c` filled with `c` gives the colormap index per pixel.
//! Where both renderers sampled the same texel (the same surface point), the
//! colormap indices should agree; the histogram of differences shows any
//! lighting bias. Sheets (with an out dir): classic | perspective | diff
//! (red = perspective darker, blue = brighter, green = different texel).
//!
//! cargo run -p doom_game --release --example light_compare -- <iwad> [out dir]
//! (`LOW_DETAIL=1` compares low detail, `BLOCKS=3..11` a view size.)

use std::collections::BTreeMap;
use std::path::PathBuf;

use doom_game::{png, InputState, Session, SessionConfig};
use doom_render::{Camera, ViewRect, SCREENHEIGHT, SCREENWIDTH};

const W: usize = SCREENWIDTH;
const H: usize = SCREENHEIGHT;

struct Pass {
    texel: Vec<u8>,
    cmap: Vec<u8>,
    image: Vec<u8>,
}

fn render_both(s: &mut Session) -> (Pass, Pass) {
    let real = s.render_data.colormaps.clone();
    let rows = real.len() / 256;
    let identity: Vec<u8> = (0..rows).flat_map(|_| 0..=255u8).collect();
    let index: Vec<u8> = (0..rows).flat_map(|r| std::iter::repeat(r as u8).take(256)).collect();
    let player = s.world.params.consoleplayer;
    let r = &s.renderer;
    let rect = ViewRect { x: r.viewwindowx, y: r.viewwindowy, width: r.scaledviewwidth, height: r.viewheight, low_detail: s.settings.low_detail };
    let cam = Camera::from_player(&s.world, player, 0.0).unwrap();
    let mut out = Vec::new();
    for classic in [true, false] {
        let mut bufs = Vec::new();
        for table in [&identity, &index, &real] {
            s.render_data.colormaps = table.clone();
            let mut screen = vec![0u8; W * H];
            if classic {
                s.renderer.render_player_view(&s.render_data, &s.world, player, &mut screen);
            } else {
                s.perspective.render(&s.render_data, &s.world, player, &cam, rect, &mut screen);
            }
            bufs.push(screen);
        }
        let image = bufs.pop().unwrap();
        let cmap = bufs.pop().unwrap();
        let texel = bufs.pop().unwrap();
        out.push(Pass { texel, cmap, image });
    }
    s.render_data.colormaps = real;
    let p = out.pop().unwrap();
    let c = out.pop().unwrap();
    (c, p)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = args.get(2).map(PathBuf::from);
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut total: BTreeMap<i32, usize> = BTreeMap::new();
    // Per source row band of the view: (rows, sum diff, count).
    let mut by_row = vec![(0i64, 0usize); H];
    let mut compared = 0usize;
    let mut texel_mismatch = 0usize;
    for demo in ["DEMO1", "DEMO2", "DEMO3"] {
        let mut cfg = SessionConfig::new(&args[1]);
        cfg.demo = Some(demo.into());
        let mut s = Session::new(&cfg).unwrap();
        let blocks = std::env::var("BLOCKS").ok().and_then(|b| b.parse().ok()).unwrap_or(10);
        s.set_view(blocks, std::env::var_os("LOW_DETAIL").is_some());
        let idle = InputState::default();
        for shot in 0..12 {
            for _ in 0..if shot == 0 { 40 } else { 150 } {
                s.tick(&idle);
            }
            if !s.is_demo() || s.world.console_player().mo.is_none() {
                break;
            }
            let (c, p) = render_both(&mut s);
            let r = &s.renderer;
            let (x0, y0) = (r.viewwindowx as usize, r.viewwindowy as usize);
            let (vw, vh) = (r.scaledviewwidth as usize, r.viewheight as usize);
            let mut hist: BTreeMap<i32, usize> = BTreeMap::new();
            let mut diffmap = vec![0u8; W * H * 4];
            for y in y0..y0 + vh {
                for x in x0..x0 + vw {
                    let k = y * W + x;
                    if c.texel[k] != p.texel[k] {
                        texel_mismatch += 1;
                        // Green: a different texel was sampled (sub-texel
                        // sampling differences, edges, the weapon sprite).
                        diffmap[k * 4..k * 4 + 4].copy_from_slice(&[0, 90, 0, 255]);
                        continue;
                    }
                    let d = p.cmap[k] as i32 - c.cmap[k] as i32;
                    *hist.entry(d).or_default() += 1;
                    by_row[y].0 += d as i64;
                    by_row[y].1 += 1;
                    compared += 1;
                    // Red: perspective darker (higher colormap), blue: brighter.
                    let m = (d.unsigned_abs() * 40).min(255) as u8;
                    diffmap[k * 4..k * 4 + 4].copy_from_slice(&if d > 0 { [m, 0, 0, 255] } else { [0, 0, m, 255] });
                }
            }
            let mean: f64 = hist.iter().map(|(&d, &n)| d as f64 * n as f64).sum::<f64>() / hist.values().sum::<usize>().max(1) as f64;
            let exact = hist.get(&0).copied().unwrap_or(0) as f64 / hist.values().sum::<usize>().max(1) as f64;
            println!("{demo} tic {:5}: same-texel px {:5}, exact {:5.1}%, mean dmap {:+.3}", s.world.gametic, hist.values().sum::<usize>(), exact * 100.0, mean);
            for (d, n) in hist {
                *total.entry(d).or_default() += n;
            }
            if let Some(dir) = &out_dir {
                let pal = *s.render_data.palettes.get(0);
                let rgba = |b: &[u8]| -> Vec<u8> { b.iter().flat_map(|&i| { let [r, g, b] = pal[i as usize]; [r, g, b, 255] }).collect() };
                let mut sheet = vec![0u8; W * 3 * H * 4];
                for (i, img) in [rgba(&c.image), rgba(&p.image), diffmap].iter().enumerate() {
                    for y in 0..H {
                        let dst = (y * W * 3 + i * W) * 4;
                        sheet[dst..dst + W * 4].copy_from_slice(&img[y * W * 4..(y + 1) * W * 4]);
                    }
                }
                std::fs::write(dir.join(format!("{demo}_{:05}.png", s.world.gametic)), png::encode_rgba(W * 3, H, &sheet)).unwrap();
            }
        }
    }
    println!("\nall frames: {compared} same-texel pixels ({texel_mismatch} differing texels skipped)");
    for (d, n) in &total {
        println!("  dmap {d:+3}: {n:8} ({:.2}%)", *n as f64 * 100.0 / compared as f64);
    }
    println!("mean dmap by screen row band:");
    for band in (0..H).step_by(16) {
        let (sum, n) = by_row[band..(band + 16).min(H)].iter().fold((0i64, 0usize), |a, b| (a.0 + b.0, a.1 + b.1));
        if n > 0 {
            println!("  rows {band:3}-{:3}: {:+.3} ({n} px)", band + 15, sum as f64 / n as f64);
        }
    }
}
