//! The pre-launch GUI. Shows the preflight checklist + the viewer URL/QR, lets
//! the operator confirm, then starts the host and **fully closes the window** so
//! Qcast keeps streaming as a pure background process — no taskbar entry (Wayland
//! gives clients no "skip taskbar" control, so hiding only minimizes; closing is
//! the only way to truly leave the taskbar). The running host is handed back to
//! `main`, which keeps the process alive until a hotkey/kill stops it.

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::host::{self, HostConfig, RunningHost};
use crate::preflight::{self, Report};

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

    let mut s = shared.lock().unwrap();
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
}

impl App {
    fn new(
        cfg: HostConfig,
        report: Report,
        shared: Arc<Mutex<Shared>>,
        quit: Arc<AtomicBool>,
    ) -> Self {
        let qr = build_qr(&report.url);
        Self {
            cfg,
            report,
            stage: Stage::Preflight,
            qr,
            shared,
            quit,
            start_rx: None,
        }
    }

    /// Start the host on a worker thread so the UI stays responsive during the
    /// capture handshake (and the portal picker dialog).
    fn begin_start(&mut self) {
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
                            let mut s = self.shared.lock().unwrap();
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
