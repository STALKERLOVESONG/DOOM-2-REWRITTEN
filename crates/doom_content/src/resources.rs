//! The layered resource stack: every loaded container contributes lumps in
//! load order, and lookups resolve overrides.
//!
//! Two lookup models coexist, because both are part of the compatibility
//! target:
//!
//! * **Global lookup** ([`ResourceStack::check_num_for_name`]) is vanilla
//!   `W_CheckNumForName`: the *last loaded* lump with that name wins,
//!   regardless of markers. Used for PLAYPAL, COLORMAP, patches (via PNAMES),
//!   TEXTURE1/2, sounds, music, map markers, etc.
//!
//! * **Namespaces** ([`Namespace`]) are lump ranges delimited by markers
//!   (`S_START`/`S_END`, `F_START`/`F_END`, ...). Vanilla resolves the markers
//!   globally, so a PWAD with its own sprites or flats *replaces the whole
//!   range* and breaks the game unless the PWAD was merged with DeuSF/NWT.
//!   We do what Boom and ZDoom do instead: each file's marked ranges are
//!   merged into one list per namespace. A lump whose name already exists
//!   replaces the earlier entry **in place** (keeping its position, which
//!   matters for flat/texture animation ranges); new names are appended.
//!   For an unmodified IWAD this produces exactly the vanilla ordering.
//!
//! **PK3 archives** (zip) are mounted with GZDoom's directory rules: files
//! in the root are global lumps named after the file (`DECORATE.txt` ->
//! `DECORATE`); `sprites/`, `flats/`, `textures/`, `patches/`,
//! `colormaps/`, `acs/`, `voxels/` and `hires/` map to namespaces;
//! `graphics/`, `sounds/` and `music/` are global; `maps/*.wad` and `.wad`
//! files in the root are mounted as embedded WADs; `filter/<game>/` trees
//! apply when `<game>` matches the IWAD. Everything else (`zscript/`,
//! `models/`, ...) is reachable only by full path
//! ([`ResourceStack::find_path`]). Entries are decompressed on first use.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::name::LumpName;
use crate::wad::{WadFile, WadKind};
use crate::zip::{Method, ZipArchive};
use crate::ContentError;

/// Vanilla-style lump number: position in global load order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct LumpId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Namespace {
    Global,
    Sprites,
    Flats,
    Patches,
    /// ZDoom `TX_START`: stand-alone textures (patch/PNG format).
    Textures,
    /// Boom `C_START`: extra colormaps.
    Colormaps,
    /// ZDoom `HI_START`: high-resolution replacements.
    Hires,
    /// ZDoom `A_START`: ACS libraries.
    Acs,
    /// ZDoom `V_START`: voxels.
    Voxels,
}

/// Marker -> (namespace, is_start). Sub-markers like `F1_START` are listed so
/// that they are recognised as markers rather than entries.
fn classify_marker(name: &LumpName) -> Option<(Namespace, bool)> {
    use Namespace::*;
    Some(match name.as_bytes() {
        b"S_START" | b"SS_START" => (Sprites, true),
        b"S_END" | b"SS_END" => (Sprites, false),
        b"F_START" | b"FF_START" => (Flats, true),
        b"F_END" | b"FF_END" => (Flats, false),
        b"P_START" | b"PP_START" => (Patches, true),
        b"P_END" | b"PP_END" => (Patches, false),
        b"TX_START" => (Textures, true),
        b"TX_END" => (Textures, false),
        b"C_START" => (Colormaps, true),
        b"C_END" => (Colormaps, false),
        b"HI_START" => (Hires, true),
        b"HI_END" => (Hires, false),
        b"A_START" => (Acs, true),
        b"A_END" => (Acs, false),
        b"V_START" => (Voxels, true),
        b"V_END" => (Voxels, false),
        _ => return None,
    })
}

/// Sub-range markers inside flat and patch namespaces (registered/retail IWADs).
fn is_sub_marker(name: &LumpName) -> bool {
    matches!(
        name.as_bytes(),
        b"F1_START" | b"F1_END" | b"F2_START" | b"F2_END" | b"F3_START" | b"F3_END"
            | b"P1_START" | b"P1_END" | b"P2_START" | b"P2_END" | b"P3_START" | b"P3_END"
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceKind {
    Iwad,
    Pwad,
    /// A zip archive (PK3/IPK3/PK7-as-zip).
    Pk3,
}

pub struct Source {
    pub path: PathBuf,
    pub kind: SourceKind,
    data: Vec<u8>,
}

/// Where a lump's bytes live.
#[derive(Clone, Debug)]
enum Storage {
    /// A slice of the source's data.
    Slice { offset: usize, size: usize },
    /// A compressed zip entry, inflated on first use.
    Deflated { offset: usize, compressed: usize, size: usize, crc: u32 },
    /// An entry that cannot be read (unsupported method, broken entry).
    Unreadable,
}

#[derive(Debug)]
pub struct LumpInfo {
    pub name: LumpName,
    pub source: usize,
    pub namespace: Namespace,
    /// Path inside a PK3 (`sprites/troo/TROOA1.png`), if any.
    pub path: Option<Box<str>>,
    storage: Storage,
    cache: OnceLock<Vec<u8>>,
    /// The lump converted to the Doom picture format (PNG, or a flat used
    /// as a wall texture).
    converted_pic: OnceLock<Vec<u8>>,
    /// The lump converted to a 64x64 flat (PNG flats).
    converted_flat: OnceLock<Vec<u8>>,
}

impl LumpInfo {
    /// Uncompressed size in bytes.
    pub fn size(&self) -> usize {
        match self.storage {
            Storage::Slice { size, .. } | Storage::Deflated { size, .. } => size,
            Storage::Unreadable => 0,
        }
    }
}

#[derive(Default)]
struct NamespaceIndex {
    order: Vec<LumpId>,
    position: HashMap<LumpName, usize>,
}

impl NamespaceIndex {
    fn insert(&mut self, name: LumpName, id: LumpId) {
        match self.position.get(&name) {
            Some(&i) => self.order[i] = id,
            None => {
                self.position.insert(name, self.order.len());
                self.order.push(id);
            }
        }
    }
}

#[derive(Default)]
pub struct ResourceStack {
    sources: Vec<Source>,
    lumps: Vec<LumpInfo>,
    by_name: HashMap<LumpName, Vec<LumpId>>,
    /// Lower-case full paths of PK3 entries (last loaded wins).
    by_path: HashMap<String, LumpId>,
    /// Lower-case file names without extension of PK3 entries longer than
    /// eight characters (GZDoom's long lump names: `PIKUPTALK1` must not
    /// match `PIKUPTALK2`, which share the 8-character short name).
    by_long_name: HashMap<String, Vec<LumpId>>,
    namespaces: HashMap<Namespace, NamespaceIndex>,
    /// Non-fatal problems (bad directory entries, unbalanced markers, ...).
    pub warnings: Vec<String>,
    /// Problems found while decompressing lumps on demand.
    late_warnings: Mutex<Vec<String>>,
    /// PLAYPAL's first palette, for converting true-colour graphics.
    palette: OnceLock<Option<crate::png::Palette>>,
}

impl ResourceStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience: base IWAD followed by PWADs in order.
    pub fn load(iwad: &Path, pwads: &[PathBuf]) -> Result<Self, ContentError> {
        let mut s = Self::new();
        s.add_file(iwad)?;
        for p in pwads {
            s.add_file(p)?;
        }
        Ok(s)
    }

    /// Add a container: a WAD, or a zip archive (PK3), told apart by content.
    pub fn add_file(&mut self, path: &Path) -> Result<(), ContentError> {
        let data = std::fs::read(path).map_err(|source| ContentError::Io { path: path.to_path_buf(), source })?;
        if data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") {
            return self.add_pk3(path.to_path_buf(), data);
        }
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pk7")) {
            return Err(ContentError::Unsupported {
                what: path.display().to_string(),
                reason: "7z archives (PK7) are not supported; repack it as a zip/PK3".into(),
            });
        }
        self.add_wad(WadFile::parse(path.to_path_buf(), data)?);
        Ok(())
    }

    /// GZDoom's filter name for the loaded IWAD (`doom.id.doom2.commercial`
    /// ...): `filter/<x>/` trees apply when `<x>` is a dotted prefix of it.
    fn game_filter(&self) -> &'static str {
        let has = |n: &str| self.check_num_for_name(n).is_some();
        if has("MAP01") {
            if has("REDTNT2") {
                "doom.id.doom2.tnt"
            } else if has("CAMO1") {
                "doom.id.doom2.plutonia"
            } else {
                "doom.id.doom2.commercial"
            }
        } else if has("E4M1") {
            "doom.id.doom1.ultimate"
        } else if has("E2M1") {
            "doom.id.doom1.registered"
        } else {
            "doom.id.doom1"
        }
    }

    /// Mount a zip archive (see the module docs for the directory rules).
    pub fn add_pk3(&mut self, path: PathBuf, data: Vec<u8>) -> Result<(), ContentError> {
        let label = path.display().to_string();
        let zip = ZipArchive::parse(&label, &data)?;
        let filter = self.game_filter();
        let filter_applies = |f: &str| {
            let f = f.to_ascii_lowercase();
            let f = f.strip_prefix("game-").unwrap_or(&f);
            filter == f || filter.starts_with(&format!("{f}."))
        };

        // Resolve each file's effective path (filters applied), then sort
        // like GZDoom so that load order is deterministic.
        let mut files: Vec<(String, usize)> = Vec::new();
        for (i, e) in zip.entries.iter().enumerate() {
            if e.is_dir() || e.name.is_empty() {
                continue;
            }
            let mut p = e.name.trim_start_matches('/').to_string();
            if p.len() > 7 && p[..7].eq_ignore_ascii_case("filter/") {
                let rest = &p[7..];
                let Some((f, inner)) = rest.split_once('/') else { continue };
                if !filter_applies(f) {
                    continue;
                }
                p = inner.to_string();
            }
            files.push((p, i));
        }
        files.sort_by_cached_key(|(p, _)| p.to_ascii_lowercase());

        let source = self.sources.len();
        self.sources.push(Source { path: path.clone(), kind: SourceKind::Pk3, data });
        let mut embedded = Vec::new();
        for (p, i) in files {
            let e = &zip.entries[i];
            let lower = p.to_ascii_lowercase();
            let (dir, file) = match lower.split_once('/') {
                Some((d, _)) => (Some(d), lower.rsplit('/').next().unwrap_or("")),
                None => (None, lower.as_str()),
            };
            // GZDoom: the lump name is the file name up to the first dot.
            let stem = file.split('.').next().unwrap_or("");
            let name = LumpName::new(stem);
            let storage = match (e.method, e.encrypted) {
                (_, true) | (Method::Other(_), _) => {
                    self.warnings.push(format!("{label}: {p}: {:?} entry cannot be read", e.method));
                    Storage::Unreadable
                }
                (method, false) => match zip.raw(&self.sources[source].data, e) {
                    Ok(raw) => {
                        let offset = raw.as_ptr() as usize - self.sources[source].data.as_ptr() as usize;
                        if method == Method::Stored {
                            Storage::Slice { offset, size: raw.len() }
                        } else {
                            Storage::Deflated { offset, compressed: raw.len(), size: e.size as usize, crc: e.crc }
                        }
                    }
                    Err(err) => {
                        self.warnings.push(format!("{label}: {err}"));
                        Storage::Unreadable
                    }
                },
            };
            let is_wad = file.ends_with(".wad");
            // (namespace, visible by short name)
            let (namespace, visible) = match dir {
                None => (Namespace::Global, !is_wad),
                Some("sprites") => (Namespace::Sprites, true),
                Some("flats") => (Namespace::Flats, true),
                Some("textures") => (Namespace::Textures, true),
                Some("patches") => (Namespace::Patches, true),
                Some("colormaps") => (Namespace::Colormaps, true),
                Some("acs") => (Namespace::Acs, true),
                Some("voxels") => (Namespace::Voxels, true),
                Some("hires") => (Namespace::Hires, true),
                Some("graphics" | "sounds" | "music") => (Namespace::Global, true),
                Some("maps") => (Namespace::Global, !is_wad),
                Some(_) => (Namespace::Global, false),
            };
            let id = LumpId(self.lumps.len() as u32);
            self.lumps.push(LumpInfo {
                name,
                source,
                namespace,
                path: Some(p.clone().into_boxed_str()),
                storage,
                cache: OnceLock::new(),
                converted_pic: OnceLock::new(),
                converted_flat: OnceLock::new(),
            });
            self.by_path.insert(lower.clone(), id);
            if visible && stem.len() > 8 {
                self.by_long_name.entry(stem.to_string()).or_default().push(id);
            }
            if visible && !name.is_empty() {
                if namespace != Namespace::Global {
                    self.namespaces.entry(namespace).or_default().insert(name, id);
                }
                self.by_name.entry(name).or_default().push(id);
            }
            if is_wad && matches!(dir, None | Some("maps")) {
                embedded.push((id, dir == Some("maps"), name));
            }
        }
        // Embedded WADs load after the archive's own lumps, like GZDoom.
        for (id, is_map, name) in embedded {
            let bytes = self.data(id).to_vec();
            let inner_path = PathBuf::from(format!("{label}:{}", self.lumps[id.0 as usize].path.as_deref().unwrap_or("")));
            match WadFile::parse(inner_path, bytes) {
                Ok(mut wad) => {
                    // maps/NAME.wad: the file name names the map, whatever
                    // the marker inside says.
                    if is_map {
                        if let Some(first) = wad.entries.first_mut() {
                            first.name = name;
                        }
                    }
                    self.add_wad(wad);
                }
                Err(err) => self.warnings.push(format!("{label}: embedded WAD skipped: {err}")),
            }
        }
        Ok(())
    }

    pub fn add_wad(&mut self, wad: WadFile) {
        let source = self.sources.len();
        self.warnings.extend(wad.warnings.iter().cloned());
        let mut open: Option<Namespace> = None;

        for e in &wad.entries {
            let id = LumpId(self.lumps.len() as u32);
            let mut namespace = open.unwrap_or(Namespace::Global);

            if let Some((ns, is_start)) = classify_marker(&e.name) {
                // Markers themselves are global lumps (vanilla finds them by name).
                namespace = Namespace::Global;
                match (is_start, open) {
                    (true, Some(cur)) => self.warnings.push(format!(
                        "{}: {} opened inside {cur:?} namespace; switching",
                        wad.path.display(),
                        e.name
                    )),
                    (false, Some(cur)) if cur != ns => self.warnings.push(format!(
                        "{}: {} closes {ns:?} but {cur:?} is open",
                        wad.path.display(),
                        e.name
                    )),
                    (false, None) => self.warnings.push(format!(
                        "{}: stray end marker {}",
                        wad.path.display(),
                        e.name
                    )),
                    _ => {}
                }
                open = if is_start { Some(ns) } else { None };
            } else if open.is_some() && is_sub_marker(&e.name) {
                // Vanilla numbers flats as `lumpnum - firstflat` over the whole
                // F_START..F_END range, so F1_START etc. occupy flat numbers.
                // That matters: P_UpdateSpecials uses the absolute flat number
                // in `(leveltime / speed + i) % numpics`, so dropping the
                // sub-markers shifts every flat animation's phase.
                if open == Some(Namespace::Flats) {
                    self.namespaces.entry(Namespace::Flats).or_default().insert(e.name, id);
                } else {
                    namespace = Namespace::Global;
                }
            } else if namespace != Namespace::Global {
                self.namespaces.entry(namespace).or_default().insert(e.name, id);
            }

            self.by_name.entry(e.name).or_default().push(id);
            let storage = if e.broken { Storage::Unreadable } else { Storage::Slice { offset: e.offset, size: e.size } };
            self.lumps.push(LumpInfo { name: e.name, source, namespace, path: None, storage, cache: OnceLock::new(), converted_pic: OnceLock::new(), converted_flat: OnceLock::new() });
        }
        if let Some(ns) = open {
            self.warnings.push(format!("{}: {ns:?} namespace is never closed", wad.path.display()));
        }

        self.sources.push(Source {
            path: wad.path,
            kind: match wad.kind {
                WadKind::Iwad => SourceKind::Iwad,
                WadKind::Pwad => SourceKind::Pwad,
            },
            data: wad.data,
        });
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    pub fn num_lumps(&self) -> usize {
        self.lumps.len()
    }

    pub fn info(&self, id: LumpId) -> &LumpInfo {
        &self.lumps[id.0 as usize]
    }

    pub fn data(&self, id: LumpId) -> &[u8] {
        let l = &self.lumps[id.0 as usize];
        let src = &self.sources[l.source].data;
        match l.storage {
            Storage::Slice { offset, size } => &src[offset..offset + size],
            Storage::Deflated { offset, compressed, size, crc } => l.cache.get_or_init(|| {
                let fail = |reason: String| {
                    let what = l.path.as_deref().unwrap_or("?");
                    if let Ok(mut w) = self.late_warnings.lock() {
                        w.push(format!("{}: {what}: {reason}", self.sources[l.source].path.display()));
                    }
                    Vec::new()
                };
                match crate::inflate::inflate(&src[offset..offset + compressed], size) {
                    Ok(out) if out.len() == size && crate::inflate::crc32(&out) == crc => out,
                    Ok(_) => fail("size or CRC mismatch".into()),
                    Err(e) => fail(e.to_string()),
                }
            }),
            Storage::Unreadable => &[],
        }
    }

    /// The game palette (PLAYPAL, palette 0) with nearest-colour lookup.
    /// Valid once all files are loaded.
    pub fn palette(&self) -> Option<&crate::png::Palette> {
        self.palette
            .get_or_init(|| {
                let d = self.lump("PLAYPAL")?;
                if d.len() < 768 {
                    return None;
                }
                let mut c = [[0u8; 3]; 256];
                for (i, e) in c.iter_mut().enumerate() {
                    *e = [d[i * 3], d[i * 3 + 1], d[i * 3 + 2]];
                }
                Some(crate::png::Palette::new(c))
            })
            .as_ref()
    }

    /// A graphic lump in the Doom picture format: PNG lumps are converted
    /// (mapped to the palette, `grAb` offsets kept), raw flats become
    /// opaque 64x64 pictures (flats used on walls); anything else is
    /// returned as stored.
    pub fn graphic(&self, id: LumpId) -> &[u8] {
        let d = self.data(id);
        let l = &self.lumps[id.0 as usize];
        if !crate::png::is_png(d) {
            if l.namespace == Namespace::Flats && d.len() >= 4096 {
                return l.converted_pic.get_or_init(|| {
                    // Indices pass through unchanged: build the picture
                    // directly rather than via colour matching.
                    let mut pic = crate::picture::Picture {
                        width: 64,
                        height: 64,
                        left_offset: 0,
                        top_offset: 0,
                        columns: Vec::with_capacity(64),
                        posts: Vec::with_capacity(64),
                        pixels: Vec::with_capacity(4096),
                    };
                    for x in 0..64usize {
                        let start = pic.pixels.len() as u32;
                        pic.pixels.extend((0..64).map(|y| d[y * 64 + x]));
                        pic.columns.push((x as u32, 1));
                        pic.posts.push(crate::picture::Post { top: 0, start, len: 64 });
                    }
                    pic.to_lump()
                });
            }
            return d;
        }
        l.converted_pic.get_or_init(|| match (crate::png::decode(d), self.palette()) {
            (Ok(img), Some(pal)) => crate::picture::Picture::from_rgba(&img, pal).to_lump(),
            (Err(e), _) => self.late_warning(id, &format!("PNG: {e}")),
            (_, None) => self.late_warning(id, "no PLAYPAL to convert PNG"),
        })
    }

    /// A graphic's (width, height, left offset, top offset) as its Doom
    /// picture would have them, without converting a PNG that has not been
    /// converted yet.
    pub fn graphic_header(&self, id: LumpId) -> Option<(i32, i32, i32, i32)> {
        let d = self.data(id);
        if crate::png::is_png(d) && self.lumps[id.0 as usize].converted_pic.get().is_none() {
            let h = crate::png::header(d)?;
            let (left, top) = h.offsets.unwrap_or((0, 0));
            return Some((h.width as i32, h.height as i32, left, top));
        }
        let g = self.graphic(id);
        let i16_at = |o: usize| i16::from_le_bytes([g[o], g[o + 1]]) as i32;
        (g.len() >= 8).then(|| (i16_at(0), i16_at(2), i16_at(4), i16_at(6)))
    }

    /// A flat as 64x64 palette indices. PNG flats are converted; other sizes
    /// are resampled to 64x64 (nearest).
    pub fn flat(&self, id: LumpId) -> &[u8] {
        let d = self.data(id);
        if !crate::png::is_png(d) {
            return d;
        }
        let l = &self.lumps[id.0 as usize];
        l.converted_flat.get_or_init(|| match (crate::png::decode(d), self.palette()) {
            (Ok(img), Some(pal)) => {
                let mut out = vec![0u8; 4096];
                for y in 0..64usize {
                    for x in 0..64usize {
                        let sx = x * img.width as usize / 64;
                        let sy = y * img.height as usize / 64;
                        let i = (sy * img.width as usize + sx) * 4;
                        out[y * 64 + x] = pal.nearest([img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]);
                    }
                }
                out
            }
            (Err(e), _) => self.late_warning(id, &format!("PNG: {e}")),
            (_, None) => self.late_warning(id, "no PLAYPAL to convert PNG"),
        })
    }

    fn late_warning(&self, id: LumpId, msg: &str) -> Vec<u8> {
        let l = &self.lumps[id.0 as usize];
        let what = l.path.as_deref().map_or_else(|| l.name.to_string(), str::to_string);
        if let Ok(mut w) = self.late_warnings.lock() {
            w.push(format!("{what}: {msg}"));
        }
        Vec::new()
    }

    /// Problems found while decompressing archive entries since the last call.
    pub fn take_late_warnings(&self) -> Vec<String> {
        self.late_warnings.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
    }

    /// A PK3 entry by full path (case-insensitive, `/` separators), e.g.
    /// `zscript/actors.zs` or `textures/walls/brick.png`.
    pub fn find_path(&self, path: &str) -> Option<LumpId> {
        let p = path.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase();
        self.by_path.get(&p).copied()
    }

    /// `W_CheckNumForName`: last loaded lump with this name, any namespace.
    pub fn check_num_for_name(&self, name: &str) -> Option<LumpId> {
        self.check_num(LumpName::new(name))
    }

    /// A sound (or music) resource as SNDINFO and scripts name it: a full
    /// path (`sounds/weapons/fire.ogg`), a long PK3 file name
    /// (`PIKUPTALK1`, matched exactly), or a lump name. Name lookups skip
    /// graphics namespaces (a sprite `PLASMA` is not the sound `PLASMA`),
    /// and the last loaded file wins, so a later mod overrides an earlier
    /// one and the IWAD.
    pub fn find_sound(&self, name: &str) -> Option<LumpId> {
        let name = name.trim().trim_matches('"');
        if name.is_empty() {
            return None;
        }
        if name.contains('/') || name.contains('\\') {
            return self.find_path(name);
        }
        let lower = name.to_ascii_lowercase();
        if lower.len() > 8 {
            if let Some(&id) = self.by_long_name.get(&lower).and_then(|v| v.last()) {
                return Some(id);
            }
            // A dotted name (`gunshot.wav`) can be a root-level file.
            if let Some(id) = self.find_path(&lower) {
                return Some(id);
            }
            // Longer than eight characters and no file has that whole name:
            // only a WAD lump (always eight) could match the truncation.
            return self
                .by_name
                .get(&LumpName::new(name))?
                .iter()
                .rev()
                .copied()
                .find(|&id| self.lumps[id.0 as usize].path.is_none() && self.lumps[id.0 as usize].namespace == Namespace::Global);
        }
        // GZDoom looks in the sounds namespace first: WAD lumps and files
        // under `sounds/` (a root-level `Plasma.txt` is not the sound
        // PLASMA). Other global entries only if nothing else matches.
        let candidates = self.by_name.get(&LumpName::new(name))?;
        let is_sound_place = |id: LumpId| {
            let l = &self.lumps[id.0 as usize];
            l.namespace == Namespace::Global && l.path.as_deref().is_none_or(|p| p.len() > 7 && p[..7].eq_ignore_ascii_case("sounds/"))
        };
        candidates
            .iter()
            .rev()
            .copied()
            .find(|&id| is_sound_place(id))
            .or_else(|| candidates.iter().rev().copied().find(|&id| self.lumps[id.0 as usize].namespace == Namespace::Global))
    }

    pub fn check_num(&self, name: LumpName) -> Option<LumpId> {
        self.by_name.get(&name).and_then(|v| v.last().copied())
    }

    /// Lump data by name with global (vanilla) resolution.
    pub fn lump(&self, name: &str) -> Option<&[u8]> {
        self.check_num_for_name(name).map(|id| self.data(id))
    }

    pub fn require(&self, name: &str) -> Result<&[u8], ContentError> {
        self.lump(name).ok_or_else(|| ContentError::MissingLump(name.to_ascii_uppercase()))
    }

    /// All lumps with this name in load order (used for lumps that stack,
    /// e.g. ZDoom's DECORATE or multiple TEXTURE lumps).
    pub fn all_named(&self, name: &str) -> &[LumpId] {
        self.by_name.get(&LumpName::new(name)).map_or(&[], Vec::as_slice)
    }

    /// Merged contents of a namespace, in stable order (see module docs).
    pub fn namespace(&self, ns: Namespace) -> &[LumpId] {
        self.namespaces.get(&ns).map_or(&[], |n| n.order.as_slice())
    }

    /// Position of `name` inside a merged namespace (the "flat number" etc.).
    pub fn namespace_index(&self, ns: Namespace, name: LumpName) -> Option<usize> {
        self.namespaces.get(&ns)?.position.get(&name).copied()
    }

    /// Lump id for `name` inside a namespace.
    pub fn find_in(&self, ns: Namespace, name: LumpName) -> Option<LumpId> {
        let n = self.namespaces.get(&ns)?;
        n.position.get(&name).map(|&i| n.order[i])
    }

    /// The lump that follows `id` in its source file (map lumps are positional).
    pub fn next_in_source(&self, id: LumpId) -> Option<LumpId> {
        let next = LumpId(id.0 + 1);
        let l = self.lumps.get(next.0 as usize)?;
        (l.source == self.lumps[id.0 as usize].source).then_some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wad::tests::build_wad;

    fn stack(files: &[Vec<u8>]) -> ResourceStack {
        let mut s = ResourceStack::new();
        for (i, f) in files.iter().enumerate() {
            s.add_wad(WadFile::parse(format!("{i}.wad").into(), f.clone()).unwrap());
        }
        s
    }

    #[test]
    fn global_last_wins() {
        let a = build_wad(b"IWAD", &[("PLAYPAL", b"iwad")]);
        let b = build_wad(b"PWAD", &[("PLAYPAL", b"pwad")]);
        let s = stack(&[a, b]);
        assert_eq!(s.lump("playpal").unwrap(), b"pwad");
        assert_eq!(s.all_named("PLAYPAL").len(), 2);
    }

    #[test]
    fn namespaces_merge_in_place() {
        let a = build_wad(
            b"IWAD",
            &[("F_START", b""), ("F1_START", b""), ("NUKAGE1", b"a"), ("NUKAGE2", b"b"), ("NUKAGE3", b"c"), ("F1_END", b""), ("F_END", b"")],
        );
        let b = build_wad(b"PWAD", &[("FF_START", b""), ("NUKAGE2", b"B"), ("NEWFLAT", b"n"), ("FF_END", b"")]);
        let s = stack(&[a, b]);
        let names: Vec<_> = s.namespace(Namespace::Flats).iter().map(|&id| s.data(id).to_vec()).collect();
        // Sub-markers keep their vanilla flat numbers (empty entries).
        assert_eq!(names, vec![b"".to_vec(), b"a".to_vec(), b"B".to_vec(), b"c".to_vec(), b"".to_vec(), b"n".to_vec()]);
        assert_eq!(s.namespace_index(Namespace::Flats, LumpName::new("NUKAGE3")), Some(3));
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    }

    #[test]
    fn markers_are_global() {
        let a = build_wad(b"IWAD", &[("S_START", b""), ("TROOA1", b"x"), ("S_END", b"")]);
        let s = stack(&[a]);
        assert!(s.check_num_for_name("S_START").is_some());
        assert_eq!(s.namespace(Namespace::Sprites).len(), 1);
    }
}

#[cfg(test)]
mod pk3_tests {
    use super::*;
    use crate::wad::tests::build_wad;

    #[test]
    fn pk3_directory_rules() {
        let mut s = ResourceStack::new();
        s.add_wad(WadFile::parse("doom2.wad".into(), build_wad(b"IWAD", &[("MAP01", b""), ("TITLEPIC", b"iwad title")])).unwrap());
        let pk3 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/layout.pk3")).unwrap();
        s.add_pk3("layout.pk3".into(), pk3).unwrap();
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        // Root files: name up to the first dot; both DECORATE files stack.
        assert_eq!(s.all_named("DECORATE").len(), 2);
        // Namespaces from directories, including subdirectories.
        assert_eq!(s.data(s.find_in(Namespace::Sprites, LumpName::new("BSH2A0")).unwrap()), b"png-bsh2");
        assert_eq!(s.data(s.find_in(Namespace::Textures, LumpName::new("WALL1")).unwrap()), b"png-wall1");
        assert_eq!(s.data(s.find_in(Namespace::Flats, LumpName::new("FLOOR1")).unwrap()).len(), 4096);
        // Filters: doom.id.doom2 applies to Doom II; heretic and names that
        // are not dotted prefixes of the game's filter do not.
        assert_eq!(s.lump("TITLEPIC").unwrap(), b"doom2 title");
        assert!(s.lump("M_DOOM").is_none());
        // Unknown directories are reachable by path only.
        assert!(s.lump("A").is_none());
        assert_eq!(s.data(s.find_path("ZScript/Actors/A.zs").unwrap()), b"class A {}");
        // maps/MAP01.wad is mounted under the file name; root WADs too.
        let m = s.check_num_for_name("MAP01").unwrap();
        assert_eq!(s.info(s.next_in_source(m).unwrap()).name, LumpName::new("TEXTMAP"));
        assert_eq!(s.lump("EXTRALMP").unwrap(), b"from embedded");
        assert!(s.take_late_warnings().is_empty());
    }
}
