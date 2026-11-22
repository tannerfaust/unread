//! The three diff dialects agent logs use.

/// Added lines of a unified diff given as text. File headers (`+++ b/x`) are not additions.
pub fn added_from_unified(diff: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut after_minus_header = false;
    for line in diff.lines() {
        if line.starts_with("--- ") {
            after_minus_header = true;
            continue;
        }
        if after_minus_header && line.starts_with("+++ ") {
            after_minus_header = false;
            continue;
        }
        after_minus_header = false;
        if let Some(rest) = line.strip_prefix('+') {
            out.push(rest);
        }
    }
    out
}

/// Added lines of a hunk given as an array of prefixed lines (`+x`, `-y`, ` z`).
pub fn added_from_hunk_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    lines.filter_map(|l| l.strip_prefix('+')).collect()
}

/// The `*** Begin Patch` envelope: one entry per touched file with the lines it adds.
/// Paths come back as written, which may be relative to the session's working directory.
pub fn parse_apply_patch(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("*** Begin Patch") {
            inside = true;
            continue;
        }
        if line.starts_with("*** End Patch") {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(p) = line
            .strip_prefix("*** Add File: ")
            .or_else(|| line.strip_prefix("*** Update File: "))
        {
            out.push((p.trim().to_string(), Vec::new()));
        } else if let Some(p) = line.strip_prefix("*** Move to: ") {
            if let Some(last) = out.last_mut() {
                last.0 = p.trim().to_string();
            }
        } else if line.starts_with("*** Delete File: ") {
            out.push((String::new(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.1.push(rest.to_string());
            }
        }
    }
    out.retain(|(p, lines)| !p.is_empty() && !lines.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_skips_headers_only() {
        let diff = "--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,3 @@\n keep\n-old\n+new\n+++ not a header\n";
        assert_eq!(added_from_unified(diff), vec!["new", "++ not a header"]);
    }

    #[test]
    fn apply_patch_envelope() {
        let patch = "*** Begin Patch\n*** Add File: a.py\n+x = 1\n+y = 2\n*** Update File: old.py\n*** Move to: new.py\n@@\n-a\n+b\n*** Delete File: gone.py\n*** End Patch\n";
        let got = parse_apply_patch(patch);
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0],
            (
                "a.py".to_string(),
                vec!["x = 1".to_string(), "y = 2".to_string()]
            )
        );
        assert_eq!(got[1], ("new.py".to_string(), vec!["b".to_string()]));
    }
}
