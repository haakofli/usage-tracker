use crate::model::{Quota, Window};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

#[derive(Debug, Deserialize)]
pub struct Credentials {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<i64>, // epoch millis
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl Credentials {
    pub fn can_read_usage(&self) -> bool {
        self.scopes.iter().any(|s| s == "user:profile")
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(ms) => Utc::now().timestamp_millis() >= ms,
            None => false,
        }
    }
}

/// The nesting of the OAuth block in `.credentials.json` is not contractual, so
/// prefer the documented `claudeAiOauth` key but fall back to locating whichever
/// object actually carries an `accessToken`.
pub fn parse_credentials(raw: &str) -> Result<Credentials> {
    let root: serde_json::Value =
        serde_json::from_str(raw).context("`.credentials.json` is not valid JSON")?;

    let block = root
        .get("claudeAiOauth")
        .filter(|v| v.get("accessToken").is_some())
        .or_else(|| find_object_with_key(&root, "accessToken"))
        .context("no object containing `accessToken` found in .credentials.json")?;

    serde_json::from_value(block.clone()).context("unexpected shape in .credentials.json")
}

fn find_object_with_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            if map.contains_key(key) {
                return Some(v);
            }
            map.values()
                .find_map(|child| find_object_with_key(child, key))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|child| find_object_with_key(child, key)),
        _ => None,
    }
}

pub fn credentials_path() -> Result<std::path::PathBuf> {
    Ok(crate::platform::home_dir()?
        .join(".claude")
        .join(".credentials.json"))
}

/// Read-only. This function must never write to the credentials file.
pub fn load_credentials() -> Result<Credentials> {
    let path = credentials_path()?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    parse_credentials(&raw)
}

#[derive(Debug, Deserialize)]
struct RawWindow {
    utilization: f64,
    resets_at: RawReset,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawReset {
    Iso(DateTime<Utc>),
    Epoch(i64),
}

#[derive(Debug, Deserialize)]
struct RawUsage {
    five_hour: Option<RawWindow>,
    seven_day: Option<RawWindow>,
}

/// VERIFIED against the live endpoint on 2026-09-11: `utilization` is a
/// percentage (`24.0` means 24%), not the 0..1 fraction shown in some published
/// examples. Divide unconditionally — a "treat >1.0 as percent" heuristic reads
/// a genuine 0.5% as 50%, and silently over-reporting usage is the worse
/// failure. If the scale ever flips, this under-reports visibly instead.
fn normalise(utilization: f64) -> f32 {
    (utilization / 100.0).clamp(0.0, 1.0) as f32
}

impl RawWindow {
    fn into_window(self) -> Window {
        Window {
            used: normalise(self.utilization),
            resets_at: match self.resets_at {
                RawReset::Iso(t) => t,
                RawReset::Epoch(s) => DateTime::from_timestamp(s, 0).unwrap_or_else(Utc::now),
            },
        }
    }
}

pub fn parse_usage(raw: &str) -> Result<Quota> {
    let u: RawUsage = serde_json::from_str(raw).context("unexpected usage response shape")?;
    Ok(Quota {
        session: u.five_hour.map(RawWindow::into_window),
        weekly: u.seven_day.map(RawWindow::into_window),
    })
}

pub enum FetchError {
    RateLimited,
    Unauthorized,
    Offline,
    Timeout,
    Other(anyhow::Error),
}

/// These strings are rendered in a 208pt-wide dock, so they stay terse.
/// `reqwest`'s own connect error runs several times the card width.
impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited => write!(f, "rate limited"),
            Self::Unauthorized => write!(f, "re-auth needed"),
            Self::Offline => write!(f, "offline"),
            Self::Timeout => write!(f, "timed out"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

fn classify(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else if e.is_connect() || e.is_request() {
        FetchError::Offline
    } else {
        FetchError::Other(e.into())
    }
}

fn client() -> std::result::Result<reqwest::blocking::Client, FetchError> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FetchError::Other(e.into()))
}

/// Returns the raw response body so callers that need the exact wire shape
/// (the `--probe` path) can inspect it without a second request.
pub fn fetch_usage_body(creds: &Credentials) -> std::result::Result<String, FetchError> {
    let resp = client()?
        .get(USAGE_URL)
        .bearer_auth(&creds.access_token)
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .map_err(classify)?;

    match resp.status().as_u16() {
        200 => resp.text().map_err(classify),
        429 => Err(FetchError::RateLimited),
        401 | 403 => Err(FetchError::Unauthorized),
        s => Err(FetchError::Other(anyhow::anyhow!(
            "usage endpoint returned {s}"
        ))),
    }
}

pub fn fetch_usage(creds: &Credentials) -> std::result::Result<Quota, FetchError> {
    let body = fetch_usage_body(creds)?;
    parse_usage(&body).map_err(FetchError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_access_token() {
        let raw = include_str!("../tests/fixtures/credentials_sample.json");
        assert_eq!(
            parse_credentials(raw).unwrap().access_token,
            "fake-token-for-tests"
        );
    }

    #[test]
    fn reports_missing_profile_scope() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"t","scopes":["user:inference"]}}"#;
        assert!(!parse_credentials(raw).unwrap().can_read_usage());
    }

    #[test]
    fn detects_expired_token() {
        let raw =
            r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":1,"scopes":["user:profile"]}}"#;
        assert!(parse_credentials(raw).unwrap().is_expired());
    }

    #[test]
    fn finds_token_under_an_unexpected_key() {
        let raw = r#"{"someOtherWrapper":{"accessToken":"t2","scopes":["user:profile"]}}"#;
        assert_eq!(parse_credentials(raw).unwrap().access_token, "t2");
    }

    #[test]
    fn parses_both_windows() {
        let raw = include_str!("../tests/fixtures/claude_usage.json");
        let q = parse_usage(raw).unwrap();
        assert!(q.session.is_some());
        assert!(q.weekly.is_some());
    }

    #[test]
    fn normalises_used_into_zero_to_one() {
        let raw = include_str!("../tests/fixtures/claude_usage.json");
        let q = parse_usage(raw).unwrap();
        let used = q.session.unwrap().used;
        assert!(
            (0.0..=1.0).contains(&used),
            "used must be a fraction, got {used}"
        );
    }

    #[test]
    fn tolerates_missing_optional_windows() {
        let q =
            parse_usage(r#"{"five_hour":{"utilization":50.0,"resets_at":"2026-03-01T00:00:00Z"}}"#)
                .unwrap();
        assert!(q.session.is_some());
        assert!(q.weekly.is_none());
    }

    #[test]
    fn reads_utilization_as_a_percentage() {
        let q =
            parse_usage(r#"{"five_hour":{"utilization":42.0,"resets_at":"2026-03-01T00:00:00Z"}}"#)
                .unwrap();
        assert!((q.session.unwrap().used - 0.42).abs() < 1e-6);
    }

    /// Regression guard for the scale bug: a genuine sub-1% reading must stay
    /// sub-1%, not get promoted to tens of percent by a fraction heuristic.
    #[test]
    fn keeps_sub_one_percent_readings_small() {
        let q =
            parse_usage(r#"{"five_hour":{"utilization":0.5,"resets_at":"2026-03-01T00:00:00Z"}}"#)
                .unwrap();
        let used = q.session.unwrap().used;
        assert!(
            (used - 0.005).abs() < 1e-6,
            "0.5% must be 0.005, got {used}"
        );
    }

    #[test]
    fn matches_the_captured_live_response() {
        let raw = include_str!("../tests/fixtures/claude_usage.json");
        let q = parse_usage(raw).unwrap();
        assert!((q.session.unwrap().used - 0.24).abs() < 1e-6);
        assert!((q.weekly.unwrap().used - 0.09).abs() < 1e-6);
    }

    #[test]
    fn accepts_epoch_resets_at() {
        let q = parse_usage(r#"{"five_hour":{"utilization":0.1,"resets_at":1788211570}}"#).unwrap();
        assert_eq!(q.session.unwrap().resets_at.timestamp(), 1788211570);
    }
}
