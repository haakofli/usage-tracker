use crate::model::{Reading, Snapshot, Window, format_reset_in};
use chrono::{DateTime, Local, Utc};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2,
    text::{LayoutJob, TextFormat},
};

pub const DOCK_WIDTH: f32 = 208.0;
pub const DOCK_HEIGHT: f32 = 228.0;

/// The window is transparent and larger than the painted card so the shadow has
/// room to fall outside it.
const MARGIN: f32 = 8.0;
const PAD: f32 = 14.0;
const CARD_RADIUS: u8 = 12;

const BAR_HEIGHT: f32 = 6.0;
const BAR_LABEL_W: f32 = 20.0;
const PCT_W: f32 = 34.0;

const CARD_BG: Color32 = Color32::from_rgba_premultiplied(17, 18, 21, 242);
const EDGE: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 18);
const TRACK: Color32 = Color32::from_rgba_premultiplied(255, 255, 255, 20);
const HEADING: Color32 = Color32::from_rgb(138, 143, 153);
const VALUE: Color32 = Color32::from_rgb(233, 235, 239);
const MUTED: Color32 = Color32::from_rgb(106, 111, 122);

const CALM: Color32 = Color32::from_rgb(78, 186, 162);
const WARM: Color32 = Color32::from_rgb(222, 170, 88);
const HOT: Color32 = Color32::from_rgb(219, 112, 112);

fn bar_color(used: f32) -> Color32 {
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

/// egui's bundled monospace face gives tabular figures, so percentages don't
/// jitter horizontally as the digits change.
fn digits(size: f32) -> FontId {
    FontId::monospace(size)
}

fn label(size: f32) -> FontId {
    FontId::proportional(size)
}

fn tracked(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    size: f32,
    tracking: f32,
    color: Color32,
) {
    tracked_within(
        painter,
        pos,
        anchor,
        text,
        size,
        tracking,
        color,
        f32::INFINITY,
    );
}

/// `max_width` clips to a single ellipsised row. Provider error strings are not
/// length-bounded — a reqwest connect error alone is far wider than the card —
/// so anything derived from one must be drawn through here.
#[allow(clippy::too_many_arguments)]
fn tracked_within(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    size: f32,
    tracking: f32,
    color: Color32,
    max_width: f32,
) {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        TextFormat {
            font_id: label(size),
            extra_letter_spacing: tracking,
            color,
            ..Default::default()
        },
    );
    if max_width.is_finite() {
        job.wrap = egui::text::TextWrapping {
            max_width,
            max_rows: 1,
            break_anywhere: true,
            ..Default::default()
        };
    }
    let galley = painter.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
}

/// A two-tone fill, brighter across the top half, so the bar reads as a lit
/// surface rather than a flat block. A vertex-coloured mesh would give a
/// smoother ramp but cannot take a rounded silhouette, and at 6px tall the
/// rounded ends matter more than the gradient being continuous.
fn lit_bar(painter: &egui::Painter, rect: Rect, color: Color32, radius: CornerRadius) {
    painter.rect_filled(rect, radius, color);

    let highlight = Color32::from_rgba_unmultiplied(
        (color.r() as f32 * 1.18).min(255.0) as u8,
        (color.g() as f32 * 1.18).min(255.0) as u8,
        (color.b() as f32 * 1.18).min(255.0) as u8,
        color.a(),
    );
    let upper = Rect::from_min_max(
        rect.min,
        Pos2::new(rect.max.x, rect.min.y + rect.height() * 0.5),
    );
    painter.rect_filled(
        upper,
        CornerRadius {
            nw: radius.nw,
            ne: radius.ne,
            sw: 0,
            se: 0,
        },
        highlight,
    );
}

struct Row<'a> {
    tag: &'a str,
    window: Option<&'a Window>,
    id: &'a str,
}

fn draw_row(
    ui: &mut egui::Ui,
    top_left: Pos2,
    width: f32,
    row: Row<'_>,
    opacity: f32,
    now: DateTime<Utc>,
) -> f32 {
    let painter = ui.painter();
    let ctx = ui.ctx().clone();

    tracked(
        painter,
        Pos2::new(top_left.x, top_left.y + 1.0),
        Align2::LEFT_TOP,
        row.tag,
        11.0,
        0.3,
        dim(MUTED, opacity),
    );

    let bar_x = top_left.x + BAR_LABEL_W;
    let bar_w = width - BAR_LABEL_W - PCT_W;
    let bar_rect = Rect::from_min_size(
        Pos2::new(bar_x, top_left.y + 4.0),
        Vec2::new(bar_w, BAR_HEIGHT),
    );
    let radius = CornerRadius::same((BAR_HEIGHT / 2.0) as u8);

    painter.rect_filled(bar_rect, radius, dim(TRACK, opacity));

    match row.window {
        Some(w) => {
            // Glide to the new value instead of snapping, so a refresh reads as
            // a change rather than a repaint.
            let target = w.used.clamp(0.0, 1.0);
            let shown = ctx.animate_value_with_time(egui::Id::new(row.id), target, 0.45);
            let fill_w = (bar_w * shown).max(if shown > 0.0 { BAR_HEIGHT } else { 0.0 });
            if fill_w > 0.0 {
                let fill = Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, BAR_HEIGHT));
                lit_bar(painter, fill, dim(bar_color(target), opacity), radius);
            }

            painter.text(
                Pos2::new(top_left.x + width, top_left.y + 1.0),
                Align2::RIGHT_TOP,
                w.percent_label(),
                digits(11.0),
                dim(VALUE, opacity),
            );

            tracked(
                painter,
                Pos2::new(bar_x, top_left.y + 14.0),
                Align2::LEFT_TOP,
                &format!("resets {}", format_reset_in(w.resets_at, now)),
                10.0,
                0.0,
                dim(MUTED, opacity),
            );
            30.0
        }
        None => {
            painter.text(
                Pos2::new(top_left.x + width, top_left.y + 1.0),
                Align2::RIGHT_TOP,
                "—",
                digits(11.0),
                dim(MUTED, opacity),
            );
            20.0
        }
    }
}

fn draw_provider(
    ui: &mut egui::Ui,
    top_left: Pos2,
    width: f32,
    name: &str,
    reading: &Reading,
    now: DateTime<Utc>,
) -> f32 {
    let mut y = top_left.y;

    let opacity = match reading {
        Reading::Ok { .. } => 1.0,
        Reading::Stale { .. } => 0.45,
        Reading::Never | Reading::Failed { .. } => 0.6,
    };

    tracked(
        ui.painter(),
        Pos2::new(top_left.x, y),
        Align2::LEFT_TOP,
        name,
        10.0,
        1.3,
        dim(HEADING, opacity),
    );

    // A small dot carries the staleness signal without a shouty text label.
    if let Reading::Stale { .. } = reading {
        ui.painter().circle_filled(
            Pos2::new(top_left.x + width - 3.0, y + 5.0),
            2.5,
            dim(WARM, 0.85),
        );
    }
    y += 17.0;

    match reading {
        Reading::Failed { reason } => {
            tracked_within(
                ui.painter(),
                Pos2::new(top_left.x, y),
                Align2::LEFT_TOP,
                reason,
                10.0,
                0.0,
                dim(HOT, 0.9),
                width,
            );
            y += 16.0;
        }
        _ => {
            let quota = reading.quota();
            y += draw_row(
                ui,
                Pos2::new(top_left.x, y),
                width,
                Row {
                    tag: "5h",
                    window: quota.and_then(|q| q.session.as_ref()),
                    id: &format!("{name}-5h"),
                },
                opacity,
                now,
            );
            y += draw_row(
                ui,
                Pos2::new(top_left.x, y),
                width,
                Row {
                    tag: "7d",
                    window: quota.and_then(|q| q.weekly.as_ref()),
                    id: &format!("{name}-7d"),
                },
                opacity,
                now,
            );

            if let Reading::Stale { reason, .. } = reading {
                tracked_within(
                    ui.painter(),
                    Pos2::new(top_left.x, y),
                    Align2::LEFT_TOP,
                    &format!("stale · {reason}"),
                    9.0,
                    0.2,
                    dim(MUTED, 0.9),
                    width,
                );
                y += 13.0;
            }
        }
    }

    y - top_left.y
}

fn as_of(reading: Option<&Reading>) -> Option<DateTime<Utc>> {
    reading.and_then(|r| r.observed_at())
}

/// Returns the window height the drawn content needs, in points.
pub fn draw(ui: &mut egui::Ui, snapshot: &Snapshot) -> f32 {
    let now = Utc::now();
    let full = ui.max_rect();
    let card = Rect::from_min_max(
        full.min + Vec2::splat(MARGIN),
        full.max - Vec2::splat(MARGIN),
    );

    // The card's height depends on content that has not been laid out yet, so
    // reserve the background slots now and fill them once the height is known.
    // They still paint behind the content because slot order is draw order.
    let painter = ui.painter();
    let shadow_idx = painter.add(egui::Shape::Noop);
    let card_idx = painter.add(egui::Shape::Noop);
    let edge_idx = painter.add(egui::Shape::Noop);

    let content_w = card.width() - PAD * 2.0;
    let mut y = card.min.y + PAD;

    let claude = snapshot.claude.clone().unwrap_or(Reading::Never);
    y += draw_provider(
        ui,
        Pos2::new(card.min.x + PAD, y),
        content_w,
        "CLAUDE",
        &claude,
        now,
    );

    y += 12.0;

    let codex = snapshot.codex.clone().unwrap_or(Reading::Never);
    y += draw_provider(
        ui,
        Pos2::new(card.min.x + PAD, y),
        content_w,
        "CODEX",
        &codex,
        now,
    );

    // Both timestamps are observation times, not fetch times — the Codex
    // source in particular is only as fresh as the last Codex request — so
    // label them rather than leaving a bare clock time to be misread as "now".
    let stamp = |r: Option<&Reading>| {
        as_of(r)
            .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
            .unwrap_or_else(|| "—".to_string())
    };
    let footer = format!(
        "claude {} · codex {}",
        stamp(snapshot.claude.as_ref()),
        stamp(snapshot.codex.as_ref())
    );

    y += 8.0;
    tracked(
        ui.painter(),
        Pos2::new(card.min.x + PAD, y),
        Align2::LEFT_TOP,
        &footer,
        9.0,
        0.2,
        dim(MUTED, 0.8),
    );
    y += 10.0;

    let card = Rect::from_min_max(card.min, Pos2::new(card.max.x, y + PAD));
    let painter = ui.painter();
    let shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    painter.set(shadow_idx, shadow.as_shape(card, CARD_RADIUS));
    painter.set(
        card_idx,
        egui::epaint::RectShape::filled(card, CARD_RADIUS, CARD_BG),
    );
    painter.set(
        edge_idx,
        egui::epaint::RectShape::stroke(
            card,
            CARD_RADIUS,
            Stroke::new(1.0, EDGE),
            StrokeKind::Inside,
        ),
    );

    // The blocks change height as readings go stale or fail, so report the
    // height actually used and let the window resize to it rather than
    // guessing a size that either clips the footer or leaves dead space.
    card.height() + MARGIN * 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_escalate_with_usage() {
        assert_eq!(bar_color(0.1), CALM);
        assert_eq!(bar_color(0.6), WARM);
        assert_eq!(bar_color(0.95), HOT);
    }

    #[test]
    fn dimming_reduces_alpha() {
        let c = Color32::from_rgb(255, 255, 255);
        assert!(dim(c, 0.5).a() < c.a());
    }
}
