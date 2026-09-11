//! GitHub Copilot premium-request quota.
//!
//! `copilot_internal/user` is what GitHub's own editor clients call, and it is
//! the only source that reports an entitlement *and* a remainder. The published
//! billing API was the obvious candidate and is the wrong one: it reports
//! consumption after the fact with no allowance to measure it against, needs a
//! classic PAT created by hand, and returns nothing at all for the org-licensed
//! seats most people have.

use crate::http::{FetchError, classify, client};
use crate::model::{Quota, Window};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

const USER_ENDPOINT: &str = "https://api.github.com/copilot_internal/user";

/// The endpoint is meant for editor clients and answers 403 to a request that
/// does not look like one.
const EDITOR_VERSION: &str = "vscode/1.96.0";

/// A GitHub token with Copilot access, or `None` when this machine has no
/// readable one.
///
/// The Copilot CLI puts its token in the OS keychain whenever there is one, and
/// that copy is deliberately not read here. Every other place Copilot is signed
/// in leaves a plaintext copy, which covers the editor and `gh` users without
/// asking the keychain for anything.
pub fn token() -> Option<String> {
    from_env()
        .or_else(from_copilot_apps)
        .or_else(from_gh_hosts)
        .filter(|t| !t.is_empty())
}

fn from_env() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .into_iter()
        .find_map(|k| std::env::var(k).ok())
}

/// Where the editor extensions keep their OAuth token. Windows puts this under
/// `%LOCALAPPDATA%`; everywhere else it follows XDG.
fn copilot_config_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows)
        && let Ok(base) = crate::platform::config_dir()
    {
        dirs.push(base.join("github-copilot"));
    }
    if let Ok(home) = crate::platform::home_dir() {
        dirs.push(home.join(".config").join("github-copilot"));
    }
    dirs
}

/// `apps.json` is keyed by "github.com:<app id>", so the host is matched by
/// prefix rather than by the whole key, which changes between clients.
fn from_copilot_apps() -> Option<String> {
    #[derive(Deserialize)]
    struct Entry {
        oauth_token: Option<String>,
    }

    for dir in copilot_config_dirs() {
        for name in ["apps.json", "hosts.json"] {
            let Ok(raw) = std::fs::read_to_string(dir.join(name)) else {
                continue;
            };
            let Ok(entries) =
                serde_json::from_str::<std::collections::BTreeMap<String, Entry>>(&raw)
            else {
                continue;
            };
            let found = entries
                .into_iter()
                .filter(|(host, _)| host.starts_with("github.com"))
                .find_map(|(_, e)| e.oauth_token);
            if found.is_some() {
                return found;
            }
        }
    }
    None
}

/// `gh`'s config is YAML, but the one field needed is a plain scalar. Scanning
/// for it avoids taking a YAML parser as a dependency for a single line.
fn from_gh_hosts() -> Option<String> {
    let path = crate::platform::home_dir()
        .ok()?
        .join(".config")
        .join("gh")
        .join("hosts.yml");
    let raw = std::fs::read_to_string(path).ok()?;
    raw.lines()
        .filter_map(|line| line.trim().strip_prefix("oauth_token:"))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .find(|v| !v.is_empty())
}

#[derive(Deserialize)]
struct UserResponse {
    quota_reset_date: Option<String>,
    quota_snapshots: Option<Snapshots>,
}

#[derive(Deserialize)]
struct Snapshots {
    premium_interactions: Option<QuotaSnapshot>,
}

#[derive(Deserialize)]
struct QuotaSnapshot {
    entitlement: Option<f64>,
    remaining: Option<f64>,
    percent_remaining: Option<f64>,
    #[serde(default)]
    unlimited: bool,
}

/// Copilot's allowance is monthly and it has no second window, so it occupies
/// the slot the dock draws in the collapsed rail and leaves the hover ring
/// empty. The rail means "how much is left", which is true of all three
/// providers even where the period differs.
pub fn parse_usage(body: &str) -> Result<Quota> {
    let parsed: UserResponse =
        serde_json::from_str(body).context("Copilot user endpoint returned unexpected JSON")?;

    let snapshot = parsed
        .quota_snapshots
        .and_then(|s| s.premium_interactions)
        .context("no premium_interactions quota in the response")?;

    if snapshot.unlimited {
        anyhow::bail!("plan has unlimited premium requests");
    }

    // `percent_remaining` is authoritative when present; the counts are the
    // fallback, and an entitlement of zero would otherwise divide by zero.
    let remaining_fraction = match snapshot.percent_remaining {
        Some(p) => p / 100.0,
        None => match (snapshot.remaining, snapshot.entitlement) {
            (Some(r), Some(e)) if e > 0.0 => r / e,
            _ => anyhow::bail!("response carried no usable premium quota"),
        },
    };

    Ok(Quota {
        session: Some(Window {
            used: (1.0 - remaining_fraction).clamp(0.0, 1.0) as f32,
            resets_at: reset_at(parsed.quota_reset_date.as_deref()),
        }),
        weekly: None,
    })
}

/// The reset date arrives as a bare `YYYY-MM-DD`. Treating it as midnight UTC
/// is close enough for a countdown measured in days.
fn reset_at(raw: Option<&str>) -> DateTime<Utc> {
    raw.and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| Utc.from_utc_datetime(&dt))
        .unwrap_or_else(Utc::now)
}

pub fn fetch_usage_body(token: &str) -> std::result::Result<String, FetchError> {
    let resp = client()?
        .get(USER_ENDPOINT)
        .bearer_auth(token)
        .header("editor-version", EDITOR_VERSION)
        .header("user-agent", "usage-tracker")
        .header("accept", "application/json")
        .send()
        .map_err(classify)?;

    match resp.status().as_u16() {
        200 => resp.text().map_err(classify),
        429 => Err(FetchError::RateLimited),
        401 | 403 => Err(FetchError::Unauthorized),
        s => Err(FetchError::Other(anyhow::anyhow!(
            "Copilot endpoint returned {s}"
        ))),
    }
}

pub fn fetch_usage(token: &str) -> std::result::Result<Quota, FetchError> {
    let body = fetch_usage_body(token)?;
    parse_usage(&body).map_err(FetchError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/copilot_user.json");

    #[test]
    fn reads_the_premium_remainder() {
        let quota = parse_usage(SAMPLE).expect("fixture must parse");
        let session = quota.session.expect("premium quota fills the rail slot");
        // 93 of 300 remaining is 69% used.
        assert!(
            (session.used - 0.689).abs() < 0.002,
            "used was {}",
            session.used
        );
        assert!(
            quota.weekly.is_none(),
            "Copilot has no second window to draw"
        );
    }

    #[test]
    fn reads_the_reset_date() {
        let quota = parse_usage(SAMPLE).unwrap();
        assert_eq!(
            quota
                .session
                .unwrap()
                .resets_at
                .format("%Y-%m-%d")
                .to_string(),
            "2026-10-01"
        );
    }

    #[test]
    fn falls_back_to_counts_without_a_percentage() {
        let body = r#"{"quota_reset_date":"2026-10-01","quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":150}}}"#;
        let quota = parse_usage(body).unwrap();
        assert!((quota.session.unwrap().used - 0.5).abs() < 0.001);
    }

    /// An unlimited plan has no remainder to draw, and inventing a full ring
    /// would read as "none used" rather than "not applicable".
    #[test]
    fn refuses_an_unlimited_plan() {
        let body =
            r#"{"quota_snapshots":{"premium_interactions":{"unlimited":true,"entitlement":0}}}"#;
        assert!(parse_usage(body).is_err());
    }

    #[test]
    fn refuses_a_response_with_no_quota() {
        assert!(parse_usage(r#"{"quota_snapshots":{}}"#).is_err());
    }

    #[test]
    fn a_zero_entitlement_does_not_divide_by_zero() {
        let body =
            r#"{"quota_snapshots":{"premium_interactions":{"entitlement":0,"remaining":0}}}"#;
        assert!(parse_usage(body).is_err());
    }
}
