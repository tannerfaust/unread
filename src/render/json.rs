//! Machine-readable report. The shape is versioned: fields are added, never repurposed.

use crate::analyze::Analysis;
use crate::util::{now, pct};
use serde_json::{json, Value};

pub fn report(a: &Analysis) -> Value {
    let t = a.total;
    let unread_pct = round1(pct(u64::from(t.unread), u64::from(t.code)));
    let e = &a.evidence;
    json!({
        "version": 1,
        "repo": a.name,
        "head": a.head,
        "base": a.base,
        "generated_at": now(),
        "lines": t,
        "unread_pct": unread_pct,
        "coverage_pct": if t.code == 0 { 100.0 } else { round1(100.0 - unread_pct) },
        "agents": a.by_agent().into_iter().map(|(name, written, unread)| json!({"name": name, "written": written, "unread": unread})).collect::<Vec<_>>(),
        "seen_by": a.by_seen().into_iter().map(|(s, n)| (s.label().to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
        "basis": a.by_basis(),
        "evidence": {
            "log_files": e.log_files,
            "log_bytes": e.log_bytes,
            "edits_in_logs": e.edits_total,
            "edits_to_this_repo": e.edits_here,
            "shell_commands": e.shell_commands,
            "sessions": e.sessions,
            "since": e.since,
            "agent_commits": e.ai_commits,
            "commits": a.commits.len(),
            "ledger_lines": e.ledger_lines,
            "read_lines": e.read_lines,
            "claimed_lines": e.claimed_lines,
            "reviewed_commits": e.reviewed_commits,
            "files_not_counted": a.skipped,
        },
        "files": a.files.iter().filter(|f| f.n.code > 0).map(|f| json!({
            "path": f.path,
            "code": f.n.code,
            "human": f.n.human,
            "seen": f.n.seen,
            "unread": f.n.unread,
            "risk": round1(f.risk),
            "reasons": f.reasons,
        })).collect::<Vec<_>>(),
    })
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
