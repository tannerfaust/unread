//! Joins the evidence and labels every line.
//!
//! Evidence is ranked. For "an agent wrote this line", strongest first:
//!
//! 1. the agent's session log shows it adding the line to this file
//! 2. the repository ledger records it
//! 3. the commit that introduced it is marked as agent-authored
//!
//! No evidence means `human`. That makes the unread figure a lower bound: unread never
//! accuses a line it cannot cite a source for.

use crate::attribution::agent_of;
use crate::git::{Blamed, Repo, ZERO_SHA};
use crate::ignore::Ignore;
use crate::lines::{hash, is_code, is_strong, is_weak, skeleton};
use crate::model::{Basis, Counts, Line, Seen, Status, NO_AGENT, NO_COMMIT};
use crate::risk;
use crate::sources::{self, Dirs, RawEdit};
use crate::store::Store;
use anyhow::Result;
use globset::GlobSet;
use memchr::memmem;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

/// Clock skew allowed between an agent's edit and the commit that carries it.
const SLACK: i64 = 900;
/// Shortest run of text worth searching for inside an agent's output.
const MIN_RUN: usize = 12;
const MIN_LINE: usize = 16;

pub struct Config {
    pub dir: PathBuf,
    /// Where to look for session logs. `Dirs::none()` analyses from the repository alone.
    pub logs: Dirs,
    pub include_docs: bool,
    /// Only count lines introduced after this revision.
    pub base: Option<String>,
    /// Ask GitHub which commits arrived through a reviewed pull request.
    pub github: bool,
    /// Ignore `unread claim`, so a claimed line counts as its commit marker says.
    pub no_claims: bool,
}

pub struct Commit {
    pub sha: String,
    pub author: String,
    pub time: i64,
    pub agent: Option<&'static str>,
}

pub struct FileReport {
    pub path: String,
    pub text: Vec<String>,
    pub lines: Vec<Line>,
    pub n: Counts,
    pub churn: u32,
    pub risk: f64,
    pub reasons: Vec<String>,
}

#[derive(Default)]
pub struct Evidence {
    pub log_files: usize,
    pub log_bytes: u64,
    /// Agent edits found in the logs, anywhere.
    pub edits_total: usize,
    /// Of those, edits to this repository.
    pub edits_here: usize,
    /// Shell commands from those sessions whose text is checked against the code.
    pub shell_commands: usize,
    /// Sessions that edited this repository, per agent.
    pub sessions: BTreeMap<String, usize>,
    /// Earliest logged edit to this repository.
    pub since: Option<i64>,
    pub ledger_lines: usize,
    pub read_lines: usize,
    pub claimed_lines: usize,
    pub ai_commits: usize,
    /// Commits GitHub says a person other than the author reviewed. `None` when not asked.
    pub reviewed_commits: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BasisCounts {
    /// A patch in a session log.
    pub log: u32,
    /// A line typed in a shell command in a session log.
    pub shell: u32,
    pub ledger: u32,
    pub commit: u32,
}

pub struct Analysis {
    pub root: PathBuf,
    pub name: String,
    pub head: String,
    pub base: Option<String>,
    pub files: Vec<FileReport>,
    pub agents: Vec<String>,
    pub commits: Vec<Commit>,
    pub total: Counts,
    /// Tracked files left out: data, lockfiles, vendored, generated, binary.
    pub skipped: usize,
    pub evidence: Evidence,
}

#[derive(Clone, Copy)]
struct Rec {
    agent: u8,
    seen: Option<Seen>,
    ts: i64,
}

/// Everything agents wrote to one path.
#[derive(Default)]
struct FileEdits {
    by_hash: HashMap<u64, Rec>,
    /// Skeletons of every added line, concatenated. Searching it finds agent text that a
    /// formatter later re-wrapped across lines.
    blob: String,
    /// End offset in `blob` of each edit, with its record.
    ends: Vec<(usize, Rec)>,
}

impl FileEdits {
    fn find(&self, needle: &str) -> Option<Rec> {
        let at = memmem::find(self.blob.as_bytes(), needle.as_bytes())?;
        let i = self.ends.partition_point(|(end, _)| *end <= at);
        self.ends.get(i).map(|(_, rec)| *rec)
    }
}

/// What the session logs say agents wrote to this repository.
#[derive(Default)]
struct Index {
    files: HashMap<String, FileEdits>,
    /// Lines agents typed in shell commands, by hash. The file is not known, so these only
    /// match whole lines, and short ones only beside another agent line.
    shell: HashMap<u64, Rec>,
}

#[derive(Clone, Copy)]
struct Hit {
    agent: u8,
    seen: Option<Seen>,
    ts: i64,
    basis: Basis,
}

struct Agents {
    names: Vec<String>,
}

impl Agents {
    fn id(&mut self, name: &str) -> u8 {
        if let Some(i) = self.names.iter().position(|n| n == name) {
            return i as u8;
        }
        self.names.push(name.to_string());
        (self.names.len() - 1).min(usize::from(NO_AGENT) - 1) as u8
    }

    fn get(&self, name: &str) -> u8 {
        self.names
            .iter()
            .position(|n| n == name)
            .map_or(NO_AGENT, |i| i as u8)
    }
}

/// `UNREAD_TIMING=1` prints how long each phase took.
fn lap(label: &str, since: &mut std::time::Instant) {
    if std::env::var_os("UNREAD_TIMING").is_some() {
        eprintln!("  {label:<10}{:>8.2}s", since.elapsed().as_secs_f64());
    }
    *since = std::time::Instant::now();
}

pub fn analyze(cfg: &Config) -> Result<Analysis> {
    let mut clock = std::time::Instant::now();
    let repo = Repo::open(&cfg.dir)?;
    let tracked = repo.tracked()?;
    let ignore = Ignore::load(&repo.root, cfg.include_docs);
    let generated = repo.generated(&tracked);
    let wanted: Vec<&String> = tracked
        .iter()
        .filter(|p| ignore.keep(p) && !generated.contains(*p))
        .collect();

    let blamed: Vec<(&String, Blamed)> = wanted
        .par_iter()
        .filter_map(|p| repo.blame(p).map(|b| (*p, b)))
        .collect();
    let skipped = tracked.len() - blamed.len();
    lap("blame", &mut clock);

    // One commit table for the whole repository.
    let mut sha_ix: HashMap<String, u32> = HashMap::new();
    let mut shas: Vec<String> = Vec::new();
    for (_, b) in &blamed {
        for sha in b.shas.iter().filter(|s| s.as_str() != ZERO_SHA) {
            if !sha_ix.contains_key(sha) {
                sha_ix.insert(sha.clone(), shas.len() as u32);
                shas.push(sha.clone());
            }
        }
    }
    let metas: HashMap<String, _> = repo
        .commits(&shas)?
        .into_iter()
        .map(|m| (m.sha.clone(), m))
        .collect();
    let commits: Vec<Commit> = shas
        .iter()
        .map(|sha| match metas.get(sha) {
            Some(m) => Commit {
                sha: sha.clone(),
                author: m.author.clone(),
                time: m.time,
                agent: agent_of(m),
            },
            None => Commit {
                sha: sha.clone(),
                author: String::new(),
                time: i64::MAX - SLACK,
                agent: None,
            },
        })
        .collect();

    let mut agents = Agents { names: Vec::new() };
    let mut evidence = Evidence::default();
    let store = Store::load(&repo.root);
    evidence.ledger_lines = store.ledger_lines();
    evidence.read_lines = store.read_lines();
    evidence.claimed_lines = store.claimed_lines();
    evidence.ai_commits = commits.iter().filter(|c| c.agent.is_some()).count();

    lap("commits", &mut clock);
    let scan = sources::scan(&cfg.logs);
    lap("logs", &mut clock);
    evidence.log_files = scan.files;
    evidence.log_bytes = scan.bytes;
    evidence.edits_total = scan.edits.len();
    let tracked_set: HashSet<&str> = tracked.iter().map(String::as_str).collect();
    let index = index_edits(scan.edits, &repo, &tracked_set, &mut agents, &mut evidence);

    for c in &commits {
        if let Some(a) = c.agent {
            agents.id(a);
        }
    }
    for rec in store.ledger.values().flat_map(HashMap::values) {
        agents.id(&rec.agent);
    }

    let in_range: Option<HashSet<u32>> = match &cfg.base {
        Some(base) => {
            let set = repo.range(base)?;
            Some(
                set.iter()
                    .filter_map(|s| sha_ix.get(s.as_str()).copied())
                    .collect(),
            )
        }
        None => None,
    };

    lap("index", &mut clock);
    let churn = repo.churn(90);
    let ctx = Ctx {
        commits: &commits,
        agents: &agents,
        index: &index,
        store: &store,
        in_range: in_range.as_ref(),
        no_claims: cfg.no_claims,
    };
    let mut files: Vec<FileReport> = blamed
        .into_par_iter()
        .map(|(path, b)| {
            let commit_of: Vec<u32> = b
                .shas
                .iter()
                .map(|s| sha_ix.get(s.as_str()).copied().unwrap_or(NO_COMMIT))
                .collect();
            let lines = classify(path, &b, &commit_of, &ctx);
            let mut n = Counts::default();
            lines.iter().for_each(|l| n.add(l.status));
            let churn = churn.get(path).copied().unwrap_or(0);
            let r = risk::assess(path, n.unread, churn);
            FileReport {
                path: path.clone(),
                text: b.text,
                lines,
                n,
                churn,
                risk: r.score,
                reasons: r.reasons,
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    lap("classify", &mut clock);

    if cfg.github {
        let origin = repo.origin().unwrap_or_default();
        let Some((owner, name)) = crate::github::slug(&origin) else {
            anyhow::bail!(
                "--github needs a github.com `origin` remote; this repository has `{origin}`"
            );
        };
        // Only commits that still hold unread code are worth asking about.
        let wanted: HashSet<u32> = files
            .iter()
            .flat_map(|f| &f.lines)
            .filter(|l| l.status == Status::Unread && l.commit != NO_COMMIT)
            .map(|l| l.commit)
            .collect();
        let shas: Vec<String> = wanted
            .iter()
            .map(|i| commits[*i as usize].sha.clone())
            .collect();
        let reviewed = crate::github::reviewed(&owner, &name, &shas)?;
        evidence.reviewed_commits = Some(reviewed.len());
        for f in &mut files {
            let mut changed = false;
            for l in f
                .lines
                .iter_mut()
                .filter(|l| l.status == Status::Unread && l.commit != NO_COMMIT)
            {
                if reviewed.contains_key(&commits[l.commit as usize].sha) {
                    l.status = Status::Seen;
                    l.seen = Some(Seen::Reviewed);
                    changed = true;
                }
            }
            if changed {
                f.n = Counts::default();
                f.lines.iter().for_each(|l| f.n.add(l.status));
                let r = risk::assess(&f.path, f.n.unread, f.churn);
                f.risk = r.score;
                f.reasons = r.reasons;
            }
        }
        lap("github", &mut clock);
    }

    let mut total = Counts::default();
    files.iter().for_each(|f| total.merge(f.n));
    Ok(Analysis {
        name: repo.name(),
        head: repo.head(),
        root: repo.root,
        base: cfg.base.clone(),
        files,
        agents: agents.names,
        commits,
        total,
        skipped,
        evidence,
    })
}

impl Analysis {
    /// Narrow the report to files matching `only`.
    pub fn retain(&mut self, only: &GlobSet) {
        self.files.retain(|f| only.is_match(&f.path));
        self.total = Counts::default();
        for f in &self.files {
            self.total.merge(f.n);
        }
    }

    /// Files with unread code, most urgent first.
    pub fn queue(&self) -> Vec<&FileReport> {
        let mut q: Vec<&FileReport> = self.files.iter().filter(|f| f.n.unread > 0).collect();
        q.sort_by(|a, b| b.risk.total_cmp(&a.risk).then_with(|| a.path.cmp(&b.path)));
        q
    }

    /// Lines written and lines still unread, per agent.
    pub fn by_agent(&self) -> Vec<(String, u32, u32)> {
        let mut out: Vec<(String, u32, u32)> =
            self.agents.iter().map(|a| (a.clone(), 0, 0)).collect();
        for l in self.files.iter().flat_map(|f| &f.lines) {
            if let Some(slot) = out.get_mut(usize::from(l.agent)) {
                match l.status {
                    Status::Seen => slot.1 += 1,
                    Status::Unread => {
                        slot.1 += 1;
                        slot.2 += 1;
                    }
                    _ => {}
                }
            }
        }
        out.retain(|(_, written, _)| *written > 0);
        out.sort_by_key(|(_, written, _)| std::cmp::Reverse(*written));
        out
    }

    pub fn by_seen(&self) -> Vec<(Seen, u32)> {
        let mut out: Vec<(Seen, u32)> = Seen::ALL.iter().map(|s| (*s, 0)).collect();
        for l in self
            .files
            .iter()
            .flat_map(|f| &f.lines)
            .filter(|l| l.status == Status::Seen)
        {
            if let Some(slot) = out.iter_mut().find(|(s, _)| Some(*s) == l.seen) {
                slot.1 += 1;
            }
        }
        out.retain(|(_, n)| *n > 0);
        out
    }

    /// Agent-written lines by what says an agent wrote them.
    pub fn by_basis(&self) -> BasisCounts {
        let mut out = BasisCounts::default();
        for l in self.files.iter().flat_map(|f| &f.lines) {
            match l.basis {
                Basis::Log => out.log += 1,
                Basis::Shell => out.shell += 1,
                Basis::Ledger => out.ledger += 1,
                Basis::Commit => out.commit += 1,
                Basis::Claimed | Basis::None => {}
            }
        }
        out
    }

    /// Where the unread code is, as a cut through the directory tree. The largest directory
    /// is split into its children until there are `limit` rows, so `src/` alone never hides
    /// everything beneath it.
    pub fn dirs(&self, limit: usize) -> Vec<(String, Counts)> {
        let mut total: BTreeMap<&str, Counts> = BTreeMap::new();
        let mut own: HashMap<&str, Counts> = HashMap::new();
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for f in &self.files {
            let dir = f.path.rfind('/').map_or("", |i| &f.path[..i]);
            own.entry(dir).or_default().merge(f.n);
            let mut d = dir;
            loop {
                let first_visit = !total.contains_key(d);
                total.entry(d).or_default().merge(f.n);
                if d.is_empty() {
                    break;
                }
                let parent = d.rfind('/').map_or("", |i| &d[..i]);
                if first_visit {
                    children.entry(parent).or_default().push(d);
                }
                d = parent;
            }
        }
        // (label, counts, directory that can still be split)
        let mut rows: Vec<(String, Counts, Option<&str>)> =
            vec![(String::new(), Counts::default(), Some(""))];
        let all_unread = total.get("").map_or(0, |n| n.unread);
        loop {
            let pick = rows
                .iter()
                .enumerate()
                .filter(|(_, (_, _, dir))| dir.is_some_and(|d| children.contains_key(d)))
                .max_by_key(|(_, (_, n, _))| n.unread)
                .map(|(i, _)| i);
            let Some(i) = pick else { break };
            let dir = rows[i].2.expect("filtered on Some");
            let kids = &children[dir];
            let own_here = own.get(dir).copied().unwrap_or_default();
            let grows_by = kids.len() + usize::from(own_here.unread > 0) - 1;
            // Always open a directory holding more than a quarter of everything unread;
            // otherwise only while there is room.
            let dominant = dir.is_empty() || rows[i].1.unread * 4 > all_unread;
            if !dominant && rows.len() + grows_by > limit {
                rows[i].2 = None;
                continue;
            }
            rows.swap_remove(i);
            rows.extend(kids.iter().map(|k| (k.to_string(), total[k], Some(*k))));
            if own_here.unread > 0 {
                let label = if dir.is_empty() {
                    "(top level)".to_string()
                } else {
                    format!("{dir}/*")
                };
                rows.push((label, own_here, None));
            }
        }
        let mut out: Vec<(String, Counts)> = rows
            .into_iter()
            .filter(|(_, n, _)| n.unread > 0)
            .map(|(l, n, _)| (l, n))
            .collect();
        out.sort_by(|a, b| b.1.unread.cmp(&a.1.unread).then_with(|| a.0.cmp(&b.0)));
        out.truncate(limit);
        out
    }
}

/// Maps the absolute paths agents logged onto paths in this repository.
struct PathMap<'a> {
    /// The checkout, its worktrees, and prefixes learned from clones elsewhere. Longest first.
    roots: Vec<String>,
    tracked: &'a HashSet<&'a str>,
}

impl<'a> PathMap<'a> {
    fn new(repo: &Repo, tracked: &'a HashSet<&'a str>, edits: &[RawEdit]) -> PathMap<'a> {
        let mut roots: Vec<String> = Vec::new();
        let mut add = |p: &str| {
            let p = p.replace('\\', "/");
            let p = format!("{}/", p.trim_end_matches('/'));
            if !roots.contains(&p) {
                roots.push(p);
            }
        };
        let root = repo.root.to_string_lossy().into_owned();
        add(&root);
        if let Ok(real) = repo.root.canonicalize() {
            add(&real.to_string_lossy());
        }
        for w in repo.worktrees() {
            add(&w);
        }
        let mut map = PathMap { roots, tracked };

        // A checkout that no longer exists (a deleted worktree, a clone in another folder)
        // still shows in the logs. Its prefix is trusted when it names this repository and
        // the rest of the path is a file tracked here.
        let name = repo.name().to_lowercase();
        if name.len() >= 5 {
            let mut learned: HashSet<String> = HashSet::new();
            for e in edits.iter().filter(|e| !e.shell) {
                let abs = e.path.replace('\\', "/");
                if map.roots.iter().any(|r| abs.starts_with(r.as_str())) {
                    continue;
                }
                if let Some(rel) = map.tracked_suffix(&abs) {
                    let prefix = &abs[..abs.len() - rel.len()];
                    if prefix.to_lowercase().contains(&name) {
                        learned.insert(prefix.to_string());
                    }
                }
            }
            map.roots.extend(learned);
        }
        map.roots
            .sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        map.roots.dedup();
        map
    }

    /// The longest tail of `path` that is a tracked file.
    fn tracked_suffix<'p>(&self, path: &'p str) -> Option<&'p str> {
        let mut rest = path;
        loop {
            if self.tracked.contains(rest) {
                return Some(rest);
            }
            rest = &rest[rest.find('/')? + 1..];
        }
    }

    fn rel(&self, abs: &str) -> Option<String> {
        let abs = abs.replace('\\', "/");
        let root = self.roots.iter().find(|r| abs.starts_with(r.as_str()))?;
        let rest = &abs[root.len()..];
        // Under a root but not tracked: a nested worktree that is gone, or a file that has
        // since been renamed. Blame knows old names, so keep the path as it was.
        Some(self.tracked_suffix(rest).unwrap_or(rest).to_string())
    }
}

fn index_edits(
    mut edits: Vec<RawEdit>,
    repo: &Repo,
    tracked: &HashSet<&str>,
    agents: &mut Agents,
    evidence: &mut Evidence,
) -> Index {
    let mut index = Index::default();
    if edits.is_empty() {
        return index;
    }
    edits.sort_by_key(|e| e.ts);
    let map = PathMap::new(repo, tracked, &edits);
    let mut sessions: HashSet<(String, String)> = HashSet::new();
    let (commands, patches): (Vec<RawEdit>, Vec<RawEdit>) =
        edits.into_iter().partition(|e| e.shell);
    for e in patches {
        let Some(rel) = map.rel(&e.path) else {
            continue;
        };
        evidence.edits_here += 1;
        evidence.since = Some(evidence.since.map_or(e.ts, |s| s.min(e.ts)));
        let rec = Rec {
            agent: agents.id(&e.agent),
            seen: e.seen,
            ts: e.ts,
        };
        sessions.insert((e.agent, e.session));
        let file = index.files.entry(rel).or_default();
        for s in &e.added {
            // Edits arrive oldest first: the first writer keeps the line, later evidence that
            // a person saw it still counts.
            file.by_hash
                .entry(hash(s))
                .and_modify(|old| old.seen = old.seen.max(rec.seen))
                .or_insert(rec);
            file.blob.push_str(s);
        }
        file.ends.push((file.blob.len(), rec));
    }

    // A shell command belongs to this repository when its session patched files here, when it
    // ran inside a checkout, or when it names one.
    let home_form = format!("~/{}", repo.name());
    for e in commands {
        let key = (e.agent, e.session);
        let cwd = format!("{}/", e.path.replace('\\', "/").trim_end_matches('/'));
        let here = sessions.contains(&key)
            || map.roots.iter().any(|r| cwd.starts_with(r.as_str()))
            || e.added.iter().any(|l| {
                l.contains(&home_form)
                    || map
                        .roots
                        .iter()
                        .any(|r| l.contains(r.trim_end_matches('/')))
            });
        if !here {
            continue;
        }
        evidence.shell_commands += 1;
        evidence.since = Some(evidence.since.map_or(e.ts, |s| s.min(e.ts)));
        let rec = Rec {
            agent: agents.id(&key.0),
            seen: e.seen,
            ts: e.ts,
        };
        sessions.insert(key);
        for s in &e.added {
            index
                .shell
                .entry(hash(s))
                .and_modify(|old| old.seen = old.seen.max(rec.seen))
                .or_insert(rec);
        }
    }
    for (agent, _) in sessions {
        *evidence.sessions.entry(agent).or_insert(0) += 1;
    }
    index
}

struct Ctx<'a> {
    commits: &'a [Commit],
    agents: &'a Agents,
    index: &'a Index,
    store: &'a Store,
    in_range: Option<&'a HashSet<u32>>,
    no_claims: bool,
}

fn classify(path: &str, b: &Blamed, commit_of: &[u32], ctx: &Ctx) -> Vec<Line> {
    let n = b.text.len();
    let skel: Vec<String> = b.text.iter().map(|t| skeleton(t)).collect();
    let code: Vec<bool> = skel.iter().map(|s| is_code(s)).collect();
    let hashes: Vec<u64> = skel.iter().map(|s| hash(s)).collect();
    let commit: Vec<u32> = b.line_sha.iter().map(|i| commit_of[*i as usize]).collect();
    let time = |c: u32| {
        if c == NO_COMMIT {
            i64::MAX - SLACK
        } else {
            ctx.commits[c as usize].time
        }
    };
    let fits = |rec: &Rec, c: u32| rec.ts <= time(c) + SLACK;

    // This path, then every name the file had when its lines were written.
    let mut names: Vec<&str> = vec![path];
    names.extend(b.paths.iter().map(String::as_str).filter(|p| *p != path));
    let edits: Vec<&FileEdits> = names
        .iter()
        .filter_map(|p| ctx.index.files.get(*p))
        .collect();
    let ledgers: Vec<_> = names
        .iter()
        .filter_map(|p| ctx.store.ledger.get(*p))
        .collect();
    let reads: Vec<_> = names
        .iter()
        .filter_map(|p| ctx.store.read.get(*p))
        .collect();
    let claims: Vec<_> = names
        .iter()
        .filter_map(|p| ctx.store.claimed.get(*p))
        .collect();

    // 1. Exact matches: patches in session logs, the ledger, then shell command text.
    let mut hit: Vec<Option<Hit>> = vec![None; n];
    for i in (0..n).filter(|i| code[*i]) {
        let h = hashes[i];
        if let Some(rec) = edits
            .iter()
            .find_map(|f| f.by_hash.get(&h))
            .filter(|r| fits(r, commit[i]))
        {
            hit[i] = Some(Hit {
                agent: rec.agent,
                seen: rec.seen,
                ts: rec.ts,
                basis: Basis::Log,
            });
        } else if let Some(l) = ledgers.iter().find_map(|m| m.get(&h)) {
            hit[i] = Some(Hit {
                agent: ctx.agents.get(&l.agent),
                seen: l.seen,
                ts: 0,
                basis: Basis::Ledger,
            });
        } else if let Some(rec) = ctx.index.shell.get(&h).filter(|r| fits(r, commit[i])) {
            hit[i] = Some(Hit {
                agent: rec.agent,
                seen: rec.seen,
                ts: rec.ts,
                basis: Basis::Shell,
            });
        }
    }

    // 2. A short line only counts next to another agent line.
    let marked =
        |i: usize| commit[i] != NO_COMMIT && ctx.commits[commit[i] as usize].agent.is_some();
    let supported: Vec<bool> = (0..n)
        .map(|i| hit[i].is_some() || (code[i] && marked(i)))
        .collect();
    for i in 0..n {
        let Some(h) = hit[i] else { continue };
        // Shell text is not tied to a file, so it has to identify itself more strongly.
        let alone_is_enough = if h.basis == Basis::Shell {
            is_strong(&skel[i])
        } else {
            !is_weak(&skel[i])
        };
        if alone_is_enough {
            continue;
        }
        let above = (0..i)
            .rev()
            .find(|j| code[*j])
            .is_some_and(|j| supported[j]);
        let below = (i + 1..n).find(|j| code[*j]).is_some_and(|j| supported[j]);
        if !above && !below {
            hit[i] = None;
        }
    }

    // 3. Text a formatter re-wrapped: look for it inside what the agent wrote. Starting at
    // each unexplained line, take the longest stretch of following lines whose joined text
    // appears in the agent's output. One long line is enough; short lines need company.
    if !edits.is_empty() {
        let find = |needle: &str| edits.iter().find_map(|f| f.find(needle));
        let mut s = 0;
        while s < n {
            if !code[s] || hit[s].is_some() {
                s += 1;
                continue;
            }
            let mut needle = String::new();
            let mut best: Option<(usize, Rec)> = None;
            let mut e = s;
            while e < n && (!code[e] || hit[e].is_none()) {
                needle.push_str(&skel[e]);
                if needle.len() >= MIN_RUN {
                    match find(&needle) {
                        Some(rec) => best = Some((e, rec)),
                        None => break,
                    }
                }
                e += 1;
            }
            let accepted = best.filter(|(last, rec)| {
                let lines = (s..=*last).filter(|k| code[*k]).count();
                let long_enough = lines > 1 || skel[s].len() >= MIN_LINE;
                long_enough
                    && (s..=*last)
                        .filter(|k| code[*k])
                        .all(|k| fits(rec, commit[k]))
            });
            match accepted {
                Some((last, rec)) => {
                    for k in (s..=last).filter(|k| code[*k]) {
                        hit[k] = Some(Hit {
                            agent: rec.agent,
                            seen: rec.seen,
                            ts: rec.ts,
                            basis: Basis::Log,
                        });
                    }
                    s = last + 1;
                }
                None => s += 1,
            }
        }
    }

    // 4. Commit markers for what is left, then the verdict.
    (0..n)
        .map(|i| {
            let mut line = Line {
                status: Status::Blank,
                agent: NO_AGENT,
                seen: None,
                basis: Basis::None,
                hash: hashes[i],
                commit: commit[i],
                ts: 0,
            };
            if !code[i] {
                return line;
            }
            if let Some(range) = ctx.in_range {
                if commit[i] != NO_COMMIT && !range.contains(&commit[i]) {
                    return line;
                }
            }
            // A claim corrects a commit marker; it never contradicts a log or the ledger.
            if hit[i].is_none() && !ctx.no_claims && claims.iter().any(|c| c.contains(&hashes[i])) {
                line.status = Status::Human;
                line.basis = Basis::Claimed;
                return line;
            }
            let found = hit[i].or_else(|| {
                let agent = ctx.commits.get(commit[i] as usize)?.agent?;
                Some(Hit {
                    agent: ctx.agents.get(agent),
                    seen: None,
                    ts: 0,
                    basis: Basis::Commit,
                })
            });
            let Some(h) = found else {
                line.status = Status::Human;
                return line;
            };
            let attested = reads.iter().any(|r| r.contains(&hashes[i]))
                || (is_strong(&skel[i]) && ctx.store.read_any.contains(&hashes[i]));
            line.agent = h.agent;
            line.basis = h.basis;
            line.ts = h.ts;
            line.seen = if attested {
                Some(Seen::Attested)
            } else {
                h.seen
            };
            line.status = if line.seen.is_some() {
                Status::Seen
            } else {
                Status::Unread
            };
            line
        })
        .collect()
}
