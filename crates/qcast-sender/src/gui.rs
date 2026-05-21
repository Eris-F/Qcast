//! The pre-launch GUI. Shows the preflight checklist + the viewer URL/QR, lets
//! the operator confirm, then starts the host and **hides the window** so Qcast
//! keeps streaming as a background process (no taskbar entry). A global hotkey
//! (Ctrl+Alt+Q) stops it cleanly; SIGTERM/Ctrl+C is the universal fallback (the
//! hotkey can't work under Wayland's security model).

use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::host::{self, HostConfig, RunningHost};
use crate::preflight::{self, Report};

/// Shared so the SIGTERM/Ctrl+C handler (which has no `&App`) can wake and close
/// the event loop from its own thread.
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// Run the GUI. Returns when the window closes / the host is stopped.
pub fn run(cfg: HostConfig) -> anyhow::Result<()> {
    let report = preflight::run(&cfg.host, cfg.web_port);

    // Universal stop path: a kill/SIGTERM (or Ctrl+C) flips this flag and wakes
    // the event loop, which then tears the host down cleanly.
    let quit = Arc::new(AtomicBool::new(false));
    {
        let q = quit.clone();
        let _ = ctrlc::set_handler(move || {
            q.store(true, Ordering::SeqCst);
            if let Some(ctx) = EGUI_CTX.get() {
                ctx.request_repaint();
            }
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 640.0])
            .with_title("Qcast")
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "Qcast",
        options,
        Box::new(move |cc| {
            let _ = EGUI_CTX.set(cc.egui_ctx.clone());
            Ok(Box::new(App::new(cfg, report, quit)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI failed: {e}"))
}

enum Stage {
    /// Showing the checklist; waiting for the operator to confirm.
    Preflight,
    /// Host is starting (capture handshake / portal dialog in flight).
    Starting,
    /// Start failed; show the error with retry/quit.
    Error(String),
    /// Streaming; window hidden, running in the background.
    Background,
}

struct App {
    cfg: HostConfig,
    report: Report,
    stage: Stage,
    quit: Arc<AtomicBool>,
    host: Option<RunningHost>,
    /// Receiver for the async start result (start blocks on the portal dialog).
    start_rx: Option<mpsc::Receiver<Result<RunningHost, String>>>,
    /// QR of the viewer URL: (side length in modules, dark-module bitmap).
    qr: Option<(usize, Vec<bool>)>,
    /// Kept alive so the hotkey registration stays active; `None` if unsupported.
    _hotkey_mgr: Option<GlobalHotKeyManager>,
    hotkey_id: Option<u32>,
    closing: bool,
}

impl App {
    fn new(cfg: HostConfig, report: Report, quit: Arc<AtomicBool>) -> Self {
        let qr = build_qr(&report.url);
        let (mgr, hotkey_id) = register_quit_hotkey();
        Self {
            cfg,
            report,
            stage: Stage::Preflight,
            quit,
            host: None,
            start_rx: None,
            qr,
            _hotkey_mgr: mgr,
            hotkey_id,
            closing: false,
        }
    }

    /// Kick off `host::start` on a worker thread so the UI stays responsive while
    /// the capture handshake (and the portal picker dialog) happens.
    fn begin_start(&mut self) {
        let cfg = self.cfg.clone();
        let (tx, rx) = mpsc::channel();
        self.start_rx = Some(rx);
        self.stage = Stage::Starting;
        std::thread::spawn(move || {
            let result = host::start(cfg).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
    }

    /// Stop the host (if any) and close the window/event loop.
    fn begin_quit(&mut self, ctx: &egui::Context) {
        if self.closing {
            return;
        }
        self.closing = true;
        if let Some(mut h) = self.host.take() {
            h.stop();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn stage_tag(&self) -> StageTag {
        match self.stage {
            Stage::Preflight => StageTag::Preflight,
            Stage::Starting => StageTag::Starting,
            Stage::Error(_) => StageTag::Error,
            Stage::Background => StageTag::Background,
        }
    }

    /// Poll the global hotkey + the kill-signal flag; trigger quit on either.
    fn poll_quit_signals(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.hotkey_id {
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.id == id && ev.state == HotKeyState::Pressed {
                    self.begin_quit(ctx);
                }
            }
        }
        if self.quit.load(Ordering::SeqCst) {
            self.begin_quit(ctx);
        }
    }
}

/// Lightweight discriminant so we can decide the layout without holding an
/// immutable borrow of `self.stage` across the (mutating) render arms.
enum StageTag {
    Preflight,
    Starting,
    Error,
    Background,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Heartbeat so we keep polling the hotkey + signal flag even while hidden.
        ctx.request_repaint_after(Duration::from_millis(200));
        self.poll_quit_signals(&ctx);

        // Pick up the async start result.
        if let Some(rx) = &self.start_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(h) => {
                        self.host = Some(h);
                        self.stage = Stage::Background;
                        // Vanish from the screen/taskbar; keep streaming.
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                    Err(e) => self.stage = Stage::Error(e),
                }
                self.start_rx = None;
            }
        }

        match self.stage_tag() {
            StageTag::Preflight => self.ui_preflight(ui),
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
                        self.begin_quit(&ctx);
                    }
                });
            }
            StageTag::Background => {
                // Window is hidden; this only renders if something un-hides it.
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.heading("Qcast is running in the background");
                    ui.add_space(8.0);
                    ui.label("Press Ctrl+Alt+Q to stop (or kill the process).");
                });
            }
        }
    }

    fn on_exit(&mut self) {
        if let Some(mut h) = self.host.take() {
            h.stop();
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

        let ready = self.report.ready();
        if ready {
            ui.strong("Viewers connect at:");
            ui.horizontal(|ui| {
                ui.monospace(&self.report.url);
                if ui.small_button("copy").clicked() {
                    ui.ctx().copy_text(self.report.url.clone());
                }
            });
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
            let btn = egui::Button::new(
                egui::RichText::new("Start & run in background").size(16.0),
            )
            .min_size(egui::vec2(280.0, 38.0));
            if ui.add_enabled(ready, btn).clicked() {
                self.begin_start();
            }
            ui.add_space(6.0);
            ui.weak("This window disappears; Qcast keeps streaming. Ctrl+Alt+Q (or kill) stops it.");
        });
    }
}

/// Build the QR matrix for `url`: returns `(side, dark)` where `dark[y*side+x]`
/// is true for a black module. `None` if the URL can't be encoded.
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
    let quiet = 2usize; // quiet-zone modules on each edge
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

/// Register Ctrl+Alt+Q as the global quit hotkey. Returns `(manager, id)`; the
/// manager must be kept alive. Both are `None` where the platform can't grab a
/// global hotkey (e.g. Wayland) — the SIGTERM/Ctrl+C path covers those.
fn register_quit_hotkey() -> (Option<GlobalHotKeyManager>, Option<u32>) {
    let mgr = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "global hotkey unavailable; use Ctrl+C / kill to stop");
            return (None, None);
        }
    };
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyQ);
    let id = hotkey.id();
    match mgr.register(hotkey) {
        Ok(()) => {
            tracing::info!("press Ctrl+Alt+Q to stop the background host");
            (Some(mgr), Some(id))
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not register Ctrl+Alt+Q; use Ctrl+C / kill to stop");
            (Some(mgr), None)
        }
    }
}
