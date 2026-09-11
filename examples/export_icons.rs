//! Regenerate committed platform assets: cargo run --example export_icons
use std::{fs, path::Path};

fn png(svg: &str, width: u32, height: u32) -> Vec<u8> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(svg, &options).unwrap();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            width as f32 / tree.size().width(),
            height as f32 / tree.size().height(),
        ),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().unwrap()
}

fn write(path: impl AsRef<Path>, bytes: &[u8]) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/branding");
    let app = fs::read_to_string(root.join("app.svg")).unwrap();
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256, 512, 1024];
    for size in sizes {
        write(
            root.join(format!("png/app-{size}.png")),
            &png(&app, size, size),
        );
    }
    let ico_sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let images: Vec<_> = ico_sizes.iter().map(|&s| png(&app, s, s)).collect();
    let mut ico = vec![0, 0, 1, 0];
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + images.len() as u32 * 16;
    for (&size, bytes) in ico_sizes.iter().zip(&images) {
        ico.extend_from_slice(&[size as u8, size as u8, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += bytes.len() as u32;
    }
    for bytes in images {
        ico.extend(bytes);
    }
    write(root.join("windows/app.ico"), &ico);

    let mut chunks = Vec::new();
    for (kind, size) in [
        ("icp4", 16),
        ("icp5", 32),
        ("icp6", 64),
        ("ic07", 128),
        ("ic08", 256),
        ("ic09", 512),
        ("ic10", 1024),
        ("ic11", 32),
        ("ic12", 64),
        ("ic13", 256),
        ("ic14", 512),
    ] {
        let bytes = png(&app, size, size);
        chunks.extend_from_slice(kind.as_bytes());
        chunks.extend_from_slice(&((bytes.len() + 8) as u32).to_be_bytes());
        chunks.extend(bytes);
    }
    let mut icns = b"icns".to_vec();
    icns.extend_from_slice(&((chunks.len() + 8) as u32).to_be_bytes());
    icns.extend(chunks);
    write(root.join("macos/app.icns"), &icns);
    for size in [16, 32, 128, 256, 512] {
        for scale in [1, 2] {
            let suffix = if scale == 2 { "@2x" } else { "" };
            write(
                root.join(format!(
                    "macos/AppIcon.iconset/icon_{size}x{size}{suffix}.png"
                )),
                &png(&app, size * scale, size * scale),
            );
        }
    }
    for variant in ["tray", "tray-template"] {
        let svg = fs::read_to_string(root.join(format!("{variant}.svg"))).unwrap();
        for size in [16, 18, 20, 22, 24, 32, 36, 40, 44, 48, 64] {
            write(
                root.join(format!("tray/{variant}-{size}.png")),
                &png(&svg, size, size),
            );
            if variant == "tray-template" {
                write(
                    root.join(format!("tray/tray-white-{size}.png")),
                    &png(&svg.replace("#000", "#fff"), size, size),
                );
            }
        }
    }
    println!("Exported app PNGs, Windows ICO, macOS ICNS/iconset and tray PNGs.");
}
