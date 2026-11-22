use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use unread::analyze::{analyze, Analysis, Config};
use unread::ignore::globs;
use unread::model::{Basis, Seen, Status};
use unread::render::{badge, html, json, term};
use unread::sources::Dirs;
use unread::store::{self, LedgerEntry, Store};
use unread::util::{now, thousands, Paint};
use unread::{review, treemap};

/// Code coverage for human eyes.
///
/// unread measures how much of a repository was written by an agent and never seen by a
/// person. It reads the session logs your coding agents already keep, plus git history, and
/// uploads nothing.
#[derive(Parser)]
#[command(name = "unread", version, max_term_width = 100)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    common: Common,
}

#[derive(Args, Clone)]
struct Common {
    /// Repository to measure
    #[arg(
        short = 'C',
        long = "dir",
        global = true,
        default_value = ".",
        value_name = "DIR"
    )]
    dir: PathBuf,
    /// Only count files matching these paths or globs (repeatable)
    #[arg(short, long = "path", global = true, value_name = "GLOB")]
    paths: Vec<String>,
    /// Only count lines added since this revision, for example `origin/main`
    #[arg(long, global = true, value_name = "REV")]
    base: Option<String>,
    /// Ignore session logs on this machine; use only the repository's ledger and commits
    #[arg(long, global = true)]
    no_logs: bool,
    /// Count prose files (Markdown and similar) as code
    #[arg(long, global = true)]
    include_docs: bool,
    /// Count pull request reviews as evidence. Asks GitHub through `gh`; sends commit hashes only
    #[arg(long, global = true)]
    github: bool,
    /// Ignore lines claimed with `unread claim`
    #[arg(long, global = true)]
    no_claims: bool,
    /// Claude Code config directory [default: $CLAUDE_CONFIG_DIR or ~/.claude]
    #[arg(long, global = true, value_name = "DIR")]
    claude_dir: Option<PathBuf>,
    /// Codex home directory [default: $CODEX_HOME or ~/.codex]
    #[arg(long, global = true, value_name = "DIR")]
    codex_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Measure the repository and print the summary (the default)
    Scan {
        /// Print the full report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Write the map of the repository as a self-contained HTML file
    Report {
        #[arg(short, long, default_value = "unread.html")]
        out: PathBuf,
        /// Do not open the file in a browser
        #[arg(long)]
        no_open: bool,
    },
    /// Write a 1200x630 picture of the number and the map, ready to post
    Card {
        #[arg(short, long, default_value = "unread.svg")]
        out: PathBuf,
    },
    /// Show one file line by line with the evidence behind each label
    Blame { file: String },
    /// Read unread code, most urgent first, and mark what you read
    Next {
        /// Stop after this many pieces
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Mark code as read without the walkthrough: `unread mark src/auth.rs` or `file.rs:10-80`
    Mark {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Claim lines you wrote that a commit marker credits to an agent: `unread claim file.rs:10-80`.
    /// Lines a session log or the ledger shows an agent writing cannot be claimed
    Claim {
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Fail when too much code is unread, for CI
    Check {
        /// Highest allowed share of unread code, in percent
        #[arg(long, value_name = "PERCENT")]
        max_unread: Option<f64>,
        /// Highest allowed number of unread lines
        #[arg(long, value_name = "LINES")]
        max_lines: Option<u32>,
    },
    /// Print a README badge
    Badge {
        /// Write the SVG here instead of printing it
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Print a shields.io endpoint payload instead of an SVG
        #[arg(long)]
        json: bool,
    },
    /// Save what your session logs know into `.unread/ledger.jsonl` so teammates and CI see it
    Record {
        #[arg(short, long)]
        quiet: bool,
    },
    /// Install or remove a pre-commit hook that runs `unread record`
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Show where the evidence comes from
    Sources,
}

#[derive(Subcommand, Clone, Copy)]
enum HookAction {
    Install,
    Remove,
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("unread: {e:#}");
            std::process::exit(2);
        }
    }
}

fn load(c: &Common) -> Result<Analysis> {
    let logs = if c.no_logs {
        Dirs::none()
    } else {
        Dirs::detect(c.claude_dir.clone(), c.codex_dir.clone())
    };
    let cfg = Config {
        dir: c.dir.clone(),
        logs,
        include_docs: c.include_docs,
        base: c.base.clone(),
        github: c.github,
        no_claims: c.no_claims,
    };
    let mut a = analyze(&cfg)?;
    if !c.paths.is_empty() {
        a.retain(&globs(&c.paths));
    }
    Ok(a)
}

fn run(cli: Cli) -> Result<i32> {
    let p = Paint::auto();
    let c = &cli.common;
    match cli.cmd.unwrap_or(Cmd::Scan { json: false }) {
        Cmd::Scan { json: as_json } => {
            let a = load(c)?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&json::report(&a))?);
            } else {
                print!("{}", term::summary(&a, p));
            }
        }
        Cmd::Report { out, no_open } => {
            let a = load(c)?;
            std::fs::write(&out, html::report(&a))
                .with_context(|| format!("cannot write {}", out.display()))?;
            println!("  {} unread. Map written to {}", pct(&a), out.display());
            if !no_open {
                open(&out);
            }
        }
        Cmd::Card { out } => {
            let a = load(c)?;
            std::fs::write(&out, treemap::card(&a.name, a.total, &html::files(&a)))
                .with_context(|| format!("cannot write {}", out.display()))?;
            println!("  {} unread. Card written to {}", pct(&a), out.display());
        }
        Cmd::Blame { file } => {
            let a = load(c)?;
            let f = find_file(&a, &file)?;
            print!("{}", term::blame(&a, f, p));
        }
        Cmd::Next { limit } => {
            let a = load(c)?;
            let by = unread::git::Repo::open(&a.root)?.user();
            let stdin = std::io::stdin();
            review::run(&a, &by, limit, p, &mut stdin.lock(), &mut std::io::stdout())?;
        }
        Cmd::Mark { targets } => {
            let a = load(c)?;
            let by = unread::git::Repo::open(&a.root)?.user();
            let mut entries = Vec::new();
            for t in &targets {
                let (file, range) = split_target(t);
                let f = find_file(&a, file)?;
                let (start, end) = range.unwrap_or((1, f.lines.len()));
                if start == 0 || start > end {
                    bail!("bad line range in `{t}`");
                }
                entries.extend(review::mark(f, start - 1, end - 1, &by, None));
            }
            let lines: usize = entries.iter().map(|e| e.h.len()).sum();
            store::append_read(&a.root, &entries)?;
            println!(
                "  Marked {} lines as read by {by}.",
                thousands(lines as u64)
            );
        }
        Cmd::Claim { targets } => {
            let a = load(c)?;
            let by = unread::git::Repo::open(&a.root)?.user();
            let mut entries = Vec::new();
            let mut refused: BTreeMap<String, (usize, String)> = BTreeMap::new();
            for t in &targets {
                let (file, range) = split_target(t);
                let f = find_file(&a, file)?;
                let (start, end) = range.unwrap_or((1, f.lines.len()));
                if start == 0 || start > end {
                    bail!("bad line range in `{t}`");
                }
                let end = end.min(f.lines.len());
                let mut h: Vec<u64> = Vec::new();
                for l in &f.lines[start - 1..end] {
                    match l.basis {
                        Basis::Commit => h.push(l.hash),
                        Basis::Log | Basis::Shell | Basis::Ledger => {
                            let why = term::reason(&a, l);
                            refused.entry(f.path.clone()).or_insert((0, why)).0 += 1;
                        }
                        Basis::Claimed | Basis::None => {}
                    }
                }
                if !h.is_empty() {
                    entries.push(store::ReadEntry {
                        v: 1,
                        path: f.path.clone(),
                        by: by.clone(),
                        at: now(),
                        secs: None,
                        h: store::hashes(h),
                    });
                }
            }
            let lines: usize = entries.iter().map(|e| e.h.len()).sum();
            store::append_authored(&a.root, &entries)?;
            println!(
                "  Claimed {} lines as written by {by}.",
                thousands(lines as u64)
            );
            if !refused.is_empty() {
                let total: usize = refused.values().map(|(n, _)| n).sum();
                println!(
                    "  Refused {} lines: a session log or the ledger shows an agent writing them.",
                    thousands(total as u64)
                );
                for (path, (n, why)) in &refused {
                    println!("    {:>6}  {path}  {}", thousands(*n as u64), p.dim(why));
                }
            }
        }
        Cmd::Check {
            max_unread,
            max_lines,
        } => {
            let a = load(c)?;
            return Ok(check(&a, &c.paths, max_unread, max_lines, p));
        }
        Cmd::Badge { out, json: as_json } => {
            let a = load(c)?;
            if as_json {
                println!("{}", badge::shields(a.total));
            } else if let Some(out) = out {
                std::fs::write(&out, badge::svg(a.total))
                    .with_context(|| format!("cannot write {}", out.display()))?;
                println!("  Badge written to {}", out.display());
            } else {
                print!("{}", badge::svg(a.total));
            }
        }
        Cmd::Record { quiet } => {
            let a = load(c)?;
            let (lines, files) = record(&a)?;
            if !quiet {
                if lines == 0 {
                    println!("  Ledger is up to date.");
                } else {
                    println!(
                        "  Recorded {} agent-written lines in {} files to .unread/ledger.jsonl.",
                        thousands(lines as u64),
                        files
                    );
                    println!("  {}", p.dim("Commit it so teammates and CI see the same picture. It holds hashes, not code."));
                }
            }
        }
        Cmd::Hook { action } => hook(&c.dir, action)?,
        Cmd::Sources => {
            let a = load(c)?;
            println!("\n{}", term::evidence(&a, p));
        }
    }
    Ok(0)
}

fn pct(a: &Analysis) -> String {
    format!("{:.1}%", a.total.unread_pct())
}

fn find_file<'a>(a: &'a Analysis, wanted: &str) -> Result<&'a unread::analyze::FileReport> {
    let wanted = wanted.trim_start_matches("./");
    if let Some(f) = a.files.iter().find(|f| f.path == wanted) {
        return Ok(f);
    }
    // A path given relative to a subdirectory, or just a file name.
    let tail = format!("/{wanted}");
    let mut matches = a.files.iter().filter(|f| f.path.ends_with(&tail));
    match (matches.next(), matches.next()) {
        (Some(f), None) => Ok(f),
        (Some(_), Some(_)) => bail!("`{wanted}` matches more than one file; give a longer path"),
        _ => bail!("`{wanted}` is not a counted file in this repository"),
    }
}

/// `file.rs:10-80` to (`file.rs`, (10, 80)); `file.rs:10` to a single line.
fn split_target(t: &str) -> (&str, Option<(usize, usize)>) {
    if let Some((file, range)) = t.rsplit_once(':') {
        let parsed = match range.split_once('-') {
            Some((a, b)) => a.parse().ok().zip(b.parse().ok()),
            None => range.parse().ok().map(|n| (n, n)),
        };
        if parsed.is_some() {
            return (file, parsed);
        }
    }
    (t, None)
}

fn check(
    a: &Analysis,
    paths: &[String],
    max_unread: Option<f64>,
    max_lines: Option<u32>,
    p: Paint,
) -> i32 {
    let t = a.total;
    let max_unread = if max_unread.is_none() && max_lines.is_none() {
        Some(0.0)
    } else {
        max_unread
    };
    let mut failures = Vec::new();
    if let Some(limit) = max_unread {
        if t.unread_pct() > limit {
            failures.push(format!("{:.1}% unread, limit {limit}%", t.unread_pct()));
        }
    }
    if let Some(limit) = max_lines {
        if t.unread > limit {
            failures.push(format!(
                "{} unread lines, limit {}",
                thousands(u64::from(t.unread)),
                thousands(u64::from(limit))
            ));
        }
    }
    let place = if paths.is_empty() {
        "the whole repository".to_string()
    } else {
        paths.join(", ")
    };
    let scope = match &a.base {
        Some(b) => format!("lines added since {b} in {place}"),
        None => place,
    };
    let claimed = a
        .files
        .iter()
        .flat_map(|f| &f.lines)
        .filter(|l| l.basis == Basis::Claimed)
        .count();
    let claims = if claimed > 0 {
        format!(
            ", {} human lines rest on claims; --no-claims ignores them",
            thousands(claimed as u64)
        )
    } else {
        String::new()
    };
    if failures.is_empty() {
        println!(
            "  {} {:.1}% unread ({} of {} lines, {scope}{claims})",
            p.ok("ok"),
            t.unread_pct(),
            thousands(u64::from(t.unread)),
            thousands(u64::from(t.code))
        );
        return 0;
    }
    println!(
        "  {} {} ({scope}{claims})",
        p.alert("unread"),
        failures.join("; ")
    );
    for f in a.queue().iter().take(15) {
        let why = if f.reasons.is_empty() {
            String::new()
        } else {
            format!("  {}", p.dim(&f.reasons.join(" · ")))
        };
        println!(
            "    {:>7}  {}{why}",
            thousands(u64::from(f.n.unread)),
            f.path
        );
    }
    println!(
        "  {}",
        p.dim("Read it with `unread next`, then commit .unread/read.jsonl.")
    );
    1
}

/// Append to the ledger every agent-written line the session logs explain and the ledger
/// does not yet hold with at least the same evidence.
fn record(a: &Analysis) -> Result<(usize, usize)> {
    let known = Store::load(&a.root);
    let mut entries = Vec::new();
    let (mut lines, mut files) = (0, 0);
    for f in &a.files {
        let have = known.ledger.get(&f.path);
        let mut groups: BTreeMap<(u8, Option<Seen>), HashSet<u64>> = BTreeMap::new();
        for l in f
            .lines
            .iter()
            .filter(|l| matches!(l.basis, Basis::Log | Basis::Shell) && l.status != Status::Blank)
        {
            // An attestation lives in read.jsonl; the ledger keeps what the log itself showed.
            let seen = l.seen.filter(|s| *s != Seen::Attested);
            if have
                .and_then(|m| m.get(&l.hash))
                .is_some_and(|old| old.seen >= seen)
            {
                continue;
            }
            groups.entry((l.agent, seen)).or_default().insert(l.hash);
        }
        if groups.is_empty() {
            continue;
        }
        files += 1;
        for ((agent, seen), set) in groups {
            lines += set.len();
            entries.push(LedgerEntry {
                v: 1,
                path: f.path.clone(),
                agent: a
                    .agents
                    .get(usize::from(agent))
                    .cloned()
                    .unwrap_or_else(|| "agent".to_string()),
                seen,
                at: now(),
                h: store::hashes(set),
            });
        }
    }
    store::append_ledger(&a.root, &entries)?;
    Ok((lines, files))
}

const HOOK_BEGIN: &str = "# >>> unread >>>";
const HOOK_END: &str = "# <<< unread <<<";

fn hook(dir: &Path, action: HookAction) -> Result<()> {
    let repo = unread::git::Repo::open(dir)?;
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(["rev-parse", "--git-path", "hooks"])
        .output()?;
    let hooks = repo.root.join(String::from_utf8_lossy(&out.stdout).trim());
    let path = hooks.join("pre-commit");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let without = strip_block(&existing);
    match action {
        HookAction::Install => {
            let block = format!(
                "{HOOK_BEGIN}\n# Keeps .unread/ledger.jsonl current so CI and teammates see who wrote what.\nif command -v unread >/dev/null 2>&1; then\n  unread record --quiet && git add .unread 2>/dev/null || true\nfi\n{HOOK_END}\n"
            );
            let base = if without.trim().is_empty() {
                "#!/bin/sh\n".to_string()
            } else {
                format!("{}\n", without.trim_end())
            };
            std::fs::create_dir_all(&hooks)?;
            std::fs::write(&path, format!("{base}{block}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
            println!("  Installed: every commit now records agent-written lines in .unread/ledger.jsonl.");
        }
        HookAction::Remove => {
            if without.trim().is_empty() || without.trim() == "#!/bin/sh" {
                let _ = std::fs::remove_file(&path);
            } else {
                std::fs::write(&path, without)?;
            }
            println!("  Removed the unread pre-commit hook.");
        }
    }
    Ok(())
}

fn strip_block(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim() == HOOK_BEGIN {
            inside = true;
        } else if line.trim() == HOOK_END {
            inside = false;
        } else if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn open(path: &Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program)
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(split_target("src/a.rs"), ("src/a.rs", None));
        assert_eq!(split_target("src/a.rs:10-80"), ("src/a.rs", Some((10, 80))));
        assert_eq!(split_target("src/a.rs:7"), ("src/a.rs", Some((7, 7))));
        assert_eq!(split_target("C:/x/a.rs"), ("C:/x/a.rs", None));
    }

    #[test]
    fn hook_block_is_removed_cleanly() {
        let text = format!("#!/bin/sh\nnpm test\n{HOOK_BEGIN}\nunread record\n{HOOK_END}\n");
        assert_eq!(strip_block(&text), "#!/bin/sh\nnpm test\n");
    }
}
