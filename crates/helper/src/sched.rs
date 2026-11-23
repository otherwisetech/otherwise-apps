//! Which job runs next, and why the others wait. Pure: the caller passes the installed apps, the
//! settings, the state and what the computer is like right now.
use crate::state::{JobState, State, key};
use chrono::{DateTime, TimeZone, Utc};
use otherwise_link::background::{Job, Registered, Schedule};
use otherwise_link::settings::BackgroundSettings;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Pressure {
    #[default]
    Normal,
    Warn,
    Critical,
}

/// The computer right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conditions {
    pub now: DateTime<Utc>,
    /// Low Power Mode / battery saver.
    pub low_power: bool,
    pub pressure: Pressure,
    pub online: bool,
}

/// Why a job isn't running now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Scheduled for later.
    Until(DateTime<Utc>),
    Paused,
    /// Turned off for its app in Settings.
    Off,
    LowPower,
    Memory,
    Offline,
    /// `whileOpen: false` and the app is open.
    AppOpen,
}

pub const MAX_BACKOFF: Duration = Duration::from_secs(6 * 3600);
/// Jitter is at most a tenth of the interval, and never more than this.
pub const MAX_JITTER: Duration = Duration::from_secs(300);

/// After `n` consecutive failures: 1, 2, 4… minutes, at most 6 h, never sooner than the interval.
pub fn backoff(n: u32, every: Duration) -> Duration {
    if n == 0 {
        return every;
    }
    let exp = Duration::from_secs(60u64.saturating_mul(1u64 << (n - 1).min(16)));
    exp.min(MAX_BACKOFF).max(every.min(MAX_BACKOFF))
}

/// A stable offset per job so several apps' "every 15 minutes" don't all start at once (and the
/// same job doesn't drift: no randomness).
pub fn jitter(key: &str, every: Duration) -> Duration {
    let span = (every / 10).min(MAX_JITTER).as_secs();
    if span == 0 {
        return Duration::ZERO;
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    Duration::from_secs(h % span)
}

/// When `job` is due, from its state.
pub fn due<Tz: TimeZone>(app: &str, job: &Job, js: &JobState, now: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>> {
    let k = key(app, job.id.as_str());
    let every = match job.schedule()? {
        Schedule::Every(d) => d,
        Schedule::Daily(_) => Duration::from_secs(86400),
    };
    let base = job.next_run(js.last_start, now, tz)?;
    let base = if js.last_start.is_some() { base + chrono::Duration::from_std(jitter(&k, every)).ok()? } else { base };
    if js.failures > 0
        && let Some(end) = js.last_end
    {
        let retry = end + chrono::Duration::from_std(backoff(js.failures, every)).ok()?;
        return Some(retry);
    }
    Some(base)
}

/// One job's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub app: String,
    pub job: String,
    pub heavy: bool,
    /// None: due now and allowed.
    pub wait: Option<Wait>,
}

/// Every job of every installed app, with what it waits for. `open(app)` says whether the app
/// has a window open.
pub fn decide<Tz: TimeZone>(apps: &[Registered], settings: &BackgroundSettings, state: &State, c: &Conditions, open: &dyn Fn(&Registered) -> bool, tz: &Tz) -> Vec<Decision> {
    let mut out = vec![];
    for r in apps {
        let app = r.spec.id.as_str();
        let is_open = r.manifest.jobs.iter().any(|j| !j.while_open) && open(r);
        for j in &r.manifest.jobs {
            let js = state.job(app, &j.id);
            let wait = if !settings.jobs_on(app) {
                Some(Wait::Off)
            } else if state.paused(c.now) {
                Some(Wait::Paused)
            } else {
                match due(app, j, &js, c.now, tz) {
                    None => continue,
                    Some(at) if at > c.now => Some(Wait::Until(at)),
                    Some(_) if c.low_power => Some(Wait::LowPower),
                    Some(_) if c.pressure == Pressure::Critical || (j.heavy && c.pressure >= Pressure::Warn) => Some(Wait::Memory),
                    Some(_) if j.network && !c.online => Some(Wait::Offline),
                    Some(_) if !j.while_open && is_open => Some(Wait::AppOpen),
                    Some(_) => None,
                }
            };
            out.push(Decision { app: app.to_string(), job: j.id.clone(), heavy: j.heavy, wait });
        }
    }
    out
}

/// The job to run now: the most overdue allowed one. Jobs run one at a time, so two heavy jobs
/// never overlap.
pub fn pick<Tz: TimeZone>(decisions: &[Decision], state: &State, now: DateTime<Utc>, apps: &[Registered], tz: &Tz) -> Option<(String, String)> {
    decisions
        .iter()
        .filter(|d| d.wait.is_none())
        .filter_map(|d| {
            let r = apps.iter().find(|r| r.spec.id == d.app)?;
            let j = r.manifest.jobs.iter().find(|j| j.id == d.job)?;
            Some((due(&d.app, j, &state.job(&d.app, &d.job), now, tz)?, d))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.app.cmp(&b.1.app)).then(a.1.job.cmp(&b.1.job)))
        .map(|(_, d)| (d.app.clone(), d.job.clone()))
}

/// The soonest time anything may become due (to sleep until then, capped by the caller).
pub fn next_wake(decisions: &[Decision]) -> Option<DateTime<Utc>> {
    decisions.iter().filter_map(|d| if let Some(Wait::Until(t)) = d.wait { Some(t) } else { None }).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_and_jitter() {
        let m15 = Duration::from_secs(900);
        assert_eq!(backoff(0, m15), m15);
        assert_eq!(backoff(1, m15), m15);
        assert_eq!(backoff(6, m15), Duration::from_secs(32 * 60));
        assert_eq!(backoff(30, m15), MAX_BACKOFF);
        assert_eq!(backoff(3, Duration::from_secs(60)), Duration::from_secs(240));
        let j = jitter("expense/mail", m15);
        assert!(j < Duration::from_secs(90));
        assert_eq!(j, jitter("expense/mail", m15), "stable");
        assert!(jitter("x/y", Duration::from_secs(86400)) < MAX_JITTER);
    }
}
