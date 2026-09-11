#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod codex;
mod icons;
mod model;
mod poller;
mod providers;
mod settings;
mod store;
mod tray;
mod ui;
mod winshape;

use model::Snapshot;
use settings::{DockEdge, Settings};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--probe") {
        probe();
        return Ok(());
    }
    // Mints the .ico that the executable and shortcuts carry, from the same
    // SVG the window and tray use, so they cannot drift apart.
    if let Some(i) = args.iter().position(|a| a == "--emit-icon") {
        let path = args
            .get(i + 1)
            .cloned()
            .unwrap_or_else(|| "icon.png".into());
        let size = args
            .get(i + 2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(256);
        match icons::encode_png(icons::APP_ICON_SVG, size) {
            Some(png) => {
                std::fs::write(&path, png).expect("write icon");
                println!("wrote {path} at {size}px");
            }
            None => eprintln!("could not rasterise the app icon"),
        }
        return Ok(());
    }

    // Sized for the expanded panel and never resized while hovering: only the
    // painted card animates, so the GL surface is left alone.
    let initial = settings::load();
    let rows = [providers::ProviderId::Claude, providers::ProviderId::Codex]
        .into_iter()
        .filter(|p| initial.is_enabled(*p))
        .count();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([ui::PANEL_W, ui::window_height(rows)])
        .with_decorations(false)
        .with_transparent(true)
        .with_has_shadow(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_resizable(false);

    if let Some(rgba) = icons::rasterise_rgba(icons::APP_ICON_SVG, 64) {
        viewport = viewport.with_icon(egui::IconData {
            rgba,
            width: 64,
            height: 64,
        });
    }

    let options = eframe::NativeOptions {
        viewport,
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

/// Providers the dock can actually draw: enabled, and with a quota reader.
fn shown_providers(settings: &Settings) -> Vec<ui::Provider> {
    [
        (providers::ProviderId::Claude, ui::Provider::Claude),
        (providers::ProviderId::Codex, ui::Provider::Codex),
    ]
    .into_iter()
    .filter(|(id, _)| settings.is_enabled(*id))
    .map(|(_, p)| p)
    .collect()
}

struct Dock {
    snapshot: Arc<Mutex<Snapshot>>,
    control: Arc<poller::Control>,
    settings: Settings,
    icons: icons::Icons,
    win: Option<winshape::Window>,
    tray: Option<tray::Tray>,
    tooltip: Option<String>,
    shown: Vec<ui::Provider>,
    zoom_keys: winshape::ZoomKeys,
    width: f32,
    visible: bool,
    placed: bool,
    frames: u32,
    expanded: bool,
    dragging: bool,
    pending_resize: bool,
    pending_reposition: bool,
    last_monitor: Option<egui::Vec2>,
}

impl Dock {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let last_good = store::load();
        let snapshot = Arc::new(Mutex::new(poller::initial_snapshot(&last_good)));
        let control = Arc::new(poller::Control::default());

        poller::spawn(snapshot.clone(), control.clone(), cc.egui_ctx.clone());

        let mut settings = settings::load();
        // Testing hook: lets the scaling path be exercised and measured
        // without driving the pointer.
        if let Some(s) = std::env::var("DOCK_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            settings.scale = s;
            settings = settings.sanitised();
        }
        cc.egui_ctx.set_zoom_factor(settings.scale);

        let installed = providers::installed();
        let shown = shown_providers(&settings);
        control.set_enabled_from(&settings);
        let tray = tray::Tray::new(&settings, &installed);

        Self {
            snapshot,
            control,
            settings,
            icons: icons::Icons::load(&cc.egui_ctx),
            win: None,
            tray,
            tooltip: None,
            shown,
            zoom_keys: winshape::ZoomKeys::default(),
            width: ui::PANEL_W,
            visible: true,
            placed: false,
            frames: 0,
            expanded: false,
            dragging: false,
            pending_resize: true,
            pending_reposition: false,
            last_monitor: None,
        }
    }

    fn update_tooltip(&mut self, snapshot: &Snapshot) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        let line = |name: &str, reading: Option<&model::Reading>| {
            let Some(r) = reading else {
                return format!("{name}: —");
            };
            let stamp = r
                .observed_at()
                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_else(|| "—".into());
            match r.quota() {
                None => format!("{name}: no reading"),
                Some(q) => {
                    let pct = |w: Option<&model::Window>| {
                        w.map(|w| w.percent_label()).unwrap_or_else(|| "—".into())
                    };
                    format!(
                        "{name}: 5h {}  7d {}  (as of {stamp})",
                        pct(q.session.as_ref()),
                        pct(q.weekly.as_ref())
                    )
                }
            }
        };

        let body = format!(
            "{}\n{}",
            line("Claude", snapshot.claude.as_ref()),
            line("Codex", snapshot.codex.as_ref())
        );
        if self.tooltip.as_deref() != Some(body.as_str()) {
            tray.set_tooltip(&body);
            self.tooltip = Some(body);
        }
    }

    /// The tray is the only durable handle on a hidden, taskbar-less window.
    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(action) = self.tray.as_ref().and_then(tray::Tray::poll) else {
            return;
        };
        match action {
            tray::Action::Quit => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            tray::Action::Toggle => self.visible = !self.visible,
            tray::Action::Show => self.visible = true,
            tray::Action::Hide => self.visible = false,
            tray::Action::SetProvider(id, on) => {
                self.settings.set_enabled(id, on);
                let _ = settings::save(&self.settings);
                self.shown = shown_providers(&self.settings);
                self.control.set_enabled_from(&self.settings);
                // The card grows or shrinks by a row, so the window has to
                // follow and be re-pinned to its edge.
                self.pending_resize = true;
                if let Some(tray) = self.tray.as_ref() {
                    tray.sync_provider_checks(&self.settings);
                }
                return;
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(self.visible));
        if self.visible {
            // Re-assert topmost: a hidden window can lose its z-order.
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            self.placed = false;
        }
    }

    fn window_size(&self) -> egui::Vec2 {
        egui::vec2(self.width, ui::window_height(self.shown.len()))
    }

    /// Scaling rides on egui's zoom factor, so the layout stays in points and
    /// every element scales together. The window is re-sent at the same point
    /// size afterwards; since points-per-pixel changed, that lands as a
    /// physically larger or smaller window with identical proportions.
    fn set_scale(&mut self, ctx: &egui::Context, scale: f32) {
        let scale = scale.clamp(settings::MIN_SCALE, settings::MAX_SCALE);
        if (scale - self.settings.scale).abs() < 0.001 {
            return;
        }
        self.settings.scale = scale;
        ctx.set_zoom_factor(scale);
        self.pending_resize = true;
    }

    /// Applies the window size. Only runs when something other than hovering
    /// changed it — the provider count or the scale — never during the hover
    /// animation, since resizing recreates the GL surface and stutters.
    fn apply_size(&mut self, ctx: &egui::Context) {
        if !self.pending_resize {
            return;
        }
        self.pending_resize = false;
        self.width = ui::PANEL_W;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            self.width,
            ui::window_height(self.shown.len()),
        )));
        // Docked right, a wider window has to start further left to keep its
        // right edge pinned to the screen.
        self.reposition(ctx);
        // A resize is exactly when the stale DWM border segment reappeared.
        if let Some(win) = self.win.as_ref() {
            win.refresh_border_suppression();
        }
    }

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
            "f{} placed={} expanded={} native_ppp={:?} ppp={ppp} screen={:?} monitor={:?} outer={:?}",
            self.frames, self.placed, self.expanded, info.0, info.1, info.2, info.3
        );
    }

    /// Monitor size in the same points the viewport commands use, cached.
    ///
    /// Two traps here, both of which made the dock disappear:
    ///
    /// 1. `monitor_size` is reported in *native* points — it ignores egui's
    ///    zoom — while `OuterPosition` and `InnerSize` are converted with
    ///    zoom-inclusive `pixels_per_point`. Mixing them put the window at
    ///    x=8019 on a 5120px screen at 1.6x. Dividing by the zoom factor puts
    ///    both in the same space.
    /// 2. It is not populated every frame, and a miss used to make
    ///    `reposition` silently do nothing, leaving the window sized for one
    ///    state but positioned for another.
    fn monitor(&mut self, ctx: &egui::Context) -> Option<egui::Vec2> {
        if let Some(m) = ctx
            .input(|i| i.viewport().monitor_size)
            .filter(|m| m.x > 0.0 && m.y > 0.0)
        {
            // Cached raw, converted on read: a cached converted value would go
            // stale the moment the zoom changed.
            self.last_monitor = Some(m);
        }
        self.last_monitor.map(|m| m / ctx.zoom_factor().max(0.01))
    }

    /// x such that the card's docked side sits flush against the screen edge.
    /// The window overhangs by the shadow margin, which is clipped off-screen.
    fn docked_x(edge: DockEdge, monitor_w: f32, width: f32) -> f32 {
        match edge {
            DockEdge::Right => monitor_w - width + ui::shadow_margin(),
            DockEdge::Left => -ui::shadow_margin(),
        }
    }

    /// Re-pins the window to its docked edge. Retries on the next frame if the
    /// monitor size is not known yet, so the window is never left sized for one
    /// state but positioned for another.
    fn reposition(&mut self, ctx: &egui::Context) {
        let Some(monitor) = self.monitor(ctx) else {
            self.pending_reposition = true;
            return;
        };
        self.pending_reposition = false;
        let x = Self::docked_x(self.settings.edge, monitor.x, self.window_size().x);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            x,
            self.settings.top,
        )));
    }

    fn place(&mut self, ctx: &egui::Context) {
        if self.placed && !self.pending_reposition {
            return;
        }
        let Some(monitor) = self.monitor(ctx) else {
            return;
        };
        self.settings.top = self
            .settings
            .top
            .clamp(0.0, (monitor.y - self.window_size().y).max(0.0));
        self.reposition(ctx);
        self.placed = true;
    }

    /// After a drag, attach to whichever edge the dock was released nearest and
    /// remember it, so it comes back attached next launch.
    fn snap_after_drag(&mut self, ctx: &egui::Context) {
        let Some(monitor) = self.monitor(ctx) else {
            return;
        };
        let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else {
            return;
        };

        let w = self.window_size();
        self.settings.edge = DockEdge::nearest(outer.min.x, w.x, monitor.x);
        self.settings.top = outer.min.y.clamp(0.0, (monitor.y - w.y).max(0.0));
        self.reposition(ctx);
        let _ = settings::save(&self.settings);
    }
}

impl eframe::App for Dock {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.win.is_none() {
            self.win = Some(winshape::Window::new(frame));
        }
        // winit re-applies window styles on resize, so re-assert them rather
        // than stripping once at startup.
        if let Some(win) = self.win.as_ref() {
            win.keep_frameless();
        }
        self.diag(&ctx);
        self.handle_tray(&ctx);
        self.place(&ctx);
        self.frames += 1;

        // Tray clicks arrive outside egui's input, so keep a slow heartbeat
        // even when nothing else asks for a repaint.
        ctx.request_repaint_after(Duration::from_millis(120));

        let snapshot = self.snapshot.lock().unwrap().clone();
        self.update_tooltip(&snapshot);
        let frame_info = ui::draw(
            ui,
            &self.icons,
            &snapshot,
            &self.shown,
            self.expanded,
            self.settings.edge,
        );

        self.apply_size(&ctx);

        let ppp = ctx.pixels_per_point();
        let origin = ui.max_rect().min;

        // Mouse input is restricted to the painted card. The window itself is
        // never resized while hovering — that recreates the GL surface and made
        // the animation stutter — so the rest of it is transparent and must not
        // swallow clicks meant for what is behind.
        let local = frame_info.card.translate(-origin.to_vec2());
        winshape::set_hit_rect(
            (local.min.x * ppp).floor() as i32,
            (local.min.y * ppp).floor() as i32,
            (local.max.x * ppp).ceil() as i32,
            (local.max.y * ppp).ceil() as i32,
        );

        // Hover comes from the real cursor, not egui's enter/leave events.
        // Instant in both directions.
        if !self.dragging {
            let inside = self
                .win
                .as_ref()
                .and_then(winshape::Window::cursor_inside)
                .is_some_and(|(x, y)| {
                    let p = origin + egui::vec2(x / ppp, y / ppp);
                    frame_info.card.contains(p)
                });
            if inside != self.expanded {
                if std::env::var("DOCK_DIAG").is_ok() {
                    eprintln!("[hover] {}", if inside { "enter" } else { "leave" });
                }
                self.expanded = inside;
            }
        }

        if frame_info.animating {
            ctx.request_repaint();
        }

        // Ctrl +/- resizes while the pointer is over the dock. Read from the
        // keyboard directly, since the dock never takes focus; gating on hover
        // keeps the shortcut working normally everywhere else.
        if self.expanded {
            if let Some(key) = self.zoom_keys.poll() {
                let next = match key {
                    winshape::ZoomKey::Bigger => self.settings.scale + 0.1,
                    winshape::ZoomKey::Smaller => self.settings.scale - 0.1,
                    winshape::ZoomKey::Reset => 1.0,
                };
                self.set_scale(&ctx, next);
                let _ = settings::save(&self.settings);
            }
        } else {
            self.zoom_keys.poll();
        }

        let response = ui.interact(
            frame_info.card,
            egui::Id::new("dock-root"),
            egui::Sense::click_and_drag(),
        );

        if response.drag_started() {
            self.dragging = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if self.dragging && !response.dragged() && !response.drag_started() {
            self.dragging = false;
            self.snap_after_drag(&ctx);
        }

        response.context_menu(|menu| {
            if menu.button("Refresh now").clicked() {
                self.control.request_refresh();
                menu.close();
            }
            let other = match self.settings.edge {
                DockEdge::Right => "Attach to left edge",
                DockEdge::Left => "Attach to right edge",
            };
            if menu.button(other).clicked() {
                self.settings.edge = match self.settings.edge {
                    DockEdge::Right => DockEdge::Left,
                    DockEdge::Left => DockEdge::Right,
                };
                let _ = settings::save(&self.settings);
                self.reposition(menu.ctx());
                menu.close();
            }
            if menu.button("Quit").clicked() {
                menu.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        // Keep countdowns ticking without spinning the CPU.
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}
