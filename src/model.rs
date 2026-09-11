use crate::providers::ProviderId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `used` is always a fraction in 0.0..=1.0 inside this app, whatever the
/// provider sent. Normalisation happens at each provider's parse boundary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub used: f32,
    pub resets_at: DateTime<Utc>,
}

impl Window {
    pub fn percent_label(&self) -> String {
        format!("{}%", (self.used * 100.0).round() as i64)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
}

#[derive(Debug, Clone)]
pub enum Reading {
    Never,
    Ok {
        quota: Quota,
        at: DateTime<Utc>,
    },
    Stale {
        quota: Quota,
        at: DateTime<Utc>,
        reason: String,
    },
    Failed {
        reason: String,
    },
}

impl Reading {
    pub fn quota(&self) -> Option<&Quota> {
        match self {
            Self::Ok { quota, .. } | Self::Stale { quota, .. } => Some(quota),
            Self::Never | Self::Failed { .. } => None,
        }
    }

    pub fn observed_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Ok { at, .. } | Self::Stale { at, .. } => Some(*at),
            Self::Never | Self::Failed { .. } => None,
        }
    }
}

/// What the poller has most recently learned, keyed by provider.
///
/// A map rather than a field per provider: the set of readable providers is
/// already three and the dock should not need surgery in five places to gain a
/// fourth.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    readings: std::collections::BTreeMap<ProviderId, Reading>,
}

impl Snapshot {
    pub fn get(&self, id: ProviderId) -> Option<&Reading> {
        self.readings.get(&id)
    }

    pub fn set(&mut self, id: ProviderId, reading: Reading) {
        self.readings.insert(id, reading);
    }
}

pub fn format_reset_in(resets_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (resets_at - now).num_seconds();
    if secs <= 0 {
        return "due".to_string();
    }
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else {
        format!("{h}h {m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn formats_hours_and_minutes() {
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let reset = now + chrono::Duration::minutes(134);
        assert_eq!(format_reset_in(reset, now), "2h 14m");
    }

    #[test]
    fn formats_days_for_long_windows() {
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let reset = now + chrono::Duration::hours(50);
        assert_eq!(format_reset_in(reset, now), "2d 2h");
    }

    #[test]
    fn formats_elapsed_resets_as_due() {
        let now = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let reset = now - chrono::Duration::minutes(5);
        assert_eq!(format_reset_in(reset, now), "due");
    }

    #[test]
    fn percent_scales_fraction_to_display() {
        let w = Window {
            used: 0.42,
            resets_at: Utc.timestamp_opt(1_000_000, 0).unwrap(),
        };
        assert_eq!(w.percent_label(), "42%");
    }
}
