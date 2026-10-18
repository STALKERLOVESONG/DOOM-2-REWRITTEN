//! A small launcher for `doom_bevy`: pick your IWAD (found automatically in
//! the usual places, or browsed to), add mods in load order, choose a map
//! and options, and start the game.
//!
//! It lives next to `doom_bevy.exe` and starts it with an ordinary command
//! line (`--iwad`, `--pwad`, `--map` ...), so everything it does can also be
//! done by hand. Settings are saved in `launcher.cfg`, and the game's
//! output goes to `doom_log.txt`, both next to the executables (or in the
//! user's temporary folder if that one isn't writable).

// No console window behind the launcher on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod setup;
mod wad;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use setup::{Setup, SKILLS};

const DOOM_RED: Color32 = Color32::from_rgb(160, 24, 18);
const WARN: Color32 = Color32::from_rgb(235, 110, 90);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DOOM Launcher")
            .with_inner_size([640.0, 640.0])
            .with_min_inner_size([520.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("DOOM Launcher", options, Box::new(|cc| Ok(Box::new(Launcher::new(cc)))))
}

/// An IWAD in the drop-down.
struct IwadChoice {
    path: PathBuf,
    /// "DOOM II: Hell on Earth", or why it can't be used.
    title: String,
    usable: bool,
}

struct Launcher {
    setup: Setup,
    /// Where the launcher (and the game) live.
    exe_dir: PathBuf,
    /// Where settings and the log go (`exe_dir` if writable).
    data_dir: PathBuf,
    iwads: Vec<IwadChoice>,
    selected_mod: Option<usize>,
    game: Option<Child>,
    status: String,
    status_is_error: bool,
    /// The end of the last run's log, shown after a failed run.
    log_tail: Vec<String>,
    show_log: bool,
}

impl Launcher {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.set_zoom_factor(1.1);
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let data_dir = writable_dir(&exe_dir);
        let setup = Setup::load(&data_dir.join("launcher.cfg"));
        let mut l = Launcher {
            setup,
            exe_dir,
            data_dir,
            iwads: Vec::new(),
            selected_mod: None,
            game: None,
            status: String::new(),
            status_is_error: false,
            log_tail: Vec::new(),
            show_log: false,
        };
        l.rescan();
        if l.game_exe().is_none() {
            l.error(format!("{} was not found next to the launcher. Keep both programs in the same folder.", game_exe_name()));
        }
        l
    }

    fn settings_path(&self) -> PathBuf {
        self.data_dir.join("launcher.cfg")
    }

    fn log_path(&self) -> PathBuf {
        self.data_dir.join("doom_log.txt")
    }

    fn save(&mut self) {
        if let Err(e) = self.setup.save(&self.settings_path()) {
            self.error(format!("Couldn't save settings: {e}"));
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_is_error = false;
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_is_error = true;
    }

    /// The game executable: next to the launcher, or (when run from a
    /// source checkout) in cargo's release/debug output.
    fn game_exe(&self) -> Option<PathBuf> {
        let name = game_exe_name();
        let mut candidates = vec![self.exe_dir.join(&name)];
        for profile in ["release", "debug"] {
            candidates.push(self.exe_dir.join("target").join(profile).join(&name));
        }
        candidates.into_iter().find(|p| p.is_file())
    }

    /// Rebuild the IWAD list: detected ones, browsed ones, and the saved
    /// choice even if it has gone missing (so the player sees why).
    fn rescan(&mut self) {
        let mut paths = wad::find_iwads(&self.exe_dir);
        for p in self.setup.extra_iwads.iter().chain(self.setup.iwad.iter()) {
            if !paths.iter().any(|q| same_file(q, p)) {
                paths.push(p.clone());
            }
        }
        self.iwads = paths
            .into_iter()
            .map(|path| match wad::identify(&path) {
                Ok(info) if info.iwad => IwadChoice { path, title: info.title, usable: true },
                Ok(_) => IwadChoice { path, title: "a mod (PWAD), not a game".into(), usable: false },
                Err(_) if !path.exists() => IwadChoice { path, title: "file not found".into(), usable: false },
                Err(e) => IwadChoice { path, title: e, usable: false },
            })
            .collect();
        // Keep the saved choice; otherwise pick the first usable IWAD.
        let saved_ok = self.setup.iwad.as_ref().is_some_and(|s| self.iwads.iter().any(|c| c.usable && same_file(&c.path, s)));
        if !saved_ok {
            if let Some(c) = self.iwads.iter().find(|c| c.usable) {
                self.setup.iwad = Some(c.path.clone());
            }
        }
    }

    fn browse_iwad(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Select your DOOM IWAD").add_filter("DOOM IWAD", &["wad", "WAD"]).pick_file() else {
            return;
        };
        self.use_iwad(path);
    }

    fn use_iwad(&mut self, path: PathBuf) {
        match wad::identify(&path) {
            Ok(info) if info.iwad => {
                if !self.setup.extra_iwads.iter().any(|p| same_file(p, &path)) {
                    self.setup.extra_iwads.push(path.clone());
                }
                self.setup.iwad = Some(path);
                self.rescan();
                self.info(format!("Using {}.", info.title));
                self.save();
            }
            Ok(_) => self.error(format!("{} is a mod (PWAD), not a game. Add it under Mods instead.", file_name(&path))),
            Err(e) => self.error(format!("{}: {e}", file_name(&path))),
        }
    }

    fn add_mods(&mut self, paths: Vec<PathBuf>) {
        let mut added = 0;
        for p in paths {
            if !self.setup.mods.iter().any(|m| same_file(m, &p)) {
                self.setup.mods.push(p);
                added += 1;
            }
        }
        if added > 0 {
            self.selected_mod = Some(self.setup.mods.len() - 1);
            self.info(format!("Added {added} mod{}.", if added == 1 { "" } else { "s" }));
            self.save();
        }
    }

    /// Files dropped on the window: IWADs become the game, mods are added.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if dropped.is_empty() {
            return;
        }
        let mut mods = Vec::new();
        for p in dropped {
            let is_iwad = wad::identify(&p).is_ok_and(|i| i.iwad);
            if is_iwad {
                self.use_iwad(p);
            } else if wad::is_mod_file(&p) {
                mods.push(p);
            } else {
                self.error(format!("{} isn't a WAD, PK3 or DeHackEd patch.", file_name(&p)));
            }
        }
        self.add_mods(mods);
    }

    fn launch(&mut self) {
        let Some(exe) = self.game_exe() else {
            return self.error(format!("{} was not found next to the launcher.", game_exe_name()));
        };
        let Some(iwad) = self.setup.iwad.clone() else {
            return self.error("Pick a game (IWAD) first.");
        };
        if !iwad.is_file() {
            return self.error(format!("The IWAD {} is missing.", iwad.display()));
        }
        let missing: Vec<String> = self.setup.mods.iter().filter(|m| !m.is_file()).map(|m| file_name(m)).collect();
        if !missing.is_empty() {
            return self.error(format!("Missing mod files: {}. Remove them or put them back.", missing.join(", ")));
        }
        self.save();
        // Output goes to a file, not a pipe: the game keeps running (and
        // logging) if the launcher is closed.
        let log = match File::create(self.log_path()) {
            Ok(f) => f,
            Err(e) => return self.error(format!("Couldn't create {}: {e}", self.log_path().display())),
        };
        let mut cmd = Command::new(&exe);
        cmd.args(self.setup.args()).current_dir(exe.parent().unwrap_or(&self.exe_dir)).stdin(Stdio::null());
        match log.try_clone() {
            Ok(out) => {
                cmd.stdout(Stdio::from(out)).stderr(Stdio::from(log));
            }
            Err(_) => {
                cmd.stdout(Stdio::null()).stderr(Stdio::from(log));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: the game is a console program; don't open a
            // console next to its window.
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(child) => {
                self.game = Some(child);
                self.log_tail.clear();
                self.show_log = false;
                self.info("DOOM is running...");
            }
            Err(e) => self.error(format!("Couldn't start {}: {e}", exe.display())),
        }
    }

    /// Notice when the game exits; show its log if it failed.
    fn poll_game(&mut self) {
        let Some(child) = &mut self.game else { return };
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                self.game = None;
                if status.success() {
                    self.info("The game closed.");
                } else {
                    self.log_tail = read_tail(&self.log_path(), 60);
                    self.show_log = true;
                    let code = status.code().map_or_else(|| "no exit code".to_string(), |c| format!("exit code {c}"));
                    self.error(format!("The game stopped with an error ({code}). Its last messages are below."));
                }
            }
            Err(e) => {
                self.game = None;
                self.error(format!("Lost track of the game: {e}"));
            }
        }
    }

    // ---- UI -------------------------------------------------------------

    fn iwad_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Game (IWAD)");
        let selected = self.setup.iwad.clone();
        let current = selected
            .as_ref()
            .and_then(|s| self.iwads.iter().find(|c| same_file(&c.path, s)))
            .map_or("Choose your game...".to_string(), |c| format!("{}  -  {}", c.title, file_name(&c.path)));
        let mut pick = None;
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 190.0).max(200.0);
            egui::ComboBox::from_id_salt("iwad").width(width).selected_text(current).show_ui(ui, |ui| {
                for (i, c) in self.iwads.iter().enumerate() {
                    let label = format!("{}  -  {}", c.title, file_name(&c.path));
                    let text = if c.usable { RichText::new(label) } else { RichText::new(label).color(WARN) };
                    let is_sel = selected.as_ref().is_some_and(|s| same_file(s, &c.path));
                    let r = ui.add_enabled(c.usable, egui::SelectableLabel::new(is_sel, text)).on_hover_text(c.path.display().to_string());
                    if r.clicked() {
                        pick = Some(i);
                    }
                }
            });
            if ui.button("Browse...").on_hover_text("Select doom.wad, doom2.wad, tnt.wad, plutonia.wad ...").clicked() {
                self.browse_iwad();
            }
            if ui.button("Rescan").on_hover_text("Look again in WADS, DOOMWADDIR, Steam and GOG folders").clicked() {
                self.rescan();
            }
        });
        if let Some(i) = pick {
            self.setup.iwad = Some(self.iwads[i].path.clone());
            self.save();
        }
        if !self.iwads.iter().any(|c| c.usable) {
            ui.label(
                RichText::new(
                    "No game found. Click Browse and select the IWAD from your copy of DOOM or DOOM II \
                     (Steam and GOG installs are found automatically), or drop it on this window.",
                )
                .color(WARN),
            );
            ui.horizontal(|ui| {
                ui.label("No copy? Freedoom is free and works with most mods:");
                ui.hyperlink_to("freedoom.github.io", "https://freedoom.github.io/");
            });
        } else if let Some(p) = &self.setup.iwad {
            ui.label(RichText::new(p.display().to_string()).small().weak());
        }
    }

    fn mods_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Mods");
        ui.label(RichText::new("Loaded top to bottom: later mods override earlier ones. Drop files here to add them.").small().weak());
        let mut clicked = None;
        ui.horizontal(|ui| {
            let list_width = (ui.available_width() - 110.0).max(200.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(list_width);
                ui.set_min_height(130.0);
                egui::ScrollArea::vertical().id_salt("mods").max_height(170.0).auto_shrink([false, false]).show(ui, |ui| {
                    if self.setup.mods.is_empty() {
                        ui.label(RichText::new("No mods: plain game.").weak());
                    }
                    for (i, m) in self.setup.mods.iter().enumerate() {
                        let missing = !m.is_file();
                        let mut text = format!("{}.  {}   [{}]", i + 1, file_name(m), mod_kind(m));
                        if missing {
                            text.push_str("  (missing)");
                        }
                        let text = if missing { RichText::new(text).color(WARN) } else { RichText::new(text) };
                        if ui.selectable_label(self.selected_mod == Some(i), text).on_hover_text(m.display().to_string()).clicked() {
                            clicked = Some(i);
                        }
                    }
                });
            });
            ui.vertical(|ui| {
                let w = egui::vec2(96.0, 0.0);
                if ui.add(egui::Button::new("Add...").min_size(w)).clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .set_title("Add mods")
                        .add_filter("Doom mods (WAD, PK3, DEH, BEX)", wad::MOD_EXTENSIONS)
                        .add_filter("All files", &["*"])
                        .pick_files()
                    {
                        self.add_mods(files);
                    }
                }
                let sel = self.selected_mod.filter(|&i| i < self.setup.mods.len());
                if ui.add_enabled(sel.is_some(), egui::Button::new("Remove").min_size(w)).clicked() {
                    if let Some(i) = sel {
                        self.setup.mods.remove(i);
                        self.selected_mod = (!self.setup.mods.is_empty()).then(|| i.min(self.setup.mods.len() - 1));
                        self.save();
                    }
                }
                if ui.add_enabled(sel.is_some_and(|i| i > 0), egui::Button::new("Move up").min_size(w)).clicked() {
                    if let Some(i) = sel {
                        self.setup.mods.swap(i, i - 1);
                        self.selected_mod = Some(i - 1);
                        self.save();
                    }
                }
                if ui.add_enabled(sel.is_some_and(|i| i + 1 < self.setup.mods.len()), egui::Button::new("Move down").min_size(w)).clicked() {
                    if let Some(i) = sel {
                        self.setup.mods.swap(i, i + 1);
                        self.selected_mod = Some(i + 1);
                        self.save();
                    }
                }
                if ui.add_enabled(!self.setup.mods.is_empty(), egui::Button::new("Clear").min_size(w)).clicked() {
                    self.setup.mods.clear();
                    self.selected_mod = None;
                    self.save();
                }
            });
        });
        if let Some(i) = clicked {
            self.selected_mod = Some(i);
        }
    }

    fn options_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Options");
        egui::Grid::new("options").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Start map");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.setup.map).hint_text("title screen").desired_width(110.0));
                ui.label(RichText::new("E1M1, MAP01 ... (empty = title screen and menu)").small().weak());
            });
            ui.end_row();

            ui.label("Skill");
            ui.add_enabled_ui(!self.setup.map.trim().is_empty(), |ui| {
                egui::ComboBox::from_id_salt("skill")
                    .width(200.0)
                    .selected_text(SKILLS[(self.setup.skill.clamp(1, 5) - 1) as usize])
                    .show_ui(ui, |ui| {
                        for (i, name) in SKILLS.iter().enumerate() {
                            ui.selectable_value(&mut self.setup.skill, i as u8 + 1, *name);
                        }
                    })
                    .response
                    .on_disabled_hover_text("Choose the skill in the game's menu, or set a start map.");
            });
            ui.end_row();

            ui.label("Play");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.setup.fullscreen, "Fullscreen");
                ui.checkbox(&mut self.setup.classic, "Classic renderer").on_hover_text("Vanilla 320x200 look, no freelook");
                ui.checkbox(&mut self.setup.nosound, "No sound");
            });
            ui.end_row();

            ui.label("Monsters");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.setup.nomonsters, "None");
                ui.checkbox(&mut self.setup.fast, "Fast");
                ui.checkbox(&mut self.setup.respawn, "Respawn");
            });
            ui.end_row();

            ui.label("Extra");
            ui.add(
                egui::TextEdit::singleline(&mut self.setup.extra_args)
                    .hint_text("more options, e.g. --scale 4 --sensitivity 1.5")
                    .desired_width(f32::INFINITY),
            )
            .on_hover_text("Passed to the game as is; see doom_bevy --help");
            ui.end_row();
        });
    }

    fn launch_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let running = self.game.is_some();
        let ready = self.setup.iwad.is_some() && self.game_exe().is_some();
        ui.horizontal(|ui| {
            let label = if running { "Running..." } else { "LAUNCH" };
            let button = egui::Button::new(RichText::new(label).size(22.0).strong().color(Color32::WHITE))
                .fill(DOOM_RED)
                .min_size(egui::vec2(180.0, 46.0));
            if ui.add_enabled(ready && !running, button).clicked() {
                self.launch();
            }
            if running && ui.button("Stop game").clicked() {
                if let Some(child) = &mut self.game {
                    let _ = child.kill();
                }
            }
            ui.vertical(|ui| {
                let color = if self.status_is_error { WARN } else { ui.visuals().text_color() };
                ui.label(RichText::new(&self.status).color(color));
                if self.log_path().exists() && ui.small_button("Open log").clicked() {
                    self.show_log = !self.show_log;
                    if self.show_log {
                        self.log_tail = read_tail(&self.log_path(), 60);
                    }
                }
            });
        });
        if self.show_log {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("log").max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                    if self.log_tail.is_empty() {
                        ui.label(RichText::new("(the log is empty)").weak());
                    }
                    for line in &self.log_tail {
                        ui.label(RichText::new(line).monospace().small());
                    }
                });
            });
        }
        ui.add_space(4.0);
    }
}

impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_game();
        if self.game.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
        self.handle_drops(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save();
        }

        egui::TopBottomPanel::bottom("launch").show(ctx, |ui| self.launch_bar(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("page").auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("DOOM").size(30.0).strong().color(DOOM_RED));
                    ui.label(RichText::new("launcher").size(18.0).weak());
                });
                ui.add_space(4.0);
                self.iwad_section(ui);
                self.mods_section(ui);
                self.options_section(ui);
            });
        });

        // A highlight while files are dragged over the window.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            let rect = ctx.screen_rect();
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(180));
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Drop to add", egui::FontId::proportional(28.0), Color32::WHITE);
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(17.0).strong());
    ui.separator();
}

fn game_exe_name() -> String {
    format!("doom_bevy{}", std::env::consts::EXE_SUFFIX)
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn mod_kind(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("wad") => "WAD",
        Some("pk3" | "zip") => "PK3",
        Some("deh" | "bex") => "DeHackEd",
        _ => "file",
    }
}

/// Same file, comparing canonical paths when both exist (Windows paths are
/// case-insensitive and may differ in form).
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// `dir` if files can be created there, else a folder in the user's temp
/// directory (e.g. when installed under Program Files).
fn writable_dir(dir: &Path) -> PathBuf {
    let probe = dir.join(".launcher_write_test");
    if File::create(&probe).is_ok() {
        let _ = std::fs::remove_file(&probe);
        return dir.to_path_buf();
    }
    let fallback = std::env::temp_dir().join("doom_bevy");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

/// The last `n` lines of a text file.
fn read_tail(path: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}
