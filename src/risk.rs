//! Which unread code to read first.
//!
//! The score is deliberately simple and every factor is printed next to the file it ranks:
//! unread lines, times a weight for what the path says the code does, times how often the
//! file has been changing.

/// (token prefix, label). A path token matches when it starts with the prefix.
const SENSITIVE: &[(&str, &str)] = &[
    ("auth", "auth"),
    ("login", "auth"),
    ("signin", "auth"),
    ("signup", "auth"),
    ("oauth", "auth"),
    ("sso", "auth"),
    ("mfa", "auth"),
    ("totp", "auth"),
    ("session", "sessions and tokens"),
    ("cookie", "sessions and tokens"),
    ("jwt", "sessions and tokens"),
    ("token", "sessions and tokens"),
    ("secret", "secrets"),
    ("credential", "secrets"),
    ("password", "secrets"),
    ("passwd", "secrets"),
    ("crypt", "crypto"),
    ("cipher", "crypto"),
    ("signature", "crypto"),
    ("permission", "access control"),
    ("acl", "access control"),
    ("rbac", "access control"),
    ("polic", "access control"),
    ("sandbox", "access control"),
    ("payment", "money"),
    ("billing", "money"),
    ("invoice", "money"),
    ("checkout", "money"),
    ("refund", "money"),
    ("payout", "money"),
    ("wallet", "money"),
    ("stripe", "money"),
    ("migration", "data migrations"),
    ("sql", "database"),
    ("security", "security"),
    ("sanitiz", "security"),
    ("csrf", "security"),
    ("cors", "security"),
    ("webhook", "untrusted input"),
    ("upload", "untrusted input"),
    ("deserial", "untrusted input"),
    ("deploy", "deploy"),
    ("delete", "destructive"),
    ("purge", "destructive"),
];

/// Styling: worth reading last.
const STYLE_EXT: &[&str] = &["css", "scss", "sass", "less", "styl"];
/// Markup and configuration: real, but rarely where the logic is.
const CONFIG_EXT: &[&str] = &[
    "html", "json", "yaml", "yml", "toml", "xml", "ini", "cfg", "conf",
];

const LOW_VALUE: &[&str] = &[
    "test", "tests", "spec", "specs", "mock", "mocks", "fixture", "fixtures", "example",
    "examples", "docs", "doc", "demo", "bench", "benches", "stories",
];

pub struct Risk {
    pub score: f64,
    pub reasons: Vec<String>,
}

pub fn assess(path: &str, unread: u32, churn: u32) -> Risk {
    let mut reasons = Vec::new();
    if unread == 0 {
        return Risk {
            score: 0.0,
            reasons,
        };
    }
    let all = tokens(path);
    // What the code does is named close to the file: `auth/models/user.py` is auth code, but
    // a whole `admin/` application is not one sensitive file.
    let near: String = path.rsplit('/').take(3).collect::<Vec<_>>().join("/");
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    let mut weight = 1.0;
    if all.iter().any(|t| LOW_VALUE.contains(&t.as_str())) {
        weight = 0.3;
    } else if STYLE_EXT.contains(&ext.as_str()) {
        weight = 0.25;
    } else if let Some(label) = sensitive(&tokens(&near)) {
        weight = 3.0;
        reasons.push(label.to_string());
    } else if CONFIG_EXT.contains(&ext.as_str()) {
        weight = 0.5;
    }
    let heat = 1.0 + f64::from(churn).ln_1p() / 2.0;
    if churn >= 3 {
        reasons.push(format!("{churn} commits in 90 days"));
    }
    Risk {
        score: f64::from(unread) * weight * heat,
        reasons,
    }
}

fn sensitive(tokens: &[String]) -> Option<&'static str> {
    tokens.iter().find_map(|t| {
        // "author" is not "auth".
        if t.starts_with("author") {
            return None;
        }
        SENSITIVE
            .iter()
            .find(|(prefix, _)| t.starts_with(prefix))
            .map(|(_, label)| *label)
    })
}

/// Lower-cased words of a path: split on punctuation and on camelCase humps.
fn tokens(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in path.chars() {
        let boundary = !c.is_alphanumeric() || (c.is_uppercase() && prev_lower);
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        }
        prev_lower = c.is_lowercase();
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_words() {
        assert_eq!(
            tokens("src/authService/JwtGuard.ts"),
            vec!["src", "auth", "service", "jwt", "guard", "ts"]
        );
    }

    #[test]
    fn sensitive_paths_outrank_tests_and_authors() {
        let auth = assess("src/auth/login.py", 100, 0);
        let plain = assess("src/util/strings.py", 100, 0);
        let test = assess("tests/test_auth.py", 100, 0);
        let author = assess("src/authors.py", 100, 0);
        assert!(auth.score > plain.score && plain.score > test.score);
        assert_eq!(auth.reasons, vec!["auth"]);
        assert_eq!(author.score, plain.score);
        assert_eq!(assess("src/auth/login.py", 0, 50).score, 0.0);
        // Sensitivity is read near the file, and styling ranks last.
        assert_eq!(
            assess("src/auth/models/user.py", 100, 0).reasons,
            vec!["auth"]
        );
        assert_eq!(
            assess("auth/ui/src/pages/Home.tsx", 100, 0).score,
            plain.score
        );
        assert!(assess("src/styles/login.css", 100, 0).score < test.score);
    }

    #[test]
    fn churn_raises_the_score_and_is_named() {
        let hot = assess("src/a.py", 10, 20);
        assert!(hot.score > assess("src/a.py", 10, 0).score);
        assert_eq!(hot.reasons, vec!["20 commits in 90 days"]);
    }
}
