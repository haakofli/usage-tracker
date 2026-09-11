# Usage Tracker artwork

A quota ring with two usage bars: a small reference to the app's provider meters.
Flat teal, warm white and deep green; no gradients or decorative AI symbols.
The SVGs are the editable masters, drawn directly to fit this repository's
existing vector rendering pipeline. No image generation service was used.

| Asset | Use |
| --- | --- |
| `app.svg` | App/window icon; transparent padding around a rounded tile |
| `png/app-*.png` | Square PNG exports, 16–1024 px |
| `windows/app.ico` | Multi-resolution executable/shortcut icon, 16–256 px |
| `macos/app.icns` | macOS app bundle icon, through 1024 px |
| `macos/AppIcon.iconset/` | Standard 16, 32, 128, 256, 512 point icons at 1× and 2× |
| `tray.svg` | Simplified transparent teal mark used by the Windows tray |
| `tray-template.svg` | Black alpha template for the future macOS menu bar |
| `tray/` | Teal, black and white PNGs at 16–64 px, including 18/36 and 22/44 pairs |
| `readme-banner.svg` | GitHub README banner, 1280×400 |

Regenerate the raster assets from the repository root:

```sh
cargo run --example export_icons
```

The exporter uses the project's existing `resvg` dependency. Commit regenerated
assets alongside SVG changes. The legacy `assets/icon.svg` and `assets/icon.ico`
are retained; the application now uses the files in this folder.

For the future macOS port, include `app.icns` in the bundle's `Contents/Resources`
and reference it with `CFBundleIconFile`. Use the black tray template with the
native template-image flag so macOS chooses its appearance. White exports are
provided for hosts that require explicit dark-background assets. These files
prepare the artwork; they do not make the Windows-specific application run on macOS.

Design brief: “A simple, elegant AI usage tracker icon: a partial quota ring
and two short usage bars. Use the application's teal, clear geometric shapes,
ample spacing and a separate monochrome tray mark. No sparkles, robots, brains,
gradients, shadows or lettering inside the icon.”
