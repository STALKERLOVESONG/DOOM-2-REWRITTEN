//! The developer console: a command registry, the drop-down text widget
//! (input line, history, scrollback, tab completion) and its small font.
//!
//! Commands are registered by name with a handler taking the session and
//! the arguments. They run the same way from the console, from code and
//! from tests ([`crate::Session::exec`]). A word that is not a command but
//! names a console variable reads or sets that variable, so every cvar is
//! reachable without registering anything.
//!
//! The widget draws into the 320x200 frame like the rest of the
//! presentation, with a 3x5 pixel font so a line holds 80 characters.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::session::Session;
use crate::ui::Key;

/// A command's action: the session and the arguments after the command
/// name. `Ok` text is printed; `Err` is printed as an error.
pub type CommandFn = Arc<dyn Fn(&mut Session, &[String]) -> Result<String, String> + Send + Sync>;

/// Candidates for argument `index` (0 = first argument) given what the
/// command line holds so far.
pub type CompleteFn = Arc<dyn Fn(&Session, usize, &[String]) -> Vec<String> + Send + Sync>;

#[derive(Clone)]
pub struct Command {
    pub name: String,
    /// Argument synopsis (`<map>`, `[0|1]`).
    pub usage: String,
    pub help: String,
    pub run: CommandFn,
    pub complete: Option<CompleteFn>,
}

/// Commands by name (case-insensitive), kept sorted for listing and
/// completion.
#[derive(Clone, Default)]
pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// Add or replace a command.
    pub fn register(&mut self, cmd: Command) {
        let name = cmd.name.to_ascii_lowercase();
        let cmd = Command { name, ..cmd };
        match self.commands.binary_search_by(|c| c.name.as_str().cmp(&cmd.name)) {
            Ok(i) => self.commands[i] = cmd,
            Err(i) => self.commands.insert(i, cmd),
        }
    }

    /// Register a command from closures (the usual way).
    pub fn add(
        &mut self,
        name: &str,
        usage: &str,
        help: &str,
        run: impl Fn(&mut Session, &[String]) -> Result<String, String> + Send + Sync + 'static,
    ) {
        self.register(Command { name: name.into(), usage: usage.into(), help: help.into(), run: Arc::new(run), complete: None });
    }

    /// Attach a completer to a registered command.
    pub fn set_completer(&mut self, name: &str, f: impl Fn(&Session, usize, &[String]) -> Vec<String> + Send + Sync + 'static) {
        if let Some(c) = self.commands.iter_mut().find(|c| c.name.eq_ignore_ascii_case(name)) {
            c.complete = Some(Arc::new(f));
        }
    }

    pub fn get(&self, name: &str) -> Option<&Command> {
        let n = name.to_ascii_lowercase();
        self.commands.binary_search_by(|c| c.name.as_str().cmp(&n)).ok().map(|i| &self.commands[i])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.iter()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|c| c.name.as_str())
    }
}

/// Split a command line into words. Double quotes group words; `;`
/// separates commands (see [`split_commands`]).
pub fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Split at `;` outside quotes.
pub fn split_commands(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

/// Levenshtein distance (for "did you mean").
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(!ca.eq_ignore_ascii_case(&cb));
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The closest few `names` to `word`: prefix matches first, then small
/// edit distances.
pub fn suggest<'a>(word: &str, names: impl Iterator<Item = &'a str>, max: usize) -> Vec<String> {
    let w = word.to_ascii_lowercase();
    let mut scored: Vec<(usize, &str)> = names
        .filter_map(|n| {
            let l = n.to_ascii_lowercase();
            if l.starts_with(&w) || (w.len() >= 3 && l.contains(&w)) {
                Some((0, n))
            } else {
                let d = edit_distance(&w, &l);
                // Short words: one typo; longer ones: about a third wrong.
                let allowed = if w.len() <= 3 { 1 } else { 2.max(w.len() / 3) };
                (d <= allowed).then_some((d, n))
            }
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    scored.dedup_by(|a, b| a.1.eq_ignore_ascii_case(b.1));
    scored.into_iter().take(max).map(|(_, n)| n.to_string()).collect()
}

/// Longest common prefix (case-insensitive) of `words`.
pub fn common_prefix(words: &[String]) -> String {
    let Some(first) = words.first() else { return String::new() };
    let mut len = first.len();
    for w in &words[1..] {
        len = len.min(first.chars().zip(w.chars()).take_while(|(a, b)| a.eq_ignore_ascii_case(b)).count());
    }
    first.chars().take(len).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// An echoed command.
    Input,
    Output,
    Error,
    /// Engine notices.
    Info,
}

#[derive(Clone, Debug)]
pub struct ConsoleLine {
    pub kind: LineKind,
    pub text: String,
}

/// What a key press asks the session to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleAction {
    /// Run this line.
    Submit(String),
    /// Complete the word under the cursor.
    Complete,
    Close,
}

/// Characters per console line.
pub const COLUMNS: usize = 79;
const SCROLLBACK: usize = 1024;
const HISTORY: usize = 256;

/// The console's text state. Drawing and command execution live in the
/// session; this is plain editing state, testable on its own.
#[derive(Clone, Debug, Default)]
pub struct Console {
    open: bool,
    pub input: String,
    /// Cursor position in `input` (chars; input is ASCII).
    pub cursor: usize,
    history: Vec<String>,
    /// Browsing the history (index into `history`).
    hist_pos: Option<usize>,
    /// What was typed before browsing the history.
    draft: String,
    lines: VecDeque<ConsoleLine>,
    /// Lines scrolled up from the bottom.
    pub scroll: usize,
    /// Tab cycling: (line before completion, candidates, next index).
    cycle: Option<(String, Vec<String>, usize)>,
    /// Live suggestions under the input line.
    pub suggestions: Vec<String>,
    /// Frames drawn while open (cursor blink).
    pub blink: u32,
}

impl Console {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.scroll = 0;
        self.cycle = None;
    }

    pub fn toggle(&mut self) {
        let o = !self.open;
        self.set_open(o);
    }

    pub fn lines(&self) -> &VecDeque<ConsoleLine> {
        &self.lines
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
    }

    /// Add text (wrapped, split at newlines) to the scrollback.
    pub fn print(&mut self, kind: LineKind, text: &str) {
        for raw in text.split('\n') {
            let mut rest = raw.trim_end();
            if rest.is_empty() {
                self.push(kind, String::new());
                continue;
            }
            while !rest.is_empty() {
                let cut = if rest.len() <= COLUMNS {
                    rest.len()
                } else {
                    // Break at the last space that fits.
                    let fit = rest.char_indices().nth(COLUMNS).map_or(rest.len(), |(i, _)| i);
                    rest[..fit].rfind(' ').filter(|&i| i > COLUMNS / 3).unwrap_or(fit)
                };
                self.push(kind, rest[..cut].to_string());
                rest = rest[cut..].trim_start();
            }
        }
    }

    fn push(&mut self, kind: LineKind, text: String) {
        if self.lines.len() == SCROLLBACK {
            self.lines.pop_front();
        }
        self.lines.push_back(ConsoleLine { kind, text });
        if self.scroll > 0 {
            // Keep the view where the reader left it.
            self.scroll = (self.scroll + 1).min(self.lines.len());
        }
    }

    /// Typed text (printable ASCII; the toggle keys are ignored).
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            if c == '`' || c == '~' || !(' '..='~').contains(&c) {
                continue;
            }
            self.input.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.edited();
    }

    fn edited(&mut self) {
        self.cycle = None;
        self.hist_pos = None;
    }

    fn set_input(&mut self, s: String) {
        self.cursor = s.len();
        self.input = s;
        self.cycle = None;
    }

    /// Handle a key while open.
    pub fn key(&mut self, key: Key) -> Option<ConsoleAction> {
        match key {
            Key::Grave | Key::Escape => return Some(ConsoleAction::Close),
            Key::Enter => {
                let line = std::mem::take(&mut self.input);
                self.cursor = 0;
                self.edited();
                self.scroll = 0;
                if !line.trim().is_empty() && self.history.last() != Some(&line) {
                    if self.history.len() == HISTORY {
                        self.history.remove(0);
                    }
                    self.history.push(line.clone());
                }
                return Some(ConsoleAction::Submit(line));
            }
            Key::Tab => return Some(ConsoleAction::Complete),
            Key::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                }
                self.edited();
            }
            Key::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
                self.edited();
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.input.len(),
            Key::Up => {
                if self.history.is_empty() {
                    return None;
                }
                let i = match self.hist_pos {
                    None => {
                        self.draft = self.input.clone();
                        self.history.len() - 1
                    }
                    Some(i) => i.saturating_sub(1),
                };
                self.hist_pos = Some(i);
                let s = self.history[i].clone();
                self.set_input(s);
                self.hist_pos = Some(i);
            }
            Key::Down => match self.hist_pos {
                Some(i) if i + 1 < self.history.len() => {
                    let s = self.history[i + 1].clone();
                    self.set_input(s);
                    self.hist_pos = Some(i + 1);
                }
                Some(_) => {
                    let d = std::mem::take(&mut self.draft);
                    self.set_input(d);
                    self.hist_pos = None;
                }
                None => {}
            },
            Key::PageUp => self.scroll = (self.scroll + 8).min(self.lines.len().saturating_sub(1)),
            Key::PageDown => self.scroll = self.scroll.saturating_sub(8),
            _ => {}
        }
        None
    }

    /// Apply a completion: `candidates` for the last word of the input
    /// (the word itself excluded). Repeated Tab cycles through them.
    pub fn complete_with(&mut self, candidates: Vec<String>) -> Option<String> {
        if let Some((base, list, next)) = &mut self.cycle {
            if !list.is_empty() {
                let pick = list[*next % list.len()].clone();
                *next += 1;
                let line = format!("{}{} ", base, quote_word(&pick));
                let (b, l, n) = (base.clone(), list.clone(), *next);
                self.set_input(line);
                self.cycle = Some((b, l, n));
                return None;
            }
        }
        let (base, word) = split_last_word(&self.input);
        let mut list: Vec<String> = candidates.into_iter().filter(|c| c.to_ascii_lowercase().starts_with(&word.to_ascii_lowercase())).collect();
        list.sort_by_key(|a| a.to_ascii_lowercase());
        list.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        match list.len() {
            0 => None,
            1 => {
                let line = format!("{}{} ", base, quote_word(&list[0]));
                self.set_input(line);
                None
            }
            _ => {
                let prefix = common_prefix(&list);
                let shown = list.join("  ");
                if prefix.len() > word.len() {
                    let line = format!("{base}{prefix}");
                    self.set_input(line);
                } else {
                    // Nothing more in common: Tab again cycles.
                    self.cycle = Some((base.to_string(), list, 0));
                }
                Some(shown)
            }
        }
    }
}

fn quote_word(w: &str) -> String {
    if w.contains(' ') {
        format!("\"{w}\"")
    } else {
        w.to_string()
    }
}

/// (everything before the last word, the last word). A trailing space
/// means an empty last word.
pub fn split_last_word(line: &str) -> (&str, &str) {
    let cut = line.rfind([' ', ';']).map_or(0, |i| i + 1);
    (&line[..cut], &line[cut..])
}

// ---- font ------------------------------------------------------------------------

/// Glyph advance and line height of the console font.
pub const CHAR_W: i32 = 4;
pub const LINE_H: i32 = 6;

/// 3x5 glyphs for ' '..='~' (lower case drawn as upper case): five rows,
/// three bits each (bit 2 = left column).
const GLYPHS: [[u8; 5]; 95] = {
    const fn g(rows: [&str; 5]) -> [u8; 5] {
        let mut out = [0u8; 5];
        let mut r = 0;
        while r < 5 {
            let b = rows[r].as_bytes();
            let mut v = 0u8;
            let mut c = 0;
            while c < 3 {
                v = (v << 1) | (b[c] == b'#') as u8;
                c += 1;
            }
            out[r] = v;
            r += 1;
        }
        out
    }
    let upper: [[u8; 5]; 64] = [
        g(["...", "...", "...", "...", "..."]), // space
        g([".#.", ".#.", ".#.", "...", ".#."]), // !
        g(["#.#", "#.#", "...", "...", "..."]), // "
        g(["#.#", "###", "#.#", "###", "#.#"]), // #
        g([".##", "##.", "###", ".##", "##."]), // $
        g(["#.#", "..#", ".#.", "#..", "#.#"]), // %
        g([".#.", "#.#", ".#.", "#.#", ".##"]), // &
        g([".#.", ".#.", "...", "...", "..."]), // '
        g(["..#", ".#.", ".#.", ".#.", "..#"]), // (
        g(["#..", ".#.", ".#.", ".#.", "#.."]), // )
        g(["...", "#.#", ".#.", "#.#", "..."]), // *
        g(["...", ".#.", "###", ".#.", "..."]), // +
        g(["...", "...", "...", ".#.", "#.."]), // ,
        g(["...", "...", "###", "...", "..."]), // -
        g(["...", "...", "...", "...", ".#."]), // .
        g(["..#", "..#", ".#.", "#..", "#.."]), // /
        g(["###", "#.#", "#.#", "#.#", "###"]), // 0
        g([".#.", "##.", ".#.", ".#.", "###"]), // 1
        g(["###", "..#", "###", "#..", "###"]), // 2
        g(["###", "..#", ".##", "..#", "###"]), // 3
        g(["#.#", "#.#", "###", "..#", "..#"]), // 4
        g(["###", "#..", "###", "..#", "###"]), // 5
        g(["###", "#..", "###", "#.#", "###"]), // 6
        g(["###", "..#", "..#", ".#.", ".#."]), // 7
        g(["###", "#.#", "###", "#.#", "###"]), // 8
        g(["###", "#.#", "###", "..#", "###"]), // 9
        g(["...", ".#.", "...", ".#.", "..."]), // :
        g(["...", ".#.", "...", ".#.", "#.."]), // ;
        g(["..#", ".#.", "#..", ".#.", "..#"]), // <
        g(["...", "###", "...", "###", "..."]), // =
        g(["#..", ".#.", "..#", ".#.", "#.."]), // >
        g(["###", "..#", ".##", "...", ".#."]), // ?
        g([".#.", "#.#", "###", "#..", ".##"]), // @
        g([".#.", "#.#", "###", "#.#", "#.#"]), // A
        g(["##.", "#.#", "##.", "#.#", "##."]), // B
        g([".##", "#..", "#..", "#..", ".##"]), // C
        g(["##.", "#.#", "#.#", "#.#", "##."]), // D
        g(["###", "#..", "##.", "#..", "###"]), // E
        g(["###", "#..", "##.", "#..", "#.."]), // F
        g([".##", "#..", "#.#", "#.#", ".##"]), // G
        g(["#.#", "#.#", "###", "#.#", "#.#"]), // H
        g(["###", ".#.", ".#.", ".#.", "###"]), // I
        g(["..#", "..#", "..#", "#.#", ".#."]), // J
        g(["#.#", "#.#", "##.", "#.#", "#.#"]), // K
        g(["#..", "#..", "#..", "#..", "###"]), // L
        g(["#.#", "###", "###", "#.#", "#.#"]), // M
        g(["##.", "#.#", "#.#", "#.#", "#.#"]), // N
        g([".#.", "#.#", "#.#", "#.#", ".#."]), // O
        g(["##.", "#.#", "##.", "#..", "#.."]), // P
        g([".#.", "#.#", "#.#", "###", ".##"]), // Q
        g(["##.", "#.#", "##.", "#.#", "#.#"]), // R
        g([".##", "#..", ".#.", "..#", "##."]), // S
        g(["###", ".#.", ".#.", ".#.", ".#."]), // T
        g(["#.#", "#.#", "#.#", "#.#", "###"]), // U
        g(["#.#", "#.#", "#.#", ".#.", ".#."]), // V
        g(["#.#", "#.#", "###", "###", "#.#"]), // W
        g(["#.#", "#.#", ".#.", "#.#", "#.#"]), // X
        g(["#.#", "#.#", ".#.", ".#.", ".#."]), // Y
        g(["###", "..#", ".#.", "#..", "###"]), // Z
        g(["##.", "#..", "#..", "#..", "##."]), // [
        g(["#..", "#..", ".#.", "..#", "..#"]), // backslash
        g([".##", "..#", "..#", "..#", ".##"]), // ]
        g([".#.", "#.#", "...", "...", "..."]), // ^
        g(["...", "...", "...", "...", "###"]), // _
    ];
    let tail: [[u8; 5]; 5] = [
        g(["#..", ".#.", "...", "...", "..."]), // `
        g([".##", ".#.", "##.", ".#.", ".##"]), // {
        g([".#.", ".#.", ".#.", ".#.", ".#."]), // |
        g(["##.", ".#.", ".##", ".#.", "##."]), // }
        g(["...", ".##", "##.", "...", "..."]), // ~
    ];
    let mut out = [[0u8; 5]; 95];
    let mut i = 0;
    while i < 64 {
        out[i] = upper[i];
        i += 1;
    }
    // a..z share the capitals.
    let mut c = 0;
    while c < 26 {
        out[(b'a' - b' ') as usize + c] = upper[(b'A' - b' ') as usize + c];
        c += 1;
    }
    out[(b'`' - b' ') as usize] = tail[0];
    let mut k = 1;
    while k < 5 {
        out[(b'{' - b' ') as usize + k - 1] = tail[k];
        k += 1;
    }
    out
};

/// Draw `text` at (x, y) in palette colour `color`, clipped to the screen.
pub fn draw_text(screen: &mut [u8], width: usize, x: i32, y: i32, text: &str, color: u8) -> i32 {
    let height = (screen.len() / width) as i32;
    let mut cx = x;
    for c in text.bytes() {
        let i = c.wrapping_sub(b' ') as usize;
        if i < GLYPHS.len() {
            for (r, bits) in GLYPHS[i].iter().enumerate() {
                for col in 0..3 {
                    if bits & (4 >> col) != 0 {
                        let (px, py) = (cx + col, y + r as i32);
                        if px >= 0 && py >= 0 && (px as usize) < width && py < height {
                            screen[py as usize * width + px as usize] = color;
                        }
                    }
                }
            }
        }
        cx += CHAR_W;
    }
    cx
}

/// Draw text with a one-pixel dark outline (readable over the 3D view).
pub fn draw_text_shadow(screen: &mut [u8], width: usize, x: i32, y: i32, text: &str, color: u8, shadow: u8) {
    draw_text(screen, width, x + 1, y + 1, text, shadow);
    draw_text(screen, width, x, y, text, color);
}

/// Palette indices the console draws with, picked from PLAYPAL.
#[derive(Clone, Copy, Debug, Default)]
pub struct Palette {
    pub white: u8,
    pub gray: u8,
    pub dark: u8,
    pub black: u8,
    pub yellow: u8,
    pub red: u8,
    pub green: u8,
    pub cyan: u8,
    pub orange: u8,
    pub magenta: u8,
    pub blue: u8,
    pub brown: u8,
    pub skin: u8,
}

/// The palette entry nearest an RGB colour.
pub fn nearest(pal: &[[u8; 3]; 256], rgb: [u8; 3]) -> u8 {
    let mut best = (u32::MAX, 0u8);
    for (i, c) in pal.iter().enumerate() {
        let d: u32 = (0..3).map(|k| (c[k] as i32 - rgb[k] as i32).pow(2) as u32).sum();
        if d < best.0 {
            best = (d, i as u8);
        }
    }
    best.1
}

impl Palette {
    pub fn from_playpal(pal: &[[u8; 3]; 256]) -> Palette {
        let n = |rgb| nearest(pal, rgb);
        Palette {
            white: n([255, 255, 255]),
            gray: n([160, 160, 160]),
            dark: n([60, 60, 60]),
            black: n([0, 0, 0]),
            yellow: n([255, 230, 60]),
            red: n([255, 60, 60]),
            green: n([80, 255, 80]),
            cyan: n([80, 220, 255]),
            orange: n([255, 150, 40]),
            magenta: n([255, 80, 255]),
            blue: n([70, 90, 255]),
            brown: n([150, 95, 50]),
            skin: n([220, 160, 120]),
        }
    }
}

/// Draw the console: a darkened band at the top of the screen with the
/// scrollback, the input line and live suggestions below it.
///
/// `darken` is a COLORMAP row (256 entries) applied to what is behind.
pub fn draw(con: &mut Console, screen: &mut [u8], width: usize, rows: i32, pal: &Palette, darken: &[u8], prompt_hint: Option<&str>) {
    con.blink = con.blink.wrapping_add(1);
    let h = rows.clamp(LINE_H * 3, (screen.len() / width) as i32);
    for px in &mut screen[..h as usize * width] {
        *px = darken[*px as usize];
    }
    for x in 0..width {
        screen[(h as usize - 1) * width + x] = pal.yellow;
    }
    // Input line.
    let iy = h - LINE_H - 1;
    let shown_cols = COLUMNS as i32 - 2;
    let start = con.cursor.saturating_sub(shown_cols as usize - 1);
    let visible: String = con.input.chars().skip(start).take(shown_cols as usize).collect();
    draw_text(screen, width, 1, iy, "]", pal.yellow);
    draw_text(screen, width, 1 + CHAR_W, iy, &visible, pal.white);
    if (con.blink / 16) % 2 == 0 {
        let cx = 1 + CHAR_W * (1 + (con.cursor - start) as i32);
        for r in 0..5 {
            let y = iy + r;
            if cx >= 0 && (cx as usize) < width {
                screen[y as usize * width + cx as usize] = pal.yellow;
            }
        }
    }
    // Scrollback, newest at the bottom.
    let rows_avail = ((iy - 1) / LINE_H) as usize;
    let total = con.lines.len();
    let end = total.saturating_sub(con.scroll);
    let first = end.saturating_sub(rows_avail);
    for (k, line) in con.lines.range(first..end).enumerate() {
        let y = 1 + (k + rows_avail - (end - first)) as i32 * LINE_H;
        let color = match line.kind {
            LineKind::Input => pal.yellow,
            LineKind::Output => pal.white,
            LineKind::Error => pal.red,
            LineKind::Info => pal.gray,
        };
        draw_text(screen, width, 1, y, &line.text, color);
    }
    if con.scroll > 0 {
        let tag = format!("^ {} more ^", con.scroll);
        let x = width as i32 - 1 - CHAR_W * tag.len() as i32;
        draw_text(screen, width, x, iy - LINE_H, &tag, pal.orange);
    }
    // Suggestions under the console.
    let mut y = h + 1;
    let list: Vec<&str> = con.suggestions.iter().map(String::as_str).chain(prompt_hint).collect();
    if !list.is_empty() {
        let lines = list.len().min(6) as i32;
        let wmax = list.iter().take(6).map(|s| s.len().min(COLUMNS)).max().unwrap_or(0) as i32;
        let box_h = lines * LINE_H + 2;
        let screen_h = (screen.len() / width) as i32;
        for yy in h..(h + box_h).min(screen_h) {
            for xx in 0..((wmax * CHAR_W + 3) as usize).min(width) {
                let p = &mut screen[yy as usize * width + xx];
                *p = darken[*p as usize];
            }
        }
        for s in list.iter().take(6) {
            let text: String = s.chars().take(COLUMNS).collect();
            draw_text(screen, width, 1, y, &text, pal.cyan);
            y += LINE_H;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_quotes_and_split() {
        assert_eq!(tokenize(r#"give  "blue card" 3"#), vec!["give", "blue card", "3"]);
        assert_eq!(split_commands(r#"god; echo "a;b""#), vec!["god", r#" echo "a;b""#]);
        assert_eq!(tokenize(r#"echo """#), vec!["echo", ""]);
    }

    #[test]
    fn suggestions_prefer_prefixes() {
        let names = ["give", "god", "noclip", "notarget", "map"];
        assert_eq!(suggest("no", names.iter().copied(), 5), vec!["noclip", "notarget"]);
        assert_eq!(suggest("gvie", names.iter().copied(), 1), vec!["give"]);
    }

    #[test]
    fn history_and_editing() {
        let mut c = Console::default();
        c.set_open(true);
        c.type_text("god");
        assert_eq!(c.key(Key::Enter), Some(ConsoleAction::Submit("god".into())));
        c.type_text("map e1m2");
        c.key(Key::Enter);
        c.type_text("x");
        c.key(Key::Up);
        assert_eq!(c.input, "map e1m2");
        c.key(Key::Up);
        assert_eq!(c.input, "god");
        c.key(Key::Down);
        c.key(Key::Down);
        assert_eq!(c.input, "x", "the draft comes back");
        c.key(Key::Home);
        c.type_text("a");
        assert_eq!(c.input, "ax");
        c.key(Key::Backspace);
        c.key(Key::Delete);
        assert_eq!(c.input, "");
        c.type_text("`~");
        assert_eq!(c.input, "", "toggle keys are not typed");
    }

    #[test]
    fn completion_extends_then_cycles() {
        let mut c = Console::default();
        c.type_text("show_c");
        assert_eq!(c.complete_with(vec!["show_collision".into(), "show_player".into()]), None);
        assert_eq!(c.input, "show_collision ");
        let mut c = Console::default();
        c.type_text("no");
        let listed = c.complete_with(vec!["noclip".into(), "notarget".into()]);
        assert_eq!(listed.as_deref(), Some("noclip  notarget"));
        assert_eq!(c.input, "no");
        c.complete_with(Vec::new());
        assert_eq!(c.input, "noclip ");
        c.complete_with(Vec::new());
        assert_eq!(c.input, "notarget ");
    }

    #[test]
    fn wrapping() {
        let mut c = Console::default();
        let long = "word ".repeat(40);
        c.print(LineKind::Output, &long);
        assert!(c.lines().len() >= 2);
        assert!(c.lines().iter().all(|l| l.text.len() <= COLUMNS));
    }
}
