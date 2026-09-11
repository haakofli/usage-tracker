fn main() {
    // Embeds the gauge icon into the executable so Explorer, the taskbar and
    // any shortcut show it. The window and tray icons are rasterised from
    // assets/branding at runtime. Regenerate exports with
    // `cargo run --example export_icons` after changing the SVG sources.
    //
    // Keyed off the target rather than `cfg!(windows)`: a build script is
    // compiled for the host, so the cfg stays true when cross-compiling to
    // macOS and the Darwin build died trying to link a Windows resource.
    println!("cargo:rerun-if-changed=assets/branding/windows/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    winresource::WindowsResource::new()
        .set_icon("assets/branding/windows/app.ico")
        .compile()
        .expect("embed application icon");
}
