//! The vocabulary: what a line can be, and what that label rests on.

use serde::{Deserialize, Serialize};

/// Evidence that a person looked at agent-written code, weakest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seen {
    /// The diff was shown in a permission prompt and a person accepted it.
    Approved,
    /// A person other than the author reviewed the pull request that brought it in.
    Reviewed,
    /// A person changed the agent's proposal before accepting it.
    Edited,
    /// A person marked it read with `unread next` or `unread mark`.
    Attested,
}

impl Seen {
    pub const ALL: [Seen; 4] = [Seen::Approved, Seen::Reviewed, Seen::Edited, Seen::Attested];

    pub fn label(self) -> &'static str {
        match self {
            Seen::Approved => "approved",
            Seen::Reviewed => "reviewed",
            Seen::Edited => "edited",
            Seen::Attested => "attested",
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Seen::Approved => "diff shown in a permission prompt and accepted",
            Seen::Reviewed => "pull request reviewed by another person",
            Seen::Edited => "a person changed the agent's proposal",
            Seen::Attested => "marked read with unread",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not counted: blank lines, lone brackets, lines outside the requested range.
    Blank,
    Human,
    Seen,
    Unread,
}

/// What says an agent wrote the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    None,
    /// The agent's own session log shows it adding this line to this file.
    Log,
    /// The session log shows the agent typing this exact line in a shell command (a heredoc,
    /// a script that writes files) in a session that worked on this repository.
    Shell,
    /// The repository's `.unread/ledger.jsonl` records it.
    Ledger,
    /// The commit that introduced it is marked as agent-authored.
    Commit,
    /// A person claimed the line as their own with `unread claim`. A claim can only correct a
    /// commit marker, which credits an agent with every line of a commit: it never overrides a
    /// session log or the ledger, which name the exact line. Every view shows it as a claim.
    Claimed,
}

pub const NO_AGENT: u8 = u8::MAX;
pub const NO_COMMIT: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct Line {
    pub status: Status,
    /// Index into `Analysis::agents`, or `NO_AGENT`.
    pub agent: u8,
    pub seen: Option<Seen>,
    pub basis: Basis,
    /// Hash of the line's skeleton.
    pub hash: u64,
    /// Index into `Analysis::commits`, or `NO_COMMIT` for uncommitted lines.
    pub commit: u32,
    /// When the agent wrote it, when a log says so.
    pub ts: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub code: u32,
    pub human: u32,
    pub seen: u32,
    pub unread: u32,
}

impl Counts {
    pub fn add(&mut self, s: Status) {
        match s {
            Status::Blank => return,
            Status::Human => self.human += 1,
            Status::Seen => self.seen += 1,
            Status::Unread => self.unread += 1,
        }
        self.code += 1;
    }

    pub fn merge(&mut self, o: Counts) {
        self.code += o.code;
        self.human += o.human;
        self.seen += o.seen;
        self.unread += o.unread;
    }

    /// Lines an agent wrote, read or not.
    pub fn ai(&self) -> u32 {
        self.seen + self.unread
    }

    pub fn unread_pct(&self) -> f64 {
        crate::util::pct(u64::from(self.unread), u64::from(self.code))
    }
}
