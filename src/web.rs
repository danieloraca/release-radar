use crate::{db::Store, github::GitHub, model::*};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub struct App {
    pub store: Store,
    pub github: GitHub,
    pub demo: bool,
    pub hosts: Vec<String>,
    pub environments: [String; 2],
    pub sync: tokio::sync::Mutex<Option<Instant>>,
}
type ApiResult = Result<Json<Value>, ApiError>;
pub struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn bad(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}
fn storage(_: anyhow::Error) -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "Could not save: duplicate record, missing parent, or database unavailable".into(),
    )
}
fn clean(s: &str, max: usize) -> Result<String, ApiError> {
    let s = s.trim();
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(bad(format!(
            "Use 1–{max} characters without control characters"
        )));
    }
    Ok(s.into())
}
pub fn router(state: Arc<App>) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../static/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(
                        header::CONTENT_TYPE,
                        "application/javascript; charset=utf-8",
                    )],
                    include_str!("../static/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../static/style.css"),
                )
            }),
        )
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/tickets", get(list).post(add_ticket))
        .route("/api/tickets/{id}", delete(remove_ticket))
        .route("/api/tickets/{id}/prs", post(add_pr))
        .route("/api/tickets/{id}/tests", post(add_test))
        .route("/api/prs/{id}", delete(remove_pr))
        .route("/api/tests/{id}", patch(set_test).delete(remove_test))
        .route("/api/sync", post(sync))
        .layer(DefaultBodyLimit::max(8192))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state)
}
async fn security(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|s| s.to_str().ok())
        .unwrap_or("");
    if !app.hosts.iter().any(|h| h == host) {
        return (StatusCode::FORBIDDEN, "Unrecognised Host").into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let allowed = origin
            .to_str()
            .ok()
            .is_some_and(|o| o == format!("http://{host}") || o == format!("https://{host}"));
        if !allowed {
            return (StatusCode::FORBIDDEN, "Cross-origin request rejected").into_response();
        }
    }
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}
async fn list(State(app): State<Arc<App>>) -> ApiResult {
    let tickets = app.store.tickets().map_err(storage)?;
    let rows: Vec<_> = tickets
        .iter()
        .map(|t| {
            let mut value = serde_json::to_value(t).unwrap();
            value["summary"] = json!(t.summary());
            value["revision"] = json!(t.revision());
            for (v, p) in value["prs"].as_array_mut().unwrap().iter_mut().zip(&t.prs) {
                v["url"] = json!(p.reference.url());
                v["fresh"] = json!(p.fresh());
                v["reviews_state"] = json!(
                    p.snapshot
                        .as_ref()
                        .map(|s| s.review_state())
                        .unwrap_or("unknown")
                );
                v["checks_state"] = json!(
                    p.snapshot
                        .as_ref()
                        .map(|s| s.checks_state())
                        .unwrap_or("unknown")
                );
            }
            value
        })
        .collect();
    Ok(Json(
        json!({"tickets":rows,"demo":app.demo,"environments":app.environments,"freshness_seconds":600}),
    ))
}
#[derive(Deserialize)]
struct NewTicket {
    key: String,
    title: String,
}
async fn add_ticket(State(app): State<Arc<App>>, Json(body): Json<NewTicket>) -> ApiResult {
    if app.store.tickets().map_err(storage)?.len() >= 100 {
        return Err(bad("MVP limit: 100 tickets"));
    }
    let key = clean(&body.key, 40)?.to_uppercase();
    if !key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(bad("Ticket key must use letters, numbers, - or _"));
    }
    let id = app
        .store
        .add_ticket(&key, &clean(&body.title, 160)?)
        .map_err(storage)?;
    Ok(Json(json!({"id":id})))
}
#[derive(Deserialize)]
struct NewPr {
    url: String,
}
async fn add_pr(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(body): Json<NewPr>,
) -> ApiResult {
    let tickets = app.store.tickets().map_err(storage)?;
    if tickets.iter().map(|t| t.prs.len()).sum::<usize>() >= 100 {
        return Err(bad("MVP limit: 100 linked PRs"));
    }
    let reference = PrRef::parse(&body.url).map_err(bad)?;
    app.store.add_pr(id, &reference).map_err(storage)?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct NewTest {
    label: String,
}
async fn add_test(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(body): Json<NewTest>,
) -> ApiResult {
    app.store
        .add_test(id, &clean(&body.label, 200)?)
        .map_err(storage)?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct TestPass {
    passed: bool,
}
async fn set_test(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(body): Json<TestPass>,
) -> ApiResult {
    if !app
        .store
        .set_test(id, body.passed)
        .map_err(|e| bad(e.to_string()))?
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "Test not found".into()));
    }
    Ok(Json(json!({"ok":true})))
}
async fn remove_ticket(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult {
    remove(&app, "tickets", id)
}
async fn remove_pr(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult {
    remove(&app, "prs", id)
}
async fn remove_test(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult {
    remove(&app, "tests", id)
}
fn remove(app: &App, kind: &str, id: i64) -> ApiResult {
    if !app.store.delete(kind, id).map_err(storage)? {
        return Err(ApiError(StatusCode::NOT_FOUND, "Record not found".into()));
    }
    Ok(Json(json!({"ok":true})))
}
async fn sync(State(app): State<Arc<App>>) -> ApiResult {
    if app.demo {
        return Ok(Json(
            json!({"message":"Demo mode: synthetic data; no GitHub calls"}),
        ));
    }
    let count = sync_all(&app)
        .await
        .map_err(|e| ApiError(StatusCode::TOO_MANY_REQUESTS, e.to_string()))?;
    Ok(Json(
        json!({"message":format!("Sync finished: {count} PRs attempted. Check each card for errors.")}),
    ))
}
pub async fn sync_all(app: &App) -> anyhow::Result<usize> {
    let mut guard = app
        .sync
        .try_lock()
        .map_err(|_| anyhow::anyhow!("Sync already running"))?;
    if guard.is_some_and(|last| last.elapsed() < Duration::from_secs(30)) {
        anyhow::bail!("Wait 30 seconds between syncs");
    }
    let tickets = app.store.tickets()?;
    let mut count = 0;
    for ticket in tickets {
        for pr in ticket.prs {
            let result =
                tokio::time::timeout(Duration::from_secs(120), app.github.snapshot(&pr.reference))
                    .await;
            match result {
                Ok(Ok(snapshot)) => app.store.save_snapshot(pr.id, &snapshot)?,
                Ok(Err(e)) => app.store.fail_sync(pr.id, &e.to_string())?,
                Err(_) => app.store.fail_sync(
                    pr.id,
                    "Sync exceeded two minutes; previous evidence retained",
                )?,
            }
            count += 1;
        }
    }
    *guard = Some(Instant::now());
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;
    fn app() -> Router {
        router(Arc::new(App {
            store: Store::open(std::path::Path::new(":memory:")).unwrap(),
            github: GitHub::new(None, ["staging".into(), "production".into()]).unwrap(),
            demo: true,
            hosts: vec!["localhost:9977".into()],
            environments: ["staging".into(), "production".into()],
            sync: tokio::sync::Mutex::new(None),
        }))
    }
    #[tokio::test]
    async fn api_validates_persists_and_blocks_cross_origin_and_rebinding() {
        let app = app();
        let req = |host: &str, origin: &str, body: &str| {
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/tickets")
                .header("host", host)
                .header("origin", origin)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(req(
                    "localhost:9977",
                    "https://evil.test",
                    r#"{"key":"D-1","title":"Example"}"#
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(req(
                    "evil.test",
                    "http://evil.test",
                    r#"{"key":"D-1","title":"Example"}"#
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(req(
                    "localhost:9977",
                    "http://localhost:9977",
                    r#"{"key":"D-1","title":"Example"}"#
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/tickets")
                    .header("host", "localhost:9977")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["tickets"][0]["summary"], "needs sync");
        assert_eq!(value["tickets"][0]["tests"].as_array().unwrap().len(), 3);
    }
}
