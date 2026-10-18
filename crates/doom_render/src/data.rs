//! Renderer resources (r_data.c, r_things.c `R_InitSprites`): composite
//! textures with vanilla column lookup, flats, sprite definitions,
//! colormaps, translation tables.

use std::sync::{Arc, Mutex, OnceLock};

use doom_content::palette::{Colormaps, Playpal};
use doom_content::picture::Picture;
use doom_content::texture::{TextureDef, TextureSet};
use doom_content::{ContentError, LumpId, Namespace, ResourceStack};

/// Where a texture column's pixels live (`texturecolumnlump/ofs`).
#[derive(Clone, Copy, Debug)]
pub enum ColumnRef {
    /// Single-patch column: offset of the first post's pixels (`+3`) in the
    /// patch lump.
    Patch { lump: LumpId, ofs: usize },
    /// Multi-patch column: offset into the texture's composite block.
    Composite { ofs: usize },
    /// No patch covers this column ("R_GenerateLookup: column without a
    /// patch"); vanilla then reads from lump -1/offset garbage.
    Missing,
}

#[derive(Clone, Debug)]
pub struct RTexture {
    pub width: i32,
    pub height: i32,
    /// `texturewidthmask`.
    pub widthmask: i32,
    /// `textureheight` (fixed).
    pub height_fixed: i32,
    pub columns: Vec<ColumnRef>,
    /// `texturecomposite`: raw (unmasked) multi-patch columns, `height`
    /// bytes each, packed in column order of first appearance.
    pub composite: Vec<u8>,
    /// Which composite bytes some patch wrote. Vanilla's block comes from
    /// `Z_Malloc` and is not cleared, so the rest is heap garbage there.
    pub composite_defined: Vec<bool>,
}

/// A fully composited image with transparency, row-major. Used by the
/// perspective renderer (which samples arbitrary texels rather than
/// vanilla's column posts).
#[derive(Clone, Debug, Default)]
pub struct TexImage {
    pub width: i32,
    pub height: i32,
    /// Palette indices, `y * width + x`.
    pub pixels: Vec<u8>,
    /// Which texels any patch covered.
    pub opaque: Vec<bool>,
    /// Any texel transparent (masked textures, sprites).
    pub has_holes: bool,
}

impl TexImage {
    pub fn new(width: i32, height: i32) -> Self {
        let n = (width.max(0) * height.max(0)) as usize;
        TexImage { width, height, pixels: vec![0; n], opaque: vec![false; n], has_holes: true }
    }

    /// Draw a picture's posts at (ox, oy), clipped.
    pub fn blit(&mut self, pic: &Picture, ox: i32, oy: i32) {
        for col in 0..pic.width {
            let x = ox + col;
            if x < 0 || x >= self.width {
                continue;
            }
            for post in pic.column(col as usize) {
                for (i, &c) in pic.post_pixels(post).iter().enumerate() {
                    let y = oy + post.top + i as i32;
                    if y >= 0 && y < self.height {
                        let k = (y * self.width + x) as usize;
                        self.pixels[k] = c;
                        self.opaque[k] = true;
                    }
                }
            }
        }
    }

    fn finish(mut self) -> Self {
        self.has_holes = self.opaque.iter().any(|&o| !o);
        self
    }

    pub fn from_picture(pic: &Picture) -> Self {
        let mut img = TexImage::new(pic.width, pic.height);
        img.blit(pic, 0, 0);
        img.finish()
    }
}

/// One sprite frame (`spriteframe_t`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteFrame {
    pub rotate: bool,
    /// Index into [`RenderData::sprite_lumps`] per rotation.
    pub lump: [u16; 8],
    pub flip: [bool; 8],
}

#[derive(Clone, Debug, Default)]
pub struct SpriteDef {
    pub frames: Vec<SpriteFrame>,
}

#[derive(Clone, Debug)]
pub struct SpriteLump {
    pub lump: LumpId,
    /// `spritewidth`, `spriteoffset`, `spritetopoffset` (fixed).
    pub width: i32,
    pub offset: i32,
    pub topoffset: i32,
}

pub struct RenderData {
    /// Texture definitions (texture numbers index these).
    defs: Vec<TextureDef>,
    textures_lazy: Vec<OnceLock<RTexture>>,
    images_lazy: Vec<OnceLock<TexImage>>,
    /// The flat namespace's lumps, in flat-number order.
    flat_lumps: Vec<LumpId>,
    /// Flat pixels (64x64) in flat-number order: the flat namespace, then
    /// one 64x64 sample of every wall texture (ZDoom allows textures on
    /// floors; such flats are numbered `num_flats + texture number`).
    flats_lazy: Vec<OnceLock<Vec<u8>>>,
    /// Number of real flats (the flat namespace) at the start of the flats.
    pub num_flats: usize,
    /// Per texture: ZDoom `XScale`/`YScale` (texels per map unit).
    pub texture_scale: Vec<(f32, f32)>,
    /// `colormaps` (all rows, 256 bytes each).
    pub colormaps: Vec<u8>,
    pub palettes: Playpal,
    /// `translationtables` (3 x 256).
    pub translations: Vec<u8>,
    pub sprites: Vec<SpriteDef>,
    pub sprite_lumps: Vec<SpriteLump>,
    /// Graphics are read (and PNGs converted) from the stack on first use;
    /// it caches them, so nothing is decoded or copied up front.
    pub stack: Arc<ResourceStack>,
    pub warnings: Vec<String>,
    late_warnings: Mutex<Vec<String>>,
}

fn le_i16(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl RenderData {
    pub fn build(shared: &Arc<ResourceStack>, textures: &TextureSet, sprnames: &[String]) -> Result<Self, ContentError> {
        let stack: &ResourceStack = shared;
        let mut warnings = Vec::new();
        // Textures, their images and texture-flats are composed on first
        // use (see `texture`, `image`, `flat`).
        // --- flats: the flat namespace, then one per texture ---------------
        let flat_lumps: Vec<LumpId> = stack.namespace(Namespace::Flats).to_vec();
        let num_flats = flat_lumps.len();

        // --- colormaps / palettes -------------------------------------------
        let colormaps = Colormaps::parse("COLORMAP", stack.require("COLORMAP")?)?.maps.concat();
        let palettes = Playpal::parse(stack.require("PLAYPAL")?)?;

        // --- translations (R_InitTranslationTables) --------------------------
        let mut translations = vec![0u8; 768];
        for i in 0..256usize {
            if (0x70..=0x7f).contains(&i) {
                // Map green ramp to gray, brown, red.
                translations[i] = 0x60 + (i as u8 & 0xf);
                translations[i + 256] = 0x40 + (i as u8 & 0xf);
                translations[i + 512] = 0x20 + (i as u8 & 0xf);
            } else {
                // Keep all other colors as is.
                translations[i] = i as u8;
                translations[i + 256] = i as u8;
                translations[i + 512] = i as u8;
            }
        }

        // --- sprites (R_InitSpriteLumps + R_InitSpriteDefs) -------------------
        let sprite_ids = stack.namespace(Namespace::Sprites);
        let mut sprite_lumps = Vec::with_capacity(sprite_ids.len());
        // Header only: a PNG sprite is decoded when first drawn. Archive
        // entries still have to be inflated to read it, so big mods do this
        // on all cores.
        let headers = parallel_map(sprite_ids, |&id| stack.graphic_header(id).unwrap_or((0, 0, 0, 0)));
        for (&id, (w, _, l, t)) in sprite_ids.iter().zip(headers) {
            sprite_lumps.push(SpriteLump { lump: id, width: w << 16, offset: l << 16, topoffset: t << 16 });
        }
        // Lumps by 4-character sprite prefix, in load order.
        let mut by_prefix: std::collections::HashMap<[u8; 4], Vec<usize>> = std::collections::HashMap::new();
        for (idx, &id) in sprite_ids.iter().enumerate() {
            let b = stack.info(id).name;
            let b = b.as_bytes();
            if b.len() >= 6 {
                by_prefix.entry([b[0], b[1], b[2], b[3]]).or_default().push(idx);
            }
        }
        let mut sprites = Vec::with_capacity(sprnames.len());
        for name in sprnames {
            let prefix = name.as_bytes();
            // sprtemp: rotate -1 = unset.
            let mut temp: Vec<(i8, [i32; 8], [bool; 8])> = vec![(-1, [-1; 8], [false; 8]); 29];
            let mut maxframe: i32 = -1;
            let mut install = |idx: usize, frame: u8, rotation: u8, flipped: bool, warnings: &mut Vec<String>| {
                let frame = frame.wrapping_sub(b'A') as i32;
                let rotation = rotation.wrapping_sub(b'0') as i32;
                if !(0..29).contains(&frame) || !(0..=8).contains(&rotation) {
                    warnings.push(format!("sprite {name}: bad frame characters"));
                    return;
                }
                maxframe = maxframe.max(frame);
                let t = &mut temp[frame as usize];
                if rotation == 0 {
                    // The lump should be used for all rotations. Vanilla
                    // errors on conflicting definitions; later ones win here.
                    t.0 = 0;
                    t.1 = [idx as i32; 8];
                    t.2 = [flipped; 8];
                    return;
                }
                // The lump is only used for one rotation.
                t.0 = 1;
                t.1[(rotation - 1) as usize] = idx as i32;
                t.2[(rotation - 1) as usize] = flipped;
            };
            let key: Option<[u8; 4]> = prefix.try_into().ok();
            for &idx in key.and_then(|k| by_prefix.get(&k)).map_or(&[][..], |v| &v[..]) {
                let n = stack.info(sprite_ids[idx]).name;
                let b = n.as_bytes();
                install(idx, b[4], b[5], false, &mut warnings);
                if b.len() >= 8 {
                    install(idx, b[6], b[7], true, &mut warnings);
                }
            }
            let mut def = SpriteDef::default();
            if maxframe >= 0 {
                for f in 0..=maxframe as usize {
                    let (rot, lumps, flips) = temp[f];
                    if rot == -1 {
                        warnings.push(format!("sprite {name}: no patches for frame {}", (b'A' + f as u8) as char));
                    }
                    if rot == 1 && lumps.iter().any(|&l| l == -1) {
                        warnings.push(format!("sprite {name}: frame {} is missing rotations", (b'A' + f as u8) as char));
                    }
                    def.frames.push(SpriteFrame {
                        rotate: rot == 1,
                        lump: lumps.map(|l| l.max(0) as u16),
                        flip: flips,
                    });
                }
            }
            sprites.push(def);
        }
        let texture_scale = textures.textures.iter().map(|d| d.scale).collect();
        let n = textures.textures.len();
        Ok(Self {
            defs: textures.textures.clone(),
            textures_lazy: (0..n).map(|_| OnceLock::new()).collect(),
            images_lazy: (0..n).map(|_| OnceLock::new()).collect(),
            flat_lumps,
            flats_lazy: (0..num_flats + n).map(|_| OnceLock::new()).collect(),
            num_flats,
            texture_scale,
            colormaps,
            palettes,
            translations,
            sprites,
            sprite_lumps,
            stack: shared.clone(),
            warnings,
            late_warnings: Mutex::new(Vec::new()),
        })
    }

    pub fn num_textures(&self) -> usize {
        self.defs.len()
    }

    /// Texture `i` with vanilla's column lookup (classic renderer), built on
    /// first use. Out-of-range numbers give an empty texture.
    pub fn texture(&self, i: usize) -> &RTexture {
        static EMPTY: OnceLock<RTexture> = OnceLock::new();
        let Some(cell) = self.textures_lazy.get(i) else {
            return EMPTY.get_or_init(|| RTexture { width: 0, height: 0, widthmask: 0, height_fixed: 0, columns: vec![], composite: vec![], composite_defined: vec![] });
        };
        cell.get_or_init(|| {
            build_rtexture(&self.defs[i], &self.stack, &mut |w| {
                if let Ok(mut l) = self.late_warnings.lock() {
                    l.push(w);
                }
            })
        })
    }

    /// Texture `i` composited with transparency (perspective renderer),
    /// built on first use.
    pub fn image(&self, i: usize) -> Option<&TexImage> {
        let def = self.defs.get(i)?;
        Some(self.images_lazy[i].get_or_init(|| {
            let mut img = TexImage::new(def.width, def.height);
            for p in &def.patches {
                let Some(l) = p.lump else { continue };
                if let Ok(pic) = Picture::parse("patch", self.stack.graphic(l)) {
                    img.blit(&pic, p.origin_x, p.origin_y);
                }
            }
            img.finish()
        }))
    }

    /// Flat `i` as 64x64 palette indices: a real flat, or (from
    /// `num_flats` on) a 64x64 sample of texture `i - num_flats`.
    pub fn flat(&self, i: usize) -> Option<&[u8]> {
        let cell = self.flats_lazy.get(i)?;
        Some(cell.get_or_init(|| {
            let mut f = vec![0u8; 4096];
            match self.flat_lumps.get(i) {
                Some(&id) => {
                    let d = self.stack.flat(id);
                    let n = d.len().min(4096);
                    f[..n].copy_from_slice(&d[..n]);
                }
                None => {
                    if let Some(img) = self.image(i - self.num_flats).filter(|im| im.width > 0 && im.height > 0) {
                        for y in 0..64 {
                            for x in 0..64 {
                                f[(y * 64 + x) as usize] = img.pixels[((y % img.height) * img.width + x % img.width) as usize];
                            }
                        }
                    }
                }
            }
            f
        }))
    }

    /// Warnings from graphics composed since the last call.
    pub fn take_late_warnings(&self) -> Vec<String> {
        self.late_warnings.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
    }

    /// A graphic lump in Doom picture format (converted from PNG on first
    /// use).
    pub fn lump(&self, id: LumpId) -> &[u8] {
        self.stack.graphic(id)
    }

    /// Parsed picture for a sprite lump index.
    /// A sprite lump as a [`TexImage`].
    pub fn sprite_image(&self, idx: usize) -> Option<TexImage> {
        self.sprite_picture(idx).map(|p| TexImage::from_picture(&p))
    }

    pub fn sprite_picture(&self, idx: usize) -> Option<Picture> {
        let l = &self.sprite_lumps[idx];
        Picture::parse("sprite", self.lump(l.lump)).ok()
    }
}

/// `R_GenerateLookup` + `R_GenerateComposite` for one texture: the vanilla
/// column table and composite block.
fn build_rtexture(def: &TextureDef, stack: &ResourceStack, warn: &mut dyn FnMut(String)) -> RTexture {
    let lump = |id: LumpId| stack.graphic(id);
        let w = def.width.max(0);
        let h = def.height.max(0);
        let mut columns = vec![ColumnRef::Missing; w as usize];
        let mut patchcount = vec![0u8; w as usize];
        for p in &def.patches {
            let Some(l) = p.lump else { continue };
            let data = lump(l);
            if data.len() < 8 {
                continue;
            }
            let pw = le_i16(data, 0) as i32;
            let x1 = p.origin_x;
            let x2 = (x1 + pw).min(w);
            for x in x1.max(0)..x2 {
                let ci = 8 + (x - x1) as usize * 4;
                if ci + 4 > data.len() {
                    break;
                }
                patchcount[x as usize] = patchcount[x as usize].wrapping_add(1);
                // `texturecolumnofs` is `unsigned short`: offsets past
                // 64K wrap (vanilla behaviour for very large patches).
                columns[x as usize] = ColumnRef::Patch { lump: l, ofs: (le_u32(data, ci).wrapping_add(3)) as u16 as usize };
            }
        }
        let mut composite_size = 0usize;
        for x in 0..w as usize {
            if patchcount[x] == 0 {
                warn(format!("R_GenerateLookup: column without a patch ({})", def.name));
                // Vanilla returns here, leaving later columns as set.
                break;
            }
            if patchcount[x] > 1 {
                // Use the cached block.
                // Same `unsigned short` truncation: composites larger
                // than 64K wrap onto earlier columns (medusa-adjacent
                // corruption on huge multi-patch textures).
                columns[x] = ColumnRef::Composite { ofs: composite_size as u16 as usize };
                composite_size += h as usize;
            }
        }
        // R_GenerateComposite: draw multi-patch columns into the block.
        let mut composite = vec![0u8; composite_size];
        let mut composite_defined = vec![false; composite_size];
        if composite_size > 0 {
            for p in &def.patches {
                let Some(l) = p.lump else { continue };
                let data = lump(l);
                if data.len() < 8 {
                    continue;
                }
                let pw = le_i16(data, 0) as i32;
                let x1 = p.origin_x;
                let x2 = (x1 + pw).min(w);
                for x in x1.max(0)..x2 {
                    let ColumnRef::Composite { ofs } = columns[x as usize] else { continue };
                    let ci = 8 + (x - x1) as usize * 4;
                    let mut pos = le_u32(data, ci) as usize;
                    // R_DrawColumnInCache.
                    loop {
                        let Some(&topdelta) = data.get(pos) else { break };
                        if topdelta == 0xff {
                            break;
                        }
                        let len = *data.get(pos + 1).unwrap_or(&0) as i32;
                        let src = pos + 3;
                        let mut count = len;
                        let mut position = p.origin_y + topdelta as i32;
                        if position < 0 {
                            count += position;
                            position = 0;
                        }
                        if position + count > h {
                            count = h - position;
                        }
                        if count > 0 {
                            // Vanilla copies from `source` unadjusted even
                            // when the post was clipped at the top, so the
                            // visible part is shifted. Preserved.
                            for i in 0..count as usize {
                                let (Some(dst), Some(&v)) = (composite.get_mut(ofs + position as usize + i), data.get(src + i)) else {
                                    continue;
                                };
                                *dst = v;
                                composite_defined[ofs + position as usize + i] = true;
                            }
                        }
                        pos += len as usize + 4;
                    }
                }
            }
        }
        let mut j = 1;
        while j * 2 <= w {
            j <<= 1;
        }
    RTexture { width: w, height: h, widthmask: j - 1, height_fixed: h << 16, columns, composite, composite_defined }
}

/// `items.iter().map(f).collect()` spread over the available cores, in order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(16);
    if threads <= 1 || items.len() < 256 {
        return items.iter().map(f).collect();
    }
    let chunk = items.len().div_ceil(threads);
    let f = &f;
    std::thread::scope(|scope| {
        let parts: Vec<_> = items.chunks(chunk).map(|part| scope.spawn(move || part.iter().map(f).collect::<Vec<R>>())).collect();
        parts.into_iter().flat_map(|h| h.join().expect("sprite header worker panicked")).collect()
    })
}
