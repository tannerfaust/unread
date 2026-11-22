//! Codex session logs: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Edits appear in three shapes across Codex versions, and all three are read:
//!
//! * an `item_completed` event whose item is a `FileChange` with a unified diff per path
//! * a `patch_apply_begin` event with `add` / `update` changes per path
//! * an `apply_patch` tool call carrying a `*** Begin Patch` envelope
//!
//! Codex applies patches inside its workspace without asking, so an edit only counts as seen
//! when the log holds an explicit patch approval request right before it.

use super::{lines_with, searcher, skeletons, RawEdit};
use crate::model::Seen;
use crate::patch::{added_from_unified, parse_apply_patch};
use crate::util::parse_ts;
use aho_corasick::AhoCorasick;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;

pub fn parse(data: &[u8]) -> Vec<RawEdit> {
    static NEEDLES: OnceLock<AhoCorasick> = OnceLock::new();
    let needles = NEEDLES.get_or_init(|| {
        searcher(&[
            b"\"session_meta\"",
            b"\"turn_context\"",
            b"apply_patch_approval_request",
            b"\"FileChange\"",
            b"\"patch_apply_begin\"",
            b"\"apply_patch\"",
            b"\"CommandExecution\"",
        ])
    });
    let mut out: Vec<RawEdit> = Vec::new();
    let mut session = String::new();
    let mut cwd = String::new();
    let mut approved = false;
    for (needle, line) in lines_with(data, needles) {
        if needle == 2 {
            approved = true;
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let payload = &v["payload"];
        let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "session_meta" => {
                session = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                cwd = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                continue;
            }
            "turn_context" => {
                if let Some(c) = payload.get("cwd").and_then(Value::as_str) {
                    cwd = c.to_string();
                }
                continue;
            }
            _ => {}
        }
        let Some(ts) = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_ts)
        else {
            continue;
        };
        let mut found: Vec<(String, Vec<String>)> = Vec::new();
        match payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "item_completed" => {
                let item = &payload["item"];
                let done = item
                    .get("status")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s == "completed");
                match item.get("type").and_then(Value::as_str) {
                    Some("FileChange") if done => found = changes(&item["changes"]),
                    // What the agent typed into a shell; see `RawEdit::shell`.
                    Some("CommandExecution") => {
                        let script = item
                            .get("command")
                            .and_then(Value::as_array)
                            .and_then(|c| c.last())
                            .and_then(Value::as_str);
                        if let Some(script) = script.filter(|s| super::writes_text(s)) {
                            out.push(RawEdit {
                                agent: "codex".to_string(),
                                session: session.clone(),
                                ts,
                                path: item
                                    .get("cwd")
                                    .and_then(Value::as_str)
                                    .unwrap_or(&cwd)
                                    .to_string(),
                                shell: true,
                                seen: None,
                                added: skeletons(script.lines()),
                            });
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            "patch_apply_begin" => found = changes(&payload["changes"]),
            "custom_tool_call" | "function_call" | "local_shell_call"
                if payload.get("name").and_then(Value::as_str) == Some("apply_patch") =>
            {
                if let Some(text) = envelope(payload) {
                    found = parse_apply_patch(&text)
                        .into_iter()
                        .map(|(p, lines)| (p, skeletons(lines.iter().map(String::as_str))))
                        .collect();
                }
            }
            _ => continue,
        }
        let seen = std::mem::take(&mut approved).then_some(Seen::Approved);
        for (path, added) in found {
            if added.is_empty() {
                continue;
            }
            let path = if Path::new(&path).is_absolute() || cwd.is_empty() {
                path
            } else {
                format!("{}/{}", cwd.trim_end_matches('/'), path)
            };
            out.push(RawEdit {
                agent: "codex".to_string(),
                session: session.clone(),
                ts,
                path,
                shell: false,
                seen,
                added,
            });
        }
    }
    out
}

/// `{path: change}` where a change holds a unified diff or the full content of a new file,
/// either directly or one level down (`{"update": {"unified_diff": ..}}`).
fn changes(map: &Value) -> Vec<(String, Vec<String>)> {
    let Some(map) = map.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (path, change) in map {
        let direct = std::iter::once(change);
        let nested = change
            .as_object()
            .into_iter()
            .flat_map(|o| o.values())
            .filter(|v| v.is_object());
        let mut added = Vec::new();
        for c in direct.chain(nested) {
            if let Some(diff) = c.get("unified_diff").and_then(Value::as_str) {
                added = skeletons(added_from_unified(diff));
            } else if let Some(content) = c.get("content").and_then(Value::as_str) {
                added = skeletons(content.lines());
            }
            if !added.is_empty() {
                break;
            }
        }
        // A rename writes its lines to the new path.
        let target = change
            .get("move_path")
            .and_then(Value::as_str)
            .unwrap_or(path);
        out.push((target.to_string(), added));
    }
    out
}

/// The patch text of an `apply_patch` call, wherever this Codex version put it.
fn envelope(payload: &Value) -> Option<String> {
    if let Some(input) = payload.get("input").and_then(Value::as_str) {
        return Some(input.to_string());
    }
    let args = payload.get("arguments").and_then(Value::as_str)?;
    if args.contains("*** Begin Patch") && !args.trim_start().starts_with('{') {
        return Some(args.to_string());
    }
    let parsed: Value = serde_json::from_str(args).ok()?;
    find_patch(&parsed)
}

fn find_patch(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if s.contains("*** Begin Patch") => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(find_patch),
        Value::Object(o) => o.values().find_map(find_patch),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn log(entries: &[Value]) -> Vec<u8> {
        entries
            .iter()
            .map(|e| e.to_string() + "\n")
            .collect::<String>()
            .into_bytes()
    }

    fn meta() -> Value {
        json!({"timestamp": "2026-10-26T23:29:39Z", "type": "session_meta", "payload": {"id": "sess-1", "cwd": "/work/repo"}})
    }

    #[test]
    fn file_change_items() {
        let change = json!({"timestamp": "2026-10-26T23:30:00Z", "type": "event_msg", "payload": {"type": "item_completed",
            "item": {"type": "FileChange", "status": "completed", "changes": {
                "/work/repo/a.js": {"type": "update", "unified_diff": "@@ -1 +1,2 @@\n const a = 1\n+export const total = sum(a, b)\n", "move_path": null},
                "/work/repo/b.js": {"type": "add", "content": "export default function main() {}\n"}}}}});
        let mut edits = parse(&log(&[meta(), change]));
        edits.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].session, "sess-1");
        assert_eq!(edits[0].added, vec!["exportconsttotal=sum(ab)"]);
        assert_eq!(edits[1].added, vec!["exportdefaultfunctionmain(){}"]);
        assert_eq!(edits[0].seen, None);
    }

    #[test]
    fn apply_patch_call_with_relative_path_and_approval() {
        let ask = json!({"timestamp": "2026-10-26T23:30:00Z", "type": "event_msg", "payload": {"type": "apply_patch_approval_request"}});
        let call = json!({"timestamp": "2026-10-26T23:30:05Z", "type": "response_item", "payload": {"type": "custom_tool_call",
            "name": "apply_patch", "input": "*** Begin Patch\n*** Add File: src/new.py\n+value = compute()\n*** End Patch\n"}});
        let edits = parse(&log(&[meta(), ask, call]));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, "/work/repo/src/new.py");
        assert_eq!(edits[0].seen, Some(Seen::Approved));
    }

    #[test]
    fn older_patch_apply_begin_events() {
        let ev = json!({"timestamp": "2025-09-01T10:00:00Z", "type": "event_msg", "payload": {"type": "patch_apply_begin",
            "changes": {"/work/repo/x.go": {"update": {"unified_diff": "@@\n-old\n+result := run(ctx)\n"}}}}});
        let edits = parse(&log(&[meta(), ev]));
        assert_eq!(edits[0].added, vec!["result:=run(ctx)"]);
    }
}
