use crate::http::FetchError;
use crate::model::{Quota, Reading, Snapshot};
use crate::providers::ProviderId;
use crate::store::{self, CachedQuota, LastGood};
use crate::{claude, codex, copilot};
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
    /// One flag per provider, indexed by position in [`ProviderId::ALL`].
    enabled: [AtomicBool; ProviderId::ALL.len()],
}

impl Default for Control {
    fn default() -> Self {
        Self {
            refresh: AtomicBool::new(false),
            enabled: std::array::from_fn(|_| AtomicBool::new(true)),
        }
    }
}

impl Control {
    pub fn request_refresh(&self) {
        self.refresh.store(true, Ordering::Relaxed);
    }

    /// A disabled provider is not polled at all. That matters most for Claude
    /// and Copilot, whose endpoints are rate limited — there is no reason to
    /// spend requests on something the dock is not showing.
    pub fn set_enabled_from(&self, settings: &crate::settings::Settings) {
        for (slot, id) in self.enabled.iter().zip(ProviderId::ALL) {
            slot.store(settings.is_enabled(id), Ordering::Relaxed);
        }
    }

    fn is_enabled(&self, id: ProviderId) -> bool {
        ProviderId::ALL
            .iter()
            .position(|p| *p == id)
            .is_some_and(|i| self.enabled[i].load(Ordering::Relaxed))
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

/// Records a fresh reading and persists it, so a later failure can fall back to
/// it rather than showing blanks.
fn accept(id: ProviderId, quota: Quota, at: chrono::DateTime<Utc>, last_good: &mut LastGood) {
    last_good.set(
        id,
        CachedQuota {
            quota,
            observed_at: at,
        },
    );
    let _ = store::save(last_good);
}

fn poll_claude(backoff: &mut Backoff, last_good: &mut LastGood) -> Reading {
    let cached = |lg: &LastGood| lg.get(ProviderId::Claude).cloned();

    let creds = match claude::load_credentials() {
        Ok(c) => c,
        Err(e) => return degrade(cached(last_good).as_ref(), e.to_string()),
    };

    if !creds.can_read_usage() {
        return degrade(
            cached(last_good).as_ref(),
            "token lacks user:profile".to_string(),
        );
    }
    if creds.is_expired() {
        return degrade(cached(last_good).as_ref(), "re-auth needed".to_string());
    }

    match claude::fetch_usage(&creds) {
        Ok(quota) => {
            backoff.reset();
            let at = Utc::now();
            accept(ProviderId::Claude, quota.clone(), at, last_good);
            Reading::Ok { quota, at }
        }
        Err(FetchError::RateLimited) => {
            backoff.penalise();
            degrade(cached(last_good).as_ref(), "rate limited".to_string())
        }
        Err(e) => degrade(cached(last_good).as_ref(), e.to_string()),
    }
}

/// Copilot's endpoint is the one GitHub's editors use, so it is polled on the
/// same cautious schedule as Claude rather than hammered.
fn poll_copilot(backoff: &mut Backoff, last_good: &mut LastGood) -> Reading {
    let cached = |lg: &LastGood| lg.get(ProviderId::Copilot).cloned();

    let Some(token) = copilot::token() else {
        return degrade(cached(last_good).as_ref(), "not signed in".to_string());
    };

    match copilot::fetch_usage(&token) {
        Ok(quota) => {
            backoff.reset();
            let at = Utc::now();
            accept(ProviderId::Copilot, quota.clone(), at, last_good);
            Reading::Ok { quota, at }
        }
        Err(FetchError::RateLimited) => {
            backoff.penalise();
            degrade(cached(last_good).as_ref(), "rate limited".to_string())
        }
        Err(e) => degrade(cached(last_good).as_ref(), e.to_string()),
    }
}

fn poll_codex(last_good: &mut LastGood) -> Reading {
    match codex::latest_reading() {
        Ok(Some(r)) => {
            accept(ProviderId::Codex, r.quota.clone(), r.observed_at, last_good);

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
            last_good.get(ProviderId::Codex),
            "no recent Codex sessions".to_string(),
        ),
        Err(e) => degrade(last_good.get(ProviderId::Codex), e.to_string()),
    }
}

/// Seed the dock from disk so it shows last-known numbers immediately rather
/// than blanks during the first poll.
pub fn initial_snapshot(last_good: &LastGood) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for id in readable() {
        let reading = match last_good.get(id) {
            Some(c) => Reading::Stale {
                quota: c.quota.clone(),
                at: c.observed_at,
                reason: "loading".to_string(),
            },
            None => Reading::Never,
        };
        snapshot.set(id, reading);
    }
    snapshot
}

fn readable() -> impl Iterator<Item = ProviderId> {
    ProviderId::ALL.into_iter().filter(|p| p.has_quota_source())
}

/// When a provider is next due, and — for the ones that talk to a rate-limited
/// endpoint — how far it has backed off.
struct Schedule {
    next: Instant,
    backoff: Backoff,
    last_attempt: Option<Instant>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            next: Instant::now(),
            backoff: Backoff::new(),
            last_attempt: None,
        }
    }
}

fn poll_one(id: ProviderId, schedule: &mut Schedule, last_good: &mut LastGood) -> Reading {
    match id {
        ProviderId::Claude => poll_claude(&mut schedule.backoff, last_good),
        ProviderId::Copilot => poll_copilot(&mut schedule.backoff, last_good),
        ProviderId::Codex => poll_codex(last_good),
        // Filtered out by `readable`; a reading would be invented.
        ProviderId::Gemini | ProviderId::Cursor => Reading::Failed {
            reason: "no quota source".to_string(),
        },
    }
}

/// Codex is a local file read that cannot be rate limited, so it runs on a
/// fixed interval. The endpoint-backed providers follow their own backoff.
fn interval(id: ProviderId, schedule: &Schedule) -> Duration {
    match id {
        ProviderId::Codex => CODEX_INTERVAL,
        _ => schedule.backoff.current(),
    }
}

pub fn spawn(shared: Arc<Mutex<Snapshot>>, control: Arc<Control>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut last_good = store::load();
        let mut schedules: Vec<(ProviderId, Schedule)> =
            readable().map(|id| (id, Schedule::default())).collect();

        loop {
            if control.take_refresh() {
                for (id, schedule) in &mut schedules {
                    // A manual refresh may pull a poll forward, but never past
                    // the floor since that provider's previous attempt.
                    schedule.next = match (*id, schedule.last_attempt) {
                        (ProviderId::Codex, _) | (_, None) => Instant::now(),
                        (_, Some(prev)) => (prev + FLOOR).max(Instant::now()),
                    };
                }
            }

            for (id, schedule) in &mut schedules {
                if Instant::now() < schedule.next || !control.is_enabled(*id) {
                    continue;
                }
                let reading = poll_one(*id, schedule, &mut last_good);
                trace(id.key(), &reading);
                shared.lock().unwrap().set(*id, reading);
                schedule.last_attempt = Some(Instant::now());
                schedule.next = Instant::now() + interval(*id, schedule);
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
