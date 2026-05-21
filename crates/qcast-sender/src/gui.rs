//! The pre-launch GUI. Shows the preflight checklist + the viewer URL/QR, lets
//! the operator confirm, then starts the host and **fully closes the window** so
//! Qcast keeps streaming as a pure background process — no taskbar entry (Wayland
//! gives clients no "skip taskbar" control, so hiding only minimizes; closing is
//! the only way to truly leave the taskbar). The running host is handed back to
//! `main`, which keeps the process alive until a hotkey/kill stops it.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use crate::access_code;
use crate::host::{self, CodecPref, HostConfig, RunningHost};
use crate::preflight::{self, Report};

/// The standard resolution presets the operator picks from in the GUI.
const PRESET_720P: (u32, u32) = (1280, 720);
const PRESET_1080P: (u32, u32) = (host::VIDEO_MAX_WIDTH, host::VIDEO_MAX_HEIGHT);

/// Which resolution-cap mode the Settings selector is in. Presets pin a known-good
/// box; Custom exposes width/height text fields with a clear warning.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResMode {
    P720,
    P1080,
    Custom,
}

/// Lock a `Mutex` tolerantly: if a previous holder panicked and poisoned it, we
/// recover the inner guard rather than propagating the panic. The data this GUI
/// guards (the started host + a bool) is plain state with no broken invariant a
/// panic could leave behind, so recovering is safe and keeps one panic from
/// cascading into a process abort.
fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("recovering from a poisoned GUI mutex");
        poisoned.into_inner()
    })
}

/// Lets the Ctrl+C/SIGTERM handler (no `&App`) wake the event loop to close it.
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// What the operator decided in the pre-launch window.
pub enum Outcome {
    /// Window closed without starting — exit.
    Quit,
    /// Streaming started; the window is closed (no taskbar entry). The caller
    /// keeps the process alive headless until told to stop.
    Background(RunningHost),
}

/// Passed out of the closed event loop: the started host + the chosen outcome.
struct Shared {
    host: Option<RunningHost>,
    background: bool,
}

/// Show the pre-launch window. Returns once it closes.
pub fn run(cfg: HostConfig, quit: Arc<AtomicBool>) -> anyhow::Result<Outcome> {
    let report = preflight::run(&cfg.host, cfg.web_port);
    let shared = Arc::new(Mutex::new(Shared {
        host: None,
        background: false,
    }));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 660.0])
            .with_title("Qcast")
            .with_resizable(false),
        ..Default::default()
    };

    let sh = shared.clone();
    eframe::run_native(
        "Qcast",
        options,
        Box::new(move |cc| {
            let _ = EGUI_CTX.set(cc.egui_ctx.clone());
            Ok(Box::new(App::new(cfg, report, sh, quit)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI failed: {e}"))?;

    let mut s = lock_or_recover(&shared);
    if s.background {
        Ok(Outcome::Background(
            s.host.take().expect("background set without a host"),
        ))
    } else {
        Ok(Outcome::Quit)
    }
}

/// Wake the GUI event loop from another thread (used by the Ctrl+C handler).
pub fn wake() {
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
}

enum Stage {
    Preflight,
    Starting,
    Error(String),
}

enum StageTag {
    Preflight,
    Starting,
    Error,
}

struct App {
    cfg: HostConfig,
    report: Report,
    stage: Stage,
    qr: Option<(usize, Vec<bool>)>,
    shared: Arc<Mutex<Shared>>,
    quit: Arc<AtomicBool>,
    start_rx: Option<mpsc::Receiver<Result<RunningHost, String>>>,
    /// Settings UI state: which resolution mode the selector is showing.
    res_mode: ResMode,
    /// Settings UI state: the custom width/height as editable text. Only consulted
    /// (and committed to `cfg`) while `res_mode == ResMode::Custom`.
    custom_w: String,
    custom_h: String,
}

impl App {
    fn new(
        cfg: HostConfig,
        report: Report,
        shared: Arc<Mutex<Shared>>,
        quit: Arc<AtomicBool>,
    ) -> Self {
        let qr = build_qr(&report.url);
        // Pre-populate the Settings selector from the incoming cfg (CLI may have
        // set a non-preset resolution; reflect that as Custom).
        let res_mode = match (cfg.max_width, cfg.max_height) {
            PRESET_720P => ResMode::P720,
            PRESET_1080P => ResMode::P1080,
            _ => ResMode::Custom,
        };
        let custom_w = cfg.max_width.to_string();
        let custom_h = cfg.max_height.to_string();
        Self {
            cfg,
            report,
            stage: Stage::Preflight,
            qr,
            shared,
            quit,
            start_rx: None,
            res_mode,
            custom_w,
            custom_h,
        }
    }

    /// Start the host on a worker thread so the UI stays responsive during the
    /// capture handshake (and the portal picker dialog).
    fn begin_start(&mut self) {
        // Backstop: the Settings UI only commits valid resolutions to `cfg`, but
        // re-validate at this boundary so no invalid value can ever reach the
        // pipeline (and surface a clear message instead of a silent bad start).
        if let Err(e) = host::validate_resolution(self.cfg.max_width, self.cfg.max_height) {
            self.stage = Stage::Error(format!("{e:#}"));
            return;
        }
        let cfg = self.cfg.clone();
        let (tx, rx) = mpsc::channel();
        self.start_rx = Some(rx);
        self.stage = Stage::Starting;
        std::thread::spawn(move || {
            let _ = tx.send(host::start(cfg).map_err(|e| format!("{e:#}")));
        });
    }

    fn stage_tag(&self) -> StageTag {
        match self.stage {
            Stage::Preflight => StageTag::Preflight,
            Stage::Starting => StageTag::Starting,
            Stage::Error(_) => StageTag::Error,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(150));

        // Ctrl+C / SIGTERM during the window phase: close (=> Outcome::Quit).
        if self.quit.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Pick up the async start result.
        if let Some(rx) = &self.start_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(h) => {
                        {
                            let mut s = lock_or_recover(&self.shared);
                            s.host = Some(h);
                            s.background = true;
                        }
                        // Close the window entirely -> no taskbar entry; main keeps
                        // the process running headless.
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => self.stage = Stage::Error(e),
                }
                self.start_rx = None;
            }
        }

        match self.stage_tag() {
            StageTag::Preflight => {
                // The window is fixed-size (540x660); the added Settings group can
                // push past that, so scroll the content rather than enlarge the
                // window.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.ui_preflight(ui));
            }
            StageTag::Starting => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.heading("Starting…");
                    ui.add_space(12.0);
                    ui.spinner();
                    ui.add_space(12.0);
                    ui.label("If a screen-share dialog appears, approve it to pick what to share.");
                });
            }
            StageTag::Error => {
                let err = match &self.stage {
                    Stage::Error(e) => e.clone(),
                    _ => String::new(),
                };
                ui.add_space(20.0);
                ui.heading("Couldn't start");
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::LIGHT_RED, err);
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.button("Retry").clicked() {
                        self.stage = Stage::Preflight;
                    }
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
        }
    }
}

impl App {
    fn ui_preflight(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading("Qcast");
        ui.label("Share your screen to any browser on your network.");
        ui.separator();

        ui.add_space(4.0);
        ui.strong("System check");
        ui.add_space(4.0);
        for c in &self.report.checks {
            ui.horizontal(|ui| {
                let (mark, color) = if c.ok {
                    ("✔", egui::Color32::from_rgb(80, 200, 120))
                } else if c.critical {
                    ("✖", egui::Color32::from_rgb(230, 90, 90))
                } else {
                    ("•", egui::Color32::GRAY)
                };
                ui.colored_label(color, mark);
                ui.label(&c.name);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(&c.detail);
                });
            });
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        self.ui_settings(ui);

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        let ready = self.report.ready();
        if ready {
            ui.strong("Viewers connect at:");
            ui.horizontal(|ui| {
                ui.monospace(&self.report.url);
                if ui.small_button("copy").clicked() {
                    ui.ctx().copy_text(self.report.url.clone());
                }
            });

            // The access code is shared SEPARATELY from the URL/QR (which only
            // reach the page). The viewer types this on the page's password gate.
            ui.add_space(10.0);
            ui.strong("Password (viewers enter this):");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&self.cfg.access_code)
                        .monospace()
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(120, 200, 255)),
                );
                if ui.small_button("copy").clicked() {
                    ui.ctx().copy_text(self.cfg.access_code.clone());
                }
                // Regenerate a fresh access code before Start. The started host
                // reads the code at start time, so this only matters pre-launch;
                // the URL/QR are unaffected (the code is not embedded in them).
                if ui.small_button("regenerate").clicked() {
                    self.cfg.access_code = access_code::generate();
                }
            });
            ui.weak("Share this with viewers separately — they type it on the page to start watching.");

            ui.add_space(8.0);
            if let Some((side, dark)) = &self.qr {
                draw_qr(ui, *side, dark, 220.0);
                ui.add_space(4.0);
                ui.weak("Scan with a phone camera to open the viewer.");
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(230, 90, 90),
                "Some required components are missing — run the Qcast setup script, then reopen.",
            );
        }

        ui.add_space(14.0);
        ui.vertical_centered(|ui| {
            let btn = egui::Button::new(egui::RichText::new("Start & run in background").size(16.0))
                .min_size(egui::vec2(280.0, 38.0));
            if ui.add_enabled(ready, btn).clicked() {
                self.begin_start();
            }
            ui.add_space(6.0);
            ui.weak("This window closes and Qcast keeps streaming in the background");
            ui.weak("(no taskbar entry). Stop it with Ctrl+Alt+Q or by killing the process.");
        });
    }

    /// Pre-launch settings the operator can tweak before Start: the resolution
    /// cap, the codec preference, and a regenerate-password action (the password
    /// itself is regenerated from its own button in the password row above). Edits
    /// here update `self.cfg`, so they take effect when Start builds the pipeline.
    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.strong("Settings");
        ui.add_space(4.0);

        // --- Resolution cap -------------------------------------------------
        ui.horizontal(|ui| {
            ui.label("Resolution:");
            // Pick a mode; switching to a preset commits its dimensions to cfg
            // immediately. Custom defers to the width/height fields below.
            let mut mode = self.res_mode;
            ui.selectable_value(&mut mode, ResMode::P720, "720p");
            ui.selectable_value(&mut mode, ResMode::P1080, "1080p");
            ui.selectable_value(&mut mode, ResMode::Custom, "Advanced (custom)");
            if mode != self.res_mode {
                self.res_mode = mode;
                if let Some((w, h)) = match mode {
                    ResMode::P720 => Some(PRESET_720P),
                    ResMode::P1080 => Some(PRESET_1080P),
                    ResMode::Custom => None,
                } {
                    self.cfg.max_width = w;
                    self.cfg.max_height = h;
                    self.custom_w = w.to_string();
                    self.custom_h = h.to_string();
                }
            }
        });

        if self.res_mode == ResMode::Custom {
            ui.add_space(2.0);
            ui.colored_label(
                egui::Color32::from_rgb(230, 180, 70),
                "WARNING: custom resolutions above 1080p may not decode on every \
                 browser. Browser WebRTC decoders have hard frame-size ceilings \
                 (e.g. Firefox H.264 ≈ 720p; VP8 ≈ 3.1 MP). 1080p is the \
                 universally-decodable baseline.",
            );
            ui.horizontal(|ui| {
                ui.label("Width:");
                let w_edit = ui.add(egui::TextEdit::singleline(&mut self.custom_w).desired_width(70.0));
                ui.label("×  Height:");
                let h_edit = ui.add(egui::TextEdit::singleline(&mut self.custom_h).desired_width(70.0));
                // Commit parsed, valid dimensions to cfg on every edit; leave the
                // last good value in cfg if the current text is invalid.
                if w_edit.changed() || h_edit.changed() {
                    if let (Ok(w), Ok(h)) =
                        (self.custom_w.trim().parse::<u32>(), self.custom_h.trim().parse::<u32>())
                    {
                        if host::validate_resolution(w, h).is_ok() {
                            self.cfg.max_width = w;
                            self.cfg.max_height = h;
                        }
                    }
                }
            });
            // Inline validation feedback for the current text.
            match (self.custom_w.trim().parse::<u32>(), self.custom_h.trim().parse::<u32>()) {
                (Ok(w), Ok(h)) => {
                    if let Err(e) = host::validate_resolution(w, h) {
                        ui.colored_label(egui::Color32::from_rgb(230, 90, 90), e.to_string());
                    }
                }
                _ => {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 90, 90),
                        "Width and height must be whole numbers.",
                    );
                }
            }
        }

        // The >1080p warning applies to ANY chosen resolution exceeding 1080p,
        // including a custom one entered as a preset-looking value.
        if self.cfg.max_width > host::VIDEO_MAX_WIDTH || self.cfg.max_height > host::VIDEO_MAX_HEIGHT
        {
            ui.colored_label(
                egui::Color32::from_rgb(230, 180, 70),
                format!(
                    "⚠ {}×{} exceeds 1080p — may not decode on some browsers.",
                    self.cfg.max_width, self.cfg.max_height
                ),
            );
        }

        ui.add_space(6.0);

        // --- Codec preference ----------------------------------------------
        ui.horizontal(|ui| {
            ui.label("Codec:");
            let mut pref = self.cfg.codec_pref;
            egui::ComboBox::from_id_salt("codec_pref")
                .selected_text(codec_pref_label(pref))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut pref, CodecPref::Auto, codec_pref_label(CodecPref::Auto));
                    ui.selectable_value(
                        &mut pref,
                        CodecPref::H264Preferred,
                        codec_pref_label(CodecPref::H264Preferred),
                    );
                    ui.selectable_value(
                        &mut pref,
                        CodecPref::Vp8Only,
                        codec_pref_label(CodecPref::Vp8Only),
                    );
                    ui.selectable_value(
                        &mut pref,
                        CodecPref::H264Only,
                        codec_pref_label(CodecPref::H264Only),
                    );
                });
            self.cfg.codec_pref = pref;
        });
        ui.weak(
            "VP8 decodes on every browser; H.264 hardware-decodes well on \
             Chrome/Safari/Android but Firefox caps it at ~720p.",
        );

        ui.add_space(6.0);

        // --- Regenerate password -------------------------------------------
        ui.horizontal(|ui| {
            if ui.button("Regenerate password").clicked() {
                // Fresh access code; the started host reads it at Start time, so
                // this only matters pre-launch. URL/QR are unaffected.
                self.cfg.access_code = access_code::generate();
            }
            ui.weak("(makes a new viewer password)");
        });
    }
}

/// Human-readable label for a codec preference, shown in the GUI selector.
fn codec_pref_label(pref: CodecPref) -> &'static str {
    match pref {
        CodecPref::Auto => "VP8 preferred (default)",
        CodecPref::H264Preferred => "H.264 preferred",
        CodecPref::Vp8Only => "VP8 only",
        CodecPref::H264Only => "H.264 only",
    }
}

/// Build the QR matrix for `url`: `(side, dark)` where `dark[y*side+x]` is a black
/// module. `None` if the URL can't be encoded.
fn build_qr(url: &str) -> Option<(usize, Vec<bool>)> {
    use qrcode::types::Color;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes()).ok()?;
    let side = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == Color::Dark)
        .collect();
    Some((side, dark))
}

/// Paint the QR as crisp black/white squares within a `size`×`size` box.
fn draw_qr(ui: &mut egui::Ui, side: usize, dark: &[bool], size: f32) {
    let quiet = 2usize;
    let modules = side + quiet * 2;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let zero = egui::CornerRadius::ZERO;
    painter.rect_filled(rect, zero, egui::Color32::WHITE);
    let cell = size / modules as f32;
    for y in 0..side {
        for x in 0..side {
            if dark[y * side + x] {
                let min = egui::pos2(
                    rect.min.x + (x + quiet) as f32 * cell,
                    rect.min.y + (y + quiet) as f32 * cell,
                );
                let r = egui::Rect::from_min_size(min, egui::vec2(cell, cell));
                painter.rect_filled(r, zero, egui::Color32::BLACK);
            }
        }
    }
}
