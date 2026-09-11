fn main() {
    // Embeds the gauge icon into the executable so Explorer, the taskbar and
    // any shortcut show it. The window and tray icons are rasterised from
    // assets/icon.svg at runtime; this .ico is generated from that same SVG
    // (see `--emit-icon`) so the two cannot drift.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("embed application icon");
    }
}
