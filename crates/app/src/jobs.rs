//! Install / update / uninstall jobs. They run on a background thread and report through
//! atomics that the window polls while anything is in flight.
use otherwise_core::catalog::AppEntry;
use otherwise_core::install::{self, Outcome, Removed, Request};
use otherwise_core::layout::Layout;
use otherwise_core::pipeline::{self, FetchError};
use otherwise_core::platform::Target;
use otherwise_core::process;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Update,
    Reinstall,
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Preparing,
    Downloading,
    Verifying,
    Quitting,
    Installing,
    Removing,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Downloading,
            2 => Phase::Verifying,
            3 => Phase::Quitting,
            4 => Phase::Installing,
            5 => Phase::Removing,
            _ => Phase::Preparing,
        }
    }
}

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    phase: AtomicU8,
}

impl Progress {
    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Relaxed))
    }

    fn set(&self, p: Phase) {
        self.phase.store(p as u8, Ordering::Relaxed);
    }

    /// 0..=1 while downloading, None when the size is unknown.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0., 1.))
    }
}

pub struct Job {
    pub kind: Kind,
    pub progress: Arc<Progress>,
    pub cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn new(kind: Kind) -> Job {
        Job { kind, progress: Arc::new(Progress::default()), cancel: Arc::new(AtomicBool::new(false)) }
    }

    /// Cancelling is possible until the files start moving.
    pub fn cancellable(&self) -> bool {
        matches!(self.progress.phase(), Phase::Preparing | Phase::Downloading | Phase::Verifying)
    }
}

pub struct Spec {
    pub entry: AppEntry,
    pub layout: Layout,
    pub target: Target,
    pub existing: Option<PathBuf>,
    /// The app is running and the user agreed to quit it.
    pub quit_first: bool,
    /// Windows self-update (see `install::Request::overlay`).
    pub overlay: bool,
}

#[derive(Debug)]
pub enum JobError {
    Cancelled,
    Failed(String),
}

impl From<FetchError> for JobError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Net(otherwise_core::net::NetError::Cancelled) => JobError::Cancelled,
            other => JobError::Failed(other.to_string()),
        }
    }
}

/// Download → verify → (quit) → install. Blocking.
pub fn install(spec: &Spec, progress: &Progress, cancel: &AtomicBool) -> Result<Outcome, JobError> {
    let e = &spec.entry;
    progress.set(Phase::Preparing);
    let plan = pipeline::plan(e, spec.target).ok_or_else(|| JobError::Failed(i18n::t!("app.error.no_build")))?;
    let digest = pipeline::fresh_digest(e, &plan);
    if cancel.load(Ordering::Relaxed) {
        return Err(JobError::Cancelled);
    }
    progress.set(Phase::Downloading);
    progress.total.store(plan.asset.size, Ordering::Relaxed);
    let downloads = otherwise_core::layout::cache_dir().join("downloads").join(format!("{}-{}", e.id, plan.tag));
    let on_progress = |done: u64, total: u64| {
        progress.done.store(done, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
        if total > 0 && done >= total {
            progress.set(Phase::Verifying);
        }
    };
    let (archive, sha) = pipeline::fetch_verified(&plan, digest.as_deref(), &downloads, &on_progress, cancel)?;
    let cleanup = || {
        // our own download folder, nothing else lives there
        let _ = std::fs::remove_dir_all(&downloads);
    };
    if cancel.load(Ordering::Relaxed) {
        cleanup();
        return Err(JobError::Cancelled);
    }
    if spec.quit_first
        && let Some(path) = &spec.existing
    {
        progress.set(Phase::Quitting);
        if !process::quit(spec.layout.os, e, path, Duration::from_secs(20)) {
            cleanup();
            return Err(JobError::Failed(i18n::t!("app.error.still_running", app = e.name)));
        }
    }
    progress.set(Phase::Installing);
    let req = Request {
        entry: e,
        layout: &spec.layout,
        archive: &archive,
        tag: &plan.tag,
        asset: &plan.asset.name,
        sha256: &sha,
        existing: spec.existing.as_deref(),
        overlay: spec.overlay,
        now: crate::util::now().max(0) as u64,
    };
    let out = install::install(&req, &install::system_trash).map_err(|e| JobError::Failed(e.to_string()));
    cleanup();
    out
}

/// Quit (if agreed) → move to the Trash. Blocking.
pub fn uninstall(spec: &Spec, progress: &Progress) -> Result<Removed, JobError> {
    let e = &spec.entry;
    let Some(path) = &spec.existing else { return Err(JobError::Failed(i18n::t!("app.error.not_installed"))) };
    if spec.quit_first {
        progress.set(Phase::Quitting);
        if !process::quit(spec.layout.os, e, path, Duration::from_secs(20)) {
            return Err(JobError::Failed(i18n::t!("app.error.still_running", app = e.name)));
        }
    }
    progress.set(Phase::Removing);
    install::uninstall(e, &spec.layout, path, &install::system_trash).map_err(|e| JobError::Failed(e.to_string()))
}
