//! The report: one self-contained HTML file. No scripts or fonts are fetched, so it can be
//! opened from disk, attached to a pull request or archived as it is.

use crate::analyze::Analysis;
use crate::model::Counts;
use crate::treemap::{self, Rect, HUMAN, SEEN};
use crate::util::{bytes, date, esc, now, pct, thousands};
use std::fmt::Write;

const TEMPLATE: &str = include_str!("report.html");

fn n(v: u32) -> String {
    thousands(u64::from(v))
}

fn share(part: u32, whole: u32) -> String {
    format!("{:.1}%", pct(u64::from(part), u64::from(whole)))
}

fn mini(c: Counts) -> String {
    let w = |v: u32| pct(u64::from(v), u64::from(c.code));
    format!(
        r#"<span class="mini"><i style="width:{:.1}%;background:{HUMAN}"></i><i style="width:{:.1}%;background:{SEEN}"></i></span>"#,
        w(c.human),
        w(c.seen)
    )
}

pub fn files(a: &Analysis) -> Vec<(String, Counts)> {
    a.files.iter().map(|f| (f.path.clone(), f.n)).collect()
}

pub fn report(a: &Analysis) -> String {
    let t = a.total;
    let map = treemap::layout(
        &files(a),
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 660.0,
        },
    );

    let mut queue = String::new();
    for (i, f) in a.queue().iter().take(40).enumerate() {
        let why = if f.reasons.is_empty() {
            String::new()
        } else {
            format!(r#"<div class="why">{}</div>"#, esc(&f.reasons.join(" · ")))
        };
        let _ = write!(
            queue,
            r#"<tr><td class="num">{}</td><td class="path">{}{why}</td><td class="num">{}</td><td class="num">{}{}</td></tr>"#,
            i + 1,
            esc(&f.path),
            n(f.n.unread),
            n(f.n.code),
            mini(f.n)
        );
    }
    if queue.is_empty() {
        queue.push_str(r#"<tr><td colspan="4">Nothing unread. Every agent-written line here has been seen by a person.</td></tr>"#);
    }

    let mut agents = String::new();
    for (name, written, unread) in a.by_agent() {
        let _ = write!(
            agents,
            r#"<tr><td>{}</td><td class="num">{}</td><td class="num">{}</td></tr>"#,
            esc(&name),
            n(written),
            n(unread)
        );
    }
    if agents.is_empty() {
        agents.push_str(r#"<tr><td colspan="3">No agent-written code found.</td></tr>"#);
    }

    let e = &a.evidence;
    let sessions: usize = e.sessions.values().sum();
    let b = a.by_basis();
    let mut rows: Vec<(String, String)> = Vec::new();
    let per: Vec<String> = e.sessions.iter().map(|(k, v)| format!("{k} {v}")).collect();
    rows.push((
        "Session logs".into(),
        if sessions > 0 {
            format!(
                "{} {} edited this repository{} ({})",
                n(sessions as u32),
                if sessions == 1 { "session" } else { "sessions" },
                e.since
                    .map(|s| format!(" since {}", date(s)))
                    .unwrap_or_default(),
                per.join(", ")
            )
        } else {
            "none for this repository on this machine".into()
        },
    ));
    if e.log_files > 0 {
        rows.push((
            "Scanned".into(),
            format!(
                "{} log files, {}",
                n(e.log_files as u32),
                bytes(e.log_bytes)
            ),
        ));
    }
    rows.push((
        "Commits".into(),
        format!(
            "{} of {} behind this code credit an agent",
            n(e.ai_commits as u32),
            n(a.commits.len() as u32)
        ),
    ));
    rows.push((
        "Repository".into(),
        format!(
            "ledger {} lines, read marks {} lines, claims {} lines",
            n(e.ledger_lines as u32),
            n(e.read_lines as u32),
            n(e.claimed_lines as u32)
        ),
    ));
    rows.push((
        "Agent lines".into(),
        format!(
            "{} by logged patch, {} by logged shell command, {} by ledger, {} by commit marker",
            n(b.log),
            n(b.shell),
            n(b.ledger),
            n(b.commit)
        ),
    ));
    if let Some(reviewed) = e.reviewed_commits {
        rows.push((
            "Pull requests".into(),
            format!("{} commits reviewed by another person", n(reviewed as u32)),
        ));
    }
    let seen_by: Vec<String> = a
        .by_seen()
        .iter()
        .map(|(s, c)| format!("{} {}", s.label(), n(*c)))
        .collect();
    if !seen_by.is_empty() {
        rows.push(("Seen by".into(), seen_by.join(", ")));
    }
    if a.skipped > 0 {
        rows.push((
            "Not counted".into(),
            format!(
                "{} tracked files (data, docs, lockfiles, vendored, binary)",
                n(a.skipped as u32)
            ),
        ));
    }
    let evidence: String = rows
        .iter()
        .map(|(k, v)| format!("<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v)))
        .collect();

    let head = match (&a.head, &a.base) {
        (h, Some(b)) if !h.is_empty() => format!(" @ {h}, lines added since {b}"),
        (h, None) if !h.is_empty() => format!(" @ {h}"),
        _ => String::new(),
    };
    TEMPLATE
        .replace("{{NAME}}", &esc(&a.name))
        .replace("{{HEAD}}", &esc(&head))
        .replace("{{PCT}}", &format!("{:.1}%", t.unread_pct()))
        .replace("{{UNREAD}}", &n(t.unread))
        .replace("{{CODE}}", &n(t.code))
        .replace("{{HUMAN}}", &n(t.human))
        .replace("{{SEEN}}", &n(t.seen))
        .replace("{{HUMAN_PCT}}", &share(t.human, t.code))
        .replace("{{SEEN_PCT}}", &share(t.seen, t.code))
        .replace("{{MAP}}", &treemap::svg_body(&map, true))
        .replace("{{QUEUE}}", &queue)
        .replace("{{AGENTS}}", &agents)
        .replace("{{EVIDENCE}}", &evidence)
        .replace("{{DATE}}", &date(now()))
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
}
