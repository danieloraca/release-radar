use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}
impl PrRef {
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        let url = reqwest::Url::parse(input.trim()).map_err(|_| "Use a GitHub PR URL")?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Use https://github.com/owner/repo/pull/123");
        }
        let parts: Vec<_> = url
            .path()
            .trim_end_matches('/')
            .split('/')
            .skip(1)
            .collect();
        if parts.len() != 4 || parts[2] != "pull" {
            return Err("Use https://github.com/owner/repo/pull/123");
        }
        let valid = |s: &str| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        if !valid(parts[0]) || !valid(parts[1]) {
            return Err("Invalid repository name");
        }
        let number = parts[3].parse::<u64>().map_err(|_| "Invalid PR number")?;
        if number == 0 {
            return Err("Invalid PR number");
        }
        Ok(Self {
            owner: parts[0].to_lowercase(),
            repo: parts[1].to_lowercase(),
            number,
        })
    }
    pub fn path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.repo)
    }
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{}/{}/pull/{}",
            self.owner, self.repo, self.number
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub user: String,
    pub state: String,
    pub commit: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub environment: String,
    pub sha: String,
    pub state: String,
    pub matches: bool,
    pub evidence_url: String,
    pub recorded_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub title: String,
    pub head: String,
    pub merge_commit: Option<String>,
    pub base: String,
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub reviews: Vec<Review>,
    pub checks: Vec<Check>,
    pub deployments: Vec<Deployment>,
    pub warnings: Vec<String>,
    pub observed_at: DateTime<Utc>,
}
impl Snapshot {
    pub fn review_state(&self) -> &'static str {
        if self.warnings.iter().any(|w| w.starts_with("Reviews:")) {
            return "unknown";
        }
        let mut latest = BTreeMap::new();
        for review in &self.reviews {
            if matches!(
                review.state.as_str(),
                "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
            ) {
                latest.insert(&review.user, review);
            }
        }
        if latest.values().any(|r| r.state == "CHANGES_REQUESTED") {
            return "changes requested";
        }
        if latest
            .values()
            .any(|r| r.state == "APPROVED" && r.commit == self.head)
        {
            return "approved at head";
        }
        if latest.values().any(|r| r.state == "APPROVED") {
            return "stale approval";
        }
        "awaiting review"
    }
    pub fn checks_state(&self) -> &'static str {
        if self
            .warnings
            .iter()
            .any(|w| w.starts_with("Checks:") || w.starts_with("Statuses:"))
        {
            return "unknown";
        }
        if self.checks.is_empty() {
            return "unknown";
        }
        if self.checks.iter().any(|c| {
            matches!(
                c.state.as_str(),
                "failure" | "error" | "cancelled" | "timed_out" | "action_required" | "stale"
            )
        }) {
            return "failing";
        }
        if self
            .checks
            .iter()
            .any(|c| !matches!(c.state.as_str(), "success" | "skipped" | "neutral"))
        {
            return "pending";
        }
        // Skipped/neutral alone are not evidence that a test passed.
        if self.checks.iter().any(|c| c.state == "success") {
            "passing"
        } else {
            "unknown"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub id: i64,
    pub reference: PrRef,
    pub snapshot: Option<Snapshot>,
    pub sync_error: Option<String>,
}
impl PullRequest {
    pub fn fresh(&self) -> bool {
        self.sync_error.is_none()
            && self.snapshot.as_ref().is_some_and(|s| {
                Utc::now()
                    .signed_duration_since(s.observed_at)
                    .num_seconds()
                    < 600
            })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestItem {
    pub id: i64,
    pub label: String,
    pub passed: bool,
    pub tested_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub id: i64,
    pub key: String,
    pub title: String,
    pub prs: Vec<PullRequest>,
    pub tests: Vec<TestItem>,
}
impl Ticket {
    pub fn revision(&self) -> String {
        self.prs
            .iter()
            .map(|p| {
                format!(
                    "{}:{}",
                    p.id,
                    p.snapshot
                        .as_ref()
                        .map(|s| s.head.as_str())
                        .unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("|")
    }
    pub fn summary(&self) -> &'static str {
        if self.prs.is_empty() || self.prs.iter().any(|p| !p.fresh()) {
            return "needs sync";
        }
        if self
            .prs
            .iter()
            .filter_map(|p| p.snapshot.as_ref())
            .any(|s| {
                s.draft
                    || (s.state == "closed" && !s.merged)
                    || s.review_state() != "approved at head"
                    || s.checks_state() != "passing"
            })
        {
            return "needs attention";
        }
        if self.tests.is_empty()
            || self
                .tests
                .iter()
                .any(|t| !t.passed || t.tested_revision != self.revision())
        {
            return "needs testing";
        }
        "observed gates clear"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn sample() -> Snapshot {
        Snapshot {
            title: "Example".into(),
            head: "a".repeat(40),
            merge_commit: None,
            base: "main".into(),
            state: "open".into(),
            draft: false,
            merged: false,
            reviews: vec![],
            checks: vec![],
            deployments: vec![],
            warnings: vec![],
            observed_at: Utc::now(),
        }
    }
    #[test]
    fn restrict_pr_urls() {
        assert!(PrRef::parse("https://github.com/acme/api/pull/12").is_ok());
        for url in [
            "http://github.com/a/b/pull/1",
            "https://evil.test/a/b/pull/1",
            "https://github.com/a/b/pull/0",
            "https://github.com/a/b/pull/1?x=y",
            "https://u@github.com/a/b/pull/1",
            "https://github.com/a%2fb/c/pull/1",
        ] {
            assert!(PrRef::parse(url).is_err(), "{url}");
        }
    }
    #[test]
    fn comments_do_not_erase_approvals_but_dismissals_do() {
        let mut s = sample();
        s.reviews = vec![
            Review {
                user: "lee".into(),
                state: "APPROVED".into(),
                commit: s.head.clone(),
            },
            Review {
                user: "lee".into(),
                state: "COMMENTED".into(),
                commit: s.head.clone(),
            },
        ];
        assert_eq!(s.review_state(), "approved at head");
        s.head = "b".repeat(40);
        assert_eq!(s.review_state(), "stale approval");
        s.reviews.push(Review {
            user: "lee".into(),
            state: "DISMISSED".into(),
            commit: s.head.clone(),
        });
        assert_eq!(s.review_state(), "awaiting review");
    }
    #[test]
    fn empty_or_skipped_checks_are_unknown_and_errors_win() {
        let mut s = sample();
        assert_eq!(s.checks_state(), "unknown");
        s.checks.push(Check {
            name: "ci".into(),
            state: "skipped".into(),
        });
        assert_eq!(s.checks_state(), "unknown");
        s.checks[0].state = "success".into();
        assert_eq!(s.checks_state(), "passing");
        s.warnings.push("Statuses: forbidden".into());
        assert_eq!(s.checks_state(), "unknown");
    }
    #[test]
    fn testing_and_readiness_follow_all_pr_heads() {
        let mut s = sample();
        s.reviews.push(Review {
            user: "lee".into(),
            state: "APPROVED".into(),
            commit: s.head.clone(),
        });
        s.checks.push(Check {
            name: "ci".into(),
            state: "success".into(),
        });
        let mut t = Ticket {
            id: 1,
            key: "DEMO-1".into(),
            title: "Test".into(),
            prs: vec![PullRequest {
                id: 1,
                reference: PrRef::parse("https://github.com/a/b/pull/1").unwrap(),
                snapshot: Some(s),
                sync_error: None,
            }],
            tests: vec![],
        };
        assert_eq!(t.summary(), "needs testing");
        t.tests.push(TestItem {
            id: 1,
            label: "Smoke test".into(),
            passed: true,
            tested_revision: t.revision(),
        });
        assert_eq!(t.summary(), "observed gates clear");
        t.prs[0].snapshot.as_mut().unwrap().head = "c".repeat(40);
        assert_ne!(t.tests[0].tested_revision, t.revision());
        t.prs[0].sync_error = Some("403".into());
        assert_eq!(t.summary(), "needs sync");
    }
}
