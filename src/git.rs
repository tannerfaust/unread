//! Everything unread asks of git, through the `git` binary.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

pub struct Repo {
    pub root: PathBuf,
}

/// One file as `git blame` sees it: its lines and the commit behind each.
pub struct Blamed {
    pub text: Vec<String>,
    /// Distinct commits, in order of first appearance.
    pub shas: Vec<String>,
    /// Per line, an index into `shas`.
    pub line_sha: Vec<u32>,
    /// Paths the lines had when they were written. Blame follows renames, so an agent's edit
    /// to `old.rs` still explains a line that now lives in `new.rs`.
    pub paths: Vec<String>,
    /// Per line, an index into `paths`, or `u32::MAX` when unknown.
    pub line_path: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct CommitMeta {
    pub sha: String,
    pub author: String,
    pub email: String,
    pub committer: String,
    pub committer_email: String,
    /// Committer time. An agent's edit can never be later than the commit that carries it.
    pub time: i64,
    pub message: String,
}

impl Repo {
    pub fn open(dir: &Path) -> Result<Repo> {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("git is not installed or not on PATH")?;
        if !out.status.success() {
            bail!("{} is not inside a git repository", dir.display());
        }
        let root = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        Ok(Repo {
            root: PathBuf::from(root),
        })
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(&self.root)
            .args(["-c", "core.quotePath=false"]);
        c
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out = self.cmd().args(args).stderr(Stdio::null()).output()?;
        if !out.status.success() {
            bail!("git {} failed", args.join(" "));
        }
        Ok(out.stdout)
    }

    fn text(&self, args: &[&str]) -> Option<String> {
        self.run(args)
            .ok()
            .map(|o| String::from_utf8_lossy(&o).trim().to_string())
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn head(&self) -> String {
        self.text(&["rev-parse", "--short", "HEAD"])
            .unwrap_or_default()
    }

    /// `name <email>` of whoever is at the keyboard, for attestations.
    pub fn user(&self) -> String {
        let name = self.text(&["config", "user.name"]).unwrap_or_default();
        let email = self.text(&["config", "user.email"]).unwrap_or_default();
        match (name.is_empty(), email.is_empty()) {
            (false, false) => format!("{name} <{email}>"),
            (false, true) => name,
            (true, false) => email,
            (true, true) => "unknown".to_string(),
        }
    }

    /// The `origin` URL, used to recognise sessions that worked on another clone.
    pub fn origin(&self) -> Option<String> {
        self.text(&["config", "--get", "remote.origin.url"])
            .filter(|s| !s.is_empty())
    }

    /// Files in the index: committed files and files staged with `git add`. Uncommitted
    /// changes to them count; files git has not been told about do not.
    pub fn tracked(&self) -> Result<Vec<String>> {
        let out = self.run(&["ls-files", "-z"])?;
        Ok(out
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    /// Checkouts of this repository other than `root`.
    pub fn worktrees(&self) -> Vec<String> {
        let Some(out) = self.text(&["worktree", "list", "--porcelain"]) else {
            return Vec::new();
        };
        out.lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(str::to_string)
            .collect()
    }

    /// Paths the repository itself declares generated or vendored in `.gitattributes`.
    pub fn generated(&self, paths: &[String]) -> HashSet<String> {
        let mut found = HashSet::new();
        let Ok(mut child) = self
            .cmd()
            .args([
                "check-attr",
                "-z",
                "--stdin",
                "linguist-generated",
                "linguist-vendored",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return found;
        };
        let mut stdin = child.stdin.take().expect("piped stdin");
        let input: Vec<u8> = paths
            .iter()
            .flat_map(|p| p.bytes().chain(std::iter::once(0)))
            .collect();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let out = child.wait_with_output();
        let _ = writer.join();
        let Ok(out) = out else { return found };
        let fields: Vec<&[u8]> = out.stdout.split(|b| *b == 0).collect();
        for triple in fields.chunks(3) {
            if let [path, _attr, value] = triple {
                if *value == b"set" || *value == b"true" {
                    found.insert(String::from_utf8_lossy(path).into_owned());
                }
            }
        }
        found
    }

    /// Blame of the working-tree version of `path`. Falls back to the plain file when git has
    /// no history for it yet (a new repository, a file added but never committed).
    pub fn blame(&self, path: &str) -> Option<Blamed> {
        let out = self
            .cmd()
            .args(["blame", "--porcelain", "--", path])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if out.status.success() {
            if out.stdout.contains(&0) {
                return None; // binary
            }
            return Some(parse_porcelain(&out.stdout));
        }
        let bytes = std::fs::read(self.root.join(path)).ok()?;
        if bytes.contains(&0) {
            return None;
        }
        let text: Vec<String> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_string)
            .collect();
        let n = text.len();
        Some(Blamed {
            text,
            shas: vec![ZERO_SHA.to_string()],
            line_sha: vec![0; n],
            paths: Vec::new(),
            line_path: vec![u32::MAX; n],
        })
    }

    pub fn commits(&self, shas: &[String]) -> Result<Vec<CommitMeta>> {
        let real: Vec<&str> = shas
            .iter()
            .map(String::as_str)
            .filter(|s| *s != ZERO_SHA)
            .collect();
        if real.is_empty() {
            return Ok(Vec::new());
        }
        let mut child = self
            .cmd()
            .args([
                "log",
                "--no-walk=unsorted",
                "--stdin",
                "--format=%H%x1f%an%x1f%ae%x1f%cn%x1f%ce%x1f%ct%x1f%B%x1d",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let input = real.join("\n");
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
        let out = child.wait_with_output()?;
        let _ = writer.join();
        let text = String::from_utf8_lossy(&out.stdout);
        let mut metas = Vec::with_capacity(real.len());
        for record in text.split('\x1d') {
            let f: Vec<&str> = record.trim_start_matches('\n').split('\x1f').collect();
            if f.len() < 7 {
                continue;
            }
            metas.push(CommitMeta {
                sha: f[0].to_string(),
                author: f[1].to_string(),
                email: f[2].to_string(),
                committer: f[3].to_string(),
                committer_email: f[4].to_string(),
                time: f[5].parse().unwrap_or(0),
                message: f[6].to_string(),
            });
        }
        Ok(metas)
    }

    /// Commits touching each file in the last `days` days.
    pub fn churn(&self, days: u32) -> HashMap<String, u32> {
        let since = format!("--since={days}.days");
        let mut map = HashMap::new();
        if let Ok(out) = self.run(&["log", &since, "--format=", "--name-only"]) {
            for line in String::from_utf8_lossy(&out)
                .lines()
                .filter(|l| !l.is_empty())
            {
                *map.entry(line.to_string()).or_insert(0) += 1;
            }
        }
        map
    }

    /// Commits in `base..HEAD`.
    pub fn range(&self, base: &str) -> Result<HashSet<String>> {
        let spec = format!("{base}..HEAD");
        let out = self
            .run(&["rev-list", &spec])
            .with_context(|| format!("cannot resolve revision `{base}`"))?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(str::to_string)
            .collect())
    }
}

fn parse_porcelain(out: &[u8]) -> Blamed {
    let mut text = Vec::new();
    let mut shas: Vec<String> = Vec::new();
    let mut sha_index: HashMap<String, u32> = HashMap::new();
    let mut line_sha = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    let mut path_index: HashMap<String, u32> = HashMap::new();
    let mut line_path = Vec::new();
    // Git prints a commit's `filename` once, or with every group when the commit contributed
    // lines from more than one path, so the latest value per commit is always the right one.
    let mut sha_path: Vec<u32> = Vec::new();
    let mut current = 0usize;
    for line in out.split(|b| *b == b'\n') {
        if line.first() == Some(&b'\t') {
            text.push(String::from_utf8_lossy(&line[1..]).into_owned());
            line_sha.push(current as u32);
            line_path.push(sha_path.get(current).copied().unwrap_or(u32::MAX));
        } else if line.len() > 40
            && line[40] == b' '
            && line[..40].iter().all(u8::is_ascii_hexdigit)
        {
            let sha = String::from_utf8_lossy(&line[..40]).into_owned();
            current = match sha_index.get(&sha) {
                Some(i) => *i as usize,
                None => {
                    let i = shas.len();
                    shas.push(sha.clone());
                    sha_path.push(u32::MAX);
                    sha_index.insert(sha, i as u32);
                    i
                }
            };
        } else if let Some(name) = line.strip_prefix(b"filename ") {
            let name = String::from_utf8_lossy(name).into_owned();
            let next = paths.len() as u32;
            let i = *path_index.entry(name.clone()).or_insert_with(|| {
                paths.push(name);
                next
            });
            if let Some(slot) = sha_path.get_mut(current) {
                *slot = i;
            }
        }
    }
    Blamed {
        text,
        shas,
        line_sha,
        paths,
        line_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_lines_follow_their_commit() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor X\nsummary one\nfilename f\n\tfirst\n{a} 2 2\n\tsecond\n{b} 1 3 1\nauthor Y\nfilename f\n\t\tthird\n"
        );
        let got = parse_porcelain(out.as_bytes());
        assert_eq!(got.text, vec!["first", "second", "\tthird"]);
        assert_eq!(got.shas, vec![a, b]);
        assert_eq!(got.line_sha, vec![0, 0, 1]);
        assert_eq!(got.paths, vec!["f"]);
        assert_eq!(got.line_path, vec![0, 0, 0]);
    }
}
