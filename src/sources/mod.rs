//! What agents wrote, read from the session logs they already keep on disk.
//!
//! Nothing here needs setup or instrumentation: Claude Code and Codex both record every edit
//! they make. Each parser turns one log file into [`RawEdit`]s and knows nothing about the
//! repository being analysed, so adding an agent means adding one file.
//!
//! Logs are scanned by memory-mapping them and jumping straight to the few lines that can
//! hold an edit. A typical log is mostly screenshots and tool output, and is never parsed.

use crate::model::Seen;
use aho_corasick::AhoCorasick;
use memchr::{memchr, memrchr};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

pub mod claude;
pub mod codex;

/// Bump when a parser changes what it extracts, so cached results are not reused.
const CACHE_VERSION: u32 = 3;

/// One edit an agent made to one file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawEdit {
    pub agent: String,
    pub session: String,
    /// Unix seconds.
    pub ts: i64,
    /// Absolute path as the agent saw it. For a shell command, the directory it ran in.
    pub path: String,
    /// The text of a shell command rather than a patch to a known file. An agent that writes
    /// code with `cat > file <<EOF` or a script leaves no patch, only what it typed.
    #[serde(default)]
    pub shell: bool,
    /// Evidence in the log itself that a person saw this edit.
    pub seen: Option<Seen>,
    /// Skeletons of the lines the edit added.
    pub added: Vec<String>,
}

/// Where each agent keeps its logs, and where unread keeps what it extracted from them.
#[derive(Clone, Debug, Default)]
pub struct Dirs {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    /// Finished sessions never change, so their edits are extracted once and kept here.
    pub cache: Option<PathBuf>,
}

impl Dirs {
    /// Explicit paths win, then each agent's own environment variable, then its default.
    pub fn detect(claude: Option<PathBuf>, codex: Option<PathBuf>) -> Dirs {
        let env = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = env("HOME").or_else(|| env("USERPROFILE"));
        let cache = match std::env::var("UNREAD_CACHE_DIR") {
            Ok(v) if v.is_empty() || v == "off" => None,
            Ok(v) => Some(PathBuf::from(v)),
            Err(_) => env("XDG_CACHE_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".cache")))
                .map(|c| c.join("unread")),
        };
        Dirs {
            claude: claude
                .or_else(|| env("CLAUDE_CONFIG_DIR"))
                .or_else(|| home.as_ref().map(|h| h.join(".claude"))),
            codex: codex
                .or_else(|| env("CODEX_HOME"))
                .or_else(|| home.as_ref().map(|h| h.join(".codex"))),
            cache,
        }
    }

    pub fn none() -> Dirs {
        Dirs::default()
    }
}

#[derive(Default)]
pub struct Scan {
    pub edits: Vec<RawEdit>,
    pub files: usize,
    pub bytes: u64,
    /// Log files that had to be read this time.
    pub read: usize,
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    version: u32,
    files: HashMap<String, Cached>,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    size: u64,
    mtime: i64,
    edits: Vec<RawEdit>,
}

struct Job {
    path: PathBuf,
    parse: fn(&[u8]) -> Vec<RawEdit>,
    size: u64,
    mtime: i64,
}

pub fn scan(dirs: &Dirs) -> Scan {
    let mut jobs: Vec<Job> = Vec::new();
    let mut add = |dir: PathBuf, parse: fn(&[u8]) -> Vec<RawEdit>| {
        for path in jsonl_under(&dir) {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            jobs.push(Job {
                path,
                parse,
                size: meta.len(),
                mtime,
            });
        }
    };
    if let Some(dir) = &dirs.claude {
        add(dir.join("projects"), claude::parse);
    }
    if let Some(dir) = &dirs.codex {
        add(dir.join("sessions"), codex::parse);
        add(dir.join("archived_sessions"), codex::parse);
    }

    let cache_file = dirs.cache.as_ref().map(|d| d.join("edits.json"));
    let mut old: Cache = cache_file
        .as_ref()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|c: &Cache| c.version == CACHE_VERSION)
        .unwrap_or_default();

    // Reuse what is known about files that have not changed; read the rest.
    let known: Vec<Option<Cached>> = jobs
        .iter()
        .map(|j| {
            old.files
                .remove(j.path.to_string_lossy().as_ref())
                .filter(|c| c.size == j.size && c.mtime == j.mtime)
        })
        .collect();
    let fresh: Vec<(Cached, bool)> = jobs
        .par_iter()
        .zip(known)
        .map(|(job, known)| match known {
            Some(c) => (c, false),
            None => {
                let edits = map_file(&job.path)
                    .map(|data| (job.parse)(&data))
                    .unwrap_or_default();
                (
                    Cached {
                        size: job.size,
                        mtime: job.mtime,
                        edits,
                    },
                    true,
                )
            }
        })
        .collect();

    let mut scan = Scan {
        files: jobs.len(),
        ..Scan::default()
    };
    let mut new = Cache {
        version: CACHE_VERSION,
        files: HashMap::with_capacity(jobs.len()),
    };
    for (job, (cached, was_read)) in jobs.iter().zip(fresh) {
        scan.bytes += job.size;
        scan.read += usize::from(was_read);
        scan.edits.extend(cached.edits.iter().cloned());
        new.files
            .insert(job.path.to_string_lossy().into_owned(), cached);
    }
    if let Some(file) = cache_file.filter(|_| scan.read > 0 || !old.files.is_empty()) {
        // Best effort: a read-only home directory only costs speed.
        if let (Some(dir), Ok(bytes)) = (file.parent(), serde_json::to_vec(&new)) {
            let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
            if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &file);
            }
        }
    }
    scan
}

fn jsonl_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() && path.extension().is_some_and(|x| x == "jsonl") => {
                    out.push(path)
                }
                _ => {}
            }
        }
    }
    out
}

enum Data {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Mapped(m) => m,
            Data::Owned(v) => v,
        }
    }
}

fn map_file(path: &Path) -> Option<Data> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() == 0 {
        return None;
    }
    // SAFETY: logs are append-only; a concurrent append only adds bytes past the mapped length.
    match unsafe { memmap2::Mmap::map(&file) } {
        Ok(m) => {
            #[cfg(unix)]
            let _ = m.advise(memmap2::Advice::Sequential);
            Some(Data::Mapped(m))
        }
        Err(_) => std::fs::read(path).ok().map(Data::Owned),
    }
}

/// The lines of `data` that contain any of the searcher's needles, in file order, each with
/// the index of the needle that found it. One pass over the data, however many needles.
pub(crate) fn lines_with<'a>(data: &'a [u8], needles: &AhoCorasick) -> Vec<(usize, &'a [u8])> {
    let mut out: Vec<(usize, &[u8])> = Vec::new();
    let mut last: Option<(usize, usize)> = None;
    for m in needles.find_iter(data) {
        let (off, i) = (m.start(), m.pattern().as_usize());
        let start = memrchr(b'\n', &data[..off]).map_or(0, |p| p + 1);
        if last == Some((start, i)) {
            continue;
        }
        last = Some((start, i));
        let end = memchr(b'\n', &data[off..]).map_or(data.len(), |p| off + p);
        out.push((i, &data[start..end]));
    }
    out
}

pub(crate) fn searcher(needles: &[&[u8]]) -> AhoCorasick {
    AhoCorasick::new(needles).expect("static needles")
}

/// A shell command can only have written whole lines of code if it spans lines or redirects
/// output. Everything else (`cargo test`, `git status`) is skipped.
pub(crate) fn writes_text(command: &str) -> bool {
    command.lines().nth(2).is_some() || command.contains('>')
}

/// Non-empty skeletons of `lines`.
pub(crate) fn skeletons<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    lines
        .into_iter()
        .map(crate::lines::skeleton)
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_whole_lines_once_per_needle() {
        let data = b"alpha one\nbeta two two\ngamma\nbeta one";
        let got = lines_with(data, &searcher(&[b"two", b"one"]));
        let as_str: Vec<(usize, &str)> = got
            .iter()
            .map(|(i, l)| (*i, std::str::from_utf8(l).unwrap()))
            .collect();
        assert_eq!(
            as_str,
            vec![(1, "alpha one"), (0, "beta two two"), (1, "beta one")]
        );
    }
}
