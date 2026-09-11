# Usage Tracker Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** A small always-on-top Windows dock, written in Rust, showing four numbers: Claude 5-hour + weekly quota, and Codex 5-hour + weekly quota.

**Architecture:** A single native binary. A background poller thread refreshes each provider on its own interval and publishes an immutable snapshot through a mutex; the egui render loop only ever reads that snapshot. Claude's numbers come from an authenticated HTTP call; Codex's come from parsing its local session files. Last-good readings are persisted so the dock shows stale data rather than blanks after a failure.

**Tech Stack:** Rust, `eframe`/`egui` (UI), `reqwest` (blocking, rustls), `serde`/`serde_json`, `chrono`, `anyhow`.

**Out of scope (deliberately):** spend/cost tracking, token counting, transcript parsing, history charts, per-model breakdowns, other providers. Only the four quota numbers.

---

## Background: what we already know

This was researched and partly verified on the target machine before writing this plan. Trust the *verified* items; the *unverified* ones are resolved in Task 1 before any code depends on them.

### Claude — VERIFIED and UNVERIFIED parts

```
GET https://api.anthropic.com/api/oauth/usage
Authorization: Bearer <accessToken>
anthropic-beta: oauth-2025-04-20
```

- **VERIFIED:** `~/.claude/.credentials.json` exists and its token carries the scopes
  `["user:file_upload","user:inference","user:mcp_servers","user:profile","user:sessions:claude_code"]`.
  The `user:profile` scope is the one this endpoint requires, so this account *can* call it.
- **UNVERIFIED:** the exact JSON key path to the token (reported elsewhere as `.claudeAiOauth.accessToken`).
- **UNVERIFIED:** whether `utilization` is a fraction (`0.42`) or a percentage (`42.0`). Published examples show
  `0.42`, but prose descriptions say "percentage used 0–100". **Do not guess — Task 1 settles this.**
- **UNVERIFIED:** whether `resets_at` is an ISO-8601 string (as published) or a unix epoch.

Response shape, as published:

```json
{
  "five_hour": { "utilization": 0.42, "resets_at": "2026-02-28T17:00:00Z" },
  "seven_day": { "utilization": 0.61, "resets_at": "2026-03-07T08:00:00Z" },
  "seven_day_sonnet": { "utilization": 0.35, "resets_at": "..." },
  "extra_usage": { }
}
```

### Claude — the hard constraint

This endpoint rate-limits aggressively. Three open issues (anthropics/claude-code #31637, #30930, #31021)
report that polling every 30–60s produces a **permanent 429 for the rest of the session**, with
`retry-after: 0` and no signal for when it clears.

This is a design constraint, not an edge case:

- Default poll interval **5 minutes**. Hard floor of 2 minutes, enforced in code.
- On 429: exponential backoff 5 → 10 → 20 → 40 → 60 min (capped), and **keep showing the cached value**.
- Never retry a 429 immediately, no matter what `retry-after` says.

### Codex — VERIFIED

Codex already writes the server's own quota snapshot into its rollout files. Confirmed present on this
machine at `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`:

```json
"rate_limits":{"limit_id":"codex","limit_name":null,
  "primary":{"used_percent":32.0,"window_minutes":300,"resets_at":1788211570},
  "secondary":{"used_percent":5.0,"window_minutes":10080,"resets_at":1788798370},
  "credits":{"has_credits":false,"unlimited":false,"balance":null},
  "plan_type":"team"}
```

- `primary` = 5-hour window (`window_minutes: 300`)
- `secondary` = weekly window (`window_minutes: 10080`)
- `used_percent` is plainly 0–100. `resets_at` is a unix epoch in seconds.

**This is the v1 source** because it is verified, needs no subprocess, and cannot be rate-limited.
Its only weakness is freshness: it is as current as your last Codex request. The UI shows an "as of"
time so that is visible rather than misleading.

There is a live alternative — spawn `codex app-server` and call `account/rateLimits/read` over
JSON-RPC on stdio — but it is **unverified**. It is Task 12, an upgrade, not the foundation.

---

## Design decisions (made deliberately — change early or not at all)

**1. egui/eframe, not Tauri.** You asked for a Rust app and want to avoid third-party bulk. egui gives one
self-contained binary with no webview and no HTML/CSS. For a strip that draws four bars and four labels,
an embedded browser is absurd. If you'd rather style it in CSS, switch now — it invalidates Tasks 10–11 only.

**2. Never write to `~/.claude/.credentials.json`.** Read-only, always. Implementing OAuth refresh means
writing tokens back, and a bug there corrupts the credentials your actual Claude Code sessions depend on.
When the token is expired or the API returns 401, show the cached value marked stale plus a "re-auth needed"
hint. Claude Code refreshes the token itself during normal use, and we pick that up on the next poll.
**This is a safety rule, not a preference.**

**3. Always-on-top overlay, not a registered appbar.** Reserving desktop space so maximized windows can't
cover the dock requires `SHAppBarMessage` and a good deal of Win32 plumbing. Start as a plain topmost
window. Add the appbar later only if it actually annoys you.

**4. The poller owns time, the UI owns nothing.** All network and file work happens on one background
thread. The render loop reads a `Mutex<Snapshot>` and draws. No async runtime, no channels-of-channels.

**5. Commits follow your global convention** — `[Patch]` / `[Minor]` / `[Major]`, English, intent-focused —
and go through the `/commit` skill. Work directly on `master`; the Jira-key branch rule doesn't apply to a
personal repo with no Jira project.

---

## Project layout

```
C:\programmering\usage-tracker\
  Cargo.toml
  docs\plans\2026-09-11-usage-tracker.md   <- this file
  src\
    main.rs        eframe bootstrap + window config
    model.rs       Quota / Window / Snapshot types, duration formatting
    claude.rs      credentials read, HTTP fetch, response parsing
    codex.rs       rollout file discovery + rate_limits parsing
    poller.rs      intervals, backoff, staleness
    store.rs       last-good cache on disk
    ui.rs          dock layout + bar widget
  tests\
    fixtures\
      claude_usage.json
      credentials_sample.json
      codex_rollout.jsonl
```

---

## Task 1: Ground truth probe (do this FIRST — no code yet)

Nothing else in this plan is safe to write until the two unverified shapes are pinned down. This task
produces the test fixtures every later task asserts against.

**Files:**
- Create: `tests/fixtures/claude_usage.json`
- Create: `tests/fixtures/codex_rollout.jsonl`
- Create: `docs/plans/probe-results.md`

**Step 1: Call the Claude usage endpoint exactly once**

> ⚠️ Run this **once**. Do not loop it, do not re-run it while iterating. A tight loop here earns you a
> session-long 429 and you will be unable to test anything for hours.

```powershell
$cred  = Get-Content "$env:USERPROFILE\.claude\.credentials.json" -Raw | ConvertFrom-Json
$token = $cred.claudeAiOauth.accessToken
if (-not $token) { $cred | ConvertTo-Json -Depth 5 }   # token path differs -> inspect the shape
Invoke-RestMethod -Uri "https://api.anthropic.com/api/oauth/usage" `
  -Headers @{ Authorization = "Bearer $token"; "anthropic-beta" = "oauth-2025-04-20" } |
  ConvertTo-Json -Depth 10
```

**Step 2: Record the answers in `docs/plans/probe-results.md`**

Write down, explicitly:
- the real JSON path to the access token
- whether `utilization` came back as `0.42`-style or `42.0`-style — **the single most important answer here**
- whether `resets_at` is an ISO string or an epoch integer
- which window keys your plan actually returns (`seven_day_opus`? `limits[]`?)

**Step 3: Save the response as a fixture**

Save it to `tests/fixtures/claude_usage.json`. **Redact nothing structural, but confirm it contains no
token or account identifier before committing it.** If it does, hand-edit those values to `"REDACTED"`.

**Step 4: Save a Codex fixture**

Take the newest rollout file, pull one `rate_limits` line, save it as `tests/fixtures/codex_rollout.jsonl`:

```powershell
$f = Get-ChildItem "$env:USERPROFILE\.codex\sessions" -Recurse -Filter "rollout-*.jsonl" |
     Sort-Object LastWriteTime -Descending | Select-Object -First 1
Select-String -Path $f.FullName -Pattern '"rate_limits"' | Select-Object -Last 1 |
  ForEach-Object { $_.Line } | Set-Content "tests\fixtures\codex_rollout.jsonl"
```

**Step 5: Commit**

```
[Patch] Capture provider response shapes before implementing readers
```

---

## Task 2: Scaffold

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `.gitignore`

**Step 1: Create the crate**

```powershell
cd C:\programmering\usage-tracker
cargo init --name usage-tracker
```

**Step 2: Add dependencies**

```powershell
cargo add eframe
cargo add reqwest --no-default-features --features "blocking,json,rustls-tls"
cargo add serde --features derive
cargo add serde_json
cargo add chrono --features serde
cargo add anyhow
```

> Version note: use whatever `cargo add` resolves to. `egui`'s viewport API has moved between releases, so
> if a call in Task 11 doesn't compile, check that version's docs rather than fighting the snippet here.
> Deliberately no `directories` crate — `std::env::var("USERPROFILE")` is enough.

**Step 3: Verify it builds**

Run: `cargo run`
Expected: prints `Hello, world!`

**Step 4: Commit**

```
[Minor] Scaffold Rust crate for the quota dock
```

---

## Task 3: Core types and duration formatting

Pure logic, no I/O — so it's fully testable, and it's where the fraction-vs-percent decision gets locked in.

**Files:**
- Create: `src/model.rs`
- Modify: `src/main.rs` (add `mod model;`)

**Step 1: Write the failing tests**

```rust
// src/model.rs
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn formats_hours_and_minutes() {
        let now   = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let reset = now + chrono::Duration::minutes(134);
        assert_eq!(format_reset_in(reset, now), "2h 14m");
    }

    #[test]
    fn formats_days_for_long_windows() {
        let now   = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let reset = now + chrono::Duration::hours(50);
        assert_eq!(format_reset_in(reset, now), "2d 2h");
    }

    #[test]
    fn formats_elapsed_resets_as_due() {
        let now   = Utc.timestamp_opt(1_000_000, 0).unwrap();
        let reset = now - chrono::Duration::minutes(5);
        assert_eq!(format_reset_in(reset, now), "due");
    }

    #[test]
    fn percent_scales_fraction_to_display() {
        let w = Window { used: 0.42, resets_at: Utc.timestamp_opt(1_000_000, 0).unwrap() };
        assert_eq!(w.percent_label(), "42%");
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test model`
Expected: compile error — `Window` and `format_reset_in` don't exist.

**Step 3: Implement**

```rust
// src/model.rs
use chrono::{DateTime, Utc};

/// `used` is always a fraction in 0.0..=1.0 inside this app, whatever the
/// provider sent. Normalisation happens at each provider's parse boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub used: f32,
    pub resets_at: DateTime<Utc>,
}

impl Window {
    pub fn percent_label(&self) -> String {
        format!("{}%", (self.used * 100.0).round() as i64)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quota {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
}

#[derive(Debug, Clone)]
pub enum Reading {
    Never,
    Ok { quota: Quota, at: DateTime<Utc> },
    Stale { quota: Quota, at: DateTime<Utc>, reason: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub claude: Option<Reading>,
    pub codex: Option<Reading>,
}

pub fn format_reset_in(resets_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (resets_at - now).num_seconds();
    if secs <= 0 {
        return "due".to_string();
    }
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    if d > 0 { format!("{d}d {h}h") } else { format!("{h}h {m}m") }
}
```

**Step 4: Run to verify pass**

Run: `cargo test model`
Expected: 4 passed.

**Step 5: Commit**

```
[Minor] Add quota model and reset-countdown formatting
```

---

## Task 4: Read the Claude credentials file

**Files:**
- Create: `src/claude.rs`
- Create: `tests/fixtures/credentials_sample.json`
- Modify: `src/main.rs` (add `mod claude;`)

**Step 1: Create a fake credentials fixture**

Use the **real key path discovered in Task 1**, with a fake token:

```json
{ "claudeAiOauth": {
    "accessToken": "fake-token-for-tests",
    "refreshToken": "fake-refresh",
    "expiresAt": 4102444800000,
    "scopes": ["user:inference", "user:profile"]
} }
```

**Step 2: Write the failing tests**

```rust
#[test]
fn extracts_access_token() {
    let raw = include_str!("../tests/fixtures/credentials_sample.json");
    assert_eq!(parse_credentials(raw).unwrap().access_token, "fake-token-for-tests");
}

#[test]
fn reports_missing_profile_scope() {
    let raw = r#"{"claudeAiOauth":{"accessToken":"t","scopes":["user:inference"]}}"#;
    assert!(!parse_credentials(raw).unwrap().can_read_usage());
}

#[test]
fn detects_expired_token() {
    let raw = r#"{"claudeAiOauth":{"accessToken":"t","expiresAt":1,"scopes":["user:profile"]}}"#;
    assert!(parse_credentials(raw).unwrap().is_expired());
}
```

**Step 3: Implement**

```rust
// src/claude.rs
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Credentials {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<i64>, // epoch millis
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Credentials,
}

impl Credentials {
    pub fn can_read_usage(&self) -> bool {
        self.scopes.iter().any(|s| s == "user:profile")
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(ms) => chrono::Utc::now().timestamp_millis() >= ms,
            None => false,
        }
    }
}

pub fn parse_credentials(raw: &str) -> Result<Credentials> {
    Ok(serde_json::from_str::<CredentialsFile>(raw)
        .context("unexpected shape in .credentials.json")?
        .claude_ai_oauth)
}

/// Read-only. This function must never write to the credentials file.
pub fn load_credentials() -> Result<Credentials> {
    let home = std::env::var("USERPROFILE").context("USERPROFILE not set")?;
    let path = std::path::Path::new(&home).join(".claude").join(".credentials.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    parse_credentials(&raw)
}
```

**Step 4: Run tests**

Run: `cargo test claude`
Expected: 3 passed.

**Step 5: Commit**

```
[Minor] Read Claude OAuth credentials read-only
```

---

## Task 5: Parse the Claude usage response

**Files:**
- Modify: `src/claude.rs`

**Step 1: Write the failing tests**

Use the **real** `tests/fixtures/claude_usage.json` from Task 1, and assert the values you actually saw:

```rust
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
    assert!((0.0..=1.0).contains(&used), "used must be a fraction, got {used}");
}

#[test]
fn tolerates_missing_optional_windows() {
    let q = parse_usage(r#"{"five_hour":{"utilization":0.5,"resets_at":"2026-03-01T00:00:00Z"}}"#).unwrap();
    assert!(q.session.is_some());
    assert!(q.weekly.is_none());
}
```

**Step 2: Implement**

> Set `UTILIZATION_IS_PERCENT` from the Task 1 answer. The `normalise` helper defends against the
> other case too, because a silently-wrong scale is the single most likely bug in this whole app.

```rust
use crate::model::{Quota, Window};
use chrono::{DateTime, Utc};

/// Set from the Task 1 probe. If utilization came back as 42.0 -> true; as 0.42 -> false.
const UTILIZATION_IS_PERCENT: bool = false;

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

fn normalise(utilization: f64) -> f32 {
    let v = if UTILIZATION_IS_PERCENT || utilization > 1.0 {
        utilization / 100.0
    } else {
        utilization
    };
    v.clamp(0.0, 1.0) as f32
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
        weekly:  u.seven_day.map(RawWindow::into_window),
    })
}
```

**Step 3: Run tests**

Run: `cargo test claude`
Expected: 6 passed.

**Step 4: Commit**

```
[Minor] Parse Claude usage windows with scale normalisation
```

---

## Task 6: Fetch Claude usage over HTTP

Kept deliberately thin — all the interesting logic already lives in the tested parse function.

**Files:**
- Modify: `src/claude.rs`

**Step 1: Implement**

```rust
pub enum FetchError {
    RateLimited,
    Unauthorized,
    Other(anyhow::Error),
}

pub fn fetch_usage(creds: &Credentials) -> std::result::Result<Quota, FetchError> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| FetchError::Other(e.into()))?
        .get("https://api.anthropic.com/api/oauth/usage")
        .bearer_auth(&creds.access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .map_err(|e| FetchError::Other(e.into()))?;

    match resp.status().as_u16() {
        200 => {
            let body = resp.text().map_err(|e| FetchError::Other(e.into()))?;
            parse_usage(&body).map_err(FetchError::Other)
        }
        429 => Err(FetchError::RateLimited),
        401 | 403 => Err(FetchError::Unauthorized),
        s => Err(FetchError::Other(anyhow::anyhow!("usage endpoint returned {s}"))),
    }
}
```

Note the three outcomes are distinct types, not strings: the poller reacts very differently to each.
`Unauthorized` must **never** trigger a credential write (see Design Decision 2).

**Step 2: Manual smoke test**

Add a temporary `--probe` arg to `main.rs` that loads credentials, calls `fetch_usage`, prints the result,
and exits. Run it **once**.

Run: `cargo run -- --probe`
Expected: prints two windows with plausible percentages.

**Step 3: Commit**

```
[Minor] Fetch Claude quota from the OAuth usage endpoint
```

---

## Task 7: Parse Codex rollout files

**Files:**
- Create: `src/codex.rs`
- Modify: `src/main.rs` (add `mod codex;`)

**Step 1: Write the failing tests**

```rust
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
    assert!((q.weekly.unwrap().used  - 0.05).abs() < 1e-6);
}

#[test]
fn ignores_lines_without_rate_limits() {
    assert!(parse_rate_limits_line(r#"{"type":"message","content":"hi"}"#).is_none());
}
```

**Step 2: Implement**

```rust
// src/codex.rs
use crate::model::{Quota, Window};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawLimit { used_percent: f64, resets_at: i64 }

#[derive(Debug, Deserialize)]
struct RawLimits { primary: Option<RawLimit>, secondary: Option<RawLimit> }

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
    Some(Quota {
        session: raw.primary.map(RawLimit::into_window),
        weekly:  raw.secondary.map(RawLimit::into_window),
    })
}

fn find_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(hit) = map.get(key) { return Some(hit); }
            map.values().find_map(|child| find_key(child, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|child| find_key(child, key)),
        _ => None,
    }
}
```

**Step 3: Run tests**

Run: `cargo test codex`
Expected: 3 passed.

**Step 4: Commit**

```
[Minor] Parse Codex rate limits from rollout files
```

---

## Task 8: Find the newest Codex reading

Scanning every rollout file is wasteful — there are hundreds. Walk newest-first and stop at the first hit.

**Files:**
- Modify: `src/codex.rs`

**Step 1: Implement**

```rust
pub struct CodexReading { pub quota: Quota, pub observed_at: DateTime<Utc> }

pub fn latest_reading() -> anyhow::Result<Option<CodexReading>> {
    let home = std::env::var("USERPROFILE")?;
    let root = std::path::Path::new(&home).join(".codex").join("sessions");

    let mut files: Vec<_> = walk_rollouts(&root)?;
    files.sort_by_key(|(_, mtime)| std::cmp::Reverse(*mtime));

    // A handful of recent files is plenty; if none of them carry a rate_limits
    // line the data is too old to be worth showing anyway.
    for (path, mtime) in files.into_iter().take(10) {
        let content = std::fs::read_to_string(&path)?;
        if let Some(quota) = content.lines().rev().find_map(parse_rate_limits_line) {
            return Ok(Some(CodexReading { quota, observed_at: mtime }));
        }
    }
    Ok(None)
}
```

Implement `walk_rollouts` with `std::fs::read_dir` recursion, collecting `rollout-*.jsonl` paths with their
modified times. No `walkdir` dependency needed for a three-level `YYYY/MM/DD` tree.

**Step 2: Write an integration test**

```rust
#[test]
#[ignore = "reads the real ~/.codex directory"]
fn reads_a_real_codex_reading() {
    let r = latest_reading().unwrap();
    assert!(r.is_some(), "no rate_limits found in recent rollouts");
}
```

Run: `cargo test -- --ignored codex`
Expected: passes on this machine.

**Step 3: Commit**

```
[Minor] Locate the most recent Codex quota reading
```

---

## Task 9: Last-good cache

So a failed poll shows yesterday's number marked stale, instead of an empty dock.

**Files:**
- Create: `src/store.rs`

**Step 1: Implement**

Serialize `Quota` plus an observation timestamp per provider to
`%LOCALAPPDATA%\usage-tracker\last-good.json`. Load on startup, save after every successful poll.

Add `#[derive(Serialize, Deserialize)]` to `Window` and `Quota` in `model.rs`.

**Step 2: Write a round-trip test**

```rust
#[test]
fn round_trips_through_disk_format() {
    let q = Quota { session: Some(Window { used: 0.42, resets_at: Utc::now() }), weekly: None };
    let encoded = serde_json::to_string(&q).unwrap();
    let decoded: Quota = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.session.unwrap().used, 0.42);
}
```

**Step 3: Commit**

```
[Minor] Persist last-good quota readings across restarts
```

---

## Task 10: The poller

The heart of the app, and where the 429 constraint is actually enforced. Keep the backoff logic pure so
it can be tested without waiting real minutes.

**Files:**
- Create: `src/poller.rs`

**Step 1: Write the failing tests**

```rust
#[test]
fn starts_at_the_default_interval() {
    assert_eq!(Backoff::new().current(), Duration::from_secs(300));
}

#[test]
fn doubles_on_rate_limit_up_to_the_cap() {
    let mut b = Backoff::new();
    b.penalise(); assert_eq!(b.current(), Duration::from_secs(600));
    b.penalise(); assert_eq!(b.current(), Duration::from_secs(1200));
    b.penalise(); assert_eq!(b.current(), Duration::from_secs(2400));
    b.penalise(); assert_eq!(b.current(), Duration::from_secs(3600));
    b.penalise(); assert_eq!(b.current(), Duration::from_secs(3600), "must cap at 60 min");
}

#[test]
fn resets_after_a_success() {
    let mut b = Backoff::new();
    b.penalise();
    b.reset();
    assert_eq!(b.current(), Duration::from_secs(300));
}

#[test]
fn never_polls_faster_than_the_floor() {
    assert!(Backoff::new().current() >= Duration::from_secs(120));
}
```

**Step 2: Implement**

```rust
pub struct Backoff { level: u32 }

const BASE: u64 = 300;   // 5 min
const CAP:  u64 = 3_600; // 60 min

impl Backoff {
    pub fn new() -> Self { Self { level: 0 } }
    pub fn current(&self) -> Duration {
        Duration::from_secs((BASE << self.level.min(8)).min(CAP))
    }
    pub fn penalise(&mut self) { self.level = (self.level + 1).min(8); }
    pub fn reset(&mut self) { self.level = 0; }
}
```

**Step 3: Wire up the thread**

```rust
pub fn spawn(shared: Arc<Mutex<Snapshot>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut claude_backoff = Backoff::new();
        let mut next_claude = Instant::now();
        let mut next_codex  = Instant::now();

        loop {
            if Instant::now() >= next_claude {
                let reading = poll_claude(&mut claude_backoff);
                shared.lock().unwrap().claude = Some(reading);
                next_claude = Instant::now() + claude_backoff.current();
                ctx.request_repaint();
            }
            if Instant::now() >= next_codex {
                shared.lock().unwrap().codex = Some(poll_codex());
                next_codex = Instant::now() + Duration::from_secs(60);
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}
```

`poll_claude` maps outcomes to `Reading`:
- `Ok` → `Reading::Ok`, save to store, `backoff.reset()`
- `RateLimited` → `backoff.penalise()`, return cached quota as `Reading::Stale { reason: "rate limited" }`
- `Unauthorized` → `Reading::Stale { reason: "re-auth needed" }` — **no credential write**
- `Other` → `Reading::Stale { reason: e.to_string() }`

Codex polls every 60s unconditionally: it's a local file read and cannot be rate-limited.

**Step 4: Run tests**

Run: `cargo test poller`
Expected: 4 passed.

**Step 5: Commit**

```
[Minor] Poll providers on independent intervals with 429 backoff
```

---

## Task 11: The dock UI

**Files:**
- Create: `src/ui.rs`

Target layout, ~200px wide:

```
┌──────────────────┐
│ CLAUDE           │
│ 5h ▓▓▓▓░░░░  42% │
│    resets 2h 14m │
│ 7d ▓▓▓▓▓▓░░  61% │
│    resets 3d 4h  │
│                  │
│ CODEX            │
│ 5h ▓▓▓░░░░░  32% │
│    resets 1h 02m │
│ 7d ▓░░░░░░░   5% │
│    resets 5d 1h  │
│                  │
│ updated 12:41    │
└──────────────────┘
```

**Step 1: Build the bar widget**

A bar is just `ui.painter().rect_filled` twice — track, then fill at `used` width. Colour by threshold:
under 50% muted green, 50–80% amber, over 80% red. Keep it a single function taking `&Window`.

**Step 2: Render staleness honestly**

This matters more than it sounds — a stale number that looks live is worse than no number:

- `Reading::Ok` → normal colours
- `Reading::Stale` → dim the whole block to ~60% opacity and append the reason (`stale · rate limited`)
- `Reading::Failed` → show the provider name and the error, no bars
- `Reading::Never` → `—` placeholders

For Codex also show `observed_at` as `as of 12:41`, since the rollout source is only as fresh as your last
Codex request.

**Step 3: Manual check**

Run: `cargo run`
Expected: a window with four bars showing real numbers.

**Step 4: Commit**

```
[Minor] Render the quota dock with staleness indicators
```

---

## Task 12: Window placement and behaviour

**Files:**
- Modify: `src/main.rs`

**Step 1: Configure the viewport**

```rust
let options = eframe::NativeOptions {
    viewport: egui::ViewportBuilder::default()
        .with_inner_size([200.0, 260.0])
        .with_decorations(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_resizable(false),
    ..Default::default()
};
```

**Step 2: Position against the right edge**

Read the monitor size from `ctx.input(|i| i.viewport().monitor_size)` on the first frame and set the
position to `(monitor_width - window_width - margin, vertical_margin)`.

> This uses full screen bounds, not the taskbar-aware work area. If the taskbar overlaps the dock, that's
> when to add the `windows` crate and call `SystemParametersInfoW(SPI_GETWORKAREA)` — and not before.

**Step 3: Make it draggable and quittable**

An undecorated window with no taskbar entry must not become unclosable:
- drag anywhere on the background: `ViewportCommand::StartDrag` on drag of the root response
- right-click → context menu with **Refresh now** and **Quit**
- `Refresh now` resets the next-poll deadline — but must still respect the 2-minute floor for Claude

**Step 4: Commit**

```
[Minor] Dock the window to the screen edge with drag and quit
```

---

## Task 13 (optional): Live Codex readings via app-server

Only worth doing if rollout-file staleness actually bothers you in daily use.

**Unverified** — validate before building on it. Spawn `codex app-server`, speak JSON-RPC over stdin/stdout,
complete the handshake, wait ~500ms, then call `account/rateLimits/read`:

```
rateLimits.primary.usedPercent   / .resetsAt   -> 5-hour
rateLimits.secondary.usedPercent / .resetsAt   -> weekly
```

Start by confirming the method name exists (`codex app-server --help`, or probe from a scratch script).
If it works, make it the primary Codex source and keep the rollout parser as the fallback — it's already
tested and costs nothing to retain.

---

## Definition of done

- [ ] `cargo test` passes; `cargo clippy -- -D warnings` is clean
- [ ] Four real numbers render, and they match `/usage` in Claude Code and Codex's own display
- [ ] Killing the network shows stale values, not blanks or a crash
- [ ] Running for an hour produces no 429 storm — check that Claude polled ~12 times, not 120
- [ ] `~/.claude/.credentials.json` is byte-identical after a long run (**verify with a hash**)
- [ ] The window stays on top, drags, and quits from the context menu

## Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| 429 lockout during development | **High** | Probe once in Task 1; test against fixtures, never the live endpoint |
| `utilization` scale misread | Medium | Task 1 resolves it; `normalise` defends both ways; test asserts 0..=1 |
| Undocumented endpoint changes | Medium | Isolated in `claude.rs`; UI degrades to stale rather than crashing |
| Token expires mid-run | Medium | Show "re-auth needed"; never write credentials |
| Codex rollout format changes | Low | `find_key` tolerates nesting changes; fixture test catches shape breaks |
