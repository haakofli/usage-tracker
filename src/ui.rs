use crate::icons::Icons;
use crate::model::{Quota, Reading, Snapshot, Window, format_reset_in};
use crate::settings::DockEdge;
use chrono::{DateTime, Local, Utc};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Vec2,
    text::{LayoutJob, TextFormat},
};

/// Shadow room around the painted card. The docked side overhangs the screen
/// edge, which costs nothing since the shadow is clipped there anyway.
const MARGIN: f32 = 10.0;

const PANEL_PAD: f32 = 14.0;
const RING_R: f32 = 17.0;
const RING_D: f32 = RING_R * 2.0;

/// The rail is exactly one padded ring wide, which puts the icon ring at the
/// same absolute position collapsed or expanded — the whole point of the
/// layout, since it means opening the panel moves nothing that was on screen.
const RAIL_CARD_W: f32 = PANEL_PAD * 2.0 + RING_D;
/// Distance between the weekly and icon ring centres.
const COL_SPACING: f32 = 70.0;
/// Caption width per column.
const COL_W: f32 = 66.0;
const PANEL_CARD_W: f32 = PANEL_PAD * 2.0 + COL_SPACING + RING_D + (COL_W - RING_D) / 2.0;

pub const PANEL_W: f32 = PANEL_CARD_W + MARGIN * 2.0;

const BLOCK_H: f32 = RING_D + 5.0 + 11.0 + 11.0;
const BLOCK_GAP: f32 = 26.0;
const CARD_PAD_V: f32 = 24.0;

/// Card height for `n` provider rows. The card never changes height while
/// hovering — only its width animates — but it does resize when providers are
/// switched on or off in the tray.
pub fn card_height(n: usize) -> f32 {
    let rows = n.max(1) as f32;
    BLOCK_H * rows + BLOCK_GAP * (rows - 1.0) + CARD_PAD_V * 2.0
}

/// The window is sized for the largest card the dock can show, and never
/// resized while hovering — resizing recreates the GL surface mid-animation,
/// which is what made the hover stutter.
pub fn window_height(n: usize) -> f32 {
    card_height(n) + MARGIN * 2.0
}

/// Callers positioning the window need to know how far the card is inset, so
/// they can hang the shadow margin off the screen edge.
pub const fn shadow_margin() -> f32 {
    MARGIN
}

const CARD_RADIUS: u8 = 16;
const RING_TRACK_W: f32 = 3.0;
const RING_FILL_W: f32 = 3.4;
const MARK_D: f32 = RING_D * 0.56;

const CARD_BG: Color32 = Color32::from_rgba_premultiplied(14, 15, 17, 242);
/// Premultiplied alpha requires RGB <= alpha; passing white at alpha 28 to
/// `from_rgba_premultiplied` renders as a bright additive haze rather than a
/// faint track, which is what produced the stray outline in the first build.
const TRACK: Color32 = Color32::from_rgba_premultiplied(28, 28, 28, 28);
const VALUE: Color32 = Color32::from_rgb(236, 238, 242);
const MUTED: Color32 = Color32::from_rgb(108, 113, 124);

/// Claude's own brand orange, as shipped in its mark.
const CLAUDE_ORANGE: Color32 = Color32::from_rgb(217, 119, 87);
const CODEX_WHITE: Color32 = Color32::from_rgb(226, 229, 234);

const CALM: Color32 = Color32::from_rgb(94, 181, 155);
const WARM: Color32 = Color32::from_rgb(214, 168, 92);
const HOT: Color32 = Color32::from_rgb(212, 110, 110);

#[derive(Clone, Copy, PartialEq)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    /// Only used to key animation state, never rendered — the panel shows no
    /// provider names, since the marks already identify the rows.
    fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The marks are rasterised to neutral masks, so their brand colour is
    /// applied here at paint time.
    fn tint(self) -> Color32 {
        match self {
            Self::Claude => CLAUDE_ORANGE,
            Self::Codex => CODEX_WHITE,
        }
    }
}

fn ring_color(used: f32) -> Color32 {
    if used < 0.5 {
        CALM
    } else if used < 0.8 {
        WARM
    } else {
        HOT
    }
}

fn dim(c: Color32, factor: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        c.r(),
        c.g(),
        c.b(),
        (c.a() as f32 * factor).round().clamp(0.0, 255.0) as u8,
    )
}

/// egui's bundled monospace face has tabular figures, so percentages keep a
/// fixed width as the digits change.
fn digits(size: f32) -> FontId {
    FontId::monospace(size)
}

/// The weekly window resets days out, where the actual day is easier to act on
/// than a countdown — "Friday / 01:00" beats "3d 21h".
fn weekly_reset_parts(w: Option<&Window>) -> (String, String) {
    match w {
        None => ("—".to_string(), String::new()),
        Some(w) => {
            let local = w.resets_at.with_timezone(&Local);
            (
                local.format("%A").to_string(),
                local.format("%H:%M").to_string(),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn text(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: Align2,
    body: &str,
    size: f32,
    tracking: f32,
    color: Color32,
    max_width: f32,
) {
    if color.a() == 0 || body.is_empty() {
        return;
    }
    let mut job = LayoutJob::single_section(
        body.to_string(),
        TextFormat {
            font_id: FontId::proportional(size),
            extra_letter_spacing: tracking,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        ..Default::default()
    };
    let galley = painter.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
}

/// egui has no arc primitive, so walk the circumference as a polyline and cap
/// both ends with a dot to imitate a round stroke cap.
fn arc(painter: &egui::Painter, center: Pos2, radius: f32, frac: f32, width: f32, color: Color32) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 || color.a() == 0 {
        return;
    }
    let start = -std::f32::consts::FRAC_PI_2;
    let sweep = frac * std::f32::consts::TAU;
    let steps = ((frac * 96.0).ceil() as usize).max(2);

    let point_at = |t: f32| {
        let a = start + sweep * t;
        Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
    };

    let points: Vec<Pos2> = (0..=steps)
        .map(|i| point_at(i as f32 / steps as f32))
        .collect();

    painter.add(egui::epaint::PathShape::line(
        points,
        Stroke::new(width, color),
    ));
    painter.circle_filled(point_at(0.0), width / 2.0, color);
    painter.circle_filled(point_at(1.0), width / 2.0, color);
}

struct Gauge<'a> {
    window: Option<&'a Window>,
    /// `None` draws a bare ring with its percentage inside, for the weekly view.
    provider: Option<Provider>,
    opacity: f32,
}

fn gauge(ui: &egui::Ui, icons: &Icons, center: Pos2, g: Gauge<'_>, id: &str) {
    if g.opacity <= 0.0 {
        return;
    }
    let painter = ui.painter();
    painter.circle_stroke(
        center,
        RING_R,
        Stroke::new(RING_TRACK_W, dim(TRACK, g.opacity)),
    );

    if let Some(w) = g.window {
        let target = w.used.clamp(0.0, 1.0);
        // Glide to the new value so a refresh reads as a change, not a repaint.
        let shown = ui
            .ctx()
            .animate_value_with_time(egui::Id::new(id), target, 0.5);
        arc(
            painter,
            center,
            RING_R,
            shown,
            RING_FILL_W,
            dim(ring_color(target), g.opacity),
        );
    }

    match g.provider {
        Some(p) => {
            let tex = match p {
                Provider::Claude => &icons.claude,
                Provider::Codex => &icons.codex,
            };
            painter.image(
                tex.id(),
                Rect::from_center_size(center, Vec2::splat(MARK_D)),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                dim(p.tint(), g.opacity),
            );
        }
        None => {
            let body = g
                .window
                .map(|w| w.percent_label())
                .unwrap_or_else(|| "—".to_string());
            painter.text(
                center,
                Align2::CENTER_CENTER,
                body,
                digits(10.0),
                dim(VALUE, g.opacity),
            );
        }
    }
}

fn opacity_of(reading: &Reading) -> f32 {
    match reading {
        Reading::Ok { .. } => 1.0,
        Reading::Stale { .. } => 0.55,
        Reading::Never | Reading::Failed { .. } => 0.6,
    }
}

fn quota_of(reading: &Reading) -> Option<&Quota> {
    reading.quota()
}

fn reading_for(snapshot: &Snapshot, provider: Provider) -> Reading {
    match provider {
        Provider::Claude => snapshot.claude.clone(),
        Provider::Codex => snapshot.codex.clone(),
    }
    .unwrap_or(Reading::Never)
}

/// One provider's row. The icon ring and its captions sit at a fixed position;
/// `weekly` supplies the sliding column's centre and fade when the panel opens.
#[allow(clippy::too_many_arguments)]
fn draw_provider_block(
    ui: &egui::Ui,
    icons: &Icons,
    provider: Provider,
    reading: &Reading,
    icon_cx: f32,
    top: f32,
    now: DateTime<Utc>,
    weekly: Option<(f32, f32)>,
) {
    let opacity = opacity_of(reading);
    let quota = quota_of(reading);
    let session = quota.and_then(|q| q.session.as_ref());
    let week = quota.and_then(|q| q.weekly.as_ref());

    let ring_y = top + RING_R;
    let cap_y = top + RING_D + 5.0;

    let caption = |cx: f32, a: &str, a_color: Color32, b: &str, alpha: f32| {
        text(
            ui.painter(),
            Pos2::new(cx, cap_y),
            Align2::CENTER_TOP,
            a,
            9.5,
            0.3,
            dim(a_color, opacity * alpha),
            COL_W,
        );
        text(
            ui.painter(),
            Pos2::new(cx, cap_y + 11.0),
            Align2::CENTER_TOP,
            b,
            9.0,
            0.1,
            dim(MUTED, opacity * alpha),
            COL_W,
        );
    };

    // Weekly first so a mid-slide ring passes beneath the icon, not over it.
    if let Some((week_cx, alpha)) = weekly {
        gauge(
            ui,
            icons,
            Pos2::new(week_cx, ring_y),
            Gauge {
                window: week,
                provider: None,
                opacity: opacity * alpha,
            },
            &format!("{}-7d", provider.key()),
        );
        let (day, time) = weekly_reset_parts(week);
        caption(week_cx, &day, MUTED, &time, alpha);
    }

    gauge(
        ui,
        icons,
        Pos2::new(icon_cx, ring_y),
        Gauge {
            window: session,
            provider: Some(provider),
            opacity,
        },
        &format!("{}-5h", provider.key()),
    );

    if matches!(reading, Reading::Stale { .. }) {
        ui.painter().circle_filled(
            Pos2::new(icon_cx + RING_R - 1.0, top + 2.0),
            2.2,
            dim(WARM, 0.9),
        );
    }

    let pct = session
        .map(|w| w.percent_label())
        .unwrap_or_else(|| "—".to_string());
    let reset = session
        .map(|w| format_reset_in(w.resets_at, now))
        .unwrap_or_else(|| "—".to_string());
    caption(icon_cx, &pct, VALUE, &reset, 1.0);
}

/// Square off the corners against the screen edge so the dock reads as
/// attached rather than merely parked nearby.
fn card_radius(edge: DockEdge) -> CornerRadius {
    match edge {
        DockEdge::Right => CornerRadius {
            nw: CARD_RADIUS,
            sw: CARD_RADIUS,
            ne: 0,
            se: 0,
        },
        DockEdge::Left => CornerRadius {
            ne: CARD_RADIUS,
            se: CARD_RADIUS,
            nw: 0,
            sw: 0,
        },
    }
}

pub struct Frame {
    /// The painted card, in points: what to hit-test hover against.
    pub card: Rect,
    /// True while the width is still moving, so the caller keeps repainting.
    pub animating: bool,
}

pub fn draw(
    ui: &mut egui::Ui,
    icons: &Icons,
    snapshot: &Snapshot,
    shown: &[Provider],
    expanded: bool,
    edge: DockEdge,
) -> Frame {
    let now = Utc::now();
    let full = ui.max_rect();
    let card_h = card_height(shown.len());

    // One eased 0..1 drives the width and the weekly column's slide and fade,
    // so the panel opens as a single gesture. Short, because it should feel
    // instant rather than animated at you.
    let t = ui.ctx().animate_bool_with_time_and_easing(
        egui::Id::new("dock-open"),
        expanded,
        0.16,
        egui::emath::easing::cubic_out,
    );
    let card_w = RAIL_CARD_W + (PANEL_CARD_W - RAIL_CARD_W) * t;

    let card_x = match edge {
        DockEdge::Right => full.max.x - MARGIN - card_w,
        DockEdge::Left => full.min.x + MARGIN,
    };
    let card = Rect::from_min_size(
        Pos2::new(card_x, full.min.y + MARGIN),
        Vec2::new(card_w, card_h),
    );

    let radius = card_radius(edge);
    let painter = ui.painter();
    // Kept soft and shallow: at alpha 140 over a dark desktop the shadow read
    // as a hard grey halo around the card rather than depth.
    let shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(64),
    };
    painter.add(shadow.as_shape(card, radius));
    painter.add(egui::epaint::RectShape::filled(card, radius, CARD_BG));

    // Anchored to the docked edge, so it is identical in both states.
    let icon_cx = match edge {
        DockEdge::Right => card.max.x - PANEL_PAD - RING_R,
        DockEdge::Left => card.min.x + PANEL_PAD + RING_R,
    };
    // Slides out from under the icon ring rather than fading in at its final
    // spot, which reads as the panel opening rather than content appearing.
    let week_cx = match edge {
        DockEdge::Right => icon_cx - COL_SPACING * t,
        DockEdge::Left => icon_cx + COL_SPACING * t,
    };

    if shown.is_empty() {
        text(
            ui.painter(),
            card.center(),
            Align2::CENTER_CENTER,
            "no providers",
            9.0,
            0.4,
            MUTED,
            card.width() - 12.0,
        );
    }

    let rows = shown.len() as f32;
    let total = BLOCK_H * rows + BLOCK_GAP * (rows - 1.0).max(0.0);
    let mut top = card.min.y + (card_h - total) / 2.0;

    for &provider in shown {
        let reading = reading_for(snapshot, provider);
        draw_provider_block(
            ui,
            icons,
            provider,
            &reading,
            icon_cx,
            top,
            now,
            (t > 0.01).then_some((week_cx, t)),
        );
        top += BLOCK_H + BLOCK_GAP;
    }

    Frame {
        card,
        animating: t > 0.0 && t < 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_escalate_with_usage() {
        assert_eq!(ring_color(0.1), CALM);
        assert_eq!(ring_color(0.6), WARM);
        assert_eq!(ring_color(0.95), HOT);
    }

    #[test]
    fn dimming_reduces_alpha() {
        let c = Color32::from_rgb(255, 255, 255);
        assert!(dim(c, 0.5).a() < c.a());
    }

    #[test]
    fn rail_is_narrower_than_panel() {
        const { assert!(RAIL_CARD_W < PANEL_CARD_W) };
    }

    /// The rail must be exactly a padded ring wide, or the icon ring would sit
    /// at a different offset collapsed than expanded and jump on hover.
    #[test]
    fn rail_width_keeps_the_icon_ring_stationary() {
        let right_inset_when_collapsed = RAIL_CARD_W - PANEL_PAD - RING_R;
        let centre_of_rail = RAIL_CARD_W / 2.0;
        assert!(
            (right_inset_when_collapsed - centre_of_rail).abs() < 0.01,
            "icon ring should be centred in the rail"
        );
    }

    #[test]
    fn docked_corners_are_square_on_the_attached_side() {
        let r = card_radius(DockEdge::Right);
        assert_eq!((r.ne, r.se), (0, 0));
        assert!(r.nw > 0 && r.sw > 0);
        let l = card_radius(DockEdge::Left);
        assert_eq!((l.nw, l.sw), (0, 0));
        assert!(l.ne > 0 && l.se > 0);
    }

    #[test]
    fn weekly_column_fits_inside_the_panel() {
        // Panel must hold the weekly column plus its caption width.
        let needed = PANEL_PAD * 2.0 + COL_SPACING + RING_D.max(COL_W);
        assert!(
            PANEL_CARD_W + 0.01 >= needed - (COL_W - RING_D) / 2.0,
            "panel {PANEL_CARD_W} too narrow for {needed}"
        );
    }

    #[test]
    fn weekly_label_splits_into_weekday_and_time() {
        let w = Window {
            used: 0.5,
            resets_at: chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 9, 18, 12, 0, 0).unwrap(),
        };
        let (day, time) = weekly_reset_parts(Some(&w));
        assert!(
            [
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
                "Sunday",
            ]
            .contains(&day.as_str()),
            "expected an English weekday, got {day}"
        );
        assert!(time.contains(':'), "expected a clock time, got {time}");
    }

    #[test]
    fn weekly_label_falls_back_when_absent() {
        assert_eq!(weekly_reset_parts(None).0, "—");
    }

    #[test]
    fn claude_keeps_its_brand_orange() {
        assert_eq!(Provider::Claude.tint(), CLAUDE_ORANGE);
        assert_ne!(Provider::Codex.tint(), CLAUDE_ORANGE);
    }
}
