//! Pull request reviews as evidence that a person saw agent-written code.
//!
//! Opt-in (`--github`), because it is the one thing unread does over the network. It asks
//! GitHub, through the `gh` CLI you are already signed in to, which commits arrived in a pull
//! request that someone other than its author reviewed. Only commit hashes are sent.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub pr: u64,
    pub by: String,
}

const BATCH: usize = 40;

/// `owner/name` from a GitHub remote URL, in any of the forms git accepts.
pub fn slug(origin: &str) -> Option<(String, String)> {
    let rest = origin.trim().split_once("github.com")?.1;
    let rest = rest.trim_start_matches([':', '/']).trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner.to_string(), name.to_string()))
}

fn query(owner: &str, name: &str, shas: &[String]) -> String {
    let mut q = format!("query {{ repository(owner: \"{owner}\", name: \"{name}\") {{");
    for (i, sha) in shas.iter().enumerate() {
        q.push_str(&format!(
            " c{i}: object(oid: \"{sha}\") {{ ... on Commit {{ associatedPullRequests(first: 5) {{ nodes {{ number author {{ login }} reviews(first: 50) {{ nodes {{ state author {{ __typename login }} }} }} }} }} }} }}"
        ));
    }
    q.push_str(" } }");
    q
}

/// Reviews found in one GraphQL response, keyed by commit.
fn parse(response: &Value, shas: &[String]) -> HashMap<String, Review> {
    let mut out = HashMap::new();
    for (i, sha) in shas.iter().enumerate() {
        let prs = response.pointer(&format!(
            "/data/repository/c{i}/associatedPullRequests/nodes"
        ));
        for pr in prs.and_then(Value::as_array).into_iter().flatten() {
            let author = pr
                .pointer("/author/login")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let reviewer = pr
                .pointer("/reviews/nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|r| {
                    let state = r.get("state").and_then(Value::as_str).unwrap_or_default();
                    let who = r
                        .pointer("/author/login")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    // A bot's review is not a person looking, and nobody reviews themselves.
                    matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED")
                        && r.pointer("/author/__typename").and_then(Value::as_str) == Some("User")
                        && !who.is_empty()
                        && who != author
                })
                .and_then(|r| r.pointer("/author/login").and_then(Value::as_str));
            if let (Some(by), Some(number)) = (reviewer, pr.get("number").and_then(Value::as_u64)) {
                out.insert(
                    sha.clone(),
                    Review {
                        pr: number,
                        by: by.to_string(),
                    },
                );
                break;
            }
        }
    }
    out
}

/// Which of `shas` came in through a pull request reviewed by someone other than its author.
pub fn reviewed(owner: &str, name: &str, shas: &[String]) -> Result<HashMap<String, Review>> {
    let mut out = HashMap::new();
    for batch in shas.chunks(BATCH) {
        let q = query(owner, name, batch);
        let result = Command::new("gh")
            .args(["api", "graphql", "-f", &format!("query={q}")])
            .output()
            .context("--github needs the GitHub CLI (`gh`) installed and signed in")?;
        let response: Value = serde_json::from_slice(&result.stdout).unwrap_or(Value::Null);
        // A commit GitHub does not have yields a per-field error next to usable data.
        if response
            .pointer("/data/repository")
            .is_none_or(Value::is_null)
        {
            let detail = String::from_utf8_lossy(&result.stderr);
            bail!(
                "GitHub did not answer for {owner}/{name}: {}",
                detail.trim()
            );
        }
        out.extend(parse(&response, batch));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remote_urls() {
        let want = Some(("acme".to_string(), "widgets".to_string()));
        assert_eq!(slug("git@github.com:acme/widgets.git"), want);
        assert_eq!(slug("https://github.com/acme/widgets"), want);
        assert_eq!(slug("https://github.com/acme/widgets.git/"), want);
        assert_eq!(slug("ssh://git@github.com/acme/widgets.git"), want);
        assert_eq!(slug("https://gitlab.com/acme/widgets.git"), None);
    }

    #[test]
    fn only_another_persons_review_counts() {
        let review = |kind: &str, login: &str, state: &str| json!({"state": state, "author": {"__typename": kind, "login": login}});
        let pr = |number: u64, reviews: Vec<Value>| json!({"associatedPullRequests": {"nodes": [{"number": number, "author": {"login": "ann"}, "reviews": {"nodes": reviews}}]}});
        let response = json!({"data": {"repository": {
            "c0": pr(1, vec![review("Bot", "review-bot", "APPROVED"), review("User", "ann", "COMMENTED")]),
            "c1": pr(2, vec![review("User", "bob", "DISMISSED"), review("User", "cat", "APPROVED")]),
            "c2": null,
        }}});
        let shas: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let got = parse(&response, &shas);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got["b"],
            Review {
                pr: 2,
                by: "cat".to_string()
            }
        );
    }

    /// Talks to GitHub. Run with:
    /// `UNREAD_LIVE_GH=owner/name:sha1,sha2 cargo test live_reviews -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_reviews() {
        let spec = std::env::var("UNREAD_LIVE_GH").expect("UNREAD_LIVE_GH=owner/name:sha,sha");
        let (repo, shas) = spec.split_once(':').unwrap();
        let (owner, name) = repo.split_once('/').unwrap();
        let shas: Vec<String> = shas.split(',').map(str::to_string).collect();
        let got = reviewed(owner, name, &shas).unwrap();
        println!("{} of {} commits reviewed: {got:?}", got.len(), shas.len());
        assert!(!got.is_empty());
    }
}
