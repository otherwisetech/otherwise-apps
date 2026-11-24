//! Nightly copies of every shared document (plan §8.2): the app's own consistent snapshot
//! (`VACUUM INTO`, `integrity_check`) into `<backups>/<app>/<doc>/<yyyy-mm-dd>.db`, kept 14
//! daily, 8 weekly and 12 monthly. Only these copies are ever removed, never a document. A
//! closed copy may go to a synced folder or a disk; a live document never should.
use chrono::{Datelike, NaiveDate};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn default_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".otherwise-backups")
}

/// The copies to keep among `days`: the newest `daily`, the newest of each of the last
/// `weekly` ISO weeks, the newest of each of the last `monthly` months.
pub fn keep(days: &[NaiveDate], daily: u32, weekly: u32, monthly: u32) -> BTreeSet<NaiveDate> {
    let mut sorted: Vec<NaiveDate> = days.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted.dedup();
    let mut keep: BTreeSet<NaiveDate> = sorted.iter().take(daily as usize).copied().collect();
    let mut weeks = BTreeSet::new();
    let mut months = BTreeSet::new();
    for d in &sorted {
        let w = (d.iso_week().year(), d.iso_week().week());
        if weeks.len() < weekly as usize && weeks.insert(w) {
            keep.insert(*d);
        }
        let m = (d.year(), d.month());
        if months.len() < monthly as usize && months.insert(m) {
            keep.insert(*d);
        }
    }
    keep
}

/// The dated copies in a document's folder (`yyyy-mm-dd.db`).
pub fn copies(dir: &Path) -> Vec<(NaiveDate, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let d = NaiveDate::parse_from_str(n.strip_suffix(".db")?, "%Y-%m-%d").ok()?;
            Some((d, e.path()))
        })
        .collect()
}

/// Remove the copies [`keep`] doesn't keep; the paths removed.
pub fn prune(dir: &Path, cfg: &crate::config::Backups) -> Vec<PathBuf> {
    let all = copies(dir);
    let k = keep(&all.iter().map(|(d, _)| *d).collect::<Vec<_>>(), cfg.keep_daily.max(1), cfg.keep_weekly, cfg.keep_monthly);
    let mut gone = Vec::new();
    for (d, p) in all {
        if !k.contains(&d) && std::fs::remove_file(&p).is_ok() {
            gone.push(p);
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_keeps_dailies_weeklies_and_monthlies() {
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let days: Vec<NaiveDate> = (0..330).map(|i| start + chrono::Duration::days(i)).collect();
        let k = keep(&days, 14, 8, 12);
        let last = *days.last().unwrap();
        assert!((0..14).all(|i| k.contains(&(last - chrono::Duration::days(i)))));
        assert_eq!(k.iter().map(|d| (d.year(), d.month())).collect::<BTreeSet<_>>().len(), 11, "every month of the year kept one");
        assert!(k.len() < 14 + 8 + 12 + 1);
        assert!(!k.contains(&(start + chrono::Duration::days(1))), "old dailies go");

        let t = tempfile::tempdir().unwrap();
        for d in &days[300..] {
            std::fs::write(t.path().join(format!("{d}.db")), b"x").unwrap();
        }
        std::fs::write(t.path().join("notes.txt"), b"keep me").unwrap();
        let gone = prune(t.path(), &crate::config::Backups::default());
        assert!(!gone.is_empty());
        assert!(t.path().join("notes.txt").exists(), "only dated copies are touched");
        assert_eq!(copies(t.path()).len(), 30 - gone.len());
    }
}
