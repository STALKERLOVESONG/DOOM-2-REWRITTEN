//! The renderer proper. Field and function names follow the C globals and
//! routines they replace so the code can be audited side by side with
//! r_*.c; all arithmetic is the original integer arithmetic.

use doom_core::angle::{ANG180, ANG90, ANGLETOFINESHIFT, DBITS, FINEANGLES};
use doom_core::bbox::{BOXBOTTOM, BOXLEFT, BOXRIGHT, BOXTOP};
use doom_core::fixed::{fixed_div, fixed_mul};
use doom_core::geom::point_to_angle;
use doom_core::tables::{finecosine, finesine, finetangent, TANTOANGLE};
use doom_core::{Angle, Fixed, FRACBITS, FRACUNIT};

use doom_sim::ids::{FlatNum, SectorId, SegId};
use doom_sim::level::{Level, NF_SUBSECTOR};
use doom_sim::mobj::flags::{MF_SHADOW, MF_TRANSLATION, MF_TRANSSHIFT};
use doom_sim::player::power;
use doom_sim::World;

use crate::data::{ColumnRef, RenderData};

pub const SCREENWIDTH: usize = 320;
pub const SCREENHEIGHT: usize = 200;

const FIELDOFVIEW: usize = 2048;
const LIGHTLEVELS: usize = 16;
const LIGHTSEGSHIFT: i32 = 4;
const MAXLIGHTSCALE: usize = 48;
const LIGHTSCALESHIFT: i32 = 12;
const MAXLIGHTZ: usize = 128;
const LIGHTZSHIFT: i32 = 20;
const NUMCOLORMAPS: i32 = 32;
const DISTMAP: i32 = 2;
const HEIGHTBITS: i32 = 12;
const HEIGHTUNIT: i32 = 1 << HEIGHTBITS;
const MINZ: i32 = FRACUNIT * 4;
const BASEYCENTER: i32 = (SCREENHEIGHT / 2) as i32;
const ANGLETOSKYSHIFT: u32 = 22;
const SBARHEIGHT: usize = 32;
const SKYTEXTUREMID: i32 = (SCREENHEIGHT as i32 / 2) * FRACUNIT;

const SIL_NONE: i32 = 0;
const SIL_BOTTOM: i32 = 1;
const SIL_TOP: i32 = 2;
const SIL_BOTH: i32 = 3;

/// "Empty" marker for visplane columns (`0xff` in the byte arrays).
const VP_EMPTY: u16 = 0xffff;

const FUZZTABLE: usize = 50;
const FUZZOFF: isize = SCREENWIDTH as isize;
#[rustfmt::skip]
const FUZZOFFSET: [isize; FUZZTABLE] = [
    FUZZOFF,-FUZZOFF,FUZZOFF,-FUZZOFF,FUZZOFF,FUZZOFF,-FUZZOFF,
    FUZZOFF,FUZZOFF,-FUZZOFF,FUZZOFF,FUZZOFF,FUZZOFF,-FUZZOFF,
    FUZZOFF,FUZZOFF,FUZZOFF,-FUZZOFF,-FUZZOFF,-FUZZOFF,-FUZZOFF,
    FUZZOFF,-FUZZOFF,-FUZZOFF,FUZZOFF,FUZZOFF,FUZZOFF,FUZZOFF,-FUZZOFF,
    FUZZOFF,-FUZZOFF,FUZZOFF,FUZZOFF,-FUZZOFF,-FUZZOFF,FUZZOFF,
    FUZZOFF,-FUZZOFF,-FUZZOFF,-FUZZOFF,-FUZZOFF,FUZZOFF,FUZZOFF,
    FUZZOFF,FUZZOFF,-FUZZOFF,FUZZOFF,FUZZOFF,-FUZZOFF,FUZZOFF,
];

/// Static renderer limits. Vanilla's values reproduce its overflow
/// behaviour (missing walls/sprites); `None` removes a limit.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// `MAXDRAWSEGS` (256): further walls are silently not drawn.
    pub drawsegs: Option<usize>,
    /// `MAXVISSPRITES` (128): further sprites are silently dropped.
    pub vissprites: Option<usize>,
    /// `MAXVISPLANES` (128): vanilla aborts ("no more visplanes"). When
    /// set, planes beyond it are not drawn and the overflow is counted.
    pub visplanes: Option<usize>,
}

impl Limits {
    pub const VANILLA: Limits = Limits { drawsegs: Some(256), vissprites: Some(128), visplanes: Some(128) };
    pub const UNLIMITED: Limits = Limits { drawsegs: None, vissprites: None, visplanes: None };
}

#[derive(Clone, Copy, Debug, Default)]
struct ClipRange {
    first: i32,
    last: i32,
}

/// What a drawseg's sprite clip array points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipRef {
    None,
    ScreenHeight,
    NegOne,
    /// `openings[base + x]`.
    Openings(isize),
}

#[derive(Clone, Copy, Debug)]
struct DrawSeg {
    curline: SegId,
    x1: i32,
    x2: i32,
    scale1: Fixed,
    scale2: Fixed,
    scalestep: Fixed,
    silhouette: i32,
    bsilheight: Fixed,
    tsilheight: Fixed,
    sprtopclip: ClipRef,
    sprbottomclip: ClipRef,
    /// `maskedtexturecol` base into `openings`.
    maskedtexturecol: Option<isize>,
}

#[derive(Clone, Debug)]
struct Visplane {
    height: Fixed,
    picnum: FlatNum,
    lightlevel: i32,
    minx: i32,
    maxx: i32,
    /// Index x + 1 (vanilla's pad bytes at -1 and SCREENWIDTH).
    top: Vec<u16>,
    bottom: Vec<u16>,
}

impl Visplane {
    fn new() -> Self {
        Self {
            height: Fixed(0),
            picnum: FlatNum(0),
            lightlevel: 0,
            minx: 0,
            maxx: 0,
            top: vec![VP_EMPTY; SCREENWIDTH + 2],
            bottom: vec![0; SCREENWIDTH + 2],
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct VisSprite {
    x1: i32,
    x2: i32,
    gx: Fixed,
    gy: Fixed,
    gz: Fixed,
    gzt: Fixed,
    startfrac: Fixed,
    scale: Fixed,
    xiscale: Fixed,
    texturemid: Fixed,
    /// Sprite lump index.
    patch: usize,
    /// Colormap offset; `None` = shadow (fuzz).
    colormap: Option<usize>,
    mobjflags: u32,
}

/// Which column drawer `colfunc` points to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColFunc {
    Base,
    Fuzz,
    Translated(usize),
}

/// A byte source for column drawing with vanilla's out-of-bounds reads
/// replaced by zeros.
#[derive(Clone, Copy)]
struct Source<'a> {
    buf: &'a [u8],
    start: isize,
    /// Per-byte "initialised" flags for `buf` (composite blocks); empty means
    /// every byte of `buf` is defined.
    defined: &'a [bool],
}

impl<'a> Source<'a> {
    fn new(buf: &'a [u8], start: isize) -> Self {
        Source { buf, start, defined: &[] }
    }

    #[inline]
    fn at(&self, i: isize) -> u8 {
        self.get(i).0
    }

    /// The byte at `start + i` and whether vanilla would have read defined
    /// data there. Reads outside the buffer (vanilla walks into whatever
    /// zone memory follows the lump) and uninitialised composite bytes yield
    /// 0 and `false`.
    #[inline]
    fn get(&self, i: isize) -> (u8, bool) {
        let p = self.start + i;
        if p < 0 {
            return (0, false);
        }
        match self.buf.get(p as usize) {
            Some(&v) => (v, self.defined.get(p as usize).copied().unwrap_or(true)),
            None => (0, false),
        }
    }
}

pub struct Renderer {
    pub limits: Limits,
    /// Counts of limit overflows during the last frame.
    pub overflows: Vec<&'static str>,

    // --- view size (R_ExecuteSetViewSize) ---------------------------------
    setblocks: i32,
    detailshift: i32,
    pub viewwidth: i32,
    pub scaledviewwidth: i32,
    pub viewheight: i32,
    pub viewwindowx: i32,
    pub viewwindowy: i32,
    centerx: i32,
    centery: i32,
    centerxfrac: Fixed,
    centeryfrac: Fixed,
    projection: Fixed,
    viewangletox: Vec<i32>,
    xtoviewangle: Vec<u32>,
    clipangle: u32,
    distscale: Vec<Fixed>,
    yslope: Vec<Fixed>,
    screenheightarray: Vec<i16>,
    negonearray: Vec<i16>,
    scalelight: [[usize; MAXLIGHTSCALE]; LIGHTLEVELS],
    zlight: [[usize; MAXLIGHTZ]; LIGHTLEVELS],
    pspritescale: Fixed,
    pspriteiscale: Fixed,

    // --- per frame (R_SetupFrame) -----------------------------------------
    viewx: Fixed,
    viewy: Fixed,
    viewz: Fixed,
    viewangle: Angle,
    viewsin: Fixed,
    viewcos: Fixed,
    extralight: i32,
    fixedcolormap: Option<usize>,
    validcount: i32,
    sector_validcount: Vec<i32>,

    // --- r_bsp ---------------------------------------------------------------
    solidsegs: Vec<ClipRange>,
    newend: usize,
    drawsegs: Vec<DrawSeg>,
    curline: SegId,
    frontsector: SectorId,
    backsector: Option<SectorId>,

    // --- r_plane ---------------------------------------------------------
    visplanes: Vec<Visplane>,
    lastvisplane: usize,
    floorplane: Option<usize>,
    ceilingplane: Option<usize>,
    openings: Vec<i16>,
    lastopening: isize,
    floorclip: Vec<i16>,
    ceilingclip: Vec<i16>,
    spanstart: Vec<i32>,
    planeheight: Fixed,
    planezlight: usize,
    basexscale: Fixed,
    baseyscale: Fixed,
    cachedheight: Vec<Fixed>,
    cacheddistance: Vec<Fixed>,
    cachedxstep: Vec<Fixed>,
    cachedystep: Vec<Fixed>,

    // --- r_segs ----------------------------------------------------------
    segtextured: bool,
    markfloor: bool,
    markceiling: bool,
    maskedtexture: bool,
    toptexture: usize,
    bottomtexture: usize,
    midtexture: usize,
    rw_normalangle: Angle,
    rw_angle1: Angle,
    rw_x: i32,
    rw_stopx: i32,
    rw_centerangle: Angle,
    rw_offset: Fixed,
    rw_distance: Fixed,
    rw_scale: Fixed,
    rw_scalestep: Fixed,
    rw_midtexturemid: Fixed,
    rw_toptexturemid: Fixed,
    rw_bottomtexturemid: Fixed,
    pixhigh: Fixed,
    pixlow: Fixed,
    pixhighstep: Fixed,
    pixlowstep: Fixed,
    topfrac: Fixed,
    topstep: Fixed,
    bottomfrac: Fixed,
    bottomstep: Fixed,
    walllights: usize,
    /// When `fixedcolormap` is set, `walllights` points at
    /// `scalelightfixed` (all entries = fixedcolormap).
    walllights_fixed: bool,
    maskedtexturecol: isize,

    // --- r_things --------------------------------------------------------
    vissprites: Vec<VisSprite>,
    spritelights: usize,
    spryscale: Fixed,
    sprtopscreen: Fixed,

    // --- r_draw ----------------------------------------------------------
    /// Optional per-pixel flags (320x200) marking pixels whose value came
    /// from memory vanilla never defined: texel reads past the end of a lump
    /// or column and uninitialised composite bytes. Enable with
    /// [`Renderer::track_undefined`]; used by differential tests to exclude
    /// pixels whose vanilla value depends on zone-heap contents.
    pub undefined: Option<Vec<bool>>,
    /// Linedefs drawn so far (`ML_MAPPED`), indexed by line number. The
    /// owner clears it on level change.
    pub mapped_lines: Vec<bool>,
    /// `fuzzpos`: vanilla never resets it, so the fuzz pattern depends on
    /// every fuzz pixel drawn in earlier frames (i.e. on the frame rate).
    pub fuzzpos: usize,
    dc_colormap: usize,
    dc_x: i32,
    dc_yl: i32,
    dc_yh: i32,
    dc_iscale: Fixed,
    dc_texturemid: Fixed,
}

impl Renderer {
    /// `R_Init` + `R_SetViewSize(screenblocks, detail)`.
    pub fn new(screenblocks: i32, low_detail: bool, limits: Limits) -> Self {
        let mut r = Renderer {
            limits,
            overflows: Vec::new(),
            setblocks: 0,
            detailshift: 0,
            viewwidth: 0,
            scaledviewwidth: 0,
            viewheight: 0,
            viewwindowx: 0,
            viewwindowy: 0,
            centerx: 0,
            centery: 0,
            centerxfrac: Fixed(0),
            centeryfrac: Fixed(0),
            projection: Fixed(0),
            viewangletox: vec![0; FINEANGLES / 2],
            xtoviewangle: vec![0; SCREENWIDTH + 1],
            clipangle: 0,
            distscale: vec![Fixed(0); SCREENWIDTH],
            yslope: vec![Fixed(0); SCREENHEIGHT],
            screenheightarray: vec![0; SCREENWIDTH],
            negonearray: vec![-1; SCREENWIDTH],
            scalelight: [[0; MAXLIGHTSCALE]; LIGHTLEVELS],
            zlight: [[0; MAXLIGHTZ]; LIGHTLEVELS],
            pspritescale: Fixed(0),
            pspriteiscale: Fixed(0),
            viewx: Fixed(0),
            viewy: Fixed(0),
            viewz: Fixed(0),
            viewangle: Angle(0),
            viewsin: Fixed(0),
            viewcos: Fixed(0),
            extralight: 0,
            fixedcolormap: None,
            validcount: 1,
            sector_validcount: Vec::new(),
            solidsegs: vec![ClipRange::default(); SCREENWIDTH / 2 + 1],
            newend: 0,
            drawsegs: Vec::new(),
            curline: SegId(0),
            frontsector: SectorId(0),
            backsector: None,
            visplanes: Vec::new(),
            lastvisplane: 0,
            floorplane: None,
            ceilingplane: None,
            openings: vec![0; SCREENWIDTH * 64],
            lastopening: 0,
            floorclip: vec![0; SCREENWIDTH],
            ceilingclip: vec![0; SCREENWIDTH],
            spanstart: vec![0; SCREENHEIGHT],
            planeheight: Fixed(0),
            planezlight: 0,
            basexscale: Fixed(0),
            baseyscale: Fixed(0),
            cachedheight: vec![Fixed(0); SCREENHEIGHT],
            cacheddistance: vec![Fixed(0); SCREENHEIGHT],
            cachedxstep: vec![Fixed(0); SCREENHEIGHT],
            cachedystep: vec![Fixed(0); SCREENHEIGHT],
            segtextured: false,
            markfloor: false,
            markceiling: false,
            maskedtexture: false,
            toptexture: 0,
            bottomtexture: 0,
            midtexture: 0,
            rw_normalangle: Angle(0),
            rw_angle1: Angle(0),
            rw_x: 0,
            rw_stopx: 0,
            rw_centerangle: Angle(0),
            rw_offset: Fixed(0),
            rw_distance: Fixed(0),
            rw_scale: Fixed(0),
            rw_scalestep: Fixed(0),
            rw_midtexturemid: Fixed(0),
            rw_toptexturemid: Fixed(0),
            rw_bottomtexturemid: Fixed(0),
            pixhigh: Fixed(0),
            pixlow: Fixed(0),
            pixhighstep: Fixed(0),
            pixlowstep: Fixed(0),
            topfrac: Fixed(0),
            topstep: Fixed(0),
            bottomfrac: Fixed(0),
            bottomstep: Fixed(0),
            walllights: 0,
            walllights_fixed: false,
            maskedtexturecol: 0,
            vissprites: Vec::new(),
            spritelights: 0,
            spryscale: Fixed(0),
            sprtopscreen: Fixed(0),
            fuzzpos: 0,
            undefined: None,
            mapped_lines: Vec::new(),
            dc_colormap: 0,
            dc_x: 0,
            dc_yl: 0,
            dc_yh: 0,
            dc_iscale: Fixed(0),
            dc_texturemid: Fixed(0),
        };
        r.init_light_tables();
        r.set_view_size(screenblocks, low_detail);
        r
    }

    /// `R_InitLightTables`: zlight (distance-based light for planes).
    fn init_light_tables(&mut self) {
        for i in 0..LIGHTLEVELS {
            let startmap = ((LIGHTLEVELS as i32 - 1 - i as i32) * 2) * NUMCOLORMAPS / LIGHTLEVELS as i32;
            for j in 0..MAXLIGHTZ {
                let mut scale = fixed_div(Fixed((SCREENWIDTH as i32 / 2) * FRACUNIT), Fixed(((j + 1) as i32) << LIGHTZSHIFT)).0;
                scale >>= LIGHTSCALESHIFT;
                let level = (startmap - scale / DISTMAP).clamp(0, NUMCOLORMAPS - 1);
                self.zlight[i][j] = (level * 256) as usize;
            }
        }
    }

    /// `R_SetViewSize` + `R_ExecuteSetViewSize`. `screenblocks` 3..=11 (11 =
    /// full screen without status bar).
    #[inline]
    fn mark_mapped(&mut self, line: usize) {
        if line >= self.mapped_lines.len() {
            self.mapped_lines.resize(line + 1, false);
        }
        self.mapped_lines[line] = true;
    }

    /// Start tracking undefined pixels (see [`Renderer::undefined`]).
    pub fn track_undefined(&mut self) {
        self.undefined = Some(vec![false; SCREENWIDTH * SCREENHEIGHT]);
    }

    pub fn set_view_size(&mut self, blocks: i32, low_detail: bool) {
        self.setblocks = blocks.clamp(3, 11);
        if self.setblocks == 11 {
            self.scaledviewwidth = SCREENWIDTH as i32;
            self.viewheight = SCREENHEIGHT as i32;
        } else {
            self.scaledviewwidth = self.setblocks * 32;
            self.viewheight = (self.setblocks * 168 / 10) & !7;
        }
        self.detailshift = low_detail as i32;
        self.viewwidth = self.scaledviewwidth >> self.detailshift;
        self.centery = self.viewheight / 2;
        self.centerx = self.viewwidth / 2;
        self.centerxfrac = Fixed(self.centerx << FRACBITS);
        self.centeryfrac = Fixed(self.centery << FRACBITS);
        self.projection = self.centerxfrac;

        // R_InitBuffer
        self.viewwindowx = (SCREENWIDTH as i32 - self.scaledviewwidth) >> 1;
        self.viewwindowy = if self.scaledviewwidth == SCREENWIDTH as i32 {
            0
        } else {
            (SCREENHEIGHT as i32 - SBARHEIGHT as i32 - self.viewheight) >> 1
        };

        self.init_texture_mapping();

        // Psprite scales.
        self.pspritescale = Fixed(FRACUNIT * self.viewwidth / SCREENWIDTH as i32);
        self.pspriteiscale = Fixed(FRACUNIT * SCREENWIDTH as i32 / self.viewwidth);

        // Thing clipping.
        for i in 0..self.viewwidth as usize {
            self.screenheightarray[i] = self.viewheight as i16;
        }

        // Planes.
        for i in 0..self.viewheight as usize {
            let dy = Fixed((((i as i32) - self.viewheight / 2) << FRACBITS) + FRACUNIT / 2).abs();
            self.yslope[i] = fixed_div(Fixed((self.viewwidth << self.detailshift) / 2 * FRACUNIT), dy);
        }
        for i in 0..self.viewwidth as usize {
            let cosadj = finecosine((self.xtoviewangle[i] >> ANGLETOFINESHIFT) as usize).abs();
            self.distscale[i] = fixed_div(Fixed::ONE, cosadj);
        }

        // Calculate the light levels to use for each level / scale
        // combination.
        for i in 0..LIGHTLEVELS {
            let startmap = ((LIGHTLEVELS as i32 - 1 - i as i32) * 2) * NUMCOLORMAPS / LIGHTLEVELS as i32;
            for j in 0..MAXLIGHTSCALE {
                let level = startmap - j as i32 * SCREENWIDTH as i32 / (self.viewwidth << self.detailshift) / DISTMAP;
                self.scalelight[i][j] = (level.clamp(0, NUMCOLORMAPS - 1) * 256) as usize;
            }
        }
    }

    /// `R_InitTextureMapping`.
    fn init_texture_mapping(&mut self) {
        // Use tangent table to generate viewangletox: viewangletox will
        // give the next greatest x after the view angle.
        //
        // Calc focallength so FIELDOFVIEW angles covers SCREENWIDTH.
        let focallength = fixed_div(self.centerxfrac, finetangent(FINEANGLES / 4 + FIELDOFVIEW / 2));
        for i in 0..FINEANGLES / 2 {
            let ft = finetangent(i);
            let t = if ft.0 > FRACUNIT * 2 {
                -1
            } else if ft.0 < -FRACUNIT * 2 {
                self.viewwidth + 1
            } else {
                let t = fixed_mul(ft, focallength);
                let t = (self.centerxfrac - t + Fixed(FRACUNIT - 1)).0 >> FRACBITS;
                t.clamp(-1, self.viewwidth + 1)
            };
            self.viewangletox[i] = t;
        }
        // Scan viewangletox[] to generate xtoviewangle[]: xtoviewangle will
        // give the smallest view angle that maps to x.
        for x in 0..=self.viewwidth as usize {
            let mut i = 0;
            while self.viewangletox[i] > x as i32 {
                i += 1;
            }
            self.xtoviewangle[x] = ((i as u32) << ANGLETOFINESHIFT).wrapping_sub(ANG90);
        }
        // Take out the fencepost cases from viewangletox.
        for i in 0..FINEANGLES / 2 {
            if self.viewangletox[i] == -1 {
                self.viewangletox[i] = 0;
            } else if self.viewangletox[i] == self.viewwidth + 1 {
                self.viewangletox[i] = self.viewwidth;
            }
        }
        self.clipangle = self.xtoviewangle[0];
    }

    // =====================================================================
    // Frame
    // =====================================================================

    /// `R_RenderPlayerView`: draw the 3D view for `player` into the 320x200
    /// framebuffer `screen` (only the view window is touched).
    pub fn render_player_view(&mut self, data: &RenderData, w: &World, player: usize, screen: &mut [u8]) {
        assert_eq!(screen.len(), SCREENWIDTH * SCREENHEIGHT);
        self.overflows.clear();
        let level = &w.level;
        self.setup_frame(w, player);
        self.clear_clip_segs();
        self.drawsegs.clear();
        self.clear_planes();
        self.vissprites.clear();
        let mut ctx = Ctx { data, w, level, screen };
        self.render_bsp_node(&mut ctx, level.bsp_root());
        self.draw_planes(&mut ctx);
        self.draw_masked(&mut ctx, player);
    }

    /// Draw only the player's weapon sprites (`R_DrawPlayerSprites`) into
    /// the view window. Used when another renderer draws the 3D view: the
    /// weapon is screen-space and unaffected by camera pitch.
    pub fn render_psprites(&mut self, data: &RenderData, w: &World, player: usize, screen: &mut [u8]) {
        assert_eq!(screen.len(), SCREENWIDTH * SCREENHEIGHT);
        if w.players[player].mo.is_none() {
            return;
        }
        let level = &w.level;
        self.setup_frame(w, player);
        let mut ctx = Ctx { data, w, level, screen };
        self.draw_player_sprites(&mut ctx, player);
    }

    /// `R_SetupFrame`.
    fn setup_frame(&mut self, w: &World, player: usize) {
        let p = &w.players[player];
        let mo = w.mo(p.mo.expect("view player has a body"));
        self.viewx = mo.x;
        self.viewy = mo.y;
        self.viewangle = mo.angle;
        self.extralight = p.extralight;
        self.viewz = p.viewz;
        self.viewsin = finesine(self.viewangle.fine());
        self.viewcos = finecosine(self.viewangle.fine());
        self.fixedcolormap = (p.fixedcolormap != 0).then_some(p.fixedcolormap as usize * 256);
        self.walllights_fixed = self.fixedcolormap.is_some();
        self.validcount = self.validcount.wrapping_add(1);
        if self.sector_validcount.len() != w.level.sectors.len() {
            self.sector_validcount = vec![0; w.level.sectors.len()];
        }
    }

    // =====================================================================
    // r_main helpers
    // =====================================================================

    /// `R_PointToAngle` relative to the view.
    #[inline]
    fn point_to_angle(&self, x: Fixed, y: Fixed) -> Angle {
        point_to_angle(x - self.viewx, y - self.viewy)
    }

    /// `R_PointToDist`.
    fn point_to_dist(&self, x: Fixed, y: Fixed) -> Fixed {
        let mut dx = (x - self.viewx).abs();
        let mut dy = (y - self.viewy).abs();
        if dy > dx {
            std::mem::swap(&mut dx, &mut dy);
        }
        let frac = if dx.0 != 0 { fixed_div(dy, dx) } else { Fixed(0) };
        let angle = (TANTOANGLE[(frac.0 >> DBITS) as usize].wrapping_add(ANG90)) >> ANGLETOFINESHIFT;
        // Use as cosine.
        fixed_div(dx, finesine(angle as usize))
    }

    /// `R_ScaleFromGlobalAngle`: returns the texture mapping scale for the
    /// current line (horizontal span) at the given angle.
    fn scale_from_global_angle(&self, visangle: Angle) -> Fixed {
        let anglea = Angle(ANG90) + (visangle - self.viewangle);
        let angleb = Angle(ANG90) + (visangle - self.rw_normalangle);
        // Both sines are always positive.
        let sinea = finesine(anglea.fine());
        let sineb = finesine(angleb.fine());
        let num = fixed_mul(self.projection, sineb) << self.detailshift as u32;
        let den = fixed_mul(self.rw_distance, sinea);
        if den.0 > num.0 >> FRACBITS {
            let scale = fixed_div(num, den);
            if scale.0 > 64 * FRACUNIT {
                Fixed(64 * FRACUNIT)
            } else if scale.0 < 256 {
                Fixed(256)
            } else {
                scale
            }
        } else {
            Fixed(64 * FRACUNIT)
        }
    }

    // =====================================================================
    // r_bsp
    // =====================================================================

    /// `R_ClearClipSegs`.
    fn clear_clip_segs(&mut self) {
        self.solidsegs[0] = ClipRange { first: -0x7fffffff, last: -1 };
        self.solidsegs[1] = ClipRange { first: self.viewwidth, last: 0x7fffffff };
        self.newend = 2;
    }

    /// `R_ClipSolidWallSegment`: does handle solid walls, e.g. single sided
    /// LineDefs (middle texture) that entirely block the view.
    fn clip_solid_wall_segment(&mut self, ctx: &mut Ctx, first: i32, last: i32) {
        // Find the first range that touches the range (adjacent pixels are
        // touching).
        let mut start = 0;
        while self.solidsegs[start].last < first - 1 {
            start += 1;
        }
        if first < self.solidsegs[start].first {
            if last < self.solidsegs[start].first - 1 {
                // Post is entirely visible (above start), so insert a new
                // clippost.
                self.store_wall_range(ctx, first, last);
                if self.newend + 1 > self.solidsegs.len() {
                    self.solidsegs.push(ClipRange::default());
                }
                let mut next = self.newend;
                self.newend += 1;
                while next != start {
                    self.solidsegs[next] = self.solidsegs[next - 1];
                    next -= 1;
                }
                self.solidsegs[next] = ClipRange { first, last };
                return;
            }
            // There is a fragment above *start.
            let sf = self.solidsegs[start].first;
            self.store_wall_range(ctx, first, sf - 1);
            // Now adjust the clip size.
            self.solidsegs[start].first = first;
        }
        // Bottom contained in start?
        if last <= self.solidsegs[start].last {
            return;
        }
        let mut next = start;
        let mut crunch = false;
        while last >= self.solidsegs[next + 1].first - 1 {
            // There is a fragment between two posts.
            let (a, b) = (self.solidsegs[next].last + 1, self.solidsegs[next + 1].first - 1);
            self.store_wall_range(ctx, a, b);
            next += 1;
            if last <= self.solidsegs[next].last {
                // Bottom is contained in next. Adjust the clip size.
                self.solidsegs[start].last = self.solidsegs[next].last;
                crunch = true;
                break;
            }
        }
        if !crunch {
            // There is a fragment after *next.
            let a = self.solidsegs[next].last + 1;
            self.store_wall_range(ctx, a, last);
            // Adjust the clip size.
            self.solidsegs[start].last = last;
        }
        // Remove start+1 to next from the clip list, because start now
        // covers their area.
        if next == start {
            // Post just extended past the bottom of one post.
            return;
        }
        let mut s = start;
        loop {
            let n = next;
            next += 1;
            if n == self.newend {
                break;
            }
            // Remove a post.
            s += 1;
            self.solidsegs[s] = self.solidsegs[next.min(self.solidsegs.len() - 1)];
            if next == self.newend {
                break;
            }
        }
        self.newend = s + 1;
    }

    /// `R_ClipPassWallSegment`: clips the given range of columns, but does
    /// not include it in the clip list. Does handle windows, e.g. LineDefs
    /// with upper and lower texture.
    fn clip_pass_wall_segment(&mut self, ctx: &mut Ctx, first: i32, last: i32) {
        // Find the first range that touches the range (adjacent pixels are
        // touching).
        let mut start = 0;
        while self.solidsegs[start].last < first - 1 {
            start += 1;
        }
        if first < self.solidsegs[start].first {
            if last < self.solidsegs[start].first - 1 {
                // Post is entirely visible (above start).
                self.store_wall_range(ctx, first, last);
                return;
            }
            // There is a fragment above *start.
            let sf = self.solidsegs[start].first;
            self.store_wall_range(ctx, first, sf - 1);
        }
        // Bottom contained in start?
        if last <= self.solidsegs[start].last {
            return;
        }
        while last >= self.solidsegs[start + 1].first - 1 {
            // There is a fragment between two posts.
            let (a, b) = (self.solidsegs[start].last + 1, self.solidsegs[start + 1].first - 1);
            self.store_wall_range(ctx, a, b);
            start += 1;
            if last <= self.solidsegs[start].last {
                return;
            }
        }
        // There is a fragment after *next.
        let a = self.solidsegs[start].last + 1;
        self.store_wall_range(ctx, a, last);
    }

    /// `R_AddLine`: clips the given segment and adds any visible pieces to
    /// the line list.
    fn add_line(&mut self, ctx: &mut Ctx, segid: SegId) {
        let level = ctx.level;
        let seg = &level.segs[segid.idx()];
        self.curline = segid;
        let v1 = level.vertex(seg.v1);
        let v2 = level.vertex(seg.v2);
        // OPTIMIZE: quickly reject orthogonal back sides.
        let mut angle1 = self.point_to_angle(v1.x, v1.y);
        let mut angle2 = self.point_to_angle(v2.x, v2.y);
        // Clip to view edges.
        let span = (angle1 - angle2).0;
        // Back side? I.e. backface culling?
        if span >= ANG180 {
            return;
        }
        // Global angle needed by segcalc.
        self.rw_angle1 = angle1;
        angle1 -= self.viewangle;
        angle2 -= self.viewangle;
        let clip = self.clipangle;
        let mut tspan = angle1.0.wrapping_add(clip);
        if tspan > 2u32.wrapping_mul(clip) {
            tspan = tspan.wrapping_sub(2u32.wrapping_mul(clip));
            // Totally off the left edge?
            if tspan >= span {
                return;
            }
            angle1 = Angle(clip);
        }
        tspan = clip.wrapping_sub(angle2.0);
        if tspan > 2u32.wrapping_mul(clip) {
            tspan = tspan.wrapping_sub(2u32.wrapping_mul(clip));
            // Totally off the left edge?
            if tspan >= span {
                return;
            }
            angle2 = Angle(clip.wrapping_neg());
        }
        // The seg is in the view range, but not necessarily visible.
        let a1 = (angle1.0.wrapping_add(ANG90) >> ANGLETOFINESHIFT) as usize;
        let a2 = (angle2.0.wrapping_add(ANG90) >> ANGLETOFINESHIFT) as usize;
        let x1 = self.viewangletox[a1];
        let x2 = self.viewangletox[a2];
        // Does not cross a pixel?
        if x1 == x2 {
            return;
        }
        self.backsector = seg.back_sector;
        let front = level.sector(self.frontsector);
        let Some(back) = self.backsector.map(|b| level.sector(b)) else {
            // Single sided line?
            self.clip_solid_wall_segment(ctx, x1, x2 - 1);
            return;
        };
        // Closed door.
        if back.ceiling_height <= front.floor_height || back.floor_height >= front.ceiling_height {
            self.clip_solid_wall_segment(ctx, x1, x2 - 1);
            return;
        }
        // Window.
        if back.ceiling_height != front.ceiling_height || back.floor_height != front.floor_height {
            self.clip_pass_wall_segment(ctx, x1, x2 - 1);
            return;
        }
        // Reject empty lines used for triggers and special events. Identical
        // floor and ceiling on both sides, identical light levels on both
        // sides, and no middle texture.
        if back.ceiling_pic == front.ceiling_pic
            && back.floor_pic == front.floor_pic
            && back.light_level == front.light_level
            && level.side(seg.sidedef).mid_texture.0 == 0
        {
            return;
        }
        self.clip_pass_wall_segment(ctx, x1, x2 - 1);
    }

    /// `R_CheckBBox`: checks BSP node/subtree bounding box. Returns true if
    /// some part of the bbox might be visible.
    fn check_bbox(&self, bspcoord: &[Fixed; 4]) -> bool {
        const CHECKCOORD: [[usize; 4]; 12] = [
            [3, 0, 2, 1],
            [3, 0, 2, 0],
            [3, 1, 2, 0],
            [0, 0, 0, 0],
            [2, 0, 2, 1],
            [0, 0, 0, 0],
            [3, 1, 3, 0],
            [0, 0, 0, 0],
            [2, 0, 3, 1],
            [2, 1, 3, 1],
            [2, 1, 3, 0],
            [0, 0, 0, 0],
        ];
        // Find the corners of the box that define the edges from current
        // viewpoint.
        let boxx = if self.viewx <= bspcoord[BOXLEFT] {
            0
        } else if self.viewx < bspcoord[BOXRIGHT] {
            1
        } else {
            2
        };
        let boxy = if self.viewy >= bspcoord[BOXTOP] {
            0
        } else if self.viewy > bspcoord[BOXBOTTOM] {
            1
        } else {
            2
        };
        let boxpos = (boxy << 2) + boxx;
        if boxpos == 5 {
            return true;
        }
        let c = CHECKCOORD[boxpos];
        let (x1, y1, x2, y2) = (bspcoord[c[0]], bspcoord[c[1]], bspcoord[c[2]], bspcoord[c[3]]);
        // Check clip list for an open space.
        let mut angle1 = self.point_to_angle(x1, y1) - self.viewangle;
        let mut angle2 = self.point_to_angle(x2, y2) - self.viewangle;
        let span = (angle1 - angle2).0;
        // Sitting on a line?
        if span >= ANG180 {
            return true;
        }
        let clip = self.clipangle;
        let mut tspan = angle1.0.wrapping_add(clip);
        if tspan > 2u32.wrapping_mul(clip) {
            tspan = tspan.wrapping_sub(2u32.wrapping_mul(clip));
            // Totally off the left edge?
            if tspan >= span {
                return false;
            }
            angle1 = Angle(clip);
        }
        tspan = clip.wrapping_sub(angle2.0);
        if tspan > 2u32.wrapping_mul(clip) {
            tspan = tspan.wrapping_sub(2u32.wrapping_mul(clip));
            // Totally off the left edge?
            if tspan >= span {
                return false;
            }
            angle2 = Angle(clip.wrapping_neg());
        }
        // Find the first clippost that touches the source post (adjacent
        // pixels are touching).
        let a1 = (angle1.0.wrapping_add(ANG90) >> ANGLETOFINESHIFT) as usize;
        let a2 = (angle2.0.wrapping_add(ANG90) >> ANGLETOFINESHIFT) as usize;
        let sx1 = self.viewangletox[a1];
        let mut sx2 = self.viewangletox[a2];
        // Does not cross a pixel.
        if sx1 == sx2 {
            return false;
        }
        sx2 -= 1;
        let mut start = 0;
        while self.solidsegs[start].last < sx2 {
            start += 1;
        }
        // The clippost contains the new span.
        !(sx1 >= self.solidsegs[start].first && sx2 <= self.solidsegs[start].last)
    }

    /// `R_Subsector`: determine floor/ceiling planes, add sprites of things
    /// in sector, draw one or more line segments.
    fn subsector(&mut self, ctx: &mut Ctx, num: usize) {
        let level = ctx.level;
        let sub = &level.subsectors[num];
        self.frontsector = sub.sector;
        let fs = level.sector(sub.sector);
        let sky = ctx.w.sky_flat;
        self.floorplane = if fs.floor_height < self.viewz {
            self.find_plane(fs.floor_height, fs.floor_pic, fs.light_level as i32, sky)
        } else {
            None
        };
        self.ceilingplane = if fs.ceiling_height > self.viewz || fs.ceiling_pic == sky {
            self.find_plane(fs.ceiling_height, fs.ceiling_pic, fs.light_level as i32, sky)
        } else {
            None
        };
        self.add_sprites(ctx, sub.sector);
        for i in 0..sub.num_lines {
            self.add_line(ctx, SegId(sub.first_line + i));
        }
    }

    /// `R_RenderBSPNode`: renders all subsectors below a given node,
    /// traversing subtree recursively.
    fn render_bsp_node(&mut self, ctx: &mut Ctx, bspnum: u32) {
        // Found a subsector? (Vanilla's -1 for a node-less map is
        // `bsp_root()` = subsector 0.)
        if bspnum & NF_SUBSECTOR != 0 {
            self.subsector(ctx, (bspnum & !NF_SUBSECTOR) as usize);
            return;
        }
        let bsp = &ctx.level.nodes[bspnum as usize];
        // Decide which side the view point is on.
        let side = doom_sim::geometry::point_on_node_side(self.viewx, self.viewy, bsp);
        let (children, bbox) = (bsp.children, bsp.bbox[side ^ 1].0);
        // Recursively divide front space.
        self.render_bsp_node(ctx, children[side]);
        // Possibly divide back space.
        if self.check_bbox(&bbox) {
            self.render_bsp_node(ctx, children[side ^ 1]);
        }
    }

    // =====================================================================
    // r_segs
    // =====================================================================

    fn wall_lights(&mut self, ctx: &Ctx, seg: SegId, sector: SectorId) {
        let level = ctx.level;
        let s = &level.segs[seg.idx()];
        let mut lightnum = (level.sector(sector).light_level as i32 >> LIGHTSEGSHIFT) + self.extralight;
        let (v1, v2) = (level.vertex(s.v1), level.vertex(s.v2));
        if v1.y == v2.y {
            lightnum -= 1;
        } else if v1.x == v2.x {
            lightnum += 1;
        }
        self.walllights = lightnum.clamp(0, LIGHTLEVELS as i32 - 1) as usize;
    }

    #[inline]
    fn wall_colormap(&self, index: usize) -> usize {
        match (self.walllights_fixed, self.fixedcolormap) {
            (true, Some(f)) => f,
            _ => self.scalelight[self.walllights][index],
        }
    }

    /// `R_RenderMaskedSegRange`.
    fn render_masked_seg_range(&mut self, ctx: &mut Ctx, dsi: usize, x1: i32, x2: i32) {
        let ds = self.drawsegs[dsi];
        let level = ctx.level;
        let seg = &level.segs[ds.curline.idx()];
        self.curline = ds.curline;
        let front = level.sector(seg.front_sector);
        let back = level.sector(seg.back_sector.expect("masked seg has a back sector"));
        let side = level.side(seg.sidedef);
        let texnum = ctx.w.texture_translation[side.mid_texture.0 as usize] as usize;
        // Calculate light table. Use different light tables for horizontal /
        // vertical / diagonal.
        if self.fixedcolormap.is_none() {
            self.wall_lights(ctx, ds.curline, seg.front_sector);
        } else {
            // Vanilla still computes walllights from scalelight here.
            self.wall_lights(ctx, ds.curline, seg.front_sector);
        }
        let mtc = ds.maskedtexturecol.unwrap();
        self.rw_scalestep = ds.scalestep;
        self.spryscale = ds.scale1 + ds.scalestep * (x1 - ds.x1);
        let floorclip = self.clip_values(ds.sprbottomclip);
        let ceilingclip = self.clip_values(ds.sprtopclip);

        // Find positioning.
        let tex = ctx.data.texture(texnum);
        let line = level.line(seg.linedef);
        self.dc_texturemid = if line.flags & doom_content::map::ml::DONTPEGBOTTOM != 0 {
            let h = if front.floor_height > back.floor_height { front.floor_height } else { back.floor_height };
            h + Fixed(tex.height_fixed) - self.viewz
        } else {
            let h = if front.ceiling_height < back.ceiling_height { front.ceiling_height } else { back.ceiling_height };
            h - self.viewz
        };
        self.dc_texturemid += side.row_offset;

        if let Some(f) = self.fixedcolormap {
            self.dc_colormap = f;
        }
        // Draw the columns.
        for x in x1..=x2 {
            self.dc_x = x;
            let col = self.openings[(mtc + x as isize) as usize];
            // Calculate lighting.
            if col != i16::MAX {
                if self.fixedcolormap.is_none() {
                    let index = ((self.spryscale.0 >> LIGHTSCALESHIFT) as usize).min(MAXLIGHTSCALE - 1);
                    self.dc_colormap = self.scalelight[self.walllights][index];
                }
                self.sprtopscreen = self.centeryfrac - fixed_mul(self.dc_texturemid, self.spryscale);
                self.dc_iscale = Fixed((0xffff_ffffu32 / self.spryscale.0 as u32) as i32);
                // Draw the texture.
                // Vanilla backs up 3 bytes to the post header; on composite
                // columns that reinterprets raw pixels as posts (medusa).
                let src = texture_column(ctx.data, texnum, col as i32, -3);
                self.draw_masked_column(ctx, src, &floorclip, &ceilingclip, ColFunc::Base);
                self.openings[(mtc + x as isize) as usize] = i16::MAX;
            }
            self.spryscale += self.rw_scalestep;
        }
    }

    fn clip_values(&self, c: ClipRef) -> Vec<i16> {
        match c {
            ClipRef::ScreenHeight => self.screenheightarray.clone(),
            ClipRef::NegOne | ClipRef::None => self.negonearray.clone(),
            ClipRef::Openings(base) => (0..SCREENWIDTH as isize)
                .map(|x| {
                    let i = base + x;
                    if i >= 0 && (i as usize) < self.openings.len() {
                        self.openings[i as usize]
                    } else {
                        0
                    }
                })
                .collect(),
        }
    }

    /// `R_RenderSegLoop`: draws zero, one, or two textures (and possibly a
    /// masked texture) for walls. Can draw or mark the starting pixel of
    /// floor and ceiling textures.
    fn render_seg_loop(&mut self, ctx: &mut Ctx) {
        while self.rw_x < self.rw_stopx {
            let x = self.rw_x as usize;
            // Mark floor / ceiling areas.
            let mut yl = (self.topfrac.0 + HEIGHTUNIT - 1) >> HEIGHTBITS;
            // No space above wall?
            if yl < self.ceilingclip[x] as i32 + 1 {
                yl = self.ceilingclip[x] as i32 + 1;
            }
            if self.markceiling {
                let top = self.ceilingclip[x] as i32 + 1;
                let mut bottom = yl - 1;
                if bottom >= self.floorclip[x] as i32 {
                    bottom = self.floorclip[x] as i32 - 1;
                }
                if top <= bottom {
                    if let Some(cp) = self.ceilingplane {
                        self.visplanes[cp].top[x + 1] = top as u16;
                        self.visplanes[cp].bottom[x + 1] = bottom as u16;
                    }
                }
            }
            let mut yh = self.bottomfrac.0 >> HEIGHTBITS;
            if yh >= self.floorclip[x] as i32 {
                yh = self.floorclip[x] as i32 - 1;
            }
            if self.markfloor {
                let mut top = yh + 1;
                let bottom = self.floorclip[x] as i32 - 1;
                if top <= self.ceilingclip[x] as i32 {
                    top = self.ceilingclip[x] as i32 + 1;
                }
                if top <= bottom {
                    if let Some(fp) = self.floorplane {
                        self.visplanes[fp].top[x + 1] = top as u16;
                        self.visplanes[fp].bottom[x + 1] = bottom as u16;
                    }
                }
            }

            // Texture column and lighting are independent of wall tiers.
            let texturecolumn = if self.segtextured {
                // Calculate texture offset.
                let angle = (self.rw_centerangle.0.wrapping_add(self.xtoviewangle[x]) >> ANGLETOFINESHIFT) as usize;
                let tc = self.rw_offset - fixed_mul(finetangent(angle), self.rw_distance);
                let index = ((self.rw_scale.0 >> LIGHTSCALESHIFT) as usize).min(MAXLIGHTSCALE - 1);
                self.dc_colormap = self.wall_colormap(index);
                self.dc_x = self.rw_x;
                self.dc_iscale = Fixed((0xffff_ffffu32 / self.rw_scale.0 as u32) as i32);
                tc.0 >> FRACBITS
            } else {
                0
            };

            // Draw the wall tiers.
            if self.midtexture != 0 {
                // Single sided line.
                self.dc_yl = yl;
                self.dc_yh = yh;
                self.dc_texturemid = self.rw_midtexturemid;
                let src = texture_column(ctx.data, self.midtexture, texturecolumn, 0);
                self.draw_column(ctx, src, ColFunc::Base);
                self.ceilingclip[x] = self.viewheight as i16;
                self.floorclip[x] = -1;
            } else {
                // Two sided line.
                if self.toptexture != 0 {
                    // Top wall.
                    let mut mid = self.pixhigh.0 >> HEIGHTBITS;
                    self.pixhigh += self.pixhighstep;
                    if mid >= self.floorclip[x] as i32 {
                        mid = self.floorclip[x] as i32 - 1;
                    }
                    if mid >= yl {
                        self.dc_yl = yl;
                        self.dc_yh = mid;
                        self.dc_texturemid = self.rw_toptexturemid;
                        let src = texture_column(ctx.data, self.toptexture, texturecolumn, 0);
                        self.draw_column(ctx, src, ColFunc::Base);
                        self.ceilingclip[x] = mid as i16;
                    } else {
                        self.ceilingclip[x] = (yl - 1) as i16;
                    }
                } else if self.markceiling {
                    // No top wall.
                    self.ceilingclip[x] = (yl - 1) as i16;
                }
                if self.bottomtexture != 0 {
                    // Bottom wall.
                    let mut mid = (self.pixlow.0 + HEIGHTUNIT - 1) >> HEIGHTBITS;
                    self.pixlow += self.pixlowstep;
                    // No space above wall?
                    if mid <= self.ceilingclip[x] as i32 {
                        mid = self.ceilingclip[x] as i32 + 1;
                    }
                    if mid <= yh {
                        self.dc_yl = mid;
                        self.dc_yh = yh;
                        self.dc_texturemid = self.rw_bottomtexturemid;
                        let src = texture_column(ctx.data, self.bottomtexture, texturecolumn, 0);
                        self.draw_column(ctx, src, ColFunc::Base);
                        self.floorclip[x] = mid as i16;
                    } else {
                        self.floorclip[x] = (yh + 1) as i16;
                    }
                } else if self.markfloor {
                    // No bottom wall.
                    self.floorclip[x] = (yh + 1) as i16;
                }
                if self.maskedtexture {
                    // Save texturecol for backdrawing of masked mid texture.
                    let i = self.maskedtexturecol + self.rw_x as isize;
                    self.set_opening(i, texturecolumn as i16);
                }
            }
            self.rw_scale += self.rw_scalestep;
            self.topfrac += self.topstep;
            self.bottomfrac += self.bottomstep;
            self.rw_x += 1;
        }
    }

    fn set_opening(&mut self, i: isize, v: i16) {
        if i < 0 {
            return;
        }
        let i = i as usize;
        if i >= self.openings.len() {
            self.openings.resize(i + 1, 0);
        }
        self.openings[i] = v;
    }

    /// `R_StoreWallRange`: a wall segment will be drawn between start and
    /// stop pixels (inclusive).
    fn store_wall_range(&mut self, ctx: &mut Ctx, start: i32, stop: i32) {
        // Don't overflow and crash.
        if self.limits.drawsegs.is_some_and(|m| self.drawsegs.len() >= m) {
            self.note("drawsegs limit");
            return;
        }
        let level = ctx.level;
        let segid = self.curline;
        let seg = &level.segs[segid.idx()];
        let sidedef = level.side(seg.sidedef);
        let linedef = level.line(seg.linedef);
        // Mark the segment as visible for the automap (ML_MAPPED). Kept in
        // the renderer rather than written into the sim's line flags.
        self.mark_mapped(seg.linedef.idx());
        let tr = &ctx.w.texture_translation;

        // Calculate rw_distance for scale calculation.
        self.rw_normalangle = seg.angle + Angle(ANG90);
        let mut offsetangle = (self.rw_normalangle.0 as i32).wrapping_sub(self.rw_angle1.0 as i32).wrapping_abs() as u32;
        if offsetangle > ANG90 {
            offsetangle = ANG90;
        }
        let distangle = ANG90 - offsetangle;
        let v1 = level.vertex(seg.v1);
        let hyp = self.point_to_dist(v1.x, v1.y);
        let sineval = finesine((distangle >> ANGLETOFINESHIFT) as usize);
        self.rw_distance = fixed_mul(hyp, sineval);

        let mut ds = DrawSeg {
            curline: segid,
            x1: start,
            x2: stop,
            scale1: Fixed(0),
            scale2: Fixed(0),
            scalestep: Fixed(0),
            silhouette: SIL_NONE,
            bsilheight: Fixed(0),
            tsilheight: Fixed(0),
            sprtopclip: ClipRef::None,
            sprbottomclip: ClipRef::None,
            maskedtexturecol: None,
        };
        self.rw_x = start;
        self.rw_stopx = stop + 1;

        // Calculate scale at both ends and step.
        self.rw_scale = self.scale_from_global_angle(self.viewangle + Angle(self.xtoviewangle[start as usize]));
        ds.scale1 = self.rw_scale;
        if stop > start {
            ds.scale2 = self.scale_from_global_angle(self.viewangle + Angle(self.xtoviewangle[stop as usize]));
            self.rw_scalestep = (ds.scale2 - self.rw_scale) / (stop - start);
            ds.scalestep = self.rw_scalestep;
        } else {
            // UNUSED: try to fix the stretched line bug (#if 0 in vanilla).
            ds.scale2 = ds.scale1;
        }

        // Calculate texture boundaries and decide if floor / ceiling marks
        // are needed.
        let front = level.sector(self.frontsector);
        let mut worldtop = front.ceiling_height - self.viewz;
        let mut worldbottom = front.floor_height - self.viewz;
        let (mut worldhigh, mut worldlow) = (Fixed(0), Fixed(0));
        self.midtexture = 0;
        self.toptexture = 0;
        self.bottomtexture = 0;
        self.maskedtexture = false;
        let sky = ctx.w.sky_flat;

        match self.backsector.map(|b| level.sector(b)) {
            None => {
                // Single sided line.
                self.midtexture = tr[sidedef.mid_texture.0 as usize] as usize;
                // A single sided line is terminal, so it must mark ends.
                self.markfloor = true;
                self.markceiling = true;
                self.rw_midtexturemid = if linedef.flags & doom_content::map::ml::DONTPEGBOTTOM != 0 {
                    let vtop = front.floor_height + Fixed(ctx.data.texture(sidedef.mid_texture.0 as usize).height_fixed);
                    // Bottom of texture at bottom.
                    vtop - self.viewz
                } else {
                    // Top of texture at top.
                    worldtop
                };
                self.rw_midtexturemid += sidedef.row_offset;
                ds.silhouette = SIL_BOTH;
                ds.sprtopclip = ClipRef::ScreenHeight;
                ds.sprbottomclip = ClipRef::NegOne;
                ds.bsilheight = Fixed::MAX;
                ds.tsilheight = Fixed::MIN;
            }
            Some(back) => {
                // Two sided line.
                ds.sprtopclip = ClipRef::None;
                ds.sprbottomclip = ClipRef::None;
                ds.silhouette = 0;
                if front.floor_height > back.floor_height {
                    ds.silhouette = SIL_BOTTOM;
                    ds.bsilheight = front.floor_height;
                } else if back.floor_height > self.viewz {
                    ds.silhouette = SIL_BOTTOM;
                    ds.bsilheight = Fixed::MAX;
                }
                if front.ceiling_height < back.ceiling_height {
                    ds.silhouette |= SIL_TOP;
                    ds.tsilheight = front.ceiling_height;
                } else if back.ceiling_height < self.viewz {
                    ds.silhouette |= SIL_TOP;
                    ds.tsilheight = Fixed::MIN;
                }
                if back.ceiling_height <= front.floor_height {
                    ds.sprbottomclip = ClipRef::NegOne;
                    ds.bsilheight = Fixed::MAX;
                    ds.silhouette |= SIL_BOTTOM;
                }
                if back.floor_height >= front.ceiling_height {
                    ds.sprtopclip = ClipRef::ScreenHeight;
                    ds.tsilheight = Fixed::MIN;
                    ds.silhouette |= SIL_TOP;
                }
                worldhigh = back.ceiling_height - self.viewz;
                worldlow = back.floor_height - self.viewz;
                // Hack to allow height changes in outdoor areas.
                if front.ceiling_pic == sky && back.ceiling_pic == sky {
                    worldtop = worldhigh;
                }
                self.markfloor =
                    worldlow != worldbottom || back.floor_pic != front.floor_pic || back.light_level != front.light_level;
                self.markceiling =
                    worldhigh != worldtop || back.ceiling_pic != front.ceiling_pic || back.light_level != front.light_level;
                if back.ceiling_height <= front.floor_height || back.floor_height >= front.ceiling_height {
                    // Closed door.
                    self.markceiling = true;
                    self.markfloor = true;
                }
                if worldhigh < worldtop {
                    // Top texture.
                    self.toptexture = tr[sidedef.top_texture.0 as usize] as usize;
                    self.rw_toptexturemid = if linedef.flags & doom_content::map::ml::DONTPEGTOP != 0 {
                        // Top of texture at top.
                        worldtop
                    } else {
                        let vtop = back.ceiling_height + Fixed(ctx.data.texture(sidedef.top_texture.0 as usize).height_fixed);
                        // Bottom of texture.
                        vtop - self.viewz
                    };
                }
                if worldlow > worldbottom {
                    // Bottom texture.
                    self.bottomtexture = tr[sidedef.bottom_texture.0 as usize] as usize;
                    self.rw_bottomtexturemid = if linedef.flags & doom_content::map::ml::DONTPEGBOTTOM != 0 {
                        // Bottom of texture at bottom, top of texture at top.
                        worldtop
                    } else {
                        // Top of texture at top.
                        worldlow
                    };
                }
                self.rw_toptexturemid += sidedef.row_offset;
                self.rw_bottomtexturemid += sidedef.row_offset;
                // Allocate space for masked texture tables.
                if sidedef.mid_texture.0 != 0 {
                    // Masked midtexture.
                    self.maskedtexture = true;
                    self.maskedtexturecol = self.lastopening - self.rw_x as isize;
                    ds.maskedtexturecol = Some(self.maskedtexturecol);
                    self.lastopening += (self.rw_stopx - self.rw_x) as isize;
                }
            }
        }

        // Calculate rw_offset (only needed for textured lines).
        self.segtextured = self.midtexture != 0 || self.toptexture != 0 || self.bottomtexture != 0 || self.maskedtexture;
        if self.segtextured {
            let mut offsetangle = (self.rw_normalangle - self.rw_angle1).0;
            if offsetangle > ANG180 {
                offsetangle = offsetangle.wrapping_neg();
            }
            if offsetangle > ANG90 {
                offsetangle = ANG90;
            }
            let sineval = finesine((offsetangle >> ANGLETOFINESHIFT) as usize);
            self.rw_offset = fixed_mul(hyp, sineval);
            if (self.rw_normalangle - self.rw_angle1).0 < ANG180 {
                self.rw_offset = -self.rw_offset;
            }
            self.rw_offset += sidedef.texture_offset + seg.offset;
            self.rw_centerangle = Angle(ANG90) + self.viewangle - self.rw_normalangle;
            // Calculate light table. Use different light tables for
            // horizontal / vertical / diagonal. OPTIMIZE: get rid of LIGHTSEGSHIFT
            // globally.
            if self.fixedcolormap.is_none() {
                self.wall_lights(ctx, segid, self.frontsector);
                self.walllights_fixed = false;
            }
        }

        // If a floor / ceiling plane is on the wrong side of the view plane,
        // it is definitely invisible and doesn't need to be marked.
        if front.floor_height >= self.viewz {
            // Above view plane.
            self.markfloor = false;
        }
        if front.ceiling_height <= self.viewz && front.ceiling_pic != sky {
            // Below view plane.
            self.markceiling = false;
        }

        // Calculate incremental stepping values for texture edges.
        worldtop = worldtop >> 4;
        worldbottom = worldbottom >> 4;
        self.topstep = -fixed_mul(self.rw_scalestep, worldtop);
        self.topfrac = (self.centeryfrac >> 4) - fixed_mul(worldtop, self.rw_scale);
        self.bottomstep = -fixed_mul(self.rw_scalestep, worldbottom);
        self.bottomfrac = (self.centeryfrac >> 4) - fixed_mul(worldbottom, self.rw_scale);
        if self.backsector.is_some() {
            worldhigh = worldhigh >> 4;
            worldlow = worldlow >> 4;
            if worldhigh < worldtop {
                self.pixhigh = (self.centeryfrac >> 4) - fixed_mul(worldhigh, self.rw_scale);
                self.pixhighstep = -fixed_mul(self.rw_scalestep, worldhigh);
            }
            if worldlow > worldbottom {
                self.pixlow = (self.centeryfrac >> 4) - fixed_mul(worldlow, self.rw_scale);
                self.pixlowstep = -fixed_mul(self.rw_scalestep, worldlow);
            }
        }

        // Render it.
        if self.markceiling {
            if let Some(cp) = self.ceilingplane {
                self.ceilingplane = self.check_plane(cp, self.rw_x, self.rw_stopx - 1);
            }
        }
        if self.markfloor {
            if let Some(fp) = self.floorplane {
                self.floorplane = self.check_plane(fp, self.rw_x, self.rw_stopx - 1);
            }
        }
        self.render_seg_loop(ctx);

        // Save sprite clipping info.
        let n = (self.rw_stopx - start) as usize;
        if (ds.silhouette & SIL_TOP != 0 || self.maskedtexture) && ds.sprtopclip == ClipRef::None {
            let lo = self.lastopening;
            for i in 0..n {
                let v = self.ceilingclip[start as usize + i];
                self.set_opening(lo + i as isize, v);
            }
            ds.sprtopclip = ClipRef::Openings(lo - start as isize);
            self.lastopening += n as isize;
        }
        if (ds.silhouette & SIL_BOTTOM != 0 || self.maskedtexture) && ds.sprbottomclip == ClipRef::None {
            let lo = self.lastopening;
            for i in 0..n {
                let v = self.floorclip[start as usize + i];
                self.set_opening(lo + i as isize, v);
            }
            ds.sprbottomclip = ClipRef::Openings(lo - start as isize);
            self.lastopening += n as isize;
        }
        if self.maskedtexture && ds.silhouette & SIL_TOP == 0 {
            ds.silhouette |= SIL_TOP;
            ds.tsilheight = Fixed::MIN;
        }
        if self.maskedtexture && ds.silhouette & SIL_BOTTOM == 0 {
            ds.silhouette |= SIL_BOTTOM;
            ds.bsilheight = Fixed::MAX;
        }
        if self.lastopening as usize > SCREENWIDTH * 64 {
            self.note("openings overflow (vanilla corrupts memory)");
        }
        self.drawsegs.push(ds);
    }

    // =====================================================================
    // r_plane
    // =====================================================================

    /// `R_ClearPlanes`: at begining of frame.
    fn clear_planes(&mut self) {
        // Opening / clipping determination.
        for i in 0..self.viewwidth as usize {
            self.floorclip[i] = self.viewheight as i16;
            self.ceilingclip[i] = -1;
        }
        self.lastvisplane = 0;
        self.lastopening = 0;
        // Texture calculation.
        for h in &mut self.cachedheight {
            *h = Fixed(0);
        }
        // Left to right mapping.
        let angle = (self.viewangle - Angle(ANG90)).fine();
        // Scale will be unit scale at SCREENWIDTH/2 distance.
        self.basexscale = fixed_div(finecosine(angle), self.centerxfrac);
        self.baseyscale = -fixed_div(finesine(angle), self.centerxfrac);
    }

    fn alloc_visplane(&mut self) -> Option<usize> {
        if self.limits.visplanes.is_some_and(|m| self.lastvisplane >= m) {
            self.note("no more visplanes (vanilla aborts)");
            return None;
        }
        if self.lastvisplane == self.visplanes.len() {
            self.visplanes.push(Visplane::new());
        }
        self.lastvisplane += 1;
        Some(self.lastvisplane - 1)
    }

    /// `R_FindPlane`.
    fn find_plane(&mut self, mut height: Fixed, picnum: FlatNum, mut lightlevel: i32, sky: FlatNum) -> Option<usize> {
        if picnum == sky {
            // All skys map together.
            height = Fixed(0);
            lightlevel = 0;
        }
        for i in 0..self.lastvisplane {
            let c = &self.visplanes[i];
            if height == c.height && picnum == c.picnum && lightlevel == c.lightlevel {
                return Some(i);
            }
        }
        let i = self.alloc_visplane()?;
        let c = &mut self.visplanes[i];
        c.height = height;
        c.picnum = picnum;
        c.lightlevel = lightlevel;
        c.minx = SCREENWIDTH as i32;
        c.maxx = -1;
        c.top.fill(VP_EMPTY);
        Some(i)
    }

    /// `R_CheckPlane`.
    fn check_plane(&mut self, pl: usize, start: i32, stop: i32) -> Option<usize> {
        let p = &self.visplanes[pl];
        let (intrl, unionl) = if start < p.minx { (p.minx, start) } else { (start, p.minx) };
        let (intrh, unionh) = if stop > p.maxx { (p.maxx, stop) } else { (stop, p.maxx) };
        let mut x = intrl;
        while x <= intrh {
            if p.top[(x + 1) as usize] != VP_EMPTY {
                break;
            }
            x += 1;
        }
        if x > intrh {
            let p = &mut self.visplanes[pl];
            p.minx = unionl;
            p.maxx = unionh;
            // Use the same one.
            return Some(pl);
        }
        // Make a new visplane.
        let (height, picnum, lightlevel) = (p.height, p.picnum, p.lightlevel);
        let n = self.alloc_visplane()?;
        let np = &mut self.visplanes[n];
        np.height = height;
        np.picnum = picnum;
        np.lightlevel = lightlevel;
        np.minx = start;
        np.maxx = stop;
        np.top.fill(VP_EMPTY);
        Some(n)
    }

    /// `R_MapPlane`.
    fn map_plane(&mut self, ctx: &mut Ctx, flat: &[u8], y: i32, x1: i32, x2: i32) {
        let yu = y as usize;
        let (distance, xstep, ystep);
        if self.planeheight != self.cachedheight[yu] {
            self.cachedheight[yu] = self.planeheight;
            distance = fixed_mul(self.planeheight, self.yslope[yu]);
            self.cacheddistance[yu] = distance;
            xstep = fixed_mul(distance, self.basexscale);
            ystep = fixed_mul(distance, self.baseyscale);
            self.cachedxstep[yu] = xstep;
            self.cachedystep[yu] = ystep;
        } else {
            distance = self.cacheddistance[yu];
            xstep = self.cachedxstep[yu];
            ystep = self.cachedystep[yu];
        }
        let length = fixed_mul(distance, self.distscale[x1 as usize]);
        let angle = (self.viewangle.0.wrapping_add(self.xtoviewangle[x1 as usize]) >> ANGLETOFINESHIFT) as usize;
        let xfrac = self.viewx + fixed_mul(finecosine(angle), length);
        let yfrac = -self.viewy - fixed_mul(finesine(angle), length);
        let colormap = match self.fixedcolormap {
            Some(f) => f,
            None => {
                let index = ((distance.0 >> LIGHTZSHIFT) as u32 as usize).min(MAXLIGHTZ - 1);
                self.zlight[self.planezlight][index]
            }
        };
        self.draw_span(ctx, flat, colormap, y, x1, x2, xfrac, yfrac, xstep, ystep);
    }

    /// `R_MakeSpans`.
    fn make_spans(&mut self, ctx: &mut Ctx, flat: &[u8], x: i32, mut t1: i32, mut b1: i32, mut t2: i32, mut b2: i32) {
        while t1 < t2 && t1 <= b1 {
            let s = self.spanstart[t1 as usize];
            self.map_plane(ctx, flat, t1, s, x - 1);
            t1 += 1;
        }
        while b1 > b2 && b1 >= t1 {
            let s = self.spanstart[b1 as usize];
            self.map_plane(ctx, flat, b1, s, x - 1);
            b1 -= 1;
        }
        while t2 < t1 && t2 <= b2 {
            self.spanstart[t2 as usize] = x;
            t2 += 1;
        }
        while b2 > b1 && b2 >= t2 {
            self.spanstart[b2 as usize] = x;
            b2 -= 1;
        }
    }

    /// `R_DrawPlanes`: at the end of each frame.
    fn draw_planes(&mut self, ctx: &mut Ctx) {
        let sky = ctx.w.sky_flat;
        for i in 0..self.lastvisplane {
            let (minx, maxx, picnum, height, lightlevel) = {
                let p = &self.visplanes[i];
                (p.minx, p.maxx, p.picnum, p.height, p.lightlevel)
            };
            if minx > maxx {
                continue;
            }
            // Sky flat.
            if picnum == sky {
                self.dc_iscale = Fixed(self.pspriteiscale.0 >> self.detailshift);
                // Sky is always drawn full bright, i.e. colormaps[0] is used.
                // Because of this hack, sky is not affected by INVUL
                // inverse mapping.
                self.dc_colormap = 0;
                self.dc_texturemid = Fixed(SKYTEXTUREMID);
                let skytex = ctx.w.sky_texture.0 as usize;
                for x in minx..=maxx {
                    self.dc_yl = self.visplanes[i].top[(x + 1) as usize] as i32;
                    self.dc_yh = self.visplanes[i].bottom[(x + 1) as usize] as i32;
                    if self.visplanes[i].top[(x + 1) as usize] == VP_EMPTY {
                        continue;
                    }
                    if self.dc_yl <= self.dc_yh {
                        let angle = self.viewangle.0.wrapping_add(self.xtoviewangle[x as usize]) >> ANGLETOSKYSHIFT;
                        self.dc_x = x;
                        let src = texture_column(ctx.data, skytex, angle as i32, 0);
                        self.draw_column(ctx, src, ColFunc::Base);
                    }
                }
                continue;
            }
            // Regular flat.
            let flatnum = if picnum == FlatNum::MISSING {
                None
            } else if picnum.0 as usize >= ctx.data.num_flats {
                // A wall texture used as a flat (64x64 sample).
                Some(picnum.0 as usize)
            } else {
                ctx.w.flat_translation.get(picnum.0 as usize).map(|&f| f as usize)
            };
            let empty = vec![0u8; 4096];
            let flat: &[u8] = match flatnum.and_then(|f| ctx.data.flat(f)) {
                Some(f) => f,
                None => &empty,
            };
            self.planeheight = (height - self.viewz).abs();
            let light = ((lightlevel >> LIGHTSEGSHIFT) + self.extralight).clamp(0, LIGHTLEVELS as i32 - 1);
            self.planezlight = light as usize;
            {
                let p = &mut self.visplanes[i];
                p.top[(maxx + 2) as usize] = VP_EMPTY;
                p.top[minx as usize] = VP_EMPTY;
            }
            let stop = maxx + 1;
            for x in minx..=stop {
                let (t1, b1, t2, b2) = {
                    let p = &self.visplanes[i];
                    (
                        vp_val(p.top[x as usize]),
                        vp_val(p.bottom[x as usize]),
                        vp_val(p.top[(x + 1) as usize]),
                        vp_val(p.bottom[(x + 1) as usize]),
                    )
                };
                self.make_spans(ctx, flat, x, t1, b1, t2, b2);
            }
        }
    }

    // =====================================================================
    // r_things
    // =====================================================================

    /// `R_AddSprites`: during BSP traversal, this adds sprites by sector.
    fn add_sprites(&mut self, ctx: &mut Ctx, sec: SectorId) {
        // BSP is traversed by subsector. A sector might have been split into
        // several subsectors during BSP building. Thus we check whether its
        // already added.
        if self.sector_validcount[sec.idx()] == self.validcount {
            return;
        }
        // Well, now it will be done.
        self.sector_validcount[sec.idx()] = self.validcount;
        let lightnum = (ctx.level.sector(sec).light_level as i32 >> LIGHTSEGSHIFT) + self.extralight;
        self.spritelights = lightnum.clamp(0, LIGHTLEVELS as i32 - 1) as usize;
        // Handle all things in sector.
        let mut thing = ctx.level.sector(sec).thing_list;
        while let Some(t) = thing {
            let Some(m) = ctx.w.thinkers.get_mobj(t) else { break };
            self.project_sprite(ctx, t);
            thing = m.snext;
        }
    }

    /// `R_ProjectSprite`: generates a vissprite for a thing if it might be
    /// visible.
    fn project_sprite(&mut self, ctx: &mut Ctx, id: doom_sim::MobjId) {
        let thing = ctx.w.mo(id);
        // Transform the origin point.
        let tr_x = thing.x - self.viewx;
        let tr_y = thing.y - self.viewy;
        let gxt = fixed_mul(tr_x, self.viewcos);
        let gyt = -fixed_mul(tr_y, self.viewsin);
        let tz = gxt - gyt;
        // Thing is behind view plane?
        if tz.0 < MINZ {
            return;
        }
        let xscale = fixed_div(self.projection, tz);
        let gxt = -fixed_mul(tr_x, self.viewsin);
        let gyt = fixed_mul(tr_y, self.viewcos);
        let mut tx = -(gyt + gxt);
        // Too far off the side?
        if tx.abs().0 > (tz.0 << 2) {
            return;
        }
        // Decide which patch to use for sprite relative to player.
        let Some(sprdef) = ctx.data.sprites.get(thing.sprite.idx()) else { return };
        let Some(sprframe) = sprdef.frames.get((thing.frame & doom_sim::info::FF_FRAMEMASK) as usize) else {
            // Vanilla: "R_ProjectSprite: invalid sprite frame".
            self.note("invalid sprite frame");
            return;
        };
        let (lump, flip) = if sprframe.rotate {
            // Choose a different rotation based on player view.
            let ang = self.point_to_angle(thing.x, thing.y);
            let rot = (ang.0.wrapping_sub(thing.angle.0).wrapping_add((doom_core::angle::ANG45 / 2).wrapping_mul(9)) >> 29) as usize;
            (sprframe.lump[rot] as usize, sprframe.flip[rot])
        } else {
            // Use single rotation for all views.
            (sprframe.lump[0] as usize, sprframe.flip[0])
        };
        let sl = &ctx.data.sprite_lumps[lump];
        // Calculate edges of the shape.
        tx -= Fixed(sl.offset);
        let x1 = (self.centerxfrac + fixed_mul(tx, xscale)).0 >> FRACBITS;
        // Off the right side?
        if x1 > self.viewwidth {
            return;
        }
        tx += Fixed(sl.width);
        let x2 = ((self.centerxfrac + fixed_mul(tx, xscale)).0 >> FRACBITS) - 1;
        // Off the left side.
        if x2 < 0 {
            return;
        }

        // Store information in a vissprite.
        let mut vis = VisSprite {
            mobjflags: thing.flags,
            scale: xscale << self.detailshift as u32,
            gx: thing.x,
            gy: thing.y,
            gz: thing.z,
            gzt: thing.z + Fixed(sl.topoffset),
            ..Default::default()
        };
        vis.texturemid = vis.gzt - self.viewz;
        vis.x1 = x1.max(0);
        vis.x2 = if x2 >= self.viewwidth { self.viewwidth - 1 } else { x2 };
        let iscale = fixed_div(Fixed::ONE, xscale);
        if flip {
            vis.startfrac = Fixed(sl.width - 1);
            vis.xiscale = -iscale;
        } else {
            vis.startfrac = Fixed(0);
            vis.xiscale = iscale;
        }
        if vis.x1 > x1 {
            vis.startfrac += vis.xiscale * (vis.x1 - x1);
        }
        vis.patch = lump;
        // Get light level.
        vis.colormap = if thing.flags & MF_SHADOW != 0 {
            // Shadow draw.
            None
        } else if let Some(f) = self.fixedcolormap {
            // Fixed map.
            Some(f)
        } else if thing.frame & doom_sim::info::FF_FULLBRIGHT != 0 {
            // Full bright.
            Some(0)
        } else {
            // Diminished light.
            let index = ((xscale.0 >> (LIGHTSCALESHIFT - self.detailshift)) as usize).min(MAXLIGHTSCALE - 1);
            Some(self.scalelight[self.spritelights][index])
        };
        // R_NewVisSprite.
        if self.limits.vissprites.is_some_and(|m| self.vissprites.len() >= m) {
            self.note("vissprites limit");
            return;
        }
        self.vissprites.push(vis);
    }

    /// `R_DrawMaskedColumn`: used for sprites and masked mid textures.
    /// Masked means: partly transparent, i.e. stored in posts/runs of opaque
    /// pixels.
    fn draw_masked_column(&mut self, ctx: &mut Ctx, column: Source, floorclip: &[i16], ceilingclip: &[i16], f: ColFunc) {
        let basetexturemid = self.dc_texturemid;
        let mut pos: isize = 0;
        // Guard against runaway parsing of garbage (vanilla would walk
        // arbitrary memory).
        for _ in 0..256 {
            let topdelta = column.at(pos);
            if topdelta == 0xff {
                break;
            }
            let length = column.at(pos + 1);
            // Calculate unclipped screen coordinates for post.
            let topscreen = self.sprtopscreen + self.spryscale * topdelta as i32;
            let bottomscreen = topscreen + self.spryscale * length as i32;
            self.dc_yl = (topscreen.0 + FRACUNIT - 1) >> FRACBITS;
            self.dc_yh = (bottomscreen.0 - 1) >> FRACBITS;
            let x = self.dc_x as usize;
            if self.dc_yh >= floorclip[x] as i32 {
                self.dc_yh = floorclip[x] as i32 - 1;
            }
            if self.dc_yl <= ceilingclip[x] as i32 {
                self.dc_yl = ceilingclip[x] as i32 + 1;
            }
            if self.dc_yl <= self.dc_yh {
                let src = Source { buf: column.buf, start: column.start + pos + 3, defined: column.defined };
                self.dc_texturemid = basetexturemid - Fixed((topdelta as i32) << FRACBITS);
                self.draw_column(ctx, src, f);
            }
            pos += length as isize + 4;
        }
        self.dc_texturemid = basetexturemid;
    }

    /// `R_DrawVisSprite`: mfloorclip and mceilingclip should also be set.
    fn draw_vis_sprite(&mut self, ctx: &mut Ctx, vis: &VisSprite, floorclip: &[i16], ceilingclip: &[i16]) {
        let lumpid = ctx.data.sprite_lumps[vis.patch].lump;
        let patch = ctx.data.lump(lumpid);
        let f = match vis.colormap {
            None => ColFunc::Fuzz,
            Some(cm) => {
                self.dc_colormap = cm;
                if vis.mobjflags & MF_TRANSLATION != 0 {
                    let t = ((vis.mobjflags & MF_TRANSLATION) >> MF_TRANSSHIFT) as usize;
                    // `translationtables - 256 + (flags >> (MF_TRANSSHIFT-8))`
                    ColFunc::Translated(t.saturating_sub(1) * 256)
                } else {
                    ColFunc::Base
                }
            }
        };
        self.dc_iscale = Fixed(vis.xiscale.abs().0 >> self.detailshift);
        self.dc_texturemid = vis.texturemid;
        let mut frac = vis.startfrac;
        self.spryscale = vis.scale;
        self.sprtopscreen = self.centeryfrac - fixed_mul(self.dc_texturemid, self.spryscale);
        let width = if patch.len() >= 2 { i16::from_le_bytes([patch[0], patch[1]]) as i32 } else { 0 };
        for x in vis.x1..=vis.x2 {
            self.dc_x = x;
            let texturecolumn = frac.0 >> FRACBITS;
            if texturecolumn < 0 || texturecolumn >= width {
                // Vanilla RANGECHECK: "R_DrawSpriteRange: bad texturecolumn".
                frac += vis.xiscale;
                continue;
            }
            let ci = 8 + texturecolumn as usize * 4;
            if ci + 4 <= patch.len() {
                let ofs = u32::from_le_bytes([patch[ci], patch[ci + 1], patch[ci + 2], patch[ci + 3]]) as isize;
                self.draw_masked_column(ctx, Source::new(patch, ofs), floorclip, ceilingclip, f);
            }
            frac += vis.xiscale;
        }
    }

    /// `R_DrawSprite`.
    fn draw_sprite(&mut self, ctx: &mut Ctx, spr: &VisSprite) {
        let mut clipbot = vec![-2i16; SCREENWIDTH];
        let mut cliptop = vec![-2i16; SCREENWIDTH];
        // Scan drawsegs from end to start for obscuring segs. The first drawseg
        // that has a greater scale is the clip seg.
        for dsi in (0..self.drawsegs.len()).rev() {
            let ds = self.drawsegs[dsi];
            // Determine if the drawseg obscures the sprite.
            if ds.x1 > spr.x2 || ds.x2 < spr.x1 || (ds.silhouette == 0 && ds.maskedtexturecol.is_none()) {
                // Does not cover sprite.
                continue;
            }
            let r1 = if ds.x1 < spr.x1 { spr.x1 } else { ds.x1 };
            let r2 = if ds.x2 > spr.x2 { spr.x2 } else { ds.x2 };
            let (lowscale, scale) = if ds.scale1 > ds.scale2 { (ds.scale2, ds.scale1) } else { (ds.scale1, ds.scale2) };
            if scale < spr.scale || (lowscale < spr.scale && !self.point_on_seg_side(ctx.level, spr.gx, spr.gy, ds.curline)) {
                // Masked mid texture?
                if ds.maskedtexturecol.is_some() {
                    self.render_masked_seg_range(ctx, dsi, r1, r2);
                }
                // Seg is behind sprite.
                continue;
            }
            // Clip this piece of the sprite.
            let mut silhouette = ds.silhouette;
            if spr.gz >= ds.bsilheight {
                silhouette &= !SIL_BOTTOM;
            }
            if spr.gzt <= ds.tsilheight {
                silhouette &= !SIL_TOP;
            }
            let bot = self.clip_values(ds.sprbottomclip);
            let top = self.clip_values(ds.sprtopclip);
            for x in r1..=r2 {
                let x = x as usize;
                if silhouette & SIL_BOTTOM != 0 && clipbot[x] == -2 {
                    clipbot[x] = bot[x];
                }
                if silhouette & SIL_TOP != 0 && cliptop[x] == -2 {
                    cliptop[x] = top[x];
                }
            }
        }
        // All clipping has been performed, so draw the sprite. Check for
        // unclipped columns.
        for x in spr.x1..=spr.x2 {
            let x = x as usize;
            if clipbot[x] == -2 {
                clipbot[x] = self.viewheight as i16;
            }
            if cliptop[x] == -2 {
                cliptop[x] = -1;
            }
        }
        self.draw_vis_sprite(ctx, spr, &clipbot, &cliptop);
    }

    /// `R_PointOnSegSide`.
    fn point_on_seg_side(&self, level: &Level, x: Fixed, y: Fixed, seg: SegId) -> bool {
        let s = &level.segs[seg.idx()];
        let (v1, v2) = (level.vertex(s.v1), level.vertex(s.v2));
        let (lx, ly) = (v1.x, v1.y);
        let ldx = v2.x - lx;
        let ldy = v2.y - ly;
        if ldx.0 == 0 {
            return if x <= lx { ldy.0 > 0 } else { ldy.0 < 0 };
        }
        if ldy.0 == 0 {
            return if y <= ly { ldx.0 < 0 } else { ldx.0 > 0 };
        }
        let dx = x - lx;
        let dy = y - ly;
        // Try to quickly decide by looking at sign bits.
        if (ldy.0 ^ ldx.0 ^ dx.0 ^ dy.0) < 0 {
            // (left is negative)
            return (ldy.0 ^ dx.0) < 0;
        }
        let left = fixed_mul(Fixed(ldy.0 >> FRACBITS), dx);
        let right = fixed_mul(dy, Fixed(ldx.0 >> FRACBITS));
        // Front side is false.
        right >= left
    }

    /// `R_DrawPSprite`.
    fn draw_psprite(&mut self, ctx: &mut Ctx, player: usize, psp: &doom_sim::player::PspDef) {
        let state = *ctx.w.info.state(psp.state.unwrap());
        // Decide which patch to use.
        let Some(sprdef) = ctx.data.sprites.get(state.sprite.idx()) else { return };
        let Some(sprframe) = sprdef.frames.get((state.frame & doom_sim::info::FF_FRAMEMASK) as usize) else { return };
        let lump = sprframe.lump[0] as usize;
        let flip = sprframe.flip[0];
        let sl = &ctx.data.sprite_lumps[lump];
        // Calculate edges of the shape.
        let mut tx = psp.sx - Fixed((SCREENWIDTH as i32 / 2) * FRACUNIT);
        tx -= Fixed(sl.offset);
        let x1 = (self.centerxfrac + fixed_mul(tx, self.pspritescale)).0 >> FRACBITS;
        // Off the right side.
        if x1 > self.viewwidth {
            return;
        }
        tx += Fixed(sl.width);
        let x2 = ((self.centerxfrac + fixed_mul(tx, self.pspritescale)).0 >> FRACBITS) - 1;
        // Off the left side.
        if x2 < 0 {
            return;
        }
        // Store information in a vissprite.
        let mut vis = VisSprite {
            mobjflags: 0,
            texturemid: Fixed((BASEYCENTER << FRACBITS) + FRACUNIT / 2) - (psp.sy - Fixed(sl.topoffset)),
            x1: x1.max(0),
            x2: if x2 >= self.viewwidth { self.viewwidth - 1 } else { x2 },
            scale: self.pspritescale << self.detailshift as u32,
            ..Default::default()
        };
        if flip {
            vis.xiscale = -self.pspriteiscale;
            vis.startfrac = Fixed(sl.width - 1);
        } else {
            vis.xiscale = self.pspriteiscale;
            vis.startfrac = Fixed(0);
        }
        if vis.x1 > x1 {
            vis.startfrac += vis.xiscale * (vis.x1 - x1);
        }
        vis.patch = lump;
        let p = &ctx.w.players[player];
        let inv = p.powers[power::INVISIBILITY];
        vis.colormap = if inv > 4 * 32 || inv & 8 != 0 {
            // Shadow draw.
            None
        } else if let Some(f) = self.fixedcolormap {
            // Fixed color.
            Some(f)
        } else if state.frame & doom_sim::info::FF_FULLBRIGHT != 0 {
            // Full bright.
            Some(0)
        } else {
            // Local light.
            Some(self.scalelight[self.spritelights][MAXLIGHTSCALE - 1])
        };
        let floor = self.screenheightarray.clone();
        let ceil = self.negonearray.clone();
        self.draw_vis_sprite(ctx, &vis, &floor, &ceil);
    }

    /// `R_DrawPlayerSprites`.
    fn draw_player_sprites(&mut self, ctx: &mut Ctx, player: usize) {
        let p = &ctx.w.players[player];
        let mo = ctx.w.mo(p.mo.unwrap());
        let sec = ctx.level.subsectors[mo.subsector.idx()].sector;
        // Get light level.
        let lightnum = (ctx.level.sector(sec).light_level as i32 >> LIGHTSEGSHIFT) + self.extralight;
        self.spritelights = lightnum.clamp(0, LIGHTLEVELS as i32 - 1) as usize;
        // Add all active psprites.
        let psprites = p.psprites;
        for psp in &psprites {
            if psp.state.is_some() {
                self.draw_psprite(ctx, player, psp);
            }
        }
    }

    /// `R_SortVisSprites`: selection sort into increasing scale, picking the
    /// first of equal scales.
    fn sorted_vissprites(&self) -> Vec<usize> {
        let mut unsorted: Vec<usize> = (0..self.vissprites.len()).collect();
        let mut out = Vec::with_capacity(unsorted.len());
        while !unsorted.is_empty() {
            let mut bestscale = i32::MAX;
            let mut best = 0;
            for (i, &ds) in unsorted.iter().enumerate() {
                if self.vissprites[ds].scale.0 < bestscale {
                    bestscale = self.vissprites[ds].scale.0;
                    best = i;
                }
            }
            out.push(unsorted.remove(best));
        }
        out
    }

    /// `R_DrawMasked`.
    fn draw_masked(&mut self, ctx: &mut Ctx, player: usize) {
        let order = self.sorted_vissprites();
        // Draw all vissprites back to front.
        for i in order {
            let spr = self.vissprites[i];
            self.draw_sprite(ctx, &spr);
        }
        // Render any remaining masked mid textures.
        for dsi in (0..self.drawsegs.len()).rev() {
            if self.drawsegs[dsi].maskedtexturecol.is_some() {
                let (x1, x2) = (self.drawsegs[dsi].x1, self.drawsegs[dsi].x2);
                self.render_masked_seg_range(ctx, dsi, x1, x2);
            }
        }
        // Draw the psprites on top of everything but does not draw on side
        // views.
        self.draw_player_sprites(ctx, player);
    }

    // =====================================================================
    // r_draw
    // =====================================================================

    #[inline]
    fn screen_index(&self, x: i32, y: i32) -> usize {
        ((y + self.viewwindowy) as usize) * SCREENWIDTH + (self.viewwindowx + x) as usize
    }

    /// `R_DrawColumn` / `R_DrawFuzzColumn` / `R_DrawTranslatedColumn` (and
    /// their low-detail variants).
    fn draw_column(&mut self, ctx: &mut Ctx, src: Source, f: ColFunc) {
        let cm = &ctx.data.colormaps;
        let low = self.detailshift != 0;
        match f {
            ColFunc::Base | ColFunc::Translated(_) => {
                let count = self.dc_yh - self.dc_yl;
                // Zero length, column does not exceed a pixel.
                if count < 0 {
                    return;
                }
                if self.dc_x < 0 || self.dc_x >= self.viewwidth || self.dc_yl < 0 || self.dc_yh >= self.viewheight {
                    // Vanilla RANGECHECK would abort.
                    self.note("column out of range");
                    return;
                }
                let fracstep = self.dc_iscale;
                let mut frac = self.dc_texturemid + fracstep * (self.dc_yl - self.centery);
                let base = self.dc_colormap;
                let x = if low { self.dc_x << 1 } else { self.dc_x };
                let mut idx = self.screen_index(x, self.dc_yl);
                for _ in 0..=count {
                    let (texel, defined) = match f {
                        // Re-map color indices from wall texture column using
                        // a lighting/special effects LUT.
                        ColFunc::Base => src.get(((frac.0 >> FRACBITS) & 127) as isize),
                        ColFunc::Translated(t) => {
                            let (v, d) = src.get((frac.0 >> FRACBITS) as isize);
                            (ctx.data.translations[t + v as usize], d)
                        }
                        ColFunc::Fuzz => unreachable!(),
                    };
                    let px = cm[base + texel as usize];
                    ctx.screen[idx] = px;
                    if low {
                        ctx.screen[idx + 1] = px;
                    }
                    if let Some(u) = self.undefined.as_mut() {
                        u[idx] = !defined;
                        if low {
                            u[idx + 1] = !defined;
                        }
                    }
                    idx += SCREENWIDTH;
                    frac += fracstep;
                }
            }
            ColFunc::Fuzz => {
                // Adjust borders. Low...
                if self.dc_yl == 0 {
                    self.dc_yl = 1;
                }
                // ... and high.
                if self.dc_yh == self.viewheight - 1 {
                    self.dc_yh = self.viewheight - 2;
                }
                let count = self.dc_yh - self.dc_yl;
                // Zero length.
                if count < 0 {
                    return;
                }
                let x = if low { self.dc_x << 1 } else { self.dc_x };
                let mut idx = self.screen_index(x, self.dc_yl) as isize;
                for _ in 0..=count {
                    // Lookup framebuffer, and retrieve a pixel that is either
                    // one column left or right of the current one. Add index
                    // from colormap to index.
                    let o = FUZZOFFSET[self.fuzzpos];
                    let read = |i: isize| ctx.screen.get(i as usize).copied().unwrap_or(0);
                    let px = cm[6 * 256 + read(idx + o) as usize];
                    let px2 = if low { cm[6 * 256 + read(idx + 1 + o) as usize] } else { 0 };
                    ctx.screen[idx as usize] = px;
                    if low {
                        ctx.screen[idx as usize + 1] = px2;
                    }
                    if let Some(u) = self.undefined.as_mut() {
                        // Fuzz copies its neighbour, defined or not.
                        let src_undef = |i: isize| u.get(i as usize).copied().unwrap_or(true);
                        let (a, b) = (src_undef(idx + o), src_undef(idx + 1 + o));
                        u[idx as usize] = a;
                        if low {
                            u[idx as usize + 1] = b;
                        }
                    }
                    // Clamp table lookup index.
                    self.fuzzpos += 1;
                    if self.fuzzpos == FUZZTABLE {
                        self.fuzzpos = 0;
                    }
                    idx += SCREENWIDTH as isize;
                }
            }
        }
    }

    /// `R_DrawSpan` (and `R_DrawSpanLow`): with the "packed position"
    /// arithmetic of Chocolate Doom's C version (identical output to the
    /// DOS assembly for 64x64 flats).
    #[allow(clippy::too_many_arguments)]
    fn draw_span(
        &mut self,
        ctx: &mut Ctx,
        flat: &[u8],
        colormap: usize,
        y: i32,
        x1: i32,
        x2: i32,
        xfrac: Fixed,
        yfrac: Fixed,
        xstep: Fixed,
        ystep: Fixed,
    ) {
        if x2 < x1 || x1 < 0 || x2 >= self.viewwidth || y < 0 || y >= self.viewheight {
            self.note("span out of range");
            return;
        }
        let mut position = (((xfrac.0 << 10) as u32) & 0xffff_0000) | (((yfrac.0 >> 6) as u32) & 0x0000_ffff);
        let step = (((xstep.0 << 10) as u32) & 0xffff_0000) | (((ystep.0 >> 6) as u32) & 0x0000_ffff);
        let cm = &ctx.data.colormaps;
        let low = self.detailshift != 0;
        let count = x2 - x1;
        let mut idx = self.screen_index(if low { x1 << 1 } else { x1 }, y);
        for _ in 0..=count {
            // Calculate current texture index in u,v.
            let ytemp = (position >> 4) & 0x0fc0;
            let xtemp = position >> 26;
            let spot = (xtemp | ytemp) as usize;
            // Lookup pixel from flat texture tile, re-index using light/colormap.
            let px = cm[colormap + flat[spot] as usize];
            ctx.screen[idx] = px;
            idx += 1;
            if low {
                ctx.screen[idx] = px;
                idx += 1;
            }
            position = position.wrapping_add(step);
        }
    }

    fn note(&mut self, what: &'static str) {
        if !self.overflows.contains(&what) {
            self.overflows.push(what);
        }
    }
}

/// Visplane entry as the int the C code sees (0xff byte -> 255; our u16
/// sentinel maps to a value larger than any row as well).
#[inline]
fn vp_val(v: u16) -> i32 {
    v as i32
}

/// `R_GetColumn`: the source for a texture column, positioned at its first
/// byte (`offset` is added to the start, e.g. -3 for masked mid textures).
fn texture_column(data: &RenderData, tex: usize, col: i32, offset: isize) -> Source<'_> {
    let t = data.texture(tex);
    let col = (col & t.widthmask) as usize;
    match t.columns.get(col).copied().unwrap_or(ColumnRef::Missing) {
        ColumnRef::Patch { lump, ofs } => {
            Source::new(data.lump(lump), ofs as isize + offset)
        }
        ColumnRef::Composite { ofs } => {
            Source { buf: &t.composite[..], start: ofs as isize + offset, defined: &t.composite_defined[..] }
        }
        ColumnRef::Missing => Source::new(&[][..], 0),
    }
}

/// Per-frame borrowed context.
struct Ctx<'a> {
    data: &'a RenderData,
    w: &'a World,
    level: &'a Level,
    screen: &'a mut [u8],
}

