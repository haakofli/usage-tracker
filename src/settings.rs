use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockEdge {
    Left,
    Right,
}

impl DockEdge {
    /// Which edge a window at `x` (its left side, in points) is nearer to.
    pub fn nearest(x: f32, width: f32, monitor_w: f32) -> Self {
        let centre = x + width / 2.0;
        if centre < monitor_w / 2.0 {
            Self::Left
        } else {
            Self::Right
        }
    }
}

/// Scaling is applied as egui's zoom factor, so the layout stays in points
/// and every element — rings, text, padding — grows together.
pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 2.5;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Settings {
    pub edge: DockEdge,
    /// Vertical offset from the top of the monitor, in points.
    pub top: f32,
    #[serde(default = "unit_scale")]
    pub scale: f32,
}

fn unit_scale() -> f32 {
    1.0
}

impl Settings {
    /// Guards against a hand-edited or corrupt file leaving the dock at a size
    /// that cannot be grabbed to fix.
    pub fn sanitised(mut self) -> Self {
        if !self.scale.is_finite() {
            self.scale = 1.0;
        }
        self.scale = self.scale.clamp(MIN_SCALE, MAX_SCALE);
        if !self.top.is_finite() {
            self.top = 0.0;
        }
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            edge: DockEdge::Right,
            top: 140.0,
            scale: 1.0,
        }
    }
}

fn path() -> Result<std::path::PathBuf> {
    let base = std::env::var("LOCALAPPDATA").context("LOCALAPPDATA not set")?;
    Ok(std::path::Path::new(&base)
        .join("usage-tracker")
        .join("settings.json"))
}

/// A missing or unreadable settings file just means first run.
pub fn load() -> Settings {
    let Ok(p) = path() else {
        return Settings::default();
    };
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Settings::default();
    };
    serde_json::from_str::<Settings>(&raw)
        .unwrap_or_default()
        .sanitised()
}

pub fn save(s: &Settings) -> Result<()> {
    let p = path()?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(s)?)
        .with_context(|| format!("cannot write {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_nearer_edge() {
        // 3413pt-wide monitor, 88pt window.
        assert_eq!(DockEdge::nearest(20.0, 88.0, 3413.0), DockEdge::Left);
        assert_eq!(DockEdge::nearest(3300.0, 88.0, 3413.0), DockEdge::Right);
    }

    #[test]
    fn uses_the_window_centre_not_its_left_edge() {
        // Left edge just past halfway, so the whole window is right of centre.
        assert_eq!(DockEdge::nearest(1700.0, 88.0, 3413.0), DockEdge::Right);
        // Straddling the midpoint from the left.
        assert_eq!(DockEdge::nearest(1620.0, 88.0, 3413.0), DockEdge::Left);
    }

    #[test]
    fn defaults_to_the_right_edge() {
        assert_eq!(Settings::default().edge, DockEdge::Right);
    }

    #[test]
    fn round_trips_through_disk_format() {
        let s = Settings {
            edge: DockEdge::Left,
            top: 42.0,
            scale: 1.4,
        };
        let decoded: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(decoded.edge, DockEdge::Left);
        assert_eq!(decoded.top, 42.0);
        assert_eq!(decoded.scale, 1.4);
    }

    /// Settings files written before scaling existed must still load.
    #[test]
    fn defaults_scale_for_older_settings_files() {
        let decoded: Settings = serde_json::from_str(r#"{"edge":"Right","top":100.0}"#).unwrap();
        assert_eq!(decoded.scale, 1.0);
    }

    #[test]
    fn clamps_absurd_scales() {
        let huge = Settings {
            scale: 99.0,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(huge.scale, MAX_SCALE);

        let tiny = Settings {
            scale: 0.001,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(tiny.scale, MIN_SCALE);
    }

    #[test]
    fn recovers_from_a_non_finite_scale() {
        let nan = Settings {
            scale: f32::NAN,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(nan.scale, 1.0);
    }

    #[test]
    fn unreadable_settings_fall_back_to_default() {
        let decoded: Settings = serde_json::from_str("nope").unwrap_or_default();
        assert_eq!(decoded.edge, DockEdge::Right);
    }
}
