use crate::model::*;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use reqwest::{Client, header};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone)]
pub struct GitHub {
    client: Client,
    base: String,
    environments: [String; 2],
}
fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
impl GitHub {
    pub fn new(token: Option<String>, environments: [String; 2]) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            header::HeaderValue::from_static("2026-03-10"),
        );
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            let mut value = header::HeaderValue::from_str(&format!("Bearer {token}"))
                .context("Invalid GitHub token format")?;
            value.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, value);
        }
        let client = Client::builder()
            .default_headers(headers)
            .user_agent("release-radar/0.1")
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            base: "https://api.github.com".into(),
            environments,
        })
    }
    async fn get(&self, path: &str) -> Result<(Value, bool)> {
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("GitHub connection failed or timed out"))?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "GitHub HTTP {} (check permissions, SSO, rate limit or repository visibility)",
                status.as_u16()
            );
        }
        let next = response
            .headers()
            .get(header::LINK)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.contains("rel=\"next\""));
        // Never retain raw upstream bodies or credential-bearing diagnostic output.
        let value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("GitHub returned invalid JSON"))?;
        Ok((value, next))
    }
    async fn list(&self, path: &str, key: Option<&str>) -> Result<Vec<Value>> {
        let mut all = vec![];
        for page in 1..=10 {
            let separator = if path.contains('?') { '&' } else { '?' };
            let (value, next) = self
                .get(&format!("{path}{separator}per_page=100&page={page}"))
                .await?;
            let values = if let Some(key) = key {
                &value[key]
            } else {
                &value
            };
            all.extend(
                values
                    .as_array()
                    .context("Unexpected GitHub response shape")?
                    .iter()
                    .cloned(),
            );
            if !next {
                return Ok(all);
            }
        }
        bail!("Pagination limit reached; evidence incomplete")
    }
    pub async fn snapshot(&self, pr: &PrRef) -> Result<Snapshot> {
        let root = pr.path();
        let path = format!("{root}/pulls/{}", pr.number);
        let (data, _) = self.get(&path).await?;
        let head = string(&data["head"], "sha");
        if head.len() != 40 || !head.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("GitHub returned an invalid head SHA");
        }
        let merged = data["merged"].as_bool().unwrap_or(false);
        let mut snapshot = Snapshot {
            title: string(&data, "title"),
            head: head.clone(),
            merge_commit: if merged {
                data["merge_commit_sha"].as_str().map(str::to_owned)
            } else {
                None
            },
            base: string(&data["base"], "ref"),
            state: string(&data, "state"),
            draft: data["draft"].as_bool().unwrap_or(true),
            merged,
            reviews: vec![],
            checks: vec![],
            deployments: vec![],
            warnings: vec![],
            observed_at: Utc::now(),
        };
        match self.list(&format!("{path}/reviews"), None).await {
            Ok(reviews) => {
                snapshot.reviews = reviews
                    .iter()
                    .map(|r| Review {
                        user: string(&r["user"], "login"),
                        state: string(r, "state"),
                        commit: string(r, "commit_id"),
                    })
                    .collect()
            }
            Err(e) => snapshot.warnings.push(format!("Reviews: {e}")),
        }
        match self
            .list(
                &format!("{root}/commits/{head}/check-runs?filter=latest"),
                Some("check_runs"),
            )
            .await
        {
            Ok(checks) => snapshot.checks.extend(checks.iter().map(|c| Check {
                name: string(c, "name"),
                state: if c["status"] == "completed" {
                    c["conclusion"].as_str().unwrap_or("unknown").into()
                } else {
                    string(c, "status")
                },
            })),
            Err(e) => snapshot.warnings.push(format!("Checks: {e}")),
        }
        match self
            .list(&format!("{root}/commits/{head}/statuses"), None)
            .await
        {
            Ok(statuses) => {
                // GitHub returns newest first. Only the latest state of each context counts.
                let mut latest = BTreeMap::new();
                for status in statuses {
                    latest.entry(string(&status, "context")).or_insert(status);
                }
                snapshot.checks.extend(latest.into_values().map(|s| Check {
                    name: string(&s, "context"),
                    state: string(&s, "state"),
                }));
            }
            Err(e) => snapshot.warnings.push(format!("Statuses: {e}")),
        }
        for environment in &self.environments {
            match self.deployment(&root, environment, &snapshot).await {
                Ok(Some(d)) => snapshot.deployments.push(d),
                Ok(None) => {}
                Err(e) => snapshot
                    .warnings
                    .push(format!("Deployment ({environment}): {e}")),
            }
        }
        // A push while gathering metadata must not make old checks look current.
        let (current, _) = self.get(&path).await?;
        if current["head"]["sha"] != head
            || current["merged"] != data["merged"]
            || current["merge_commit_sha"] != data["merge_commit_sha"]
        {
            bail!("PR changed during sync; sync again");
        }
        snapshot.observed_at = Utc::now();
        Ok(snapshot)
    }
    async fn deployment(
        &self,
        root: &str,
        environment: &str,
        snapshot: &Snapshot,
    ) -> Result<Option<Deployment>> {
        let query: String = reqwest::Url::parse_with_params(
            "https://api.github.com/",
            [("environment", environment)],
        )
        .unwrap()
        .query()
        .unwrap()
        .into();
        let (value, _) = self
            .get(&format!("{root}/deployments?{query}&per_page=1"))
            .await?;
        let values = value.as_array().context("Unexpected deployment response")?;
        let Some(d) = values.first() else {
            return Ok(None);
        };
        let id = d["id"].as_u64().context("Missing deployment ID")?;
        let (statuses, _) = self
            .get(&format!("{root}/deployments/{id}/statuses?per_page=1"))
            .await?;
        let statuses = statuses
            .as_array()
            .context("Unexpected deployment statuses")?;
        let latest = statuses.first();
        let sha = string(d, "sha");
        let matches =
            sha == snapshot.head || snapshot.merge_commit.as_ref().is_some_and(|m| m == &sha);
        Ok(Some(Deployment {
            environment: environment.into(),
            sha,
            state: latest
                .map(|s| string(s, "state"))
                .unwrap_or_else(|| "unknown".into()),
            matches,
            evidence_url: format!("https://api.github.com{root}/deployments/{id}"),
            recorded_at: latest
                .map(|s| string(s, "created_at"))
                .unwrap_or_else(|| string(d, "created_at")),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::Uri, response::IntoResponse, routing::get};
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Mock {
        requests: Mutex<Vec<String>>,
        failure: Mutex<bool>,
        deployment_sha: Mutex<Option<String>>,
        merged: Mutex<bool>,
        reviews_forbidden: Mutex<bool>,
    }
    async fn fixture(State(state): State<Arc<Mock>>, uri: Uri) -> axum::response::Response {
        use serde_json::json;
        state.requests.lock().unwrap().push(uri.to_string());
        if *state.failure.lock().unwrap() {
            return (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"message":"rate limit"})),
            )
                .into_response();
        }
        let path = uri.path();
        if path.ends_with("/reviews") && *state.reviews_forbidden.lock().unwrap() {
            return axum::http::StatusCode::FORBIDDEN.into_response();
        }
        let merged = *state.merged.lock().unwrap();
        let deployment_sha = state
            .deployment_sha
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "a".repeat(40));
        let sha = "a".repeat(40);
        let value = if path.ends_with("/pulls/1") {
            json!({"title":"Fixture PR","head":{"sha":sha},"base":{"ref":"main"},"state":"open","draft":false,"merged":merged,"merge_commit_sha":if merged {Some("c".repeat(40))} else {None}})
        } else if path.ends_with("/reviews") && !uri.query().unwrap_or("").contains("page=2") {
            return (
                [("link", "<ignored>; rel=\"next\"")],
                Json(
                    json!([{"user":{"login":"lee"},"state":"APPROVED","commit_id":"b".repeat(40)}]),
                ),
            )
                .into_response();
        } else if path.ends_with("/reviews") {
            json!([{"user":{"login":"lee"},"state":"COMMENTED","commit_id":sha}])
        } else if path.ends_with("/check-runs") {
            json!({"check_runs":[{"name":"CI","status":"completed","conclusion":"success"}]})
        } else if path.ends_with("/deployments") {
            json!([{"id":7,"sha":deployment_sha,"created_at":"2026-11-03T00:00:00Z"}])
        } else if path.ends_with("/deployments/7/statuses") {
            json!([{"state":"success","created_at":"2026-11-03T00:00:00Z"}])
        } else if path.ends_with("/statuses") {
            json!([{"context":"legacy-ci","state":"failure"},{"context":"legacy-ci","state":"success"}])
        } else {
            return axum::http::StatusCode::NOT_IMPLEMENTED.into_response();
        };
        Json(value).into_response()
    }
    #[tokio::test]
    async fn real_http_sync_pagination_latest_status_exact_deployment_and_rate_limit() {
        let state = Arc::new(Mock::default());
        let app = Router::new()
            .fallback(get(fixture))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut github = GitHub::new(None, ["staging".into(), "production".into()]).unwrap();
        github.base = format!("http://{address}");
        let pr = PrRef::parse("https://github.com/a/b/pull/1").unwrap();
        let snapshot = github.snapshot(&pr).await.unwrap();
        assert_eq!(snapshot.review_state(), "stale approval");
        assert_eq!(snapshot.checks_state(), "failing");
        assert!(
            snapshot
                .deployments
                .iter()
                .all(|d| d.matches && d.state == "success")
        );
        assert!(
            state
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with("page=2"))
        );
        // A different latest deployment is not evidence for this PR.
        *state.deployment_sha.lock().unwrap() = Some("c".repeat(40));
        let different = github.snapshot(&pr).await.unwrap();
        assert!(different.deployments.iter().all(|d| !d.matches));
        // A completed merge may use the exact merge commit instead of the head.
        *state.merged.lock().unwrap() = true;
        let merged = github.snapshot(&pr).await.unwrap();
        assert!(merged.deployments.iter().all(|d| d.matches));
        // Missing access must not turn reviews into an empty successful result.
        *state.reviews_forbidden.lock().unwrap() = true;
        let partial = github.snapshot(&pr).await.unwrap();
        assert_eq!(partial.review_state(), "unknown");
        assert!(partial.warnings.iter().any(|w| w.contains("403")));
        *state.failure.lock().unwrap() = true;
        assert!(
            github
                .snapshot(&pr)
                .await
                .unwrap_err()
                .to_string()
                .contains("429")
        );
        let store = crate::db::Store::open(std::path::Path::new(":memory:")).unwrap();
        let ticket = store.add_ticket("D-1", "Failure retention").unwrap();
        store.add_pr(ticket, &pr).unwrap();
        let id = store.tickets().unwrap()[0].prs[0].id;
        store.save_snapshot(id, &snapshot).unwrap();
        let app = crate::web::App {
            store,
            github,
            demo: false,
            hosts: vec![],
            environments: ["staging".into(), "production".into()],
            sync: tokio::sync::Mutex::new(None),
        };
        crate::web::sync_all(&app).await.unwrap();
        let tickets = app.store.tickets().unwrap();
        let retained = &tickets[0].prs[0];
        assert_eq!(retained.snapshot.as_ref().unwrap().head, snapshot.head);
        assert!(retained.sync_error.as_ref().unwrap().contains("429"));
        assert!(!retained.fresh());
        assert_eq!(tickets[0].summary(), "needs sync");
        server.abort();
    }
}
