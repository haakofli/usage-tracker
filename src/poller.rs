use crate::model::{Reading, Snapshot};
use crate::store::{self, CachedQuota, LastGood};
use crate::{claude, codex};
use chrono::Utc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BASE: u64 = 300; // 5 min
const CAP: u64 = 3_600; // 60 min
/// Hard floor. The usage endpoint answers a sustained 30-60s poll with a
/// session-long 429, so nothing — not even a manual refresh — may go faster.
pub const FLOOR: Duration = Duration::from_secs(120);
const CODEX_INTERVAL: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_secs(5);

pub struct Backoff {
    level: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Backoff {
    pub fn new() -> Self {
        Self { level: 0 }
    }

    pub fn current(&self) -> Duration {
        Duration::from_secs((BASE << self.level.min(8)).min(CAP))
    }

    pub fn penalise(&mut self) {
        self.level = (self.level + 1).min(8);
    }

    pub fn reset(&mut self) {
        self.level = 0;
    }
}

pub struct Control {
    refresh: AtomicBool,
    claude_enabled: AtomicBool,
    codex_enabled: AtomicBool,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            refresh: AtomicBool::new(false),
            claude_enabled: AtomicBool::new(true),
            codex_enabled: AtomicBool::new(true),
        }
    }
}

impl Control {
    pub fn request_refresh(&self) {
        self.refresh.store(true, Ordering::Relaxed);
    }

    /// A disabled provider is not polled at all. That matters most for Claude,
    /// whose endpoint is rate-limited — there is no reason to spend requests on
    /// something the dock is not showing.
    pub fn set_enabled_from(&self, settings: &crate::settings::Settings) {
        use crate::providers::ProviderId;
        self.claude_enabled
            .store(settings.is_enabled(ProviderId::Claude), Ordering::Relaxed);
        self.codex_enabled
            .store(settings.is_enabled(ProviderId::Codex), Ordering::Relaxed);
    }

    fn take_refresh(&self) -> bool {
        self.refresh.swap(false, Ordering::Relaxed)
    }
}

/// Gated behind `DOCK_DIAG` so the poll cadence — the app's most consequential
/// behaviour, given the endpoint's 429 policy — can be audited from outside.
fn trace(provider: &str, reading: &Reading) {
    if std::env::var("DOCK_DIAG").is_err() {
        return;
    }
    let outcome = match reading {
        Reading::Ok { quota, .. } => format!("ok {quota:?}"),
        Reading::Stale { reason, .. } => format!("stale: {reason}"),
        Reading::Failed { reason } => format!("failed: {reason}"),
        Reading::Never => "never".to_string(),
    };
    eprintln!("[poll] {} {provider} {outcome}", Utc::now().to_rfc3339());
}

fn degrade(cached: Option<&CachedQuota>, reason: String) -> Reading {
    match cached {
        Some(c) => Reading::Stale {
            quota: c.quota.clone(),
            at: c.observed_at,
            reason,
        },
        None => Reading::Failed { reason },
    }
}

fn poll_claude(backoff: &mut Backoff, last_good: &mut LastGood) -> Reading {
    let creds = match claude::load_credentials() {
        Ok(c) => c,
        Err(e) => return degrade(last_good.claude.as_ref(), e.to_string()),
    };

    if !creds.can_read_usage() {
        return degrade(
            last_good.claude.as_ref(),
            "token lacks user:profile".to_string(),
        );
    }
    if creds.is_expired() {
        return degrade(last_good.claude.as_ref(), "re-auth needed".to_string());
    }

    match claude::fetch_usage(&creds) {
        Ok(quota) => {
            backoff.reset();
            let at = Utc::now();
            last_good.claude = Some(CachedQuota {
                quota: quota.clone(),
                observed_at: at,
            });
            let _ = store::save(last_good);
            Reading::Ok { quota, at }
        }
        Err(claude::FetchError::RateLimited) => {
            backoff.penalise();
            degrade(last_good.claude.as_ref(), "rate limited".to_string())
        }
        Err(e) => degrade(last_good.claude.as_ref(), e.to_string()),
    }
}

fn poll_codex(last_good: &mut LastGood) -> Reading {
    match codex::latest_reading() {
        Ok(Some(r)) => {
            last_good.codex = Some(CachedQuota {
                quota: r.quota.clone(),
                observed_at: r.observed_at,
            });
            let _ = store::save(last_good);

            // Codex only writes a snapshot when it runs, so the newest one can
            // easily describe a 5-hour window that has since elapsed. That is
            // not a live reading — the quota reset and we have not been told
            // the new figure — so present it as stale rather than current.
            let elapsed = r
                .quota
                .session
                .as_ref()
                .is_some_and(|w| w.resets_at <= Utc::now());
            if elapsed {
                return Reading::Stale {
                    quota: r.quota,
                    at: r.observed_at,
                    reason: "window reset".to_string(),
                };
            }

            Reading::Ok {
                quota: r.quota,
                at: r.observed_at,
            }
        }
        Ok(None) => degrade(
            last_good.codex.as_ref(),
            "no recent Codex sessions".to_string(),
        ),
        Err(e) => degrade(last_good.codex.as_ref(), e.to_string()),
    }
}

/// Seed the dock from disk so it shows last-known numbers immediately rather
/// than blanks during the first poll.
pub fn initial_snapshot(last_good: &LastGood) -> Snapshot {
    let seed = |c: Option<&CachedQuota>| {
        c.map(|c| Reading::Stale {
            quota: c.quota.clone(),
            at: c.observed_at,
            reason: "loading".to_string(),
        })
        .unwrap_or(Reading::Never)
    };
    Snapshot {
        claude: Some(seed(last_good.claude.as_ref())),
        codex: Some(seed(last_good.codex.as_ref())),
    }
}

pub fn spawn(shared: Arc<Mutex<Snapshot>>, control: Arc<Control>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut last_good = store::load();
        let mut claude_backoff = Backoff::new();
        let mut last_claude_attempt: Option<Instant> = None;
        let mut next_claude = Instant::now();
        let mut next_codex = Instant::now();

        loop {
            if control.take_refresh() {
                next_codex = Instant::now();
                // A manual refresh may pull the Claude poll forward, but never
                // past the floor since the previous attempt.
                next_claude = match last_claude_attempt {
                    Some(prev) => (prev + FLOOR).max(Instant::now()),
                    None => Instant::now(),
                };
            }

            if Instant::now() >= next_claude && control.claude_enabled.load(Ordering::Relaxed) {
                let reading = poll_claude(&mut claude_backoff, &mut last_good);
                trace("claude", &reading);
                shared.lock().unwrap().claude = Some(reading);
                last_claude_attempt = Some(Instant::now());
                next_claude = Instant::now() + claude_backoff.current();
                ctx.request_repaint();
            }

            if Instant::now() >= next_codex && control.codex_enabled.load(Ordering::Relaxed) {
                let reading = poll_codex(&mut last_good);
                trace("codex", &reading);
                shared.lock().unwrap().codex = Some(reading);
                next_codex = Instant::now() + CODEX_INTERVAL;
                ctx.request_repaint();
            }

            std::thread::sleep(TICK);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_the_default_interval() {
        assert_eq!(Backoff::new().current(), Duration::from_secs(300));
    }

    #[test]
    fn doubles_on_rate_limit_up_to_the_cap() {
        let mut b = Backoff::new();
        b.penalise();
        assert_eq!(b.current(), Duration::from_secs(600));
        b.penalise();
        assert_eq!(b.current(), Duration::from_secs(1200));
        b.penalise();
        assert_eq!(b.current(), Duration::from_secs(2400));
        b.penalise();
        assert_eq!(b.current(), Duration::from_secs(3600));
        b.penalise();
        assert_eq!(b.current(), Duration::from_secs(3600), "must cap at 60 min");
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
        assert!(Backoff::new().current() >= FLOOR);
    }

    #[test]
    fn saturating_penalties_never_overflow_the_shift() {
        let mut b = Backoff::new();
        for _ in 0..64 {
            b.penalise();
        }
        assert_eq!(b.current(), Duration::from_secs(3600));
    }

    #[test]
    fn refresh_flag_is_consumed_once() {
        let c = Control::default();
        c.request_refresh();
        assert!(c.take_refresh());
        assert!(!c.take_refresh());
    }

    #[test]
    fn degrades_to_stale_when_a_cached_value_exists() {
        let cached = CachedQuota {
            quota: Default::default(),
            observed_at: Utc::now(),
        };
        let r = degrade(Some(&cached), "rate limited".to_string());
        assert!(matches!(r, Reading::Stale { .. }));
    }

    #[test]
    fn degrades_to_failed_without_a_cached_value() {
        let r = degrade(None, "rate limited".to_string());
        assert!(matches!(r, Reading::Failed { .. }));
    }
}
