//! Tiny settings file (`settings.json` in the launcher's config dir): a flat JSON object of
//! strings and string lists, parsed by hand (same format as the other apps).
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub language: Option<String>,
    /// `"light"`, `"dark"`, else follows the system.
    pub theme: Option<String>,
    /// Hours between automatic update checks; `"0"` turns them off. Default [`DEFAULT_HOURS`].
    pub check_hours: Option<String>,
    /// Windows / Linux install folder; the platform default when unset.
    pub install_root: Option<String>,
    /// `"list"`, else the grid.
    pub view: Option<String>,
    /// `"updated"`, `"installed"`, else by name.
    pub sort: Option<String>,
    /// `"off"` silences the "updates available" notification.
    pub notify: Option<String>,
    /// Unix seconds of the last successful check.
    pub last_check: Option<String>,
    /// `id@tag` pairs already announced in a notification, so each update is announced once.
    pub notified: Vec<String>,
}

pub const DEFAULT_HOURS: u64 = 6;
pub const HOUR_CHOICES: [u64; 6] = [0, 1, 3, 6, 12, 24];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    S(String),
    L(Vec<String>),
}

fn file() -> Option<PathBuf> {
    Some(otherwise_core::layout::config_dir().join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| Settings::parse(&s)).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, self.to_json());
    }

    /// Load, change, save: keeps whatever else is on disk.
    pub fn update(f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = Settings::load();
        f(&mut s);
        s.save();
        s
    }

    fn parse(s: &str) -> Settings {
        let m = parse_object(s).unwrap_or_default();
        let st = |k: &str| match m.get(k) {
            Some(Val::S(v)) if !v.is_empty() => Some(v.clone()),
            _ => None,
        };
        let list = |k: &str| match m.get(k) {
            Some(Val::L(v)) => v.clone(),
            _ => Vec::new(),
        };
        Settings {
            language: st("language"),
            theme: st("theme"),
            check_hours: st("check_hours"),
            install_root: st("install_root"),
            view: st("view"),
            sort: st("sort"),
            notify: st("notify"),
            last_check: st("last_check"),
            notified: list("notified"),
        }
    }

    fn to_json(&self) -> String {
        let strings = [
            ("language", &self.language),
            ("theme", &self.theme),
            ("check_hours", &self.check_hours),
            ("install_root", &self.install_root),
            ("view", &self.view),
            ("sort", &self.sort),
            ("notify", &self.notify),
            ("last_check", &self.last_check),
        ];
        let lists = [("notified", &self.notified)];
        let mut fields: Vec<String> = strings.iter().filter_map(|(k, v)| v.as_ref().map(|v| format!("  \"{k}\": {}", quote(v)))).collect();
        fields.extend(lists.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("  \"{k}\": [{}]", v.iter().map(|x| quote(x)).collect::<Vec<_>>().join(", "))));
        if fields.is_empty() { "{}\n".into() } else { format!("{{\n{}\n}}\n", fields.join(",\n")) }
    }

    pub fn check_hours(&self) -> u64 {
        self.check_hours.as_deref().and_then(|h| h.parse().ok()).unwrap_or(DEFAULT_HOURS)
    }

    pub fn notify(&self) -> bool {
        self.notify.as_deref() != Some("off")
    }

    pub fn last_check(&self) -> Option<i64> {
        self.last_check.as_deref().and_then(|s| s.parse().ok())
    }

    pub fn install_root(&self) -> Option<PathBuf> {
        self.install_root.as_deref().filter(|s| !s.trim().is_empty()).map(PathBuf::from)
    }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A flat object of strings and arrays of strings; anything else is skipped or fails.
fn parse_object(s: &str) -> Option<BTreeMap<String, Val>> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let ws = |i: &mut usize| {
        while *i < c.len() && c[*i].is_whitespace() {
            *i += 1;
        }
    };
    fn string(c: &[char], i: &mut usize) -> Option<String> {
        if c.get(*i) != Some(&'"') {
            return None;
        }
        *i += 1;
        let mut out = String::new();
        loop {
            let ch = *c.get(*i)?;
            *i += 1;
            match ch {
                '"' => return Some(out),
                '\\' => {
                    let e = *c.get(*i)?;
                    *i += 1;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'u' => {
                            let hex: String = c.get(*i..*i + 4)?.iter().collect();
                            *i += 4;
                            out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                        }
                        other => out.push(other),
                    }
                }
                ch => out.push(ch),
            }
        }
    }
    let mut m = BTreeMap::new();
    ws(&mut i);
    if c.get(i) != Some(&'{') {
        return None;
    }
    i += 1;
    loop {
        ws(&mut i);
        match c.get(i) {
            Some('}') => return Some(m),
            Some(',') => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let k = string(&c, &mut i)?;
        ws(&mut i);
        if c.get(i) != Some(&':') {
            return None;
        }
        i += 1;
        ws(&mut i);
        let v = if c.get(i) == Some(&'[') {
            i += 1;
            let mut l = Vec::new();
            loop {
                ws(&mut i);
                match c.get(i) {
                    Some(']') => {
                        i += 1;
                        break;
                    }
                    Some(',') => i += 1,
                    _ => l.push(string(&c, &mut i)?),
                }
            }
            Val::L(l)
        } else {
            Val::S(string(&c, &mut i)?)
        };
        m.insert(k, v);
    }
}

fn system_locale() -> Option<String> {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
            && v != "C"
            && v != "POSIX"
        {
            return Some(v);
        }
    }
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output().ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

/// Saved language, else the system one if bundled, else the default.
pub fn initial_language(settings: &Settings) -> &'static str {
    let saved = settings.language.as_deref().and_then(|l| i18n::available().into_iter().map(|(c, _)| c).find(|c| *c == l));
    saved.or_else(|| system_locale().as_deref().and_then(i18n::match_system)).unwrap_or(i18n::DEFAULT)
}


#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn roundtrip() {
        let s = Settings { language: Some("en".into()), ..Default::default() };
        assert_eq!(Settings::parse(&s.to_json()), s);
        let s = Settings {
            language: Some("pt".into()),
            theme: Some("dark".into()),
            check_hours: Some("12".into()),
            install_root: Some("/home/ana \"x\"/apps".into()),
            view: Some("list".into()),
            sort: Some("updated".into()),
            notify: Some("off".into()),
            last_check: Some("1795000000".into()),
            notified: vec!["rama@v0.2.0".into(), "diskmap@v0.4.0".into()],
        };
        assert_eq!(Settings::parse(&s.to_json()), s);
        assert_eq!(s.check_hours(), 12);
        assert!(!s.notify());
        assert_eq!(s.last_check(), Some(1_795_000_000));
        assert_eq!(Settings::default().check_hours(), super::DEFAULT_HOURS);
        assert!(Settings::default().notify());
        assert_eq!(Settings::parse(&Settings::default().to_json()), Settings::default());
        assert_eq!(Settings::parse("garbage"), Settings::default());
    }
}
