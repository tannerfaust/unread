//! End to end: a real git repository, real log files in the formats the agents write, and the
//! real binary. Every number asserted here is counted by hand in the comments.
#![cfg(unix)]

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct World {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
}

impl World {
    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.repo)
            .args(args)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Ann Person")
            .env("GIT_AUTHOR_EMAIL", "ann@example.com")
            .env("GIT_COMMITTER_NAME", "Ann Person")
            .env("GIT_COMMITTER_EMAIL", "ann@example.com")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.repo.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn unread(&self, args: &[&str]) -> Output {
        self.unread_with(args, "")
    }

    fn unread_with(&self, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_unread"))
            .current_dir(&self.repo)
            .args(args)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("NO_COLOR", "1")
            .env("UNREAD_CACHE_DIR", self.home.join("cache"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("EDITOR")
            .env_remove("VISUAL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("unread runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn report(&self, extra: &[&str]) -> Value {
        let mut args = vec!["scan", "--json"];
        args.extend_from_slice(extra);
        let out = self.unread(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("json report")
    }
}

fn lines(v: &Value) -> (u64, u64, u64, u64) {
    let l = &v["lines"];
    (
        l["code"].as_u64().unwrap(),
        l["human"].as_u64().unwrap(),
        l["seen"].as_u64().unwrap(),
        l["unread"].as_u64().unwrap(),
    )
}

fn jsonl(path: &Path, entries: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        entries
            .iter()
            .map(|e| e.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}

const HUMAN_V1: &str = "def greet(name):\n    return \"hello \" + name\n\ndef farewell(name):\n    return \"bye \" + name\n";
const HUMAN_ADDED: &str = "\ndef shout(name):\n    return greet(name).upper()\n";
/// What the agent wrote.
const AGENT_AS_WRITTEN: &str = "import os\n\ndef compute_total(alpha, beta, gamma):\n    return alpha + beta + gamma\n\nresult = compute_total(alpha, beta, gamma)\nprint(os.getcwd(), result)\n";
/// What a formatter turned it into: one statement re-wrapped over four lines.
const AGENT_ON_DISK: &str = "import os\n\ndef compute_total(alpha, beta, gamma):\n    return alpha + beta + gamma\n\nresult = compute_total(\n    alpha,\n    beta, gamma\n)\nprint(os.getcwd(), result)\n";
const CODEX_FILE: &str = "export function double(value) {\n  return value * 2;\n}\n";
const OLD_AI: &str = "def legacy(a, b):\n    total = a + b\n    return total\n";

/// Lines of code by hand:
///   src/human.py   4 written by Ann, then 2 added by Claude Code and approved in a prompt
///   src/agent.py   7 written by Claude Code with nobody asked (3 of them re-wrapped)
///   lib/codex.js   2 written by Codex with nobody asked (`}` is not code)
///   old_ai.py      3 in a commit that credits Claude, with no session log
/// 18 lines of code: 4 human, 2 seen, 12 unread.
fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("checkout-of-demo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&repo).unwrap();
    let w = World {
        _tmp: tmp,
        home,
        repo,
    };
    w.git(&["init", "-q", "-b", "main"]);
    w.git(&["config", "user.name", "Ann Person"]);
    w.git(&["config", "user.email", "ann@example.com"]);

    w.write("src/human.py", HUMAN_V1);
    w.git(&["add", "."]);
    w.git(&["commit", "-q", "-m", "Greetings"]);

    w.write("src/human.py", &format!("{HUMAN_V1}{HUMAN_ADDED}"));
    w.write("src/agent.py", AGENT_ON_DISK);
    w.write("lib/codex.js", CODEX_FILE);
    w.git(&["add", "."]);
    w.git(&["commit", "-q", "-m", "Totals and doubling"]);

    w.write("old_ai.py", OLD_AI);
    w.git(&["add", "."]);
    w.git(&[
        "commit",
        "-q",
        "-m",
        "Legacy helper\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
    ]);

    // Claude Code logged the path as the shell saw it; Codex logged the resolved one.
    let as_typed = w.repo.to_string_lossy().into_owned();
    let resolved = w
        .repo
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let auto = json!({"decision": "accept", "source": "config", "reasonType": "mode"});
    let asked = json!({"decision": "accept", "source": "user_temporary", "grantScope": "once"});
    jsonl(
        &w.home.join(".claude/projects/-demo/session-1.jsonl"),
        &[
            json!({"type": "user", "sessionId": "c1", "timestamp": "2020-01-01T10:00:00Z", "permissionDecision": auto,
                "toolUseResult": {"type": "create", "filePath": format!("{as_typed}/src/agent.py"), "content": AGENT_AS_WRITTEN, "structuredPatch": []}}),
            json!({"type": "user", "sessionId": "c1", "timestamp": "2020-01-01T10:05:00Z", "permissionDecision": asked,
                "toolUseResult": {"filePath": format!("{as_typed}/src/human.py"), "userModified": false, "structuredPatch": [
                    {"oldStart": 5, "oldLines": 1, "newStart": 5, "newLines": 4,
                     "lines": ["     return \"bye \" + name", "+", "+def shout(name):", "+    return greet(name).upper()"]}]}}),
            // Years after Ann committed it, an agent writes a line identical to one of hers.
            // The commit is older than the edit, so the line stays hers.
            json!({"type": "user", "sessionId": "c2", "timestamp": "2099-01-01T00:00:00Z", "permissionDecision": auto,
                "toolUseResult": {"filePath": format!("{as_typed}/src/human.py"), "userModified": false, "structuredPatch": [
                    {"oldStart": 4, "oldLines": 0, "newStart": 4, "newLines": 1, "lines": ["+def farewell(name):"]}]}}),
            // An edit to some other project never counts here.
            json!({"type": "user", "sessionId": "c3", "timestamp": "2020-01-01T11:00:00Z", "permissionDecision": auto,
                "toolUseResult": {"type": "create", "filePath": "/somewhere/else/src/human.py", "content": HUMAN_V1, "structuredPatch": []}}),
        ],
    );
    jsonl(
        &w.home.join(".codex/sessions/2020/01/01/rollout-1.jsonl"),
        &[
            json!({"timestamp": "2020-01-01T12:00:00Z", "type": "session_meta", "payload": {"id": "x1", "cwd": resolved}}),
            json!({"timestamp": "2020-01-01T12:00:01Z", "type": "turn_context", "payload": {"cwd": resolved, "approval_policy": "never"}}),
            json!({"timestamp": "2020-01-01T12:01:00Z", "type": "event_msg", "payload": {"type": "item_completed", "item": {
                "type": "FileChange", "status": "completed",
                "changes": {format!("{resolved}/lib/codex.js"): {"type": "add", "content": CODEX_FILE}}}}}),
        ],
    );
    w
}

#[test]
fn every_line_gets_the_label_its_evidence_supports() {
    let w = world();
    let r = w.report(&[]);
    assert_eq!(lines(&r), (18, 4, 2, 12), "{r:#}");
    assert_eq!(r["unread_pct"], 66.7);
    assert_eq!(
        r["basis"],
        json!({"log": 11, "shell": 0, "ledger": 0, "commit": 3})
    );
    assert_eq!(r["seen_by"], json!({"approved": 2}));
    // Two Claude Code sessions touched this repository: the real one and the 2099 one.
    assert_eq!(
        r["evidence"]["sessions"],
        json!({"claude-code": 2, "codex": 1})
    );
    assert_eq!(r["evidence"]["edits_in_logs"], 5);
    assert_eq!(r["evidence"]["edits_to_this_repo"], 4);

    let agents = r["agents"].as_array().unwrap();
    assert_eq!(
        agents[0],
        json!({"name": "claude-code", "written": 12, "unread": 10})
    );
    assert_eq!(
        agents[1],
        json!({"name": "codex", "written": 2, "unread": 2})
    );

    let file = |path: &str| {
        r["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .cloned()
            .unwrap()
    };
    assert_eq!(file("src/agent.py")["unread"], 7);
    assert_eq!(file("src/human.py")["human"], 4);
    assert_eq!(file("src/human.py")["seen"], 2);
    assert_eq!(file("lib/codex.js")["unread"], 2);
    assert_eq!(file("old_ai.py")["unread"], 3);
}

/// An agent that writes a file with `cat > file <<EOF` leaves no patch in its log, only the
/// command it typed. Those lines are still the agent's.
#[test]
fn code_written_through_the_shell_is_found_by_the_text_of_the_command() {
    let w = world();
    let script =
        "#!/bin/sh\necho generating the report now\nexit_code=$(run_generator --all --strict)\n";
    w.write("scripts/gen.sh", script);
    w.git(&["add", "."]);
    w.git(&["commit", "-q", "-m", "Generator"]);
    let command = format!("cat > scripts/gen.sh <<'EOF'\n{script}EOF");
    let said = |text: &str| {
        json!({"type": "assistant", "sessionId": "c9", "timestamp": "2020-01-01T10:10:00Z",
        "cwd": w.repo.to_string_lossy(),
        "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": text}}]}})
    };
    let allowed = |source: &str| {
        json!({"type": "user", "sessionId": "c9", "timestamp": "2020-01-01T10:10:01Z",
        "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": ""}]},
        "toolUseResult": {"stdout": "", "stderr": ""},
        "permissionDecision": {"decision": "accept", "source": source}})
    };
    let log = w.home.join(".claude/projects/-demo/session-9.jsonl");

    // Nobody was asked: three more unread lines, on the strength of the command text.
    // A `grep` for one of Ann's lines is not a write and changes nothing.
    jsonl(
        &log,
        &[
            said(&command),
            allowed("config"),
            said("grep -n 'def greet(name):' src/human.py"),
        ],
    );
    let r = w.report(&[]);
    assert_eq!(lines(&r), (21, 4, 2, 15), "{r:#}");
    assert_eq!(
        r["basis"],
        json!({"log": 11, "shell": 3, "ledger": 0, "commit": 3})
    );
    let blame =
        String::from_utf8_lossy(&w.unread(&["blame", "scripts/gen.sh"]).stdout).into_owned();
    assert!(blame.contains("typed in a shell command"), "{blame}");

    // A person approved the command, with the code in front of them.
    jsonl(&log, &[said(&command), allowed("user_temporary")]);
    assert_eq!(lines(&w.report(&[])), (21, 4, 5, 12));
}

#[test]
fn a_claim_corrects_a_commit_marker_but_never_a_session_log() {
    let w = world();
    // old_ai.py is credited to Claude only by its commit trailer: Ann may claim it.
    // src/agent.py is in Claude's session log: the claim is refused.
    let out = w.unread(&["claim", "old_ai.py", "src/agent.py"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("Claimed 3 lines as written by Ann Person"),
        "{text}"
    );
    assert!(text.contains("Refused 7 lines"), "{text}");
    let r = w.report(&[]);
    assert_eq!(lines(&r), (18, 7, 2, 9), "{r:#}");
    assert_eq!(r["evidence"]["claimed_lines"], 3);
    let blame = String::from_utf8_lossy(&w.unread(&["blame", "old_ai.py"]).stdout).into_owned();
    assert!(
        blame.contains("claimed by a person with unread claim"),
        "{blame}"
    );
    let summary = String::from_utf8_lossy(&w.unread(&["scan"]).stdout).into_owned();
    assert!(summary.contains("(claimed 3)"), "{summary}");

    // CI can refuse to take anyone's word for it.
    assert_eq!(lines(&w.report(&["--no-claims"])), (18, 4, 2, 12));
    let check =
        String::from_utf8_lossy(&w.unread(&["check", "--max-unread", "90"]).stdout).into_owned();
    assert!(check.contains("3 human lines rest on claims"), "{check}");
}

#[test]
fn renames_keep_their_history() {
    let w = world();
    w.git(&["mv", "src/agent.py", "src/renamed.py"]);
    w.git(&["commit", "-q", "-m", "Rename"]);
    let r = w.report(&[]);
    assert_eq!(lines(&r), (18, 4, 2, 12));
    let renamed = r["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "src/renamed.py")
        .unwrap()
        .clone();
    assert_eq!(renamed["unread"], 7);
}

#[test]
fn the_ledger_carries_the_evidence_to_machines_without_logs() {
    let w = world();
    // Without logs and without a ledger only the commit marker is left.
    assert_eq!(lines(&w.report(&["--no-logs"])), (18, 15, 0, 3));

    let out = w.unread(&["record"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Recorded 11 agent-written lines in 3 files"),
        "{out:?}"
    );
    let ledger = fs::read_to_string(w.repo.join(".unread/ledger.jsonl")).unwrap();
    assert!(
        !ledger.contains("compute_total"),
        "the ledger holds hashes, never code"
    );

    let r = w.report(&["--no-logs"]);
    assert_eq!(lines(&r), (18, 4, 2, 12));
    assert_eq!(
        r["basis"],
        json!({"log": 0, "shell": 0, "ledger": 11, "commit": 3})
    );

    // Recording twice adds nothing.
    let again = w.unread(&["record"]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("up to date"));
}

#[test]
fn reading_turns_unread_into_seen_until_the_code_changes() {
    let w = world();
    let out = w.unread(&["mark", "src/agent.py", "lib/codex.js:1-2"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Marked 9 lines as read by Ann Person"),
        "{out:?}"
    );
    let r = w.report(&[]);
    assert_eq!(lines(&r), (18, 4, 11, 3));
    assert_eq!(r["seen_by"], json!({"approved": 2, "attested": 9}));

    // A read mark is bound to the line's content. The agent rewrites one line; it is unread again.
    let edited =
        AGENT_ON_DISK.replace("return alpha + beta + gamma", "return alpha * beta * gamma");
    w.write("src/agent.py", &edited);
    w.git(&[
        "commit",
        "-qam",
        "Multiply\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
    ]);
    assert_eq!(lines(&w.report(&[])), (18, 4, 10, 4));
}

#[test]
fn next_walks_the_queue_and_saves_each_piece() {
    let w = world();
    // Read the first piece (and confirm, because seven lines in no time is suspicious),
    // skip the second, quit.
    let out = w.unread_with(&["next"], "r\ny\ns\nq\n");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("7 lines in 0 seconds. Count it as read anyway?"),
        "{text}"
    );
    assert!(text.contains("src/agent.py:1-10"), "{text}");
    assert!(text.contains("Read 7 lines in 1 pieces"), "{text}");
    assert_eq!(lines(&w.report(&[])), (18, 4, 9, 5));
    let marks = fs::read_to_string(w.repo.join(".unread/read.jsonl")).unwrap();
    assert!(
        marks.contains("\"by\":\"Ann Person <ann@example.com>\"") && marks.contains("\"secs\":")
    );
}

#[test]
fn check_gates_on_share_lines_paths_and_new_code() {
    let w = world();
    let code = |args: &[&str]| w.unread(args).status.code().unwrap();
    assert_eq!(code(&["check", "--max-unread", "70"]), 0);
    assert_eq!(code(&["check", "--max-unread", "50"]), 1);
    assert_eq!(
        code(&["check"]),
        1,
        "with no limit given, any unread code fails"
    );
    assert_eq!(code(&["check", "--path", "src/human.py"]), 0);
    assert_eq!(code(&["check", "--path", "lib", "--max-lines", "1"]), 1);
    assert_eq!(code(&["check", "--path", "lib", "--max-lines", "2"]), 0);

    // Only what came after the first commit: Ann's four lines are out of scope.
    let first = w.git(&["rev-list", "--max-parents=0", "HEAD"]);
    assert_eq!(lines(&w.report(&["--base", &first])), (14, 0, 2, 12));
    assert_eq!(
        w.unread(&["check", "--base", "no-such-rev"]).status.code(),
        Some(2)
    );
}

#[test]
fn blame_report_card_and_badge() {
    let w = world();
    let blame = String::from_utf8_lossy(&w.unread(&["blame", "agent.py"]).stdout).into_owned();
    assert!(blame.contains("7 unread"), "{blame}");
    assert!(
        blame.contains("claude-code 2020-01-01 · session log, nobody was asked"),
        "{blame}"
    );
    let blame = String::from_utf8_lossy(&w.unread(&["blame", "src/human.py"]).stdout).into_owned();
    assert!(
        blame.contains("Ann Person")
            && blame.contains("diff shown in a permission prompt and accepted"),
        "{blame}"
    );
    let blame = String::from_utf8_lossy(&w.unread(&["blame", "old_ai.py"]).stdout).into_owned();
    assert!(blame.contains("credits it"), "{blame}");

    let html = w.repo.join("out.html");
    assert!(w
        .unread(&["report", "--no-open", "-o", html.to_str().unwrap()])
        .status
        .success());
    let page = fs::read_to_string(&html).unwrap();
    assert!(
        page.contains("66.7%") && page.contains("data-p=\"src/agent.py\"") && !page.contains("{{")
    );
    assert!(!page.contains("http://") || page.contains("http://www.w3.org/2000/svg"));

    let svg = w.repo.join("card.svg");
    assert!(w
        .unread(&["card", "-o", svg.to_str().unwrap()])
        .status
        .success());
    let card = fs::read_to_string(&svg).unwrap();
    assert!(
        card.contains(">67%<")
            && card.contains("never read by a person")
            && card.contains("12 lines")
    );

    let badge = String::from_utf8_lossy(&w.unread(&["badge"]).stdout).into_owned();
    assert!(badge.starts_with("<svg") && badge.contains(">67%<"));
    let shields: Value = serde_json::from_slice(&w.unread(&["badge", "--json"]).stdout).unwrap();
    assert_eq!(shields["message"], "67%");
}

#[test]
fn hook_installs_beside_existing_hooks_and_removes_itself() {
    let w = world();
    let hook = w.repo.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho mine\n").unwrap();
    assert!(w.unread(&["hook", "install"]).status.success());
    let text = fs::read_to_string(&hook).unwrap();
    assert!(text.starts_with("#!/bin/sh\necho mine\n") && text.contains("unread record --quiet"));
    assert!(w.unread(&["hook", "install"]).status.success());
    assert_eq!(
        fs::read_to_string(&hook)
            .unwrap()
            .matches("unread record")
            .count(),
        1
    );
    assert!(w.unread(&["hook", "remove"]).status.success());
    assert_eq!(fs::read_to_string(&hook).unwrap(), "#!/bin/sh\necho mine\n");
}

#[test]
fn outside_a_repository_it_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_unread"))
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not inside a git repository"));
}
