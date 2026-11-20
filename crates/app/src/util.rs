//! Formatting helpers with no UI.

/// `2026-11-19T22:37:42Z` → Unix seconds (UTC; offsets other than `Z` are applied).
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, u32, u32) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let (time, offset) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0)
    } else if let Some(i) = rest.rfind(['+', '-']) {
        let (t, o) = rest.split_at(i);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let (oh, om) = o[1..].split_once(':')?;
        (t, sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60))
    } else {
        (rest, 0)
    };
    let mut t = time.split(':');
    let (hh, mm): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let ss: f64 = t.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss as i64 - offset)
}

/// Howard Hinnant's algorithm: (year, month, day) → days since 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// How long ago `secs` was, from `now`: "3 days ago".
pub fn ago(secs: i64, now: i64) -> String {
    let d = (now - secs).max(0);
    match d {
        0..60 => i18n::t!("app.ago.now"),
        60..3600 => i18n::tn!("app.ago.minutes", d / 60),
        3600..86_400 => i18n::tn!("app.ago.hours", d / 3600),
        86_400..2_592_000 => i18n::tn!("app.ago.days", d / 86_400),
        2_592_000..31_536_000 => i18n::tn!("app.ago.months", d / 2_592_000),
        _ => i18n::tn!("app.ago.years", d / 31_536_000),
    }
}

/// 1_234_567 → "1.2 MB" (units from the catalog: fr "Mo").
pub fn bytes(n: u64) -> String {
    const K: f64 = 1024.;
    let f = n as f64;
    if f < K * K {
        i18n::t!("app.size.kb", n = format!("{:.0}", (f / K).ceil()))
    } else if f < K * K * K {
        i18n::t!("app.size.mb", n = format!("{:.1}", f / K / K))
    } else {
        i18n::t!("app.size.gb", n = format!("{:.1}", f / K / K / K))
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_sizes() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2026-11-19T22:37:42Z"), Some(1_795_127_862));
        assert_eq!(parse_iso("2026-11-19T17:37:42-05:00"), Some(1_795_127_862));
        assert_eq!(parse_iso("2000-02-29T00:00:00.5Z"), Some(951_782_400));
        assert_eq!(parse_iso("yesterday"), None);
        i18n::set_locale("en");
        assert_eq!(ago(100, 100 + 3 * 86_400), "3 days ago");
        assert_eq!(ago(100, 130), "just now");
        assert_eq!(bytes(19_608_733), "18.7 MB");
        assert_eq!(bytes(500), "1 KB");
    }
}
