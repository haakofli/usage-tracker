use egui::{ColorImage, TextureHandle, TextureOptions};

const CLAUDE_SVG: &str = include_str!("../assets/claude.svg");
const CODEX_SVG: &str = include_str!("../assets/codex.svg");
pub const APP_ICON_SVG: &str = include_str!("../assets/branding/app.svg");

/// The mark the tray draws on this platform.
///
/// macOS tints a template image to match the menu bar in either appearance, so
/// the flat black mask is the right source there. Windows draws the icon exactly
/// as given, which is what the teal one is for.
#[cfg(target_os = "macos")]
pub const TRAY_ICON_SVG: &str = include_str!("../assets/branding/tray-template.svg");
#[cfg(windows)]
pub const TRAY_ICON_SVG: &str = include_str!("../assets/branding/tray.svg");

/// Rasterised well above display size so the marks stay crisp at any DPI and
/// when the panel animates.
const RASTER: u32 = 128;

pub struct Icons {
    pub claude: TextureHandle,
    pub codex: TextureHandle,
}

impl Icons {
    pub fn load(ctx: &egui::Context) -> Self {
        Self {
            claude: upload(ctx, "claude-mark", CLAUDE_SVG),
            codex: upload(ctx, "codex-mark", CODEX_SVG),
        }
    }
}

fn upload(ctx: &egui::Context, name: &str, svg: &str) -> TextureHandle {
    let image =
        rasterise(svg).unwrap_or_else(|| ColorImage::filled([1, 1], egui::Color32::TRANSPARENT));
    ctx.load_texture(name, image, TextureOptions::LINEAR)
}

/// Renders the SVG and keeps only its coverage, rewriting every pixel to white
/// so the mark can be tinted at paint time — one texture serves the live,
/// stale and dimmed states instead of three.
fn rasterise(svg: &str) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }

    // Fit the longer side to RASTER and centre the other, so non-square
    // viewBoxes (the OpenAI mark is 256x260) keep their aspect ratio.
    let scale = (RASTER as f32 / size.width()).min(RASTER as f32 / size.height());
    let dx = (RASTER as f32 - size.width() * scale) / 2.0;
    let dy = (RASTER as f32 - size.height() * scale) / 2.0;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(RASTER, RASTER)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_translate(dx, dy).pre_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let pixels = pixmap
        .pixels()
        .iter()
        .map(|p| egui::Color32::from_rgba_unmultiplied(255, 255, 255, p.alpha()))
        .collect();

    Some(ColorImage::new([RASTER as usize, RASTER as usize], pixels))
}

/// Full-colour raster of an SVG at `size`, as **unpremultiplied** RGBA — the
/// layout winit and the tray both expect, and the opposite of what tiny-skia
/// stores, so each pixel is demultiplied on the way out.
pub fn rasterise_rgba(svg: &str, size: u32) -> Option<Vec<u8>> {
    let pixmap = render_to_pixmap(svg, size)?;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(out)
}

/// PNG bytes for an SVG, used to mint the `.ico` the executable carries.
pub fn encode_png(svg: &str, size: u32) -> Option<Vec<u8>> {
    render_to_pixmap(svg, size)?.encode_png().ok()
}

fn render_to_pixmap(svg: &str, size: u32) -> Option<resvg::tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).ok()?;
    let s = tree.size();
    if s.width() <= 0.0 || s.height() <= 0.0 {
        return None;
    }
    let scale = (size as f32 / s.width()).min(size as f32 / s.height());
    let dx = (size as f32 - s.width() * scale) / 2.0;
    let dy = (size as f32 - s.height() * scale) / 2.0;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_translate(dx, dy).pre_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterises_the_claude_mark() {
        let img = rasterise(CLAUDE_SVG).expect("claude svg should rasterise");
        assert_eq!(img.size, [RASTER as usize, RASTER as usize]);
    }

    #[test]
    fn rasterises_the_codex_mark() {
        let img = rasterise(CODEX_SVG).expect("codex svg should rasterise");
        assert_eq!(img.size, [RASTER as usize, RASTER as usize]);
    }

    /// Guards the tint contract. `ColorImage` stores premultiplied bytes, so a
    /// neutral white mask is RGB == alpha on every pixel; any colour surviving
    /// from the source artwork would tint wrong (the Claude mark ships in brand
    /// orange, so this is a real risk, not a theoretical one).
    #[test]
    fn marks_are_neutral_alpha_masks_with_real_coverage() {
        for svg in [CLAUDE_SVG, CODEX_SVG] {
            let img = rasterise(svg).unwrap();
            let px = img.as_raw();
            assert!(
                px.chunks(4).any(|c| c[3] > 200),
                "expected opaque pixels in the mark"
            );
            assert!(
                px.chunks(4)
                    .all(|c| c[0] == c[3] && c[1] == c[3] && c[2] == c[3]),
                "mask must be neutral white premultiplied by coverage"
            );
        }
    }

    #[test]
    fn rejects_invalid_svg() {
        assert!(rasterise("not an svg").is_none());
    }

    #[test]
    fn app_icon_rasterises_to_rgba() {
        let px = rasterise_rgba(APP_ICON_SVG, 64).expect("app icon should rasterise");
        assert_eq!(px.len(), 64 * 64 * 4);
        assert!(
            px.chunks(4).any(|c| c[3] > 200),
            "icon must have opaque area"
        );
    }

    /// The tray icon is drawn at 16px; if the gauge collapses to nothing there
    /// it is useless, so assert it still has real coverage that small.
    #[test]
    fn app_icon_survives_tray_size() {
        let px = rasterise_rgba(TRAY_ICON_SVG, 16).unwrap();
        let opaque = px.chunks(4).filter(|c| c[3] > 128).count();
        assert!(
            opaque > 30,
            "expected a legible mark at 16px, got {opaque}px"
        );
    }

    #[test]
    fn app_icon_encodes_as_png() {
        let png = encode_png(APP_ICON_SVG, 256).expect("png encode");
        assert_eq!(&png[1..4], b"PNG", "expected a PNG signature");
    }
}
