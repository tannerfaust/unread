//! What the repository itself remembers, in `.unread/`.
//!
//! Session logs live on one person's machine. Two append-only files make the evidence travel
//! with the code, so teammates and CI see the same picture:
//!
//! * `ledger.jsonl`  lines an agent wrote (`unread record`)
//! * `read.jsonl`    lines a person marked read (`unread next`, `unread mark`)
//!
//! Both hold hashes of line skeletons, never code and never prompts. A line that changes stops
//! matching its hash, so an edited line has to earn its label again.

use crate::lines::{from_hex, hex};
use crate::model::Seen;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

pub const DIR: &str = ".unread";
const LEDGER: &str = "ledger.jsonl";
const READ: &str = "read.jsonl";
const AUTHORED: &str = "authored.jsonl";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LedgerEntry {
    pub v: u8,
    pub path: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<Seen>,
    /// When the entry was recorded.
    pub at: i64,
    pub h: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReadEntry {
    pub v: u8,
    pub path: String,
    pub by: String,
    pub at: i64,
    /// Seconds the reader spent, when the reading happened in `unread next`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secs: Option<u32>,
    pub h: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LedgerRec {
    pub agent: String,
    pub seen: Option<Seen>,
}

#[derive(Default)]
pub struct Store {
    pub ledger: HashMap<String, HashMap<u64, LedgerRec>>,
    pub read: HashMap<String, HashSet<u64>>,
    pub read_any: HashSet<u64>,
    /// Lines a person claimed as their own (`unread claim`).
    pub claimed: HashMap<String, HashSet<u64>>,
}

impl Store {
    pub fn load(root: &Path) -> Store {
        let mut s = Store::default();
        for entry in read_jsonl::<LedgerEntry>(&root.join(DIR).join(LEDGER)) {
            let file = s.ledger.entry(entry.path.clone()).or_default();
            for h in entry.h.iter().filter_map(|h| from_hex(h)) {
                let rec = LedgerRec {
                    agent: entry.agent.clone(),
                    seen: entry.seen,
                };
                // Later entries only ever add evidence.
                let keep = |old: &LedgerRec| old.seen >= rec.seen;
                if !file.get(&h).is_some_and(keep) {
                    file.insert(h, rec);
                }
            }
        }
        for entry in read_jsonl::<ReadEntry>(&root.join(DIR).join(READ)) {
            let file = s.read.entry(entry.path.clone()).or_default();
            for h in entry.h.iter().filter_map(|h| from_hex(h)) {
                file.insert(h);
                s.read_any.insert(h);
            }
        }
        for entry in read_jsonl::<ReadEntry>(&root.join(DIR).join(AUTHORED)) {
            let file = s.claimed.entry(entry.path.clone()).or_default();
            file.extend(entry.h.iter().filter_map(|h| from_hex(h)));
        }
        s
    }

    pub fn claimed_lines(&self) -> usize {
        self.claimed.values().map(HashSet::len).sum()
    }

    pub fn ledger_lines(&self) -> usize {
        self.ledger.values().map(HashMap::len).sum()
    }

    pub fn read_lines(&self) -> usize {
        self.read.values().map(HashSet::len).sum()
    }
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn append<T: Serialize>(root: &Path, file: &str, entries: &[T]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let dir = root.join(DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    // Two people appending to the same file is the normal case, not a conflict.
    let attrs = dir.join(".gitattributes");
    if !attrs.exists() {
        std::fs::write(&attrs, "*.jsonl merge=union\n")?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file))?;
    for e in entries {
        writeln!(f, "{}", serde_json::to_string(e)?)?;
    }
    Ok(())
}

pub fn append_ledger(root: &Path, entries: &[LedgerEntry]) -> Result<()> {
    append(root, LEDGER, entries)
}

pub fn append_read(root: &Path, entries: &[ReadEntry]) -> Result<()> {
    append(root, READ, entries)
}

/// Claims of authorship. Same shape as read marks: who, when, which line hashes.
pub fn append_authored(root: &Path, entries: &[ReadEntry]) -> Result<()> {
    append(root, AUTHORED, entries)
}

pub fn hashes(h: impl IntoIterator<Item = u64>) -> Vec<String> {
    let mut v: Vec<String> = h.into_iter().map(hex).collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_union() {
        let dir = tempfile::tempdir().unwrap();
        let e = |seen| LedgerEntry {
            v: 1,
            path: "a.rs".into(),
            agent: "codex".into(),
            seen,
            at: 1,
            h: hashes([7, 9]),
        };
        append_ledger(dir.path(), &[e(Some(Seen::Approved))]).unwrap();
        append_ledger(dir.path(), &[e(None)]).unwrap();
        append_read(
            dir.path(),
            &[ReadEntry {
                v: 1,
                path: "a.rs".into(),
                by: "me".into(),
                at: 2,
                secs: Some(30),
                h: hashes([9]),
            }],
        )
        .unwrap();

        let s = Store::load(dir.path());
        assert_eq!(s.ledger_lines(), 2);
        // The weaker, later entry does not erase the approval.
        assert_eq!(s.ledger["a.rs"][&7].seen, Some(Seen::Approved));
        assert!(s.read["a.rs"].contains(&9));
        assert!(s.read_any.contains(&9) && !s.read_any.contains(&7));
        assert!(dir.path().join(".unread/.gitattributes").exists());
    }
}
