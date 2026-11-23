//! The helper without its menu: which apps are installed with jobs, running the due one,
//! remembering how it went, and draining notifications — only ever for installed apps.
use crate::runner::Outcome;
use crate::sched::{self, Conditions, Decision, Wait};
use crate::state::{self, State};
use chrono::{DateTime, TimeZone, Utc};
use otherwise_link::Root;
use otherwise_link::apps::{AppSpec, Locations};
use otherwise_link::background::{self, NotRunnable, Notification, Plan, Registered};
use otherwise_link::settings::BackgroundSettings;
use otherwise_link::background::Lease;
use otherwise_link::lock::{Lock, Me};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub struct Engine {
    pub loc: Locations,
    pub specs: Vec<AppSpec>,
    pub state_dir: PathBuf,
    pub state: State,
    /// Installed apps that declare jobs, as of the last scan or run.
    pub apps: Vec<Registered>,
    /// Apps that had jobs and are gone (deleted, moved): not scheduled until a scan finds them.
    pub gone: BTreeSet<String>,
    /// Leases this computer holds (one per app per data root; see `background::Lease`).
    pub held: BTreeMap<String, Lock>,
    /// Apps whose lease is somewhere else, and why.
    pub waiting: BTreeMap<String, Wait>,
    /// Who this computer is in lease files (tests set it; None: the real one).
    pub me: Option<Me>,
    /// Lease clock (tests set it; None: now).
    pub clock: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing due (or everything waits).
    Nothing,
    Ran { app: String, job: String, outcome: Outcome },
    /// Couldn't start: spawn failed (counted as a failure).
    Failed { app: String, job: String },
    /// The app vanished between the scan and the run: dropped quietly.
    Gone { app: String },
}

impl Engine {
    pub fn new(loc: Locations, specs: Vec<AppSpec>, state_dir: PathBuf) -> Engine {
        let state = State::load(&state_dir);
        let mut e = Engine { loc, specs, state_dir, state, apps: vec![], gone: BTreeSet::new(), held: BTreeMap::new(), waiting: BTreeMap::new(), me: None, clock: None };
        e.rescan();
        e
    }

    /// Finds installed apps with jobs again; apps that disappeared go to `gone`, ones that came
    /// back (reinstalled) leave it.
    pub fn rescan(&mut self) {
        let found = background::scan_specs(&self.specs, &self.loc);
        let now: BTreeSet<String> = found.iter().map(|r| r.spec.id.clone()).collect();
        for r in &self.apps {
            if !now.contains(&r.spec.id) {
                self.gone.insert(r.spec.id.clone());
            }
        }
        self.gone.retain(|id| !now.contains(id));
        self.apps = found;
    }

    fn drop_app(&mut self, app: &str) {
        self.apps.retain(|r| r.spec.id != app);
        self.gone.insert(app.to_string());
        self.held.remove(app);
        self.waiting.remove(app);
    }

    /// Takes or refreshes this computer's lease for every installed app whose jobs are on (cheap:
    /// the lock file is rewritten at most once a minute); lets go of the others' bookkeeping.
    pub fn refresh_leases(&mut self, root: &Root, settings: &BackgroundSettings) {
        let now = self.clock.unwrap_or_else(otherwise_link::lock::now);
        let me = self.me.clone().unwrap_or_else(Me::current);
        let ids: Vec<String> = self.apps.iter().map(|r| r.spec.id.clone()).filter(|id| settings.jobs_on(id)).collect();
        self.held.retain(|id, _| ids.contains(id));
        self.waiting.retain(|id, _| ids.contains(id));
        for id in ids {
            match background::take_lease_as(root, &id, me.clone(), now) {
                Lease::Held(l) => {
                    self.waiting.remove(&id);
                    self.held.insert(id, l);
                }
                Lease::Elsewhere(host) => {
                    self.held.remove(&id);
                    self.waiting.insert(id, Wait::Elsewhere(host));
                }
                Lease::Unavailable => {
                    self.held.remove(&id);
                    self.waiting.insert(id, Wait::NoLease);
                }
            }
        }
    }

    fn lease_wait(&self, app: &str) -> Option<Wait> {
        if self.held.contains_key(app) {
            return None;
        }
        Some(self.waiting.get(app).cloned().unwrap_or(Wait::NoLease))
    }

    pub fn decisions<Tz: TimeZone>(&self, settings: &BackgroundSettings, c: &Conditions, open: &dyn Fn(&Registered) -> bool, tz: &Tz) -> Vec<Decision> {
        sched::decide(&self.apps, settings, &self.state, c, open, &|a| self.lease_wait(a), tz)
    }

    /// Runs the most overdue allowed job, if any. The install is verified right before
    /// (`background::plan`); an app that is gone is dropped silently.
    pub fn step<Tz: TimeZone>(&mut self, settings: &BackgroundSettings, c: &Conditions, open: &dyn Fn(&Registered) -> bool, run: &mut dyn FnMut(&Plan) -> std::io::Result<Outcome>, tz: &Tz) -> Step {
        let d = self.decisions(settings, c, open, tz);
        let Some((app, job)) = sched::pick(&d, &self.state, c.now, &self.apps, tz) else { return Step::Nothing };
        let Some(reg) = self.apps.iter().find(|r| r.spec.id == app).cloned() else { return Step::Nothing };
        let plan = match background::plan(&reg, &job, &self.loc) {
            Ok(p) => p,
            Err(NotRunnable::NoSuchJob) => {
                // updated: read its declaration again
                self.rescan();
                return Step::Nothing;
            }
            Err(NotRunnable::NotInstalled | NotRunnable::BadExecutable(_)) => {
                self.drop_app(&app);
                return Step::Gone { app };
            }
        };
        let k = state::key(&app, &job);
        let start = c.now;
        let result = run(&plan);
        let end = Utc::now().max(start);
        let js = self.state.jobs.entry(k).or_default();
        js.last_start = Some(start);
        js.last_end = Some(end);
        let step = match result {
            Ok(outcome) => {
                if outcome.ok {
                    js.failures = 0;
                    js.last_ok = Some(end);
                    js.error = None;
                    if let Some(r) = &outcome.report {
                        js.status = r.status.clone();
                        js.idle = r.idle;
                    } else {
                        js.idle = false;
                    }
                } else {
                    js.failures = js.failures.saturating_add(1);
                    js.error = outcome.error();
                }
                Step::Ran { app, job, outcome }
            }
            Err(_) => {
                js.failures = js.failures.saturating_add(1);
                js.error = Some("start".into());
                Step::Failed { app, job }
            }
        };
        let _ = self.state.save(&self.state_dir);
        step
    }

    /// Takes the pending notifications of the installed apps (each install re-checked) and
    /// returns the ones to show: not quiet, the app's notifications on, not shown before.
    /// Outboxes of apps that aren't installed are never read or emptied, and only the computer
    /// holding an app's lease drains it (the one that ran its jobs).
    pub fn notifications(&mut self, root: &Root, settings: &BackgroundSettings, now: DateTime<Utc>) -> Vec<Notification> {
        let mut out = vec![];
        let ids: Vec<String> = self.apps.iter().map(|r| r.spec.id.clone()).filter(|id| self.held.contains_key(id)).collect();
        let mut changed = false;
        for id in ids {
            if !background::pending(root, &id) {
                continue;
            }
            let Some(spec) = self.specs.iter().find(|s| s.id == id) else { continue };
            if background::registered(spec, &self.loc).is_none() {
                self.drop_app(&id);
                continue;
            }
            for n in background::drain(root, &id, now) {
                if settings.notifications_on(&id) && self.state.mark_shown(&n.key(), now) {
                    changed = true;
                    out.push(n);
                }
            }
        }
        if changed {
            let _ = self.state.save(&self.state_dir);
        }
        out
    }

    pub fn pause(&mut self, until: Option<DateTime<Utc>>) {
        self.state.paused_until = until;
        let _ = self.state.save(&self.state_dir);
    }
}
