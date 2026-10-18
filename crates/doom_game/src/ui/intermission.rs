//! Intermission (wi_stuff.c), single-player: level-finished tallies with
//! counting sounds, then the episode map with "you are here" for Doom 1.
//! Netgame and deathmatch tallies are not implemented.

use std::sync::Arc;

use doom_content::game::{GameMode, GameVersion};
use doom_content::picture::Picture;
use doom_content::ResourceStack;
use doom_sim::info::*;

use super::gfx::{draw_patch, Patches, SCREENHEIGHT, SCREENWIDTH};

const TICRATE: i32 = 35;
const NUMMAPS: usize = 9;
const NUMCMAPS: usize = 32;
const SP_STATSX: i32 = 50;
const SP_STATSY: i32 = 50;
const SP_TIMEX: i32 = 16;
const SP_TIMEY: i32 = SCREENHEIGHT as i32 - 32;
const SHOWNEXTLOCDELAY: i32 = 4;

const LNODES: [[(i32, i32); NUMMAPS]; 3] = [
    [(185, 164), (148, 143), (69, 122), (209, 102), (116, 89), (166, 55), (71, 56), (135, 29), (71, 24)],
    [(254, 25), (97, 50), (188, 64), (128, 78), (214, 92), (133, 130), (208, 136), (148, 140), (235, 158)],
    [(156, 168), (48, 154), (174, 95), (265, 75), (130, 48), (279, 23), (198, 48), (140, 25), (281, 136)],
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimType {
    Always,
    Level,
}

/// (type, period, frames, x, y, data1)
const ANIMS: [&[(AnimType, i32, usize, i32, i32, i32)]; 3] = [
    &[
        (AnimType::Always, TICRATE / 3, 3, 224, 104, 0),
        (AnimType::Always, TICRATE / 3, 3, 184, 160, 0),
        (AnimType::Always, TICRATE / 3, 3, 112, 136, 0),
        (AnimType::Always, TICRATE / 3, 3, 72, 112, 0),
        (AnimType::Always, TICRATE / 3, 3, 88, 96, 0),
        (AnimType::Always, TICRATE / 3, 3, 64, 48, 0),
        (AnimType::Always, TICRATE / 3, 3, 192, 40, 0),
        (AnimType::Always, TICRATE / 3, 3, 136, 16, 0),
        (AnimType::Always, TICRATE / 3, 3, 80, 16, 0),
        (AnimType::Always, TICRATE / 3, 3, 64, 24, 0),
    ],
    &[
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 1),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 2),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 3),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 4),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 5),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 6),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 7),
        (AnimType::Level, TICRATE / 3, 3, 192, 144, 8),
        (AnimType::Level, TICRATE / 3, 1, 128, 136, 8),
    ],
    &[
        (AnimType::Always, TICRATE / 3, 3, 104, 168, 0),
        (AnimType::Always, TICRATE / 3, 3, 40, 136, 0),
        (AnimType::Always, TICRATE / 3, 3, 160, 96, 0),
        (AnimType::Always, TICRATE / 3, 3, 104, 80, 0),
        (AnimType::Always, TICRATE / 3, 3, 120, 32, 0),
        (AnimType::Always, TICRATE / 4, 3, 40, 0, 0),
    ],
];

/// g_game.c par times (seconds).
const PARS: [[i32; 10]; 4] = [
    [0; 10],
    [0, 30, 75, 120, 90, 165, 180, 180, 30, 165],
    [0, 90, 90, 90, 120, 90, 360, 240, 30, 170],
    [0, 90, 45, 90, 150, 90, 90, 165, 30, 135],
];
const CPARS: [i32; 32] = [
    30, 90, 120, 120, 90, 150, 120, 120, 270, 90, 210, 150, 150, 150, 210, 150, 420, 150, 210, 150, 240, 150, 180,
    150, 150, 300, 330, 420, 300, 180, 120, 30,
];

/// `wbstartstruct_t` for one player.
#[derive(Clone, Debug)]
pub struct WbStart {
    /// 0-based episode, last map and next map.
    pub epsd: i32,
    pub last: i32,
    pub next: i32,
    pub didsecret: bool,
    pub maxkills: i32,
    pub maxitems: i32,
    pub maxsecret: i32,
    /// Par time in tics.
    pub partime: i32,
    pub kills: i32,
    pub items: i32,
    pub secret: i32,
    /// Level time in tics.
    pub time: i32,
}

impl WbStart {
    pub fn par_time(mode: GameMode, episode: i32, map: i32) -> i32 {
        if mode == GameMode::Commercial {
            // MAP33 would read past the table in vanilla; use 0.
            CPARS.get((map - 1) as usize).map_or(0, |p| p * TICRATE)
        } else if (1..=3).contains(&episode) && (0..10).contains(&map) {
            PARS[episode as usize][map as usize] * TICRATE
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    StatCount,
    ShowNextLoc,
    NoState,
}

struct Anim {
    kind: AnimType,
    period: i32,
    x: i32,
    y: i32,
    data1: i32,
    frames: Vec<Arc<Picture>>,
    ctr: i32,
    nexttic: i32,
}

pub struct Intermission {
    wbs: WbStart,
    commercial: bool,
    state: State,
    accelerate: bool,
    sp_state: i32,
    cnt: i32,
    bcnt: i32,
    cnt_kills: i32,
    cnt_items: i32,
    cnt_secret: i32,
    cnt_time: i32,
    cnt_par: i32,
    cnt_pause: i32,
    pointer_on: bool,
    anims: Vec<Anim>,
    // Graphics.
    background: Option<Arc<Picture>>,
    lnames: Vec<Option<Arc<Picture>>>,
    yah: [Option<Arc<Picture>>; 2],
    splat: Option<Arc<Picture>>,
    num: Vec<Arc<Picture>>,
    minus: Option<Arc<Picture>>,
    percent: Arc<Picture>,
    finished: Arc<Picture>,
    entering: Arc<Picture>,
    kills: Arc<Picture>,
    items: Arc<Picture>,
    sp_secret: Arc<Picture>,
    colon: Arc<Picture>,
    time: Arc<Picture>,
    sucks: Arc<Picture>,
    par: Arc<Picture>,
    /// Remembers previous-tic button state for acceleration.
    attackdown: bool,
    usedown: bool,
}

/// What the session must do after an intermission tic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WiEvent {
    StartSound(SfxNum),
    ChangeMusic(MusicNum),
    /// `G_WorldDone`.
    Done,
}

impl Intermission {
    /// `WI_Start`.
    pub fn new(stack: &ResourceStack, p: &mut Patches, mode: GameMode, version: GameVersion, mut wbs: WbStart, rng: &mut doom_core::DoomRandom) -> Self {
        let commercial = mode == GameMode::Commercial;
        wbs.maxkills = wbs.maxkills.max(1);
        wbs.maxitems = wbs.maxitems.max(1);
        wbs.maxsecret = wbs.maxsecret.max(1);
        if version < GameVersion::Ultimate && wbs.epsd > 2 {
            wbs.epsd -= 3;
        }
        let lnames = if commercial {
            (0..NUMCMAPS).map(|i| p.get(stack, &format!("CWILV{i:02}"))).collect()
        } else {
            (0..NUMMAPS).map(|i| p.get(stack, &format!("WILV{}{i}", wbs.epsd))).collect()
        };
        let mut anims = Vec::new();
        if !commercial && wbs.epsd < 3 {
            for (j, &(kind, period, nframes, x, y, data1)) in ANIMS[wbs.epsd as usize].iter().enumerate() {
                let frames = (0..nframes)
                    .map(|i| {
                        // Episode 2's last anim reuses anim 4's graphics.
                        let jj = if wbs.epsd == 1 && j == 8 { 4 } else { j };
                        p.need(stack, &format!("WIA{}{jj:02}{i:02}", wbs.epsd))
                    })
                    .collect();
                anims.push(Anim { kind, period, x, y, data1, frames, ctr: -1, nexttic: 0 });
            }
        }
        let background_name = if commercial || (version >= GameVersion::Ultimate && wbs.epsd == 3) {
            "INTERPIC".to_string()
        } else {
            format!("WIMAP{}", wbs.epsd)
        };
        let mut wi = Intermission {
            commercial,
            state: State::StatCount,
            accelerate: false,
            sp_state: 1,
            cnt: 0,
            bcnt: 0,
            cnt_kills: -1,
            cnt_items: -1,
            cnt_secret: -1,
            cnt_time: -1,
            cnt_par: -1,
            cnt_pause: TICRATE,
            pointer_on: false,
            anims,
            background: p.get(stack, &background_name),
            lnames,
            yah: [p.get(stack, "WIURH0"), p.get(stack, "WIURH1")],
            splat: p.get(stack, "WISPLAT"),
            num: (0..10).map(|i| p.need(stack, &format!("WINUM{i}"))).collect(),
            minus: p.get(stack, "WIMINUS"),
            percent: p.need(stack, "WIPCNT"),
            finished: p.need(stack, "WIF"),
            entering: p.need(stack, "WIENTER"),
            kills: p.need(stack, "WIOSTK"),
            items: p.need(stack, "WIOSTI"),
            sp_secret: p.need(stack, "WISCRT2"),
            colon: p.need(stack, "WICOLON"),
            time: p.need(stack, "WITIME"),
            sucks: p.need(stack, "WISUCKS"),
            par: p.need(stack, "WIPAR"),
            attackdown: true,
            usedown: true,
            wbs,
        };
        wi.init_animated_back(rng);
        wi
    }

    fn init_animated_back(&mut self, rng: &mut doom_core::DoomRandom) {
        let bcnt = self.bcnt;
        for a in &mut self.anims {
            a.ctr = -1;
            a.nexttic = match a.kind {
                AnimType::Always => bcnt + 1 + (rng.m_random() % a.period),
                AnimType::Level => bcnt + 1,
            };
        }
    }

    fn update_animated_back(&mut self) {
        let (bcnt, state, next) = (self.bcnt, self.state, self.wbs.next);
        for (i, a) in self.anims.iter_mut().enumerate() {
            if bcnt != a.nexttic {
                continue;
            }
            match a.kind {
                AnimType::Always => {
                    a.ctr += 1;
                    if a.ctr >= a.frames.len() as i32 {
                        a.ctr = 0;
                    }
                    a.nexttic = bcnt + a.period;
                }
                AnimType::Level => {
                    // Gross hack: don't show the last anim during stats.
                    if !(state == State::StatCount && i == 7) && next == a.data1 {
                        a.ctr += 1;
                        if a.ctr == a.frames.len() as i32 {
                            a.ctr -= 1;
                        }
                        a.nexttic = bcnt + a.period;
                    }
                }
            }
        }
    }

    /// `WI_Ticker`. `attack`/`use_` are the console player's buttons.
    pub fn ticker(&mut self, attack: bool, use_: bool, rng: &mut doom_core::DoomRandom, out: &mut Vec<WiEvent>) {
        self.bcnt += 1;
        if self.bcnt == 1 {
            out.push(WiEvent::ChangeMusic(if self.commercial { MUS_DM2INT } else { MUS_INTER }));
        }
        // WI_checkForAccelerate
        if attack {
            if !self.attackdown {
                self.accelerate = true;
            }
            self.attackdown = true;
        } else {
            self.attackdown = false;
        }
        if use_ {
            if !self.usedown {
                self.accelerate = true;
            }
            self.usedown = true;
        } else {
            self.usedown = false;
        }
        match self.state {
            State::StatCount => self.update_stats(rng, out),
            State::ShowNextLoc => {
                self.update_animated_back();
                self.cnt -= 1;
                if self.cnt == 0 || self.accelerate {
                    self.init_no_state();
                } else {
                    self.pointer_on = (self.cnt & 31) < 20;
                }
            }
            State::NoState => {
                self.update_animated_back();
                self.cnt -= 1;
                if self.cnt == 0 {
                    out.push(WiEvent::Done);
                }
            }
        }
    }

    fn init_no_state(&mut self) {
        self.state = State::NoState;
        self.accelerate = false;
        self.cnt = 10;
    }

    fn update_stats(&mut self, rng: &mut doom_core::DoomRandom, out: &mut Vec<WiEvent>) {
        self.update_animated_back();
        let w = &self.wbs;
        let kills = w.kills * 100 / w.maxkills;
        let items = w.items * 100 / w.maxitems;
        let secret = w.secret * 100 / w.maxsecret;
        let time = w.time / TICRATE;
        let par = w.partime / TICRATE;
        let tick_sound = self.bcnt & 3 == 0;
        if self.accelerate && self.sp_state != 10 {
            self.accelerate = false;
            self.cnt_kills = kills;
            self.cnt_items = items;
            self.cnt_secret = secret;
            self.cnt_time = time;
            self.cnt_par = par;
            out.push(WiEvent::StartSound(SFX_BAREXP));
            self.sp_state = 10;
        }
        match self.sp_state {
            2 | 4 | 6 => {
                let (cnt, target) = match self.sp_state {
                    2 => (&mut self.cnt_kills, kills),
                    4 => (&mut self.cnt_items, items),
                    _ => (&mut self.cnt_secret, secret),
                };
                *cnt += 2;
                if tick_sound {
                    out.push(WiEvent::StartSound(SFX_PISTOL));
                }
                if *cnt >= target {
                    *cnt = target;
                    out.push(WiEvent::StartSound(SFX_BAREXP));
                    self.sp_state += 1;
                }
            }
            8 => {
                if tick_sound {
                    out.push(WiEvent::StartSound(SFX_PISTOL));
                }
                self.cnt_time = (self.cnt_time + 3).min(time);
                self.cnt_par += 3;
                if self.cnt_par >= par {
                    self.cnt_par = par;
                    if self.cnt_time >= time {
                        out.push(WiEvent::StartSound(SFX_BAREXP));
                        self.sp_state += 1;
                    }
                }
            }
            10 => {
                if self.accelerate {
                    out.push(WiEvent::StartSound(SFX_SGCOCK));
                    if self.commercial {
                        self.init_no_state();
                    } else {
                        self.state = State::ShowNextLoc;
                        self.accelerate = false;
                        self.cnt = SHOWNEXTLOCDELAY * TICRATE;
                        self.init_animated_back(rng);
                    }
                }
            }
            s if s & 1 != 0 => {
                self.cnt_pause -= 1;
                if self.cnt_pause == 0 {
                    self.sp_state += 1;
                    self.cnt_pause = TICRATE;
                }
            }
            _ => {}
        }
    }

    /// `WI_drawNum`: right-aligned at x; returns the new left x.
    fn draw_num(&self, screen: &mut [u8], x: i32, y: i32, n: i32, digits: i32) -> i32 {
        let fontwidth = self.num[0].width;
        let mut digits = digits;
        if digits < 0 {
            digits = if n == 0 {
                1
            } else {
                let (mut d, mut t) = (0, n);
                while t != 0 {
                    t /= 10;
                    d += 1;
                }
                d
            };
        }
        let neg = n < 0;
        let mut n = n.abs();
        if n == 1994 {
            return 0;
        }
        let mut x = x;
        while digits > 0 {
            x -= fontwidth;
            draw_patch(screen, x, y, &self.num[(n % 10) as usize]);
            n /= 10;
            digits -= 1;
        }
        if neg {
            if let Some(m) = &self.minus {
                x -= 8;
                draw_patch(screen, x, y, m);
            }
        }
        x
    }

    fn draw_percent(&self, screen: &mut [u8], x: i32, y: i32, p: i32) {
        if p < 0 {
            return;
        }
        draw_patch(screen, x, y, &self.percent);
        self.draw_num(screen, x, y, p, -1);
    }

    fn draw_time(&self, screen: &mut [u8], x: i32, y: i32, t: i32) {
        if t < 0 {
            return;
        }
        if t <= 61 * 59 {
            let mut x = x;
            let mut div = 1;
            loop {
                let n = (t / div) % 60;
                x = self.draw_num(screen, x, y, n, 2) - self.colon.width;
                div *= 60;
                if div == 60 || t / div != 0 {
                    draw_patch(screen, x, y, &self.colon);
                }
                if t / div == 0 {
                    break;
                }
            }
        } else {
            draw_patch(screen, x - self.sucks.width, y, &self.sucks);
        }
    }

    fn draw_lf(&self, screen: &mut [u8]) {
        let mut y = 2;
        let last = self.wbs.last as usize;
        if let Some(Some(name)) = self.lnames.get(last) {
            draw_patch(screen, (SCREENWIDTH as i32 - name.width) / 2, y, name);
            y += (5 * name.height) / 4;
        }
        draw_patch(screen, (SCREENWIDTH as i32 - self.finished.width) / 2, y, &self.finished);
    }

    fn draw_el(&self, screen: &mut [u8]) {
        let mut y = 2;
        draw_patch(screen, (SCREENWIDTH as i32 - self.entering.width) / 2, y, &self.entering);
        if let Some(Some(name)) = self.lnames.get(self.wbs.next as usize) {
            y += (5 * name.height) / 4;
            draw_patch(screen, (SCREENWIDTH as i32 - name.width) / 2, y, name);
        }
    }

    fn draw_on_lnode(&self, screen: &mut [u8], n: usize, c: &[Option<Arc<Picture>>]) {
        let (nx, ny) = LNODES[self.wbs.epsd as usize][n];
        for pic in c.iter().take(2).flatten() {
            let left = nx - pic.left_offset;
            let top = ny - pic.top_offset;
            if left >= 0 && left + pic.width < SCREENWIDTH as i32 && top >= 0 && top + pic.height < SCREENHEIGHT as i32 {
                draw_patch(screen, nx, ny, pic);
                return;
            }
        }
    }

    fn draw_background(&self, screen: &mut [u8]) {
        if let Some(b) = &self.background {
            draw_patch(screen, 0, 0, b);
        }
        for a in &self.anims {
            if a.ctr >= 0 {
                draw_patch(screen, a.x, a.y, &a.frames[a.ctr as usize]);
            }
        }
    }

    /// `WI_Drawer`.
    pub fn draw(&self, screen: &mut [u8]) {
        match self.state {
            State::StatCount => {
                self.draw_background(screen);
                self.draw_lf(screen);
                let lh = (3 * self.num[0].height) / 2;
                let right = SCREENWIDTH as i32 - SP_STATSX;
                draw_patch(screen, SP_STATSX, SP_STATSY, &self.kills);
                self.draw_percent(screen, right, SP_STATSY, self.cnt_kills);
                draw_patch(screen, SP_STATSX, SP_STATSY + lh, &self.items);
                self.draw_percent(screen, right, SP_STATSY + lh, self.cnt_items);
                draw_patch(screen, SP_STATSX, SP_STATSY + 2 * lh, &self.sp_secret);
                self.draw_percent(screen, right, SP_STATSY + 2 * lh, self.cnt_secret);
                draw_patch(screen, SP_TIMEX, SP_TIMEY, &self.time);
                self.draw_time(screen, SCREENWIDTH as i32 / 2 - SP_TIMEX, SP_TIMEY, self.cnt_time);
                if self.wbs.epsd < 3 {
                    draw_patch(screen, SCREENWIDTH as i32 / 2 + SP_TIMEX, SP_TIMEY, &self.par);
                    self.draw_time(screen, SCREENWIDTH as i32 - SP_TIMEX, SP_TIMEY, self.cnt_par);
                }
            }
            State::ShowNextLoc | State::NoState => {
                let pointer = self.state == State::NoState || self.pointer_on;
                self.draw_background(screen);
                if !self.commercial {
                    if self.wbs.epsd > 2 {
                        self.draw_el(screen);
                        return;
                    }
                    let last = if self.wbs.last == 8 { self.wbs.next - 1 } else { self.wbs.last };
                    let splat = [self.splat.clone()];
                    for i in 0..=last.max(-1) {
                        if i >= 0 {
                            self.draw_on_lnode(screen, i as usize, &splat);
                        }
                    }
                    if self.wbs.didsecret {
                        self.draw_on_lnode(screen, 8, &splat);
                    }
                    if pointer {
                        self.draw_on_lnode(screen, self.wbs.next as usize, &self.yah);
                    }
                }
                if !self.commercial || self.wbs.next != 30 {
                    self.draw_el(screen);
                }
            }
        }
    }
}
