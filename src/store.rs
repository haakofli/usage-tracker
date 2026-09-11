use crate::model::Quota;
use crate::providers::ProviderId;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedQuota {
    pub quota: Quota,
    pub observed_at: DateTime<Utc>,
}

/// Keyed by [`ProviderId::key`], and flattened so the file keeps the shape it
/// already had on disk — `{"claude": {...}, "codex": {...}}` — rather than
/// discarding everyone's cache to gain a provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LastGood {
    #[serde(flatten)]
    entries: std::collections::BTreeMap<String, CachedQuota>,
}

impl LastGood {
    pub fn get(&self, id: ProviderId) -> Option<&CachedQuota> {
        self.entries.get(id.key())
    }

    pub fn set(&mut self, id: ProviderId, cached: CachedQuota) {
        self.entries.insert(id.key().to_string(), cached);
    }
}

fn store_path() -> Result<std::path::PathBuf> {
    Ok(crate::platform::config_dir()?
        .join("usage-tracker")
        .join("last-good.json"))
}

/// A missing or corrupt cache is not an error worth surfacing — the dock simply
/// starts empty and fills in on the first successful poll.
pub fn load() -> LastGood {
    let Ok(path) = store_path() else {
        return LastGood::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return LastGood::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save(last_good: &LastGood) -> Result<()> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let encoded = serde_json::to_string_pretty(last_good)?;
    std::fs::write(&path, encoded).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Window;

    #[test]
    fn round_trips_through_disk_format() {
        let q = Quota {
            session: Some(Window {
                used: 0.42,
                resets_at: Utc::now(),
            }),
            weekly: None,
        };
        let encoded = serde_json::to_string(&q).unwrap();
        let decoded: Quota = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.session.unwrap().used, 0.42);
    }

    #[test]
    fn round_trips_both_providers() {
        let cached = CachedQuota {
            quota: Quota {
                session: Some(Window {
                    used: 0.5,
                    resets_at: Utc::now(),
                }),
                weekly: None,
            },
            observed_at: Utc::now(),
        };
        let mut lg = LastGood::default();
        lg.set(ProviderId::Claude, cached.clone());
        lg.set(ProviderId::Codex, cached);

        let encoded = serde_json::to_string(&lg).unwrap();
        let decoded: LastGood = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.get(ProviderId::Claude).is_some());
        assert!(decoded.get(ProviderId::Codex).is_some());
        assert!(decoded.get(ProviderId::Copilot).is_none());
    }

    /// The cache predates the move to a map, so a file written by an older
    /// build must still load rather than resetting everyone to blanks.
    #[test]
    fn reads_the_shape_written_before_providers_were_keyed() {
        let raw = r#"{"claude":{"quota":{"session":null,"weekly":null},"observed_at":"2026-09-01T10:00:00Z"}}"#;
        let decoded: LastGood = serde_json::from_str(raw).expect("old cache must still parse");
        assert!(decoded.get(ProviderId::Claude).is_some());
    }

    #[test]
    fn treats_unreadable_cache_as_empty() {
        let decoded: LastGood = serde_json::from_str("not json").unwrap_or_default();
        assert!(decoded.get(ProviderId::Claude).is_none());
        assert!(decoded.get(ProviderId::Codex).is_none());
    }
}
