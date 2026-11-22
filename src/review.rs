//! Paying the debt down: read unread code one piece at a time, most urgent first.
//!
//! A piece is marked read only when a person says so, and the time they took is stored with
//! the mark. Marks are bound to the content of each line, so a line that changes later has to
//! be read again.

use crate::analyze::{Analysis, FileReport};
use crate::lines::{is_code, skeleton};
use crate::model::Status;
use crate::render::term;
use crate::store::{append_read, hashes, ReadEntry};
use crate::util::{now, thousands, Paint};
use anyhow::Result;
use std::io::{BufRead, Write};
use std::time::Instant;

/// Unread lines this close together are read as one piece.
const GAP: usize = 4;
const MAX_PIECE: usize = 60;
/// Below this pace the reader is asked whether they really read it.
const SECS_PER_LINE: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub start: usize,
    /// Inclusive.
    pub end: usize,
    pub unread: usize,
}

/// Split a file's unread lines into reviewable pieces.
pub fn pieces(f: &FileReport) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for (i, l) in f.lines.iter().enumerate() {
        if l.status != Status::Unread {
            continue;
        }
        match out.last_mut() {
            Some(p) if i - p.end <= GAP && i - p.start < MAX_PIECE => {
                p.end = i;
                p.unread += 1;
            }
            _ => out.push(Piece {
                start: i,
                end: i,
                unread: 1,
            }),
        }
    }
    out
}

/// A read mark for the unread code lines of `f` in `start..=end`.
pub fn mark(
    f: &FileReport,
    start: usize,
    end: usize,
    by: &str,
    secs: Option<u32>,
) -> Option<ReadEntry> {
    let end = end.min(f.lines.len().saturating_sub(1));
    if f.lines.is_empty() || start > end {
        return None;
    }
    let h: Vec<u64> = (start..=end)
        .filter(|i| f.lines[*i].status == Status::Unread && is_code(&skeleton(&f.text[*i])))
        .map(|i| f.lines[i].hash)
        .collect();
    if h.is_empty() {
        return None;
    }
    Some(ReadEntry {
        v: 1,
        path: f.path.clone(),
        by: by.to_string(),
        at: now(),
        secs,
        h: hashes(h),
    })
}

pub struct Outcome {
    pub pieces: usize,
    pub lines: usize,
}

/// The interactive loop. Reads answers from `input` so it can be driven by a test.
pub fn run(
    a: &Analysis,
    by: &str,
    limit: usize,
    p: Paint,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<Outcome> {
    let queue = a.queue();
    let todo: Vec<(&FileReport, Piece)> = queue
        .iter()
        .flat_map(|f| pieces(f).into_iter().map(move |pc| (*f, pc)))
        .collect();
    let total = todo.len().min(limit);
    let mut done = Outcome {
        pieces: 0,
        lines: 0,
    };
    if total == 0 {
        writeln!(out, "\n  Nothing unread here.\n")?;
        return Ok(done);
    }
    let mut skip_file: Option<&str> = None;
    'pieces: for (i, (f, pc)) in todo.iter().take(limit).enumerate() {
        if skip_file == Some(f.path.as_str()) {
            continue;
        }
        let why = if f.reasons.is_empty() {
            String::new()
        } else {
            format!(" · {}", f.reasons.join(" · "))
        };
        writeln!(
            out,
            "\n  {} {}",
            p.bold(&format!("{}:{}-{}", f.path, pc.start + 1, pc.end + 1)),
            p.dim(&format!(
                "[{}/{}] {} unread lines{why}",
                i + 1,
                total,
                pc.unread
            ))
        )?;
        writeln!(out)?;
        write!(out, "{}", term::chunk(a, f, pc.start, pc.end, p))?;
        let shown = Instant::now();
        loop {
            write!(
                out,
                "\n  {}ead it   {}kip   skip {}ile   {}pen in editor   {}uit  > ",
                p.bold("[r]"),
                p.bold("[s]"),
                p.bold("[f]"),
                p.bold("[o]"),
                p.bold("[q]")
            )?;
            out.flush()?;
            let mut answer = String::new();
            if input.read_line(&mut answer)? == 0 {
                break 'pieces;
            }
            match answer.trim().to_lowercase().as_str() {
                "r" | "read" | "y" => {
                    let secs = shown.elapsed().as_secs_f64();
                    if secs < SECS_PER_LINE * pc.unread as f64 && pc.unread > 5 {
                        write!(
                            out,
                            "  {} lines in {:.0} seconds. Count it as read anyway? [y/N] ",
                            pc.unread, secs
                        )?;
                        out.flush()?;
                        let mut sure = String::new();
                        input.read_line(&mut sure)?;
                        if !matches!(sure.trim().to_lowercase().as_str(), "y" | "yes") {
                            continue;
                        }
                    }
                    if let Some(entry) = mark(f, pc.start, pc.end, by, Some(secs.round() as u32)) {
                        done.lines += entry.h.len();
                        done.pieces += 1;
                        // Written piece by piece so quitting never loses work.
                        append_read(&a.root, &[entry])?;
                    }
                    break;
                }
                "s" | "skip" | "" => break,
                "f" => {
                    skip_file = Some(f.path.as_str());
                    break;
                }
                "o" | "open" => open_editor(a, f, pc.start + 1, out)?,
                "q" | "quit" => break 'pieces,
                _ => {}
            }
        }
    }
    let before = a.total.unread_pct();
    let after = crate::util::pct(
        u64::from(a.total.unread).saturating_sub(done.lines as u64),
        u64::from(a.total.code),
    );
    writeln!(
        out,
        "\n  Read {} lines in {} pieces. Unread: {before:.1}% to {after:.1}%.",
        thousands(done.lines as u64),
        done.pieces
    )?;
    if done.lines > 0 {
        writeln!(
            out,
            "  {}\n",
            p.dim("Saved to .unread/read.jsonl. Commit it so your team and CI see it too.")
        )?;
    } else {
        writeln!(out)?;
    }
    Ok(done)
}

fn open_editor(a: &Analysis, f: &FileReport, line: usize, out: &mut dyn Write) -> Result<()> {
    let Some(editor) = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty())
    else {
        writeln!(out, "  Set $EDITOR to open files from here.")?;
        return Ok(());
    };
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or_default();
    let path = a.root.join(&f.path);
    let mut cmd = std::process::Command::new(program);
    cmd.args(parts);
    let name = std::path::Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ["code", "cursor", "windsurf", "codium"]
        .iter()
        .any(|e| name.starts_with(e))
    {
        cmd.arg("-g").arg(format!("{}:{line}", path.display()));
    } else if ["zed", "subl", "idea", "pycharm", "webstorm"]
        .iter()
        .any(|e| name.starts_with(e))
    {
        cmd.arg(format!("{}:{line}", path.display()));
    } else {
        cmd.arg(format!("+{line}")).arg(&path);
    }
    if cmd.status().is_err() {
        writeln!(out, "  Could not start `{editor}`.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Basis, Counts, Line, NO_AGENT, NO_COMMIT};

    fn file(statuses: &[Status]) -> FileReport {
        let text: Vec<String> = (0..statuses.len())
            .map(|i| format!("let value_{i} = compute({i});"))
            .collect();
        let lines = statuses
            .iter()
            .zip(&text)
            .map(|(s, t)| Line {
                status: *s,
                agent: NO_AGENT,
                seen: None,
                basis: Basis::None,
                hash: crate::lines::hash(&skeleton(t)),
                commit: NO_COMMIT,
                ts: 0,
            })
            .collect();
        FileReport {
            path: "a.rs".into(),
            text,
            lines,
            n: Counts::default(),
            churn: 0,
            risk: 1.0,
            reasons: vec![],
        }
    }

    #[test]
    fn nearby_unread_lines_form_one_piece() {
        use Status::{Human as H, Unread as U};
        let f = file(&[U, U, H, U, H, H, H, H, H, U, U]);
        assert_eq!(
            pieces(&f),
            vec![
                Piece {
                    start: 0,
                    end: 3,
                    unread: 3
                },
                Piece {
                    start: 9,
                    end: 10,
                    unread: 2
                }
            ]
        );
    }

    #[test]
    fn long_runs_are_cut() {
        let f = file(&[Status::Unread; 150]);
        let p = pieces(&f);
        assert_eq!(p.len(), 3);
        assert_eq!(p.iter().map(|x| x.unread).sum::<usize>(), 150);
        assert!(p.iter().all(|x| x.end - x.start < MAX_PIECE));
    }

    #[test]
    fn marks_cover_only_unread_lines() {
        use Status::{Human as H, Unread as U};
        let f = file(&[U, H, U]);
        let e = mark(&f, 0, 99, "me", None).unwrap();
        assert_eq!(e.h.len(), 2);
        assert!(mark(&f, 1, 1, "me", None).is_none());
    }
}
