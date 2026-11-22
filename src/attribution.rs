//! Which commits say an agent wrote them.
//!
//! Agents sign their work in three ways: a `Co-Authored-By` trailer, a bot account as author
//! or committer, or a "Generated with" line in the message. This is commit-level evidence. It
//! is weaker than a session log, which names the exact lines, and the classifier ranks it so.

use crate::git::CommitMeta;

/// (needle in the lower-cased haystack, agent name). First match wins.
const MARKERS: &[(&str, &str)] = &[
    ("noreply@anthropic.com", "claude-code"),
    ("co-authored-by: claude", "claude-code"),
    ("generated with [claude code]", "claude-code"),
    ("claude[bot]", "claude-code"),
    ("co-authored-by: codex", "codex"),
    ("chatgpt-codex-connector[bot]", "codex"),
    ("codex@openai.com", "codex"),
    ("cursoragent@cursor.com", "cursor"),
    ("co-authored-by: cursor", "cursor"),
    ("copilot-swe-agent[bot]", "copilot"),
    ("co-authored-by: copilot", "copilot"),
    ("+copilot@users.noreply.github.com", "copilot"),
    ("devin-ai-integration[bot]", "devin"),
    ("google-labs-jules[bot]", "jules"),
    ("co-authored-by: gemini", "gemini"),
    ("gemini-cli", "gemini"),
    ("co-authored-by: opencode", "opencode"),
    ("noreply@opencode.ai", "opencode"),
    ("co-authored-by: aider", "aider"),
    ("(aider)", "aider"),
    ("co-authored-by: amp", "amp"),
    ("amp@ampcode.com", "amp"),
    ("factory-droid[bot]", "droid"),
    ("co-authored-by: cline", "cline"),
    ("co-authored-by: windsurf", "windsurf"),
];

/// The agent a commit credits, if any.
pub fn agent_of(c: &CommitMeta) -> Option<&'static str> {
    let hay = format!(
        "{} <{}>\n{} <{}>\n{}",
        c.author, c.email, c.committer, c.committer_email, c.message
    )
    .to_lowercase();
    MARKERS
        .iter()
        .find(|(needle, _)| hay.contains(needle))
        .map(|(_, agent)| *agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(author: &str, email: &str, message: &str) -> CommitMeta {
        CommitMeta {
            author: author.into(),
            email: email.into(),
            message: message.into(),
            ..Default::default()
        }
    }

    #[test]
    fn trailers_bots_and_plain_humans() {
        let c = commit(
            "Ann",
            "ann@example.com",
            "Fix parser\n\nCo-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>\n",
        );
        assert_eq!(agent_of(&c), Some("claude-code"));
        let c = commit(
            "copilot-swe-agent[bot]",
            "198982749+Copilot@users.noreply.github.com",
            "Add tests",
        );
        assert_eq!(agent_of(&c), Some("copilot"));
        let c = commit(
            "Ann",
            "ann@example.com",
            "Co-authored-by: Bob <bob@example.com>",
        );
        assert_eq!(agent_of(&c), None);
    }
}
