//! The skateboard model for Skate mode, loaded from an asset on disk.
//!
//! A small text file (`RESOURCES/skateboard.model`) names the mesh (an OBJ
//! file) and its base colour texture (PNG), and says how the asset's axes
//! and size map onto the board: which model axis is the nose and which is
//! up, how long the board should be, and where the ground is. The source
//! files are only read; every correction lives in that file and in the
//! transform here.
//!
//! The mesh is kept in board space: x along the board (nose +), y to the
//! board's right, z up, in map units for a board of [`BoardModel::length`],
//! with the lowest point (the wheels) at z = 0. Drawing scales it to the
//! current `skate_board_length` and places it on the skate physics' board
//! pose.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use doom_render::SceneTexture;

/// A model triangle in board space.
#[derive(Clone, Copy, Debug)]
pub struct ModelTri {
    pub p: [[f32; 3]; 3],
    pub uv: [[f32; 2]; 3],
    /// Face normal (board space, unit).
    pub n: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct BoardModel {
    pub tris: Vec<ModelTri>,
    pub texture: Arc<SceneTexture>,
    /// Board length the mesh is stored at (map units).
    pub length: f32,
    /// Height of the deck's top above the ground (for the rider's feet).
    pub deck_top: f32,
    /// The config file it came from.
    pub source: PathBuf,
}

/// The settings in a `.model` file.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelConfig {
    pub mesh: PathBuf,
    pub texture: PathBuf,
    /// Model axis that is the board's nose / up: `+x`, `-z`, ...
    pub forward: String,
    pub up: String,
    /// Board length in map units (`None`: keep the asset's size times
    /// `scale`).
    pub length: Option<f32>,
    pub scale: f32,
    /// Lift the lowest point to the ground.
    pub ground_auto: bool,
    /// Extra offset (forward, right, up) in map units.
    pub offset: [f32; 3],
    /// Largest texture side kept.
    pub texture_size: u32,
    /// The texture's v axis points up (OBJ convention).
    pub flip_v: bool,
}

/// Parse a `.model` file (`key = value` lines, `#` comments). Paths are
/// relative to the file.
pub fn parse_config(text: &str, dir: &Path) -> Result<ModelConfig, String> {
    let mut c = ModelConfig {
        mesh: PathBuf::new(),
        texture: PathBuf::new(),
        forward: "+x".into(),
        up: "+y".into(),
        length: Some(36.0),
        scale: 1.0,
        ground_auto: true,
        offset: [0.0; 3],
        texture_size: 256,
        flip_v: true,
    };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or_else(|| format!("line {}: expected key = value", n + 1))?;
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"'));
        let num = |v: &str| v.parse::<f32>().map_err(|_| format!("line {}: bad number {v}", n + 1));
        match k.as_str() {
            "mesh" => c.mesh = dir.join(v),
            "texture" => c.texture = dir.join(v),
            "forward" => c.forward = v.to_ascii_lowercase(),
            "up" => c.up = v.to_ascii_lowercase(),
            "length" => c.length = if v.eq_ignore_ascii_case("asset") { None } else { Some(num(v)?) },
            "scale" => c.scale = num(v)?,
            "ground" => c.ground_auto = v.eq_ignore_ascii_case("auto"),
            "offset" => {
                let p: Vec<f32> = v.split_whitespace().map(num).collect::<Result<_, _>>()?;
                if p.len() != 3 {
                    return Err(format!("line {}: offset needs three numbers", n + 1));
                }
                c.offset = [p[0], p[1], p[2]];
            }
            "texture_size" => c.texture_size = num(v)? as u32,
            "flip_v" => c.flip_v = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
            other => return Err(format!("line {}: unknown key {other}", n + 1)),
        }
    }
    if c.mesh.as_os_str().is_empty() {
        return Err("no mesh given".into());
    }
    Ok(c)
}

/// An axis spec (`+x`, `-y`, ...) as a unit vector in model space.
fn axis(spec: &str) -> Result<[f32; 3], String> {
    let (sign, name) = match spec.as_bytes().first() {
        Some(b'-') => (-1.0, &spec[1..]),
        Some(b'+') => (1.0, &spec[1..]),
        _ => (1.0, spec),
    };
    Ok(match name {
        "x" => [sign, 0.0, 0.0],
        "y" => [0.0, sign, 0.0],
        "z" => [0.0, 0.0, sign],
        _ => return Err(format!("bad axis {spec}")),
    })
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// A parsed OBJ: positions, texture coordinates and faces (vertex, uv)
/// index lists.
struct Obj {
    v: Vec<[f32; 3]>,
    vt: Vec<[f32; 2]>,
    faces: Vec<Vec<(usize, Option<usize>)>>,
}

fn parse_obj(text: &str) -> Result<Obj, String> {
    let mut o = Obj { v: Vec::new(), vt: Vec::new(), faces: Vec::new() };
    for (n, line) in text.lines().enumerate() {
        let mut it = line.split_whitespace();
        let bad = || format!("OBJ line {}: {line}", n + 1);
        match it.next() {
            Some("v") => {
                let p: Vec<f32> = it.take(3).map(|s| s.parse::<f32>().map_err(|_| bad())).collect::<Result<_, _>>()?;
                if p.len() < 3 {
                    return Err(bad());
                }
                o.v.push([p[0], p[1], p[2]]);
            }
            Some("vt") => {
                let p: Vec<f32> = it.take(2).map(|s| s.parse::<f32>().map_err(|_| bad())).collect::<Result<_, _>>()?;
                o.vt.push([p.first().copied().unwrap_or(0.0), p.get(1).copied().unwrap_or(0.0)]);
            }
            Some("f") => {
                let mut face = Vec::new();
                for corner in it {
                    let mut parts = corner.split('/');
                    let resolve = |s: Option<&str>, len: usize| -> Result<Option<usize>, String> {
                        match s.filter(|s| !s.is_empty()) {
                            None => Ok(None),
                            Some(s) => {
                                let i: i64 = s.parse().map_err(|_| bad())?;
                                // 1-based; negative counts from the end.
                                let idx = if i < 0 { len as i64 + i } else { i - 1 };
                                if idx < 0 || idx as usize >= len {
                                    return Err(bad());
                                }
                                Ok(Some(idx as usize))
                            }
                        }
                    };
                    let vi = resolve(parts.next(), o.v.len())?.ok_or_else(bad)?;
                    let ti = resolve(parts.next(), o.vt.len())?;
                    face.push((vi, ti));
                }
                if face.len() >= 3 {
                    o.faces.push(face);
                }
            }
            _ => {}
        }
    }
    Ok(o)
}

/// Load a board model from its config file, quantising the texture to
/// `palette`.
pub fn load(config_path: &Path, palette: &[[u8; 3]; 256]) -> Result<BoardModel, String> {
    let text = std::fs::read_to_string(config_path).map_err(|e| format!("{}: {e}", config_path.display()))?;
    let dir = config_path.parent().unwrap_or(Path::new("."));
    let cfg = parse_config(&text, dir).map_err(|e| format!("{}: {e}", config_path.display()))?;
    let obj_text = std::fs::read_to_string(&cfg.mesh).map_err(|e| format!("{}: {e}", cfg.mesh.display()))?;
    let obj = parse_obj(&obj_text)?;
    if obj.faces.is_empty() {
        return Err(format!("{}: no faces", cfg.mesh.display()));
    }
    // Model axes -> board axes (forward, right, up). In the world the
    // board's frame has forward x right = -up, so right = forward x up keeps
    // the model unmirrored (a rotation, not a reflection).
    let fwd = axis(&cfg.forward)?;
    let up = axis(&cfg.up)?;
    if dot(fwd, up).abs() > 0.5 {
        return Err("forward and up are the same axis".into());
    }
    let right = cross(fwd, up);
    let to_board = |p: [f32; 3]| [dot(p, fwd), dot(p, right), dot(p, up)];
    let pts: Vec<[f32; 3]> = obj.v.iter().map(|&p| to_board(p)).collect();
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &pts {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let asset_len = (max[0] - min[0]).max(1e-6);
    let scale = match cfg.length {
        Some(l) => l / asset_len,
        None => cfg.scale,
    };
    let length = asset_len * scale;
    let centre = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let lift = if cfg.ground_auto { -min[2] * scale } else { 0.0 };
    let place = |p: [f32; 3]| {
        [(p[0] - centre[0]) * scale + cfg.offset[0], (p[1] - centre[1]) * scale + cfg.offset[1], p[2] * scale + lift + cfg.offset[2]]
    };
    let mut tris = Vec::with_capacity(obj.faces.len() * 2);
    for face in &obj.faces {
        let corner = |i: usize| {
            let (vi, ti) = face[i];
            let uv = ti.map(|t| obj.vt[t]).unwrap_or([0.0, 0.0]);
            (place(pts[vi]), [uv[0], if cfg.flip_v { 1.0 - uv[1] } else { uv[1] }])
        };
        // Fan triangulation (faces are convex quads and triangles). The
        // outward normal comes from the OBJ's counter-clockwise winding in
        // model space, then is expressed in board axes like the points.
        let (p0, t0) = corner(0);
        let m0 = obj.v[face[0].0];
        for k in 1..face.len() - 1 {
            let ((p1, t1), (p2, t2)) = (corner(k), corner(k + 1));
            let (m1, m2) = (obj.v[face[k].0], obj.v[face[k + 1].0]);
            let e1 = [m1[0] - m0[0], m1[1] - m0[1], m1[2] - m0[2]];
            let e2 = [m2[0] - m0[0], m2[1] - m0[1], m2[2] - m0[2]];
            let nm = cross(e1, e2);
            let l = dot(nm, nm).sqrt();
            if l < 1e-12 {
                continue;
            }
            let n = to_board([nm[0] / l, nm[1] / l, nm[2] / l]);
            tris.push(ModelTri { p: [p0, p1, p2], uv: [t0, t1, t2], n });
        }
    }
    let deck_top = (max[2] - min[2]) * scale * 0.92 + cfg.offset[2];
    let png = std::fs::read(&cfg.texture).map_err(|e| format!("{}: {e}", cfg.texture.display()))?;
    let img = doom_content::png::decode(&png).map_err(|e| format!("{}: {e}", cfg.texture.display()))?;
    let texture = Arc::new(SceneTexture::from_rgba(img.width, img.height, &img.pixels, palette, cfg.texture_size.max(8)));
    Ok(BoardModel { tris, texture, length, deck_top, source: config_path.to_path_buf() })
}

/// Where the engine's resource folder is: `$DOOM_RESOURCES`, else a
/// `RESOURCES` folder in or above the working directory or the executable.
pub fn find_resources() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("DOOM_RESOURCES") {
        return Some(PathBuf::from(d));
    }
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        starts.push(exe);
    }
    for s in starts {
        for dir in s.ancestors().take(6) {
            let r = dir.join("RESOURCES");
            if r.join("skateboard.model").exists() {
                return Some(r);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_axes() {
        let c = parse_config("mesh = \"a/b.obj\"\ntexture = t.png # comment\nforward = -z\nup = +y\nlength = 40\noffset = 0 0 1\n", Path::new("R")).unwrap();
        assert_eq!(c.mesh, Path::new("R").join("a/b.obj"));
        assert_eq!((c.forward.as_str(), c.length, c.offset), ("-z", Some(40.0), [0.0, 0.0, 1.0]));
        assert!(parse_config("mesh = x\nbogus = 1", Path::new(".")).is_err());
        assert_eq!(axis("-z").unwrap(), [0.0, 0.0, -1.0]);
        // Board right for nose +x, up +y (Blender OBJ): +z.
        assert_eq!(cross(axis("+x").unwrap(), axis("+y").unwrap()), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn obj_faces_and_negative_indices() {
        let o = parse_obj("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 1\nf 1/1 2/2 3/2 4/1\nf -1 -2 -3\n").unwrap();
        assert_eq!(o.faces.len(), 2);
        assert_eq!(o.faces[0].len(), 4);
        assert_eq!(o.faces[1][0], (3, None));
    }
}
