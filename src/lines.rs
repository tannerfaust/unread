//! Line normalisation shared by every matcher.
//!
//! An agent writes a line, then a formatter re-indents it, swaps its quotes or adds a trailing
//! comma. The line is still the agent's. Matching therefore runs on a "skeleton" that drops
//! everything a formatter is free to change.

/// Whitespace, commas and semicolons dropped, double quotes folded into single quotes.
pub fn skeleton(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        match c {
            ',' | ';' => {}
            '"' => out.push('\''),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

/// A line counts as code when it carries at least three letters or digits. Lone brackets,
/// blank lines and `})` do not count: nobody needs to "read" them.
pub fn is_code(skel: &str) -> bool {
    skel.chars().filter(|c| c.is_alphanumeric()).take(3).count() == 3
}

/// Short lines (`return x`, `else {`) recur by chance inside one file, so an exact match on
/// one of them only counts when a neighbouring line matches too.
pub fn is_weak(skel: &str) -> bool {
    skel.len() < 10
}

/// Lines long enough to identify themselves outside their original file (renames, moves).
pub fn is_strong(skel: &str) -> bool {
    skel.len() >= 24
}

/// FNV-1a, 64 bit. Stable across versions and platforms, which the on-disk ledger relies on.
pub fn hash(skel: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in skel.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn hex(h: u64) -> String {
    format!("{h:016x}")
}

pub fn from_hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_survives_formatters() {
        let agent = "    foo(a, b, \"x\");";
        let formatted = "foo(a,b,'x',)";
        assert_eq!(skeleton(agent), skeleton(formatted));
    }

    #[test]
    fn brackets_are_not_code() {
        assert!(!is_code(&skeleton("  });")));
        assert!(!is_code(&skeleton("")));
        assert!(is_code(&skeleton("return x + 1;")));
    }

    #[test]
    fn hash_is_stable() {
        // The ledger stores these values, so they must never change.
        assert_eq!(hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hex(hash("a")), "af63dc4c8601ec8c");
        assert_eq!(from_hex(&hex(42)), Some(42));
    }
}
