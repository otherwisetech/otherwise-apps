//! Seat counting (SPEC §12.6): distinct members with an authenticated document request in the
//! last 30 days, against the team tier's maximum. Counted and logged here only: nothing leaves
//! the computer (the Vcity licence API doesn't exist yet) and nobody is made read-only.
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const WINDOW_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    pub first: String,
    pub last: String,
    /// Device → last day.
    #[serde(default)]
    pub devices: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seats {
    #[serde(default)]
    pub members: BTreeMap<String, Seen>,
    /// First day above the tier, while above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_since: Option<String>,
    /// The count last logged (a line is logged when it changes).
    #[serde(default)]
    pub last_logged: Option<u32>,
}

fn day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn parse(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

impl Seats {
    pub fn path(server_dir: &Path) -> PathBuf {
        server_dir.join("seats.json")
    }

    pub fn load(server_dir: &Path) -> Seats {
        std::fs::read_to_string(Self::path(server_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, server_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(server_dir)?;
        otherwise_link::fsutil::write_atomic(&Self::path(server_dir), &serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        crate::config::private_file(&Self::path(server_dir))
    }

    /// A request by `login` from `device` on `today`. Whether anything changed (to save).
    pub fn record(&mut self, login: &str, device: &str, today: NaiveDate) -> bool {
        let d = day(today);
        let s = self.members.entry(login.to_ascii_lowercase()).or_insert_with(|| Seen { first: d.clone(), last: d.clone(), devices: BTreeMap::new() });
        let changed = s.last != d || s.devices.get(device) != Some(&d);
        s.last = d.clone();
        s.devices.insert(device.into(), d);
        changed
    }

    /// Members active in the window. `members`: the current members (a removed one frees the
    /// seat at once).
    pub fn active(&self, members: &[String], today: NaiveDate) -> Vec<String> {
        let from = today - chrono::Duration::days(WINDOW_DAYS - 1);
        self.members.iter().filter(|(l, s)| members.iter().any(|m| m.eq_ignore_ascii_case(l)) && parse(&s.last).is_some_and(|d| d >= from)).map(|(l, _)| l.clone()).collect()
    }

    /// Distinct devices of the active members in the window (a signal, no limit).
    pub fn devices(&self, members: &[String], today: NaiveDate) -> usize {
        let from = today - chrono::Duration::days(WINDOW_DAYS - 1);
        let active = self.active(members, today);
        self.members.iter().filter(|(l, _)| active.contains(l)).map(|(_, s)| s.devices.values().filter(|d| parse(d).is_some_and(|d| d >= from)).count()).sum()
    }

    /// Re-evaluate against `max`: (active count, a log line when the count or the over-tier
    /// state changed).
    pub fn check(&mut self, members: &[String], max: Option<u32>, today: NaiveDate) -> (u32, Option<String>) {
        let n = self.active(members, today).len() as u32;
        let over = max.is_some_and(|m| n > m);
        let was_over = self.over_since.is_some();
        if over && !was_over {
            self.over_since = Some(day(today));
        } else if !over {
            self.over_since = None;
        }
        let line = (self.last_logged != Some(n) || over != was_over).then(|| match max {
            Some(m) if over => format!("seats: {n} active members in 30 days, the tier allows {m} (over since {})", self.over_since.as_deref().unwrap_or("")),
            Some(m) => format!("seats: {n} of {m} active members in 30 days"),
            None => format!("seats: {n} active members in 30 days (no tier set)"),
        });
        self.last_logged = Some(n);
        (n, line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse(s).unwrap()
    }

    #[test]
    fn a_rolling_window_of_distinct_members() {
        let mut s = Seats::default();
        let members: Vec<String> = ["ana@x.com", "bob@x.com", "eve@x.com", "dan@x.com"].iter().map(|s| s.to_string()).collect();
        assert!(s.record("ana@x.com", "laptop", d("2026-10-01")));
        assert!(!s.record("ANA@x.com", "laptop", d("2026-10-01")), "same day, same device: nothing to save");
        s.record("ana@x.com", "phone", d("2026-11-20"));
        s.record("bob@x.com", "pc", d("2026-11-21"));
        s.record("eve@x.com", "pc", d("2026-11-22"));
        let (n, line) = s.check(&members, Some(3), d("2026-11-24"));
        assert_eq!(n, 3);
        assert!(line.unwrap().contains("3 of 3"));
        assert_eq!(s.check(&members, Some(3), d("2026-11-24")).1, None, "logged once");
        s.record("dan@x.com", "pc", d("2026-11-24"));
        let (n, line) = s.check(&members, Some(3), d("2026-11-24"));
        assert_eq!((n, s.over_since.as_deref()), (4, Some("2026-11-24")));
        assert!(line.unwrap().contains("over since 2026-11-24"));
        assert_eq!(s.devices(&members, d("2026-11-24")), 4, "ana's laptop is outside the window");
        // the owner removes a member: the seat is free at once
        let fewer: Vec<String> = members.iter().filter(|m| *m != "dan@x.com").cloned().collect();
        assert_eq!(s.check(&fewer, Some(3), d("2026-11-24")).0, 3);
        assert_eq!(s.over_since, None);
        // 30 days on, only recent activity counts
        assert_eq!(s.active(&members, d("2026-12-20")), vec!["bob@x.com", "dan@x.com", "eve@x.com"]);
        assert!(s.active(&members, d("2027-02-01")).is_empty());
    }
}
