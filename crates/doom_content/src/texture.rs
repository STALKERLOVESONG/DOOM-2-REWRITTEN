//! Wall textures: PNAMES + TEXTURE1/TEXTURE2 (r_data.c `R_InitTextures`).
//!
//! Vanilla semantics reproduced here:
//! - PNAMES, TEXTURE1 and TEXTURE2 are each resolved globally (last loaded
//!   lump wins); TEXTURE2 is optional.
//! - Texture numbers are TEXTURE1 entries followed by TEXTURE2 entries.
//! - Name lookup returns the *first* texture with a matching name.
//! - Any name starting with `-` is texture 0, the "no texture" marker. As a
//!   consequence the first texture defined in TEXTURE1 (AASTINKY/AASHITTY in
//!   the IWADs) can never be displayed on a wall.
//! - Patch names are resolved with a global lookup at load time.
//!
//! Deviation: vanilla aborts (`I_Error`) on a missing patch; we keep the
//! texture with that patch omitted and record a warning.

use std::collections::HashMap;

use crate::le;
use crate::name::LumpName;
use crate::picture::{IndexedImage, Picture};
use crate::resources::{LumpId, ResourceStack};
use crate::ContentError;

#[derive(Clone, Debug)]
pub struct TexturePatch {
    pub origin_x: i32,
    pub origin_y: i32,
    /// Resolved patch lump (`patchlookup[]`); `None` if PNAMES names a lump
    /// that does not exist.
    pub lump: Option<LumpId>,
    pub pname: LumpName,
}

#[derive(Clone, Debug)]
pub struct TextureDef {
    pub name: LumpName,
    pub width: i32,
    pub height: i32,
    /// The `masked` field. Unused by the vanilla renderer; kept for tools.
    pub masked: bool,
    pub patches: Vec<TexturePatch>,
    /// ZDoom `XScale`/`YScale`: texels per map unit (1.0 = vanilla). The
    /// classic renderer ignores it; the perspective renderer honours it.
    pub scale: (f32, f32),
}

impl TextureDef {
    /// `texturewidthmask`: width rounded *down* to a power of two, minus one.
    /// Columns wrap with this mask, which is why non-power-of-two widths
    /// repeat incorrectly in vanilla.
    pub fn width_mask(&self) -> i32 {
        let mut j = 1;
        while j * 2 <= self.width {
            j <<= 1;
        }
        j - 1
    }
}

#[derive(Default)]
pub struct TextureSet {
    pub textures: Vec<TextureDef>,
    by_name: HashMap<LumpName, usize>,
    pub warnings: Vec<String>,
}

impl TextureSet {
    pub fn load(stack: &ResourceStack) -> Result<Self, ContentError> {
        stack.require("PNAMES")?;
        let mut set = TextureSet::default();
        for lump in ["TEXTURE1", "TEXTURE2"] {
            let Some(&tex_id) = stack.all_named(lump).last() else {
                if lump == "TEXTURE1" {
                    return Err(ContentError::MissingLump(lump.into()));
                }
                continue;
            };
            // ZDoom pairs a TEXTUREx lump with the PNAMES of its own file (or
            // the latest one loaded before it). Vanilla takes the last
            // PNAMES, which scrambles the IWAD's textures when a PWAD ships
            // a PNAMES without a TEXTURE1.
            let src = stack.info(tex_id).source;
            let pnames_id = stack
                .all_named("PNAMES")
                .iter()
                .copied()
                .filter(|&id| stack.info(id).source <= src)
                .last()
                .or_else(|| stack.all_named("PNAMES").first().copied())
                .ok_or_else(|| ContentError::MissingLump("PNAMES".into()))?;
            let pnames = parse_pnames(stack.data(pnames_id))?;
            let lookup: Vec<Option<LumpId>> = pnames.iter().map(|&n| stack.check_num(n)).collect();
            let data = stack.data(tex_id);
            for raw in parse_texture_lump(lump, data)? {
                let mut patches = Vec::with_capacity(raw.patches.len());
                for (ox, oy, pi) in raw.patches {
                    let Some(&pname) = usize::try_from(pi).ok().and_then(|i| pnames.get(i)) else {
                        set.warnings.push(format!("texture {}: patch index {pi} outside PNAMES", raw.name));
                        continue;
                    };
                    let resolved = lookup[pi as usize];
                    if resolved.is_none() {
                        set.warnings.push(format!("texture {}: missing patch {pname}", raw.name));
                    }
                    patches.push(TexturePatch { origin_x: ox, origin_y: oy, lump: resolved, pname });
                }
                let index = set.textures.len();
                set.by_name.entry(raw.name).or_insert(index); // first definition wins
                set.textures.push(TextureDef {
                    name: raw.name,
                    width: raw.width,
                    height: raw.height,
                    masked: raw.masked,
                    patches,
                    scale: (1.0, 1.0),
                });
            }
        }
        set.add_standalone(stack);
        for &id in stack.all_named("TEXTURES") {
            let text = String::from_utf8_lossy(stack.data(id)).into_owned();
            set.add_textures_lump(stack, &text);
        }
        set.add_flats(stack);
        Ok(set)
    }

    /// Add a texture, replacing an earlier one with the same name in place
    /// (ZDoom: later definitions override, numbering stays stable).
    pub fn define(&mut self, def: TextureDef) {
        match self.by_name.get(&def.name) {
            Some(&i) if i != 0 => self.textures[i] = def,
            _ => {
                self.by_name.insert(def.name, self.textures.len());
                self.textures.push(def);
            }
        }
    }

    /// ZDoom `TX_START`/`textures/`: every lump in the Textures namespace
    /// is a single-patch texture of its own size.
    fn add_standalone(&mut self, stack: &ResourceStack) {
        for &id in stack.namespace(crate::Namespace::Textures) {
            let name = stack.info(id).name;
            // Size from the header: PNGs are decoded when first drawn.
            let Some((w, h, _, _)) = stack.graphic_header(id) else {
                self.warnings.push(format!("texture {name}: unreadable graphic"));
                continue;
            };
            self.define(TextureDef {
                name,
                width: w,
                height: h,
                masked: true,
                patches: vec![TexturePatch { origin_x: 0, origin_y: 0, lump: Some(id), pname: name }],
                scale: (1.0, 1.0),
            });
        }
    }

    /// ZDoom lets walls use flats: every flat whose name is not a texture
    /// becomes one (appended, so texture numbers of real textures are
    /// unchanged).
    fn add_flats(&mut self, stack: &ResourceStack) {
        for &id in stack.namespace(crate::Namespace::Flats) {
            let name = stack.info(id).name;
            if self.by_name.contains_key(&name) || stack.info(id).size() == 0 {
                continue;
            }
            let Some((width, height, _, _)) = stack.graphic_header(id) else { continue };
            self.by_name.insert(name, self.textures.len());
            self.textures.push(TextureDef {
                name,
                width,
                height,
                masked: false,
                patches: vec![TexturePatch { origin_x: 0, origin_y: 0, lump: Some(id), pname: name }],
                scale: (1.0, 1.0),
            });
        }
    }

    /// Resolve a patch name used by a TEXTURES definition: a full path, or a
    /// short name in the patches, textures, sprites or global namespace.
    fn find_patch(stack: &ResourceStack, name: &str) -> Option<LumpId> {
        if name.contains('/') {
            return stack.find_path(name);
        }
        let n = LumpName::new(name);
        stack
            .find_in(crate::Namespace::Patches, n)
            .or_else(|| stack.find_in(crate::Namespace::Textures, n))
            .or_else(|| stack.check_num(n))
            .or_else(|| stack.find_in(crate::Namespace::Sprites, n))
            .or_else(|| stack.find_in(crate::Namespace::Flats, n))
    }

    /// The ZDoom `TEXTURES` lump (subset): `Texture`/`WallTexture`/`Flat`/
    /// `Sprite`/`Graphic` definitions with `XScale`/`YScale`/`Scale` and
    /// `Patch`/`Graphic` entries. Patch rotation, translation and blending
    /// are not supported (warned).
    pub fn add_textures_lump(&mut self, stack: &ResourceStack, text: &str) {
        use crate::script::{Scanner, Token};
        let mut sc = Scanner::new(text);
        while let Some(t) = sc.next() {
            let kind = t.text().unwrap_or("").to_ascii_lowercase();
            if !matches!(kind.as_str(), "texture" | "walltexture" | "flat" | "sprite" | "graphic") {
                if kind == "define" || kind == "remap" {
                    // Unsupported top-level forms: skip to the next line-ish.
                    while sc.peek().is_some_and(|t| !t.is_word("texture") && !t.is_word("walltexture") && !t.is_word("flat")) {
                        sc.next();
                    }
                }
                continue;
            }
            sc.eat_word("optional");
            let Some(name) = sc.next().and_then(|t| t.text().map(str::to_string)) else { break };
            sc.eat(',');
            let w = sc.number().unwrap_or(0.0) as i32;
            sc.eat(',');
            let h = sc.number().unwrap_or(0.0) as i32;
            let mut def = TextureDef {
                name: LumpName::new(&name),
                width: w,
                height: h,
                masked: true,
                patches: Vec::new(),
                scale: (1.0, 1.0),
            };
            if name.len() > 8 {
                self.warnings.push(format!("TEXTURES: long name {name} truncated to {}", def.name));
            }
            if !sc.eat('{') {
                continue;
            }
            let mut depth = 1;
            while depth > 0 {
                let Some(t) = sc.next() else { break };
                match t {
                    Token::Punct('{') => depth += 1,
                    Token::Punct('}') => depth -= 1,
                    Token::Ident(ref p) if depth == 1 => match p.to_ascii_lowercase().as_str() {
                        "xscale" => def.scale.0 = sc.number().unwrap_or(1.0) as f32,
                        "yscale" => def.scale.1 = sc.number().unwrap_or(1.0) as f32,
                        "scale" => {
                            let v = sc.number().unwrap_or(1.0) as f32;
                            def.scale = (v, v);
                        }
                        "patch" | "graphic" | "sprite" => {
                            let Some(pname) = sc.next().and_then(|t| t.text().map(str::to_string)) else { continue };
                            sc.eat(',');
                            let x = sc.number().unwrap_or(0.0) as i32;
                            sc.eat(',');
                            let y = sc.number().unwrap_or(0.0) as i32;
                            let lump = Self::find_patch(stack, &pname);
                            if lump.is_none() {
                                self.warnings.push(format!("TEXTURES {name}: missing patch {pname}"));
                            }
                            if sc.peek().is_some_and(|t| t.is_punct('{')) {
                                let start_warned = self.warnings.len();
                                sc.skip_block();
                                if self.warnings.len() == start_warned {
                                    self.warnings.push(format!("TEXTURES {name}: patch properties ignored"));
                                }
                            }
                            def.patches.push(TexturePatch { origin_x: x, origin_y: y, lump, pname: LumpName::new(&pname) });
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            if def.scale.0 <= 0.0 || def.scale.1 <= 0.0 {
                def.scale = (1.0, 1.0);
            }
            self.define(def);
        }
    }

    /// `R_CheckTextureNumForName`: `Some(0)` for names starting with `-`.
    pub fn check_num(&self, name: LumpName) -> Option<usize> {
        if name.0[0] == b'-' {
            return Some(0);
        }
        self.by_name.get(&name).copied()
    }

    pub fn get(&self, num: usize) -> &TextureDef {
        &self.textures[num]
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

pub fn parse_pnames(data: &[u8]) -> Result<Vec<LumpName>, ContentError> {
    if data.len() < 4 {
        return Err(ContentError::malformed("PNAMES", "truncated"));
    }
    let n = le::i32(data, 0);
    if n < 0 || 4 + n as usize * 8 > data.len() {
        return Err(ContentError::malformed("PNAMES", "count exceeds lump size"));
    }
    Ok((0..n as usize).map(|i| LumpName::from_raw(&data[4 + i * 8..12 + i * 8])).collect())
}

struct RawTexture {
    name: LumpName,
    masked: bool,
    width: i32,
    height: i32,
    /// (originx, originy, PNAMES index)
    patches: Vec<(i32, i32, i32)>,
}

/// Doom-format `maptexture_t`: name[8], masked:i32, width:i16, height:i16,
/// columndirectory:i32 (obsolete), patchcount:i16, then 10-byte `mappatch_t`s
/// (originx, originy, patch, stepdir, colormap).
///
/// TODO(phase 12): Strife-format TEXTURE lumps (no columndirectory, 6-byte
/// patches) used by some mods; ZDoom detects them heuristically.
fn parse_texture_lump(lump: &str, data: &[u8]) -> Result<Vec<RawTexture>, ContentError> {
    if data.len() < 4 {
        return Err(ContentError::malformed(lump, "truncated"));
    }
    let n = le::i32(data, 0);
    if n < 0 || 4 + n as usize * 4 > data.len() {
        return Err(ContentError::malformed(lump, "texture count exceeds lump size"));
    }
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let ofs = le::i32(data, 4 + i * 4);
        if ofs < 0 || ofs as usize + 22 > data.len() {
            // Vanilla: "R_InitTextures: bad texture directory".
            return Err(ContentError::malformed(lump, format!("texture #{i} offset {ofs} out of range")));
        }
        let o = ofs as usize;
        let count = le::i16(data, o + 20).max(0) as usize;
        if o + 22 + count * 10 > data.len() {
            return Err(ContentError::malformed(lump, format!("texture #{i} patch list truncated")));
        }
        let patches = (0..count)
            .map(|p| {
                let po = o + 22 + p * 10;
                (le::i16(data, po) as i32, le::i16(data, po + 2) as i32, le::i16(data, po + 4) as i32)
            })
            .collect();
        out.push(RawTexture {
            name: LumpName::from_raw(&data[o..o + 8]),
            masked: le::i32(data, o + 8) != 0,
            width: le::i16(data, o + 12) as i32,
            height: le::i16(data, o + 14) as i32,
            patches,
        });
    }
    Ok(out)
}

/// Straightforward composite of a texture into an indexed image: patches are
/// painted in order, transparent post gaps keep what is underneath.
///
/// This is **not** the vanilla column cache (`R_GenerateComposite` /
/// `R_GenerateLookup`), which differs for multi-patch masked textures
/// ("Medusa" effect) and in clipping details. It exists for the preview
/// renderer and tools; the software renderer reproduces the vanilla path.
pub fn compose_simple(
    def: &TextureDef,
    stack: &ResourceStack,
    cache: &mut HashMap<LumpId, Option<Picture>>,
) -> IndexedImage {
    let (w, h) = (def.width.max(0) as u32, def.height.max(0) as u32);
    let mut img = IndexedImage::new(w, h);
    for p in &def.patches {
        let Some(lump) = p.lump else { continue };
        let pic = cache
            .entry(lump)
            .or_insert_with(|| Picture::parse(&p.pname.to_string(), stack.graphic(lump)).ok());
        let Some(pic) = pic else { continue };
        for x in 0..pic.width {
            let dx = p.origin_x + x;
            if dx < 0 || dx >= def.width {
                continue;
            }
            for post in pic.column(x as usize) {
                for (i, &c) in pic.post_pixels(post).iter().enumerate() {
                    let dy = p.origin_y + post.top + i as i32;
                    if (0..def.height).contains(&dy) {
                        img.set(dx as u32, dy as u32, c);
                    }
                }
            }
        }
    }
    img
}
