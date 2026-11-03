# Local API contract (v0.1)

Default origin: `http://127.0.0.1:9977`. No authentication. Host allowlist and
same-origin browser policy apply to all routes. JSON request bodies; maximum 8 KiB.
No endpoint writes to GitHub. IDs are local SQLite integers.

| Method | Path | Request | Success |
| --- | --- | --- | --- |
| GET | `/healthz` | — | `{"status":"ok"}` |
| GET | `/api/tickets` | — | `{tickets: [...], demo, environments: [staging, production], freshness_seconds: 600}` |
| POST | `/api/tickets` | `{key, title}` | `{id}`; adds three default checklist steps |
| DELETE | `/api/tickets/{id}` | — | `{ok: true}`; cascades local PR links/tests |
| POST | `/api/tickets/{id}/prs` | `{url}` | `{ok: true}`; does not immediately sync |
| DELETE | `/api/prs/{id}` | — | `{ok: true}` |
| POST | `/api/tickets/{id}/tests` | `{label}` | `{ok: true}` |
| PATCH | `/api/tests/{id}` | `{passed: boolean}` | `{ok: true}`; pass requires fresh linked PR snapshots |
| DELETE | `/api/tests/{id}` | — | `{ok: true}` |
| POST | `/api/sync` | — | `{message}`; waits for all linked PR attempts |

Errors return `{error: string}` for application failures: 400 invalid input or
unavailable testing evidence, 404 missing local record, 409 duplicate/storage issue,
429 sync running/cooldown. Axum extraction errors may be plain text (e.g. malformed
JSON, body too large, unsupported content type). Security rejections are 403 text.
Upstream failures are recorded per PR or endpoint and do not make the whole sync
request fail; inspect cards/response snapshots. An unsuccessful upstream call never
replaces a previous snapshot with an empty success.

Each ticket includes `id, key, title, summary, revision, prs, tests`. PRs contain
`reference: {owner, repo, number}, url, snapshot, sync_error, fresh, reviews_state,
checks_state`. A snapshot includes exact head/merge SHAs, base/state, review and
check summaries, deployment evidence and warning strings, plus `observed_at`.
Tests contain `id, label, passed, tested_revision`. Compare tested_revision to the
ticket revision before treating a stored pass as current.

This is a private local API intended for the embedded UI. There is no stable public
versioning guarantee yet. Readiness is observational; see README limitations.
