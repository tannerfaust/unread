//! Small helpers: timestamps, number formatting, terminal colour.

use std::io::IsTerminal;

/// RFC 3339 timestamp to unix seconds. Agent logs write `2026-10-07T22:10:15.083Z`.
pub fn parse_ts(s: &str) -> Option<i64> {
    if s.len() < 19 || !s.is_char_boundary(19) {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut t = days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + se;
    let rest = s[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let sign = rest.chars().next();
    if matches!(sign, Some('+') | Some('-')) {
        let oh: i64 = rest.get(1..3)?.parse().ok()?;
        let om: i64 = rest.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
        let off = oh * 3_600 + om * 60;
        t += if sign == Some('+') { -off } else { off };
    }
    Some(t)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix seconds to `YYYY-MM-DD` (UTC).
pub fn date(ts: i64) -> String {
    let z = ts.div_euclid(86_400) + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `12345` to `12,345`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Minimal XML/HTML text escaping.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Terminal colour that switches itself off for pipes and `NO_COLOR`.
#[derive(Clone, Copy)]
pub struct Paint {
    pub on: bool,
}

impl Paint {
    pub fn auto() -> Paint {
        let on = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Paint { on }
    }

    fn wrap(self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn bold(self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(self, s: &str) -> String {
        self.wrap("38;5;244", s)
    }
    /// Blue: a person wrote it.
    pub fn human(self, s: &str) -> String {
        self.wrap("38;5;117", s)
    }
    /// Green: AI wrote it, a person read it.
    pub fn seen(self, s: &str) -> String {
        self.wrap("38;5;77", s)
    }
    /// Orange: AI wrote it, nobody read it.
    pub fn unread(self, s: &str) -> String {
        self.wrap("38;5;208", s)
    }
    pub fn alert(self, s: &str) -> String {
        self.wrap("1;38;5;203", s)
    }
    pub fn ok(self, s: &str) -> String {
        self.wrap("1;38;5;114", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip() {
        assert_eq!(parse_ts("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_ts("2026-10-07T22:10:15.083Z"), Some(1_791_411_015));
        assert_eq!(parse_ts("2026-10-08T00:10:15+02:00"), Some(1_791_411_015));
        assert_eq!(date(1_791_411_015), "2026-10-07");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(parse_ts("nonsense"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(pct(1, 4), 25.0);
        assert_eq!(pct(1, 0), 0.0);
    }
}
