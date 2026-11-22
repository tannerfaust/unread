//! Terminal output: the summary, the per-line view, the evidence listing.

use crate::analyze::{Analysis, FileReport};
use crate::model::{Basis, Counts, Line, Status};
use crate::util::{bytes, date, pct, thousands, Paint};
use std::fmt::Write;

const BAR: usize = 50;

fn n(v: u32) -> String {
    thousands(u64::from(v))
}

fn share(part: u32, whole: u32) -> String {
    format!("{:.1}%", pct(u64::from(part), u64::from(whole)))
}

/// Shorten a path by dropping middle directories: the top directory and the file name are
/// what a reader looks for.
pub fn fit(path: &str, width: usize) -> String {
    let len = path.chars().count();
    if len <= width {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 2 {
        let head = parts[0];
        let mut tail: Vec<&str> = Vec::new();
        let mut used = head.chars().count() + 3; // "head/…/"
        for part in parts[1..].iter().rev() {
            let need = part.chars().count() + usize::from(!tail.is_empty());
            if used + need > width {
                break;
            }
            used += need;
            tail.insert(0, part);
        }
        if !tail.is_empty() {
            return format!("{head}/…/{}", tail.join("/"));
        }
    }
    let chars: Vec<char> = path.chars().collect();
    let tail: String = chars[len - width.saturating_sub(1)..].iter().collect();
    format!("…{tail}")
}

/// One bar whose three segments are exactly the three shares.
pub fn bar(c: Counts, width: usize, p: Paint) -> String {
    if c.code == 0 {
        return p.unread(&"░".repeat(width));
    }
    let cells = |v: u32| (f64::from(v) * width as f64 / f64::from(c.code)).round() as usize;
    let human = cells(c.human).min(width);
    let seen = cells(c.seen).min(width - human);
    let unread = width - human - seen;
    format!(
        "{}{}{}",
        p.human(&"█".repeat(human)),
        p.seen(&"█".repeat(seen)),
        p.unread(&"░".repeat(unread))
    )
}

pub fn summary(a: &Analysis, p: Paint) -> String {
    let mut o = String::new();
    let t = a.total;
    let head = if a.head.is_empty() {
        String::new()
    } else {
        format!(" @ {}", a.head)
    };
    let _ = writeln!(
        o,
        "\n  {} {}",
        p.bold("unread"),
        p.dim(&format!("· {}{head}", a.name))
    );
    if let Some(base) = &a.base {
        let _ = writeln!(o, "  {}", p.dim(&format!("only lines added since {base}")));
    }
    if t.code == 0 {
        let _ = writeln!(o, "\n  No code to measure here.\n");
        return o;
    }

    let _ = writeln!(
        o,
        "\n  {}",
        p.bold(&format!("{} unread", share(t.unread, t.code)))
    );
    let _ = writeln!(o, "  {}", bar(t, BAR, p));
    let _ = writeln!(
        o,
        "  {} of {} lines of code were written by an agent and never seen by a person.\n",
        n(t.unread),
        n(t.code)
    );

    let seen_by: Vec<String> = a
        .by_seen()
        .iter()
        .map(|(s, c)| format!("{} {}", s.label(), n(*c)))
        .collect();
    let claimed = a
        .files
        .iter()
        .flat_map(|f| &f.lines)
        .filter(|l| l.basis == Basis::Claimed)
        .count() as u32;
    let human_note = if claimed > 0 {
        format!("  (claimed {})", n(claimed))
    } else {
        String::new()
    };
    let seen_note = if seen_by.is_empty() {
        String::new()
    } else {
        format!("  ({})", seen_by.join(", "))
    };
    let row = |mark: String, name: &str, v: u32, what: &str| {
        format!(
            "  {mark} {name:<7}{:>9}  {:>6}   {}",
            n(v),
            share(v, t.code),
            p.dim(what)
        )
    };
    let _ = writeln!(
        o,
        "{}",
        row(
            p.human("█"),
            "human",
            t.human,
            &format!("a person wrote it{human_note}")
        )
    );
    let _ = writeln!(
        o,
        "{}",
        row(
            p.seen("█"),
            "seen",
            t.seen,
            &format!("an agent wrote it, a person saw it{seen_note}")
        )
    );
    let _ = writeln!(
        o,
        "{}",
        row(
            p.unread("░"),
            "unread",
            t.unread,
            "an agent wrote it, no sign anyone looked"
        )
    );

    let agents = a.by_agent();
    if !agents.is_empty() {
        let _ = writeln!(
            o,
            "\n  {}",
            p.dim(&format!(
                "{:<22}{:>9}{:>10}",
                "written by", "lines", "unread"
            ))
        );
        for (name, written, unread) in &agents {
            let _ = writeln!(o, "  {name:<22}{:>9}{:>10}", n(*written), n(*unread));
        }
    }

    let dirs = a.dirs(7);
    if !dirs.is_empty() {
        let _ = writeln!(
            o,
            "\n  {}",
            p.dim(&format!("{:<44}{:>8}{:>8}", "where it is", "unread", "of"))
        );
        for (dir, c) in &dirs {
            let _ = writeln!(
                o,
                "  {:<44}{:>8}{:>8}  {}",
                fit(dir, 43),
                n(c.unread),
                n(c.code),
                bar(*c, 12, p)
            );
        }
    }

    let queue = a.queue();
    if !queue.is_empty() {
        let _ = writeln!(
            o,
            "\n  {}",
            p.dim(&format!("{:<48}{:>8}", "read first", "unread"))
        );
        for (i, f) in queue.iter().take(8).enumerate() {
            let why = if f.reasons.is_empty() {
                String::new()
            } else {
                format!("  {}", p.dim(&f.reasons.join(" · ")))
            };
            let _ = writeln!(
                o,
                "  {:>2}  {:<44}{:>8}{why}",
                i + 1,
                fit(&f.path, 43),
                n(f.n.unread)
            );
        }
        if queue.len() > 8 {
            let _ = writeln!(
                o,
                "  {}",
                p.dim(&format!(
                    "    and {} more files",
                    n((queue.len() - 8) as u32)
                ))
            );
        }
    }

    let _ = writeln!(o, "\n{}", evidence(a, p));
    let _ = writeln!(
        o,
        "  {}",
        p.dim("Unread is a lower bound: a line with no evidence either way counts as human.")
    );
    let _ = writeln!(
        o,
        "\n  {}  read the riskiest unread code, one piece at a time",
        p.bold("unread next        ")
    );
    let _ = writeln!(
        o,
        "  {}  open the map of this repository",
        p.bold("unread report      ")
    );
    let _ = writeln!(
        o,
        "  {}  see the evidence behind every line\n",
        p.bold("unread blame FILE  ")
    );
    o
}

/// Where the labels come from.
pub fn evidence(a: &Analysis, p: Paint) -> String {
    let e = &a.evidence;
    let mut o = String::new();
    let _ = writeln!(o, "  {}", p.dim("evidence"));
    let sessions: u32 = e.sessions.values().map(|v| *v as u32).sum();
    if sessions > 0 {
        let per: Vec<String> = e.sessions.iter().map(|(k, v)| format!("{k} {v}")).collect();
        let since = e
            .since
            .map(|s| format!(" since {}", date(s)))
            .unwrap_or_default();
        let _ = writeln!(
            o,
            "  session logs   {} {} edited this repository{since} ({})",
            n(sessions),
            if sessions == 1 { "session" } else { "sessions" },
            per.join(", ")
        );
    } else if e.log_files > 0 {
        let _ = writeln!(
            o,
            "  session logs   none of the {} log files touch this repository",
            n(e.log_files as u32)
        );
    } else {
        let _ = writeln!(o, "  session logs   none found on this machine");
    }
    if e.log_files > 0 {
        let _ = writeln!(
            o,
            "  {}",
            p.dim(&format!(
                "               {} log files, {} scanned",
                n(e.log_files as u32),
                bytes(e.log_bytes)
            ))
        );
    }
    let _ = writeln!(
        o,
        "  commits        {} of {} behind this code credit an agent",
        n(e.ai_commits as u32),
        n(a.commits.len() as u32)
    );
    let _ = writeln!(
        o,
        "  repository     ledger {} lines, read marks {} lines, claims {} lines (.unread/)",
        n(e.ledger_lines as u32),
        n(e.read_lines as u32),
        n(e.claimed_lines as u32)
    );
    if let Some(reviewed) = e.reviewed_commits {
        let _ = writeln!(
            o,
            "  pull requests  {} commits with unread code were reviewed by another person",
            n(reviewed as u32)
        );
    }
    let b = a.by_basis();
    let _ = writeln!(
        o,
        "  agent lines    {} by logged patch, {} by logged shell command, {} by ledger, {} by commit marker",
        n(b.log),
        n(b.shell),
        n(b.ledger),
        n(b.commit)
    );
    if a.skipped > 0 {
        let _ = writeln!(o, "  {}", p.dim(&format!("               {} tracked files not counted (data, docs, lockfiles, vendored, binary)", n(a.skipped as u32))));
    }
    o
}

/// Why a line has its label, in a few words.
pub fn reason(a: &Analysis, l: &Line) -> String {
    let agent = a
        .agents
        .get(usize::from(l.agent))
        .map(String::as_str)
        .unwrap_or("agent");
    let commit = a.commits.get(l.commit as usize);
    match l.status {
        Status::Blank => String::new(),
        Status::Human if l.basis == Basis::Claimed => {
            "claimed by a person with unread claim".to_string()
        }
        Status::Human => match commit {
            Some(c) => format!("{} {}", c.author, date(c.time)),
            None => "not committed yet".to_string(),
        },
        Status::Seen => format!(
            "{agent} · {}",
            l.seen.map(|s| s.explain()).unwrap_or_default()
        ),
        Status::Unread => match l.basis {
            Basis::Log => format!("{agent} {} · session log, nobody was asked", date(l.ts)),
            Basis::Shell => format!(
                "{agent} {} · typed in a shell command, nobody was asked",
                date(l.ts)
            ),
            Basis::Ledger => format!("{agent} · repository ledger"),
            Basis::Commit => {
                let sha = commit.map(|c| &c.sha[..c.sha.len().min(7)]).unwrap_or("");
                format!("{agent} · commit {sha} credits it")
            }
            Basis::Claimed | Basis::None => agent.to_string(),
        },
    }
}

pub fn mark(status: Status, p: Paint) -> String {
    match status {
        Status::Blank => " ".to_string(),
        Status::Human => p.human("█"),
        Status::Seen => p.seen("█"),
        Status::Unread => p.unread("░"),
    }
}

fn file_header(f: &FileReport, p: Paint) -> String {
    format!(
        "  {}  {}",
        p.bold(&f.path),
        p.dim(&format!(
            "{} lines of code · {} unread · {} seen · {} human",
            n(f.n.code),
            n(f.n.unread),
            n(f.n.seen),
            n(f.n.human)
        ))
    )
}

/// Every line of one file with its label, and the evidence at the start of each run.
pub fn blame(a: &Analysis, f: &FileReport, p: Paint) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "\n{}\n", file_header(f, p));
    let mut prev = String::new();
    for (i, (text, l)) in f.text.iter().zip(&f.lines).enumerate() {
        // The evidence is printed where it changes, not on every line.
        let mut note = String::new();
        if l.status != Status::Blank {
            let why = reason(a, l);
            if why != prev {
                note = why.clone();
                prev = why;
            }
        }
        let shown: String = text.replace('\t', "    ").chars().take(88).collect();
        let body = if l.status == Status::Unread {
            p.unread(&format!("{shown:<88}"))
        } else {
            format!("{shown:<88}")
        };
        let _ = writeln!(
            o,
            "  {} {} {body}  {}",
            p.dim(&format!("{:>5}", i + 1)),
            mark(l.status, p),
            p.dim(&note)
        );
    }
    let _ = writeln!(
        o,
        "\n  {} human   {} seen   {} unread\n",
        p.human("█"),
        p.seen("█"),
        p.unread("░")
    );
    o
}

/// One reviewable piece of unread code with two lines of context either side.
pub fn chunk(a: &Analysis, f: &FileReport, start: usize, end: usize, p: Paint) -> String {
    let mut o = String::new();
    let from = start.saturating_sub(2);
    let to = (end + 3).min(f.text.len());
    for i in from..to {
        let l = &f.lines[i];
        let shown = f.text[i].replace('\t', "    ");
        let inside = (start..=end).contains(&i);
        let body = if inside { shown } else { p.dim(&shown) };
        let _ = writeln!(
            o,
            "  {} {} {body}",
            p.dim(&format!("{:>5}", i + 1)),
            mark(l.status, p)
        );
    }
    let first = f.lines[start..=end]
        .iter()
        .find(|l| l.status == Status::Unread);
    if let Some(l) = first {
        let _ = writeln!(o, "\n  {}", p.dim(&reason(a, l)));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_segments_add_up() {
        let p = Paint { on: false };
        let c = Counts {
            code: 10,
            human: 2,
            seen: 3,
            unread: 5,
        };
        let b = bar(c, 10, p);
        assert_eq!(b.chars().count(), 10);
        assert_eq!(b.matches('█').count(), 5);
        assert_eq!(b.matches('░').count(), 5);
        assert_eq!(bar(Counts::default(), 4, p), "░░░░");
    }

    #[test]
    fn long_paths_keep_both_ends() {
        assert_eq!(
            fit("src/example_application/services/mail_view.py", 30),
            "src/…/services/mail_view.py"
        );
        assert_eq!(
            fit("src/example_application/services/mail_view.py", 20),
            "src/…/mail_view.py"
        );
        assert_eq!(fit("a/b.rs", 24), "a/b.rs");
        assert_eq!(
            fit("a_very_long_file_name_indeed.rs", 12).chars().count(),
            12
        );
    }
}
