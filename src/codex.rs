use crate::model::{Quota, Window};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawLimit {
    used_percent: f64,
    resets_at: i64,
}

#[derive(Debug, Deserialize)]
struct RawLimits {
    primary: Option<RawLimit>,
    secondary: Option<RawLimit>,
}

impl RawLimit {
    fn into_window(self) -> Window {
        Window {
            used: (self.used_percent / 100.0).clamp(0.0, 1.0) as f32,
            resets_at: DateTime::from_timestamp(self.resets_at, 0).unwrap_or_else(Utc::now),
        }
    }
}

/// Rollout lines nest `rate_limits` at varying depths across Codex versions,
/// so locate the key in the parsed tree rather than mirroring the full envelope.
pub fn parse_rate_limits_line(line: &str) -> Option<Quota> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let found = find_key(&v, "rate_limits")?;
    let raw: RawLimits = serde_json::from_value(found.clone()).ok()?;
    if raw.primary.is_none() && raw.secondary.is_none() {
        return None;
    }
    Some(Quota {
        session: raw.primary.map(RawLimit::into_window),
        weekly: raw.secondary.map(RawLimit::into_window),
    })
}

fn find_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(hit) = map.get(key).filter(|hit| !hit.is_null()) {
                return Some(hit);
            }
            map.values().find_map(|child| find_key(child, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|child| find_key(child, key)),
        _ => None,
    }
}

pub struct CodexReading {
    pub quota: Quota,
    pub observed_at: DateTime<Utc>,
}

pub fn sessions_root() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::platform::home_dir()?.join(".codex").join("sessions"))
}

pub fn latest_reading() -> anyhow::Result<Option<CodexReading>> {
    let root = sessions_root()?;
    if !root.is_dir() {
        return Ok(None);
    }

    let mut files = walk_rollouts(&root)?;
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    // A handful of recent files is plenty; if none of them carry a rate_limits
    // line the data is too old to be worth showing anyway.
    for (path, mtime) in files.into_iter().take(10) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(quota) = content.lines().rev().find_map(parse_rate_limits_line) {
            return Ok(Some(CodexReading {
                quota,
                observed_at: mtime,
            }));
        }
    }
    Ok(None)
}

fn walk_rollouts(
    dir: &std::path::Path,
) -> anyhow::Result<Vec<(std::path::PathBuf, DateTime<Utc>)>> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let is_rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
            if !is_rollout {
                continue;
            }
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            found.push((path, mtime));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primary_and_secondary_windows() {
        let line = include_str!("../tests/fixtures/codex_rollout.jsonl");
        let q = parse_rate_limits_line(line).unwrap();
        assert!(q.session.is_some());
        assert!(q.weekly.is_some());
    }

    #[test]
    fn converts_used_percent_to_fraction() {
        let line = r#"{"rate_limits":{"primary":{"used_percent":32.0,"window_minutes":300,"resets_at":1788211570},
                        "secondary":{"used_percent":5.0,"window_minutes":10080,"resets_at":1788798370}}}"#;
        let q = parse_rate_limits_line(line).unwrap();
        assert!((q.session.unwrap().used - 0.32).abs() < 1e-6);
        assert!((q.weekly.unwrap().used - 0.05).abs() < 1e-6);
    }

    #[test]
    fn ignores_lines_without_rate_limits() {
        assert!(parse_rate_limits_line(r#"{"type":"message","content":"hi"}"#).is_none());
    }

    #[test]
    fn finds_rate_limits_nested_under_payload() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":7.0,"window_minutes":300,"resets_at":1788211570},"secondary":null}}}"#;
        let q = parse_rate_limits_line(line).unwrap();
        assert!((q.session.unwrap().used - 0.07).abs() < 1e-6);
        assert!(q.weekly.is_none());
    }

    #[test]
    fn ignores_lines_whose_rate_limits_are_null() {
        assert!(parse_rate_limits_line(r#"{"payload":{"rate_limits":null}}"#).is_none());
    }

    #[test]
    #[ignore = "reads the real ~/.codex directory"]
    fn reads_a_real_codex_reading() {
        let r = latest_reading().unwrap();
        assert!(r.is_some(), "no rate_limits found in recent rollouts");
    }
}
