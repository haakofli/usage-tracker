//! Everything the dock needs from the host that winit does not surface: frame
//! suppression, cursor hit-testing, the screen layout, and the two directories
//! the app reads and writes.
//!
//! Each backend presents the same API, so nothing above this module is
//! platform-aware.

use anyhow::{Context, Result};
use std::path::PathBuf;

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("usage-tracker supports Windows and macOS");

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
mod backend;

pub use backend::{Window, ZoomKey, ZoomKeys, monitors, take_display_changed, use_dark_menus};

/// One display, in physical pixels, plus the name that identifies it across
/// restarts so the dock can return to the screen it was left on.
///
/// Physical pixels rather than points, because they are the only unit that does
/// not move when the zoom changes. Callers convert once, at the point of
/// sending a viewport command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub name: String,
}

impl Monitor {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// The user's home directory, where the provider CLIs keep their state.
pub fn home_dir() -> Result<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var)
        .map(PathBuf::from)
        .with_context(|| format!("{var} not set"))
}

/// Per-user application data, where the dock keeps its settings and its
/// last-good cache.
pub fn config_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .context("LOCALAPPDATA not set")
    }
    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?.join("Library").join("Application Support"))
    }
}
