//! Claude Code session logs: `~/.claude/projects/<project>/**/*.jsonl`.
//!
//! Every file edit is logged with the patch it applied:
//!
//! * `toolUseResult.structuredPatch`  Edit and Write to an existing file
//! * `toolUseResult.content`          Write of a new file (`type: "create"`)
//! * `toolUseResult.bashEditDiff`     files changed by a shell command
//!
//! and with how it was allowed. `permissionDecision.source` starting with `user` means a
//! person answered a prompt that showed the diff. Anything else (a mode, a rule, a classifier,
//! a hook) means nobody was asked. `userModified` means the person changed the proposal.

use super::{lines_with, searcher, skeletons, RawEdit};
use crate::model::Seen;
use crate::patch::added_from_hunk_lines;
use crate::util::parse_ts;
use aho_corasick::AhoCorasick;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

const MODE: &[u8] = b"\"permissionMode\":\"";
const TOOL_ID: &[u8] = b"\"tool_use_id\":\"";
const DECISION: &[u8] = b"\"permissionDecision\":";

pub fn parse(data: &[u8]) -> Vec<RawEdit> {
    static NEEDLES: OnceLock<AhoCorasick> = OnceLock::new();
    let needles = NEEDLES.get_or_init(|| {
        searcher(&[
            MODE,
            b"\"structuredPatch\"",
            b"\"bashEditDiff\"",
            b"\"name\":\"Bash\"",
            DECISION,
        ])
    });
    let mut out: Vec<RawEdit> = Vec::new();
    // Logs written before `permissionDecision` existed only carry the session's mode.
    let mut mode: Option<String> = None;
    // Shell commands waiting for the result line that says how they were allowed.
    let mut pending: HashMap<String, usize> = HashMap::new();
    for (needle, line) in lines_with(data, needles) {
        match needle {
            0 => {
                if let Some(value) = after(line, MODE) {
                    mode = Some(value);
                }
            }
            1 | 2 => {
                if let Ok(v) = serde_json::from_slice::<Value>(line) {
                    entry(&v, mode.as_deref(), &mut out);
                }
            }
            3 => {
                if let Ok(v) = serde_json::from_slice::<Value>(line) {
                    shell(&v, &mut out, &mut pending);
                }
            }
            _ => {
                // Only the two fields that matter are read; tool output is never parsed.
                let Some(i) = after(line, TOOL_ID).and_then(|id| pending.remove(&id)) else {
                    continue;
                };
                let at =
                    memchr::memmem::find(line, DECISION).map_or(line.len(), |p| p + DECISION.len());
                let decision = serde_json::Deserializer::from_slice(&line[at..])
                    .into_iter::<Value>()
                    .next();
                if let Some(Ok(d)) = decision {
                    out[i].seen = asked_a_person(&d).then_some(Seen::Approved);
                }
            }
        }
    }
    // A line holding both a patch and a shell diff was visited twice.
    out.dedup_by(|a, b| a.ts == b.ts && a.path == b.path && a.added == b.added);
    out
}

/// The JSON string value that follows `key` on this line.
fn after(line: &[u8], key: &[u8]) -> Option<String> {
    let rest = &line[memchr::memmem::find(line, key)? + key.len()..];
    let end = memchr::memchr(b'"', rest)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// A person answered a prompt, as opposed to a mode, a rule, a classifier or a hook.
fn asked_a_person(decision: &Value) -> bool {
    decision.get("decision").and_then(Value::as_str) == Some("accept")
        && decision
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|s| s.starts_with("user"))
}

/// Shell commands the agent ran, kept as text. Whether a line of that text ended up in a
/// file is decided later, against the repository.
fn shell(v: &Value, out: &mut Vec<RawEdit>, pending: &mut HashMap<String, usize>) {
    // Other entry types repeat tool calls without saying when they happened. Without a time
    // there is no telling whether the text was written before or after the code it matches.
    let Some(ts) = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_ts)
    else {
        return;
    };
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let session = v
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let cwd = v.get("cwd").and_then(Value::as_str).unwrap_or_default();
    for item in v
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if item.get("name").and_then(Value::as_str) != Some("Bash") {
            continue;
        }
        let command = item
            .pointer("/input/command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !super::writes_text(command) {
            continue;
        }
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            pending.insert(id.to_string(), out.len());
        }
        out.push(RawEdit {
            agent: "claude-code".to_string(),
            session: session.to_string(),
            ts,
            path: cwd.to_string(),
            shell: true,
            // Set when the result line shows a person approved the command.
            seen: None,
            added: skeletons(command.lines()),
        });
    }
}

fn entry(v: &Value, mode: Option<&str>, out: &mut Vec<RawEdit>) {
    let Some(result) = v.get("toolUseResult").filter(|r| r.is_object()) else {
        return;
    };
    let Some(ts) = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_ts)
    else {
        return;
    };
    let session = v
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let edit = |path: &str, added: Vec<String>, seen: Option<Seen>| RawEdit {
        agent: "claude-code".to_string(),
        session: session.to_string(),
        ts,
        path: path.to_string(),
        shell: false,
        seen,
        added,
    };

    if let (Some(path), Some(patch)) = (
        result.get("filePath").and_then(Value::as_str),
        result.get("structuredPatch").and_then(Value::as_array),
    ) {
        let mut added = hunks(patch);
        if added.is_empty() && result.get("type").and_then(Value::as_str) == Some("create") {
            added = skeletons(
                result
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .lines(),
            );
        }
        if !added.is_empty() {
            let asked = match v.get("permissionDecision") {
                Some(d) => asked_a_person(d),
                // Logs older than `permissionDecision`: default mode always prompts for edits,
                // so the mode is the evidence. An unknown mode is evidence of nothing.
                None => mode == Some("default"),
            };
            let seen = if result.get("userModified").and_then(Value::as_bool) == Some(true) {
                Some(Seen::Edited)
            } else if asked {
                Some(Seen::Approved)
            } else {
                None
            };
            out.push(edit(path, added, seen));
        }
    }

    // Approving a shell command is not seeing the code it writes.
    for file in result
        .pointer("/bashEditDiff/files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(path), Some(patch)) = (
            file.get("filePath").and_then(Value::as_str),
            file.get("hunks").and_then(Value::as_array),
        ) else {
            continue;
        };
        let added = hunks(patch);
        if !added.is_empty() {
            out.push(edit(path, added, None));
        }
    }
}

fn hunks(patch: &[Value]) -> Vec<String> {
    let lines = patch
        .iter()
        .filter_map(|h| h.get("lines").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str);
    skeletons(added_from_hunk_lines(lines))
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

    fn edit_entry(decision: Option<Value>, user_modified: bool) -> Value {
        let mut e = json!({
            "type": "user", "sessionId": "s1", "timestamp": "2026-10-07T22:10:15.083Z",
            "toolUseResult": {
                "filePath": "/repo/src/a.rs",
                "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 2,
                    "lines": [" fn main() {", "-    old();", "+    let total = compute(a, b);", "+}"]}],
                "userModified": user_modified
            }
        });
        if let Some(d) = decision {
            e["permissionDecision"] = d;
        }
        e
    }

    #[test]
    fn auto_accepted_edit_is_unseen() {
        let d = json!({"decision": "accept", "source": "config", "reasonType": "mode"});
        let edits = parse(&log(&[edit_entry(Some(d), false)]));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].seen, None);
        assert_eq!(edits[0].path, "/repo/src/a.rs");
        assert_eq!(edits[0].added, vec!["lettotal=compute(ab)", "}"]);
        assert_eq!(edits[0].ts, 1_791_411_015);
    }

    #[test]
    fn prompt_answered_by_a_person_is_approved() {
        let d = json!({"decision": "accept", "source": "user_temporary", "grantScope": "once"});
        assert_eq!(
            parse(&log(&[edit_entry(Some(d), false)]))[0].seen,
            Some(Seen::Approved)
        );
        assert_eq!(
            parse(&log(&[edit_entry(None, true)]))[0].seen,
            Some(Seen::Edited)
        );
    }

    #[test]
    fn older_logs_fall_back_to_the_session_mode() {
        let bypass = json!({"type": "user", "permissionMode": "bypassPermissions", "message": {}});
        assert_eq!(
            parse(&log(&[bypass, edit_entry(None, false)]))[0].seen,
            None
        );
        let default = json!({"type": "user", "permissionMode": "default", "message": {}});
        assert_eq!(
            parse(&log(&[default, edit_entry(None, false)]))[0].seen,
            Some(Seen::Approved)
        );
        // Nothing says how it was allowed: that is not evidence a person looked.
        assert_eq!(parse(&log(&[edit_entry(None, false)]))[0].seen, None);
    }

    #[test]
    fn shell_commands_need_a_time_and_an_explicit_approval() {
        let call = |ts: Option<&str>| {
            let mut e = json!({"type": "assistant", "sessionId": "s", "cwd": "/repo",
                "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash",
                    "input": {"command": "cat > a.py <<'EOF'\nvalue = compute_everything()\nEOF"}}]}});
            if let Some(ts) = ts {
                e["timestamp"] = json!(ts);
            }
            e
        };
        let result = |source: &str| {
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1"}]},
            "toolUseResult": {"stdout": ""}, "permissionDecision": {"decision": "accept", "source": source}})
        };
        assert!(parse(&log(&[call(None)])).is_empty());
        let edits = parse(&log(&[call(Some("2026-10-07T22:10:15Z"))]));
        assert!(edits[0].shell && edits[0].path == "/repo" && edits[0].seen.is_none());
        assert!(edits[0]
            .added
            .contains(&"value=compute_everything()".to_string()));
        let edits = parse(&log(&[
            call(Some("2026-10-07T22:10:15Z")),
            result("user_temporary"),
        ]));
        assert_eq!(edits[0].seen, Some(Seen::Approved));
        let edits = parse(&log(&[
            call(Some("2026-10-07T22:10:15Z")),
            result("config"),
        ]));
        assert_eq!(edits[0].seen, None);
        // One-line commands cannot carry whole lines of code.
        let quick = json!({"type": "assistant", "timestamp": "2026-10-07T22:10:15Z",
            "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "cargo test"}}]}});
        assert!(parse(&log(&[quick])).is_empty());
    }

    #[test]
    fn new_files_and_shell_edits() {
        let create = json!({"sessionId": "s", "timestamp": "2026-10-07T22:10:15Z",
            "permissionDecision": {"decision": "accept", "source": "config"},
            "toolUseResult": {"type": "create", "filePath": "/repo/new.py", "content": "import os\n\nprint(os.getcwd())\n", "structuredPatch": []}});
        let bash = json!({"sessionId": "s", "timestamp": "2026-10-07T22:10:16Z",
            "permissionDecision": {"decision": "accept", "source": "user_temporary"},
            "toolUseResult": {"stdout": "", "bashEditDiff": {"files": [{"filePath": "/repo/x.sh",
                "hunks": [{"oldStart": 1, "oldLines": 0, "newStart": 1, "newLines": 1, "lines": ["+echo generated"]}]}]}}});
        let edits = parse(&log(&[create, bash]));
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].added, vec!["importos", "print(os.getcwd())"]);
        // The person approved a command, not a diff.
        assert_eq!(edits[1].seen, None);
        assert_eq!(edits[1].path, "/repo/x.sh");
    }
}
