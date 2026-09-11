#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod codex;
mod model;
mod poller;
mod store;
mod ui;

use model::Snapshot;
use std::sync::{Arc, Mutex};

const EDGE_MARGIN: f32 = 12.0;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--probe") {
        probe();
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([ui::DOCK_WIDTH, ui::DOCK_HEIGHT])
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "usage-tracker",
        options,
        Box::new(|cc| Ok(Box::new(Dock::new(cc)))),
    )
}

/// One-shot diagnostic for the two response shapes the plan could not verify
/// up front. Prints the raw body so the `utilization` scale and `resets_at`
/// type are visible, and never prints the token.
fn probe() {
    println!("--- claude ---");
    match claude::load_credentials() {
        Err(e) => println!("credentials: {e:#}"),
        Ok(creds) => {
            println!("token len: {}", creds.access_token.len());
            println!("scopes: {:?}", creds.scopes);
            println!("can_read_usage: {}", creds.can_read_usage());
            println!("is_expired: {}", creds.is_expired());
            match claude::fetch_usage_body(&creds) {
                Err(e) => println!("fetch: {e}"),
                Ok(body) => {
                    println!("raw body:\n{body}");
                    match claude::parse_usage(&body) {
                        Ok(q) => println!("parsed: {q:?}"),
                        Err(e) => println!("parse failed: {e:#}"),
                    }
                }
            }
        }
    }

    println!("--- codex ---");
    match codex::latest_reading() {
        Err(e) => println!("codex: {e:#}"),
        Ok(None) => println!("codex: no rate_limits in recent rollouts"),
        Ok(Some(r)) => println!("observed_at: {}\nparsed: {:?}", r.observed_at, r.quota),
    }
}

struct Dock {
    snapshot: Arc<Mutex<Snapshot>>,
    control: Arc<poller::Control>,
    placed: bool,
    frames: u32,
    height: f32,
}

impl Dock {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let last_good = store::load();
        let snapshot = Arc::new(Mutex::new(poller::initial_snapshot(&last_good)));
        let control = Arc::new(poller::Control::default());

        poller::spawn(snapshot.clone(), control.clone(), cc.egui_ctx.clone());

        Self {
            snapshot,
            control,
            placed: false,
            frames: 0,
            height: ui::DOCK_HEIGHT,
        }
    }

    /// Window/DPI facts, for diagnosing placement and scaling on multi-monitor
    /// setups. Everything egui reports here is in points, not pixels.
    fn diag(&self, ctx: &egui::Context) {
        if std::env::var("DOCK_DIAG").is_err() || self.frames >= 8 {
            return;
        }
        // Read everything out of the input lock first: calling another `ctx`
        // method inside `ctx.input(..)` re-enters the lock and deadlocks.
        let ppp = ctx.pixels_per_point();
        let info = ctx.input(|i| {
            (
                i.viewport().native_pixels_per_point,
                i.raw.screen_rect,
                i.viewport().monitor_size,
                i.viewport().outer_rect,
            )
        });
        eprintln!(
            "f{} placed={} native_ppp={:?} ppp={ppp} screen={:?} monitor={:?} outer={:?}",
            self.frames, self.placed, info.0, info.1, info.2, info.3
        );
    }

    /// Park against the top-right of the monitor on the first frame, once the
    /// monitor size is actually known. Anchoring the top edge means the dock
    /// stays put as its height changes.
    fn place(&mut self, ctx: &egui::Context) {
        if self.placed {
            return;
        }

        let monitor = ctx.input(|i| i.viewport().monitor_size);
        let Some(monitor) = monitor else {
            return;
        };
        if monitor.x <= 0.0 {
            return;
        }
        let pos = egui::pos2(monitor.x - ui::DOCK_WIDTH - EDGE_MARGIN, EDGE_MARGIN);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        self.placed = true;
    }
}

impl eframe::App for Dock {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.diag(&ctx);
        self.place(&ctx);
        self.frames += 1;

        let snapshot = self.snapshot.lock().unwrap().clone();
        let needed = ui::draw(ui, &snapshot);

        // Resize to fit, but only on a real change: sending InnerSize every
        // frame fights the compositor and makes the dock jitter.
        if (needed - self.height).abs() > 0.5 {
            self.height = needed;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                ui::DOCK_WIDTH,
                needed,
            )));
        }

        let response = ui.interact(
            ui.max_rect(),
            egui::Id::new("dock-root"),
            egui::Sense::click_and_drag(),
        );

        if response.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        response.context_menu(|menu| {
            if menu.button("Refresh now").clicked() {
                self.control.request_refresh();
                menu.close();
            }
            if menu.button("Quit").clicked() {
                menu.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        // Keep countdowns ticking without spinning the CPU.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}
