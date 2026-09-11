#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod claude;
mod codex;
mod icons;
mod model;
mod poller;
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

    // Starts collapsed. The window is resized only when the hover state
    // settles, never during the animation.
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([ui::RAIL_W, ui::WINDOW_H])
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

struct Dock {
    snapshot: Arc<Mutex<Snapshot>>,
    control: Arc<poller::Control>,
    settings: Settings,
    icons: icons::Icons,
    win: Option<winshape::Window>,
    tray: Option<tray::Tray>,
    tooltip: Option<String>,
    width: f32,
    visible: bool,
    placed: bool,
    frames: u32,
    expanded: bool,
    dragging: bool,
    resizing: bool,
    pending_resize: bool,
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

        Self {
            snapshot,
            control,
            settings,
            icons: icons::Icons::load(&cc.egui_ctx),
            win: None,
            tray: tray::Tray::new(),
            tooltip: None,
            width: ui::RAIL_W,
            visible: true,
            placed: false,
            frames: 0,
            expanded: false,
            dragging: false,
            resizing: false,
            pending_resize: true,
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
        egui::vec2(self.width, ui::WINDOW_H)
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

    fn resize_to(&mut self, ctx: &egui::Context, width: f32) {
        if (width - self.width).abs() < 0.5 && !self.pending_resize {
            return;
        }
        self.pending_resize = false;
        self.width = width;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            width,
            ui::WINDOW_H,
        )));
        // Docked right, a wider window has to start further left to keep its
        // right edge pinned to the screen.
        if let Some(monitor) = Self::monitor(ctx) {
            self.reposition(ctx, monitor);
        }
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

    fn monitor(ctx: &egui::Context) -> Option<egui::Vec2> {
        ctx.input(|i| i.viewport().monitor_size)
            .filter(|m| m.x > 0.0 && m.y > 0.0)
    }

    /// x such that the card's docked side sits flush against the screen edge.
    /// The window overhangs by the shadow margin, which is clipped off-screen.
    fn docked_x(edge: DockEdge, monitor_w: f32, width: f32) -> f32 {
        match edge {
            DockEdge::Right => monitor_w - width + ui::shadow_margin(),
            DockEdge::Left => -ui::shadow_margin(),
        }
    }

    fn reposition(&self, ctx: &egui::Context, monitor: egui::Vec2) {
        let x = Self::docked_x(self.settings.edge, monitor.x, self.window_size().x);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            x,
            self.settings.top,
        )));
    }

    fn place(&mut self, ctx: &egui::Context) {
        if self.placed {
            return;
        }
        let Some(monitor) = Self::monitor(ctx) else {
            return;
        };
        self.settings.top = self
            .settings
            .top
            .clamp(0.0, (monitor.y - self.window_size().y).max(0.0));
        self.reposition(ctx, monitor);
        self.placed = true;
    }

    /// After a drag, attach to whichever edge the dock was released nearest and
    /// remember it, so it comes back attached next launch.
    fn snap_after_drag(&mut self, ctx: &egui::Context) {
        let Some(monitor) = Self::monitor(ctx) else {
            return;
        };
        let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else {
            return;
        };

        let w = self.window_size();
        self.settings.edge = DockEdge::nearest(outer.min.x, w.x, monitor.x);
        self.settings.top = outer.min.y.clamp(0.0, (monitor.y - w.y).max(0.0));
        self.reposition(ctx, monitor);
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
            self.expanded,
            self.settings.edge,
        );

        // Clip the window to the card, so the fixed expanded footprint does not
        // swallow clicks over the transparent gap. Inflated by the shadow
        // margin because a window region clips painting as well as input, and
        // a tight clip would shear the card's shadow off.
        let ppp = ctx.pixels_per_point();
        let origin = ui.max_rect().min;

        // Resize the real window rather than clipping it with a region: a
        // window region makes Windows paint its frame along the region edge,
        // which showed as a white bar above the card and flashed down the side
        // as the panel opened. This changes size twice per hover cycle, never
        // mid-animation, so the GL surface is not churned.
        self.resize_to(&ctx, frame_info.wanted_width);

        // Hover comes from the real cursor, not egui's enter/leave events, and
        // is judged against the painted card so the transparent gap never
        // triggers the panel. Instant in both directions.
        // Not while dragging or resizing: a resize drag pulls the pointer off
        // the card by design, and re-deriving hover from that would collapse
        // the dock out from under the drag.
        if !self.dragging && !self.resizing {
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

        let response = ui.interact(
            frame_info.card,
            egui::Id::new("dock-root"),
            egui::Sense::click_and_drag(),
        );

        // A drag that starts on the inner edge resizes; anywhere else moves.
        let on_grip = ctx
            .input(|i| i.pointer.latest_pos())
            .is_some_and(|p| frame_info.resize_handle.contains(p));
        if on_grip || self.resizing {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if response.drag_started() {
            if on_grip {
                self.resizing = true;
            } else {
                self.dragging = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }

        if self.resizing {
            // Dragging away from the screen edge grows the dock.
            let dx = response.drag_delta().x;
            let outward = match self.settings.edge {
                DockEdge::Right => -dx,
                DockEdge::Left => dx,
            };
            if outward != 0.0 {
                self.set_scale(&ctx, self.settings.scale * (1.0 + outward / 220.0));
            }
            if response.drag_stopped() || !response.dragged() {
                self.resizing = false;
                let _ = settings::save(&self.settings);
            }
        } else if self.dragging && !response.dragged() && !response.drag_started() {
            self.dragging = false;
            self.snap_after_drag(&ctx);
        }

        // Wheel over the dock scales too — quicker than finding the grip.
        let wheel = ctx.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 && self.expanded {
            self.set_scale(&ctx, self.settings.scale * (1.0 + wheel / 900.0));
            let _ = settings::save(&self.settings);
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
                if let Some(monitor) = Self::monitor(menu.ctx()) {
                    self.reposition(menu.ctx(), monitor);
                }
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
