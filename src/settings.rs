use crate::providers::ProviderId;
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
/// A little below the vertical middle, which reads as deliberate placement
/// rather than a corner.
pub const DEFAULT_TOP: f32 = 0.35;

pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 2.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub edge: DockEdge,
    /// Vertical position as a fraction (0..=1) of the room available on the
    /// monitor, rather than an absolute offset.
    ///
    /// An absolute value cannot survive moving between differently sized
    /// screens: a position saved near the bottom of a 2400px display clamped to
    /// the bottom edge of a 1440px one. A fraction keeps the dock at the same
    /// relative height on any screen.
    pub top: f32,
    #[serde(default = "unit_scale")]
    pub scale: f32,
    /// Device name of the display the dock was left on, so it returns there
    /// rather than to whichever screen Windows happens to open it on. `None`
    /// means "wherever it lands".
    #[serde(default)]
    pub monitor: Option<String>,
    /// Provider keys the dock shows. `None` means "not chosen yet", which is
    /// distinct from an explicitly empty list — the first run enables every
    /// readable provider, but someone who turns them all off stays that way.
    #[serde(default)]
    pub enabled: Option<Vec<String>>,
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
        // `top` used to be an absolute pixel offset. Anything above 1 is such a
        // value from an older build, and cannot be reinterpreted as a fraction,
        // so fall back to a sensible height rather than pinning to an edge.
        if !self.top.is_finite() || self.top > 1.0 {
            self.top = DEFAULT_TOP;
        }
        self.top = self.top.clamp(0.0, 1.0);
        // Drop keys this build does not know, so a provider removed in a later
        // version cannot linger and be counted as enabled.
        if let Some(keys) = &mut self.enabled {
            keys.retain(|k| ProviderId::from_key(k).is_some());
        }
        self
    }

    pub fn is_enabled(&self, id: ProviderId) -> bool {
        match &self.enabled {
            Some(keys) => keys.iter().any(|k| k == id.key()),
            // First run: everything the dock can actually read.
            None => id.has_quota_source(),
        }
    }

    pub fn set_enabled(&mut self, id: ProviderId, on: bool) {
        let mut keys = match self.enabled.take() {
            Some(keys) => keys,
            None => ProviderId::ALL
                .into_iter()
                .filter(|p| p.has_quota_source())
                .map(|p| p.key().to_string())
                .collect(),
        };
        keys.retain(|k| k != id.key());
        if on {
            keys.push(id.key().to_string());
        }
        self.enabled = Some(keys);
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            edge: DockEdge::Right,
            top: DEFAULT_TOP,
            scale: 1.0,
            monitor: None,
            enabled: None,
        }
    }
}

fn path() -> Result<std::path::PathBuf> {
    Ok(crate::platform::config_dir()?
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
    fn discards_provider_keys_this_build_does_not_know() {
        let s = Settings {
            enabled: Some(vec!["claude".into(), "ancient-provider".into()]),
            ..Default::default()
        }
        .sanitised();
        assert_eq!(s.enabled.as_deref(), Some(&["claude".to_string()][..]));
    }

    #[test]
    fn enables_readable_providers_on_first_run() {
        let s = Settings::default();
        assert!(s.is_enabled(ProviderId::Claude));
        assert!(s.is_enabled(ProviderId::Codex));
        assert!(!s.is_enabled(ProviderId::Gemini));
    }

    #[test]
    fn toggling_persists_an_explicit_choice() {
        let mut s = Settings::default();
        s.set_enabled(ProviderId::Codex, false);
        assert!(!s.is_enabled(ProviderId::Codex));
        assert!(s.is_enabled(ProviderId::Claude));

        s.set_enabled(ProviderId::Codex, true);
        assert!(s.is_enabled(ProviderId::Codex));
    }

    /// Turning everything off must stick, rather than falling back to the
    /// first-run default of "all readable providers".
    #[test]
    fn an_empty_selection_is_respected() {
        let mut s = Settings::default();
        s.set_enabled(ProviderId::Claude, false);
        s.set_enabled(ProviderId::Codex, false);
        assert_eq!(s.enabled.as_deref(), Some(&[][..]));
        assert!(!s.is_enabled(ProviderId::Claude));
        assert!(!s.is_enabled(ProviderId::Codex));
    }

    #[test]
    fn toggling_twice_does_not_duplicate_a_key() {
        let mut s = Settings::default();
        s.set_enabled(ProviderId::Claude, true);
        s.set_enabled(ProviderId::Claude, true);
        let keys = s.enabled.unwrap();
        assert_eq!(keys.iter().filter(|k| *k == "claude").count(), 1);
    }

    #[test]
    fn round_trips_through_disk_format() {
        let s = Settings {
            edge: DockEdge::Left,
            top: 0.25,
            scale: 1.4,
            monitor: None,
            enabled: Some(vec!["claude".into()]),
        };
        let decoded: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(decoded.edge, DockEdge::Left);
        assert_eq!(decoded.top, 0.25);
        assert_eq!(decoded.scale, 1.4);
    }

    /// Settings files written before scaling existed must still load.
    #[test]
    fn defaults_scale_for_older_settings_files() {
        let decoded: Settings = serde_json::from_str(r#"{"edge":"Right","top":0.4}"#).unwrap();
        assert_eq!(decoded.scale, 1.0);
    }

    /// `top` used to be absolute pixels. Such a value must not be mistaken for
    /// a fraction, or the dock pins itself to the bottom of the screen.
    #[test]
    fn migrates_a_legacy_pixel_offset() {
        let s = Settings {
            top: 1690.0,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(s.top, DEFAULT_TOP);
    }

    #[test]
    fn keeps_a_fractional_position() {
        let s = Settings {
            top: 0.6,
            ..Default::default()
        }
        .sanitised();
        assert_eq!(s.top, 0.6);
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
