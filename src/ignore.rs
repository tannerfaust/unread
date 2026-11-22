//! Which tracked files count as code.

use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "uv.lock",
    "poetry.lock",
    "pipfile.lock",
    "go.sum",
    "composer.lock",
    "gemfile.lock",
    "flake.lock",
    "pubspec.lock",
    "packages.lock.json",
];

const VENDOR_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "third_party",
    "dist",
    "build",
    ".unread",
    "__snapshots__",
];

/// Not code: media, archives, fonts, data dumps.
const DATA_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "icns", "bmp", "svg", "pdf", "zip", "gz", "tar",
    "tgz", "bz2", "xz", "7z", "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4", "mov", "wav",
    "ogg", "webm", "csv", "tsv", "jsonl", "ndjson", "parquet", "sqlite", "db", "bin", "exe", "dll",
    "so", "dylib", "class", "jar", "pyc", "wasm", "map", "snap", "lock", "pem", "crt", "key",
];

/// Prose. Counted only with `--include-docs`.
const DOC_EXT: &[&str] = &["md", "mdx", "rst", "txt", "adoc", "org"];

pub struct Ignore {
    user: GlobSet,
    include_docs: bool,
}

impl Ignore {
    /// Reads `.unreadignore` at the repository root: one pattern per line, gitignore style.
    pub fn load(root: &Path, include_docs: bool) -> Ignore {
        let patterns: Vec<String> = std::fs::read_to_string(root.join(".unreadignore"))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect();
        Ignore {
            user: globs(&patterns),
            include_docs,
        }
    }

    pub fn keep(&self, path: &str) -> bool {
        let lower = path.to_lowercase();
        let name = lower.rsplit('/').next().unwrap_or(&lower);
        if LOCKFILES.contains(&name) || name.contains(".min.") {
            return false;
        }
        if lower
            .split('/')
            .rev()
            .skip(1)
            .any(|dir| VENDOR_DIRS.contains(&dir))
        {
            return false;
        }
        if let Some((_, ext)) = name.rsplit_once('.') {
            if DATA_EXT.contains(&ext) || (!self.include_docs && DOC_EXT.contains(&ext)) {
                return false;
            }
        }
        if !self.include_docs && matches!(name, "license" | "notice" | "authors" | "changelog") {
            return false;
        }
        !self.user.is_match(path)
    }
}

/// Gitignore-flavoured patterns: a bare name matches anywhere, `dir/` matches everything
/// below it, anything with a slash is anchored at the repository root.
pub fn globs(patterns: &[String]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for raw in patterns {
        let p = raw.trim_start_matches("./").trim_start_matches('/');
        let mut variants = Vec::new();
        if let Some(dir) = p.strip_suffix('/') {
            variants.push(format!("{dir}/**"));
            if !dir.contains('/') {
                variants.push(format!("**/{dir}/**"));
            }
        } else {
            variants.push(p.to_string());
            variants.push(format!("{p}/**"));
            if !p.contains('/') {
                variants.push(format!("**/{p}"));
                variants.push(format!("**/{p}/**"));
            }
        }
        for v in variants {
            if let Ok(g) = Glob::new(&v) {
                b.add(g);
            }
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignore(docs: bool) -> Ignore {
        Ignore {
            user: globs(&[
                "fixtures/".to_string(),
                "*.gen.ts".to_string(),
                "docs/api".to_string(),
            ]),
            include_docs: docs,
        }
    }

    #[test]
    fn built_in_rules() {
        let i = ignore(false);
        assert!(i.keep("src/main.rs"));
        assert!(i.keep("config/settings.yaml"));
        assert!(!i.keep("Cargo.lock"));
        assert!(!i.keep("web/node_modules/x/index.js"));
        assert!(!i.keep("assets/logo.png"));
        assert!(!i.keep("app.min.js"));
        assert!(!i.keep("README.md"));
        assert!(ignore(true).keep("README.md"));
    }

    #[test]
    fn user_patterns() {
        let i = ignore(false);
        assert!(!i.keep("fixtures/a.json"));
        assert!(!i.keep("tests/fixtures/a.json"));
        assert!(!i.keep("src/api.gen.ts"));
        assert!(!i.keep("docs/api/index.html"));
        assert!(i.keep("src/api.ts"));
    }
}
