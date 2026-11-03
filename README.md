# Release Radar

A small Rust dashboard for tracking a ticket across GitHub pull requests, reviews,
checks, deployment records and a manual testing checklist. Designed for a Raspberry
Pi: one binary, SQLite, embedded HTML/CSS/JavaScript, no Node runtime, no LLM, no telemetry.

## Try it locally

Install a current stable Rust toolchain (edition 2024; Rust 1.85+ and a C compiler).

```sh
cargo run --locked -- --demo
```

Open <http://127.0.0.1:9977>. Demo mode contains **synthetic** API/widget PRs,
an approval on an older commit, and a missing staging record. The database is
in memory and disappears at exit. Demo mode never contacts GitHub, even if a token
is present. Adding a real PR in demo mode does not fetch its data.

For a real workspace:

```sh
cargo run --locked --release
```

1. Add a ticket key and short title. Keys are labels; no Jira/Linear integration.
2. Link each related `https://github.com/owner/repo/pull/123` URL.
3. Click **Sync GitHub**. Automatic sync also runs at startup and every five minutes.
4. Review each PR's exact head, review/check status and staging/production records.
5. Complete or extend the checklist. A pass records all linked PR heads; changing
   a head, adding or unlinking a PR invalidates the previous revision's passes.

GitHub requests are **GET-only**. The app cannot approve, merge, deploy or alter a
GitHub repository. Adding and removing cards affects only the local database.
Deleting a ticket removes its local PR links and checklist. There is no undo.

## What the evidence means

- **Review:** the latest decisive review per reviewer. Comments do not erase an
  approval; dismissed reviews do. An approval must match the current head SHA.
  An older approval is marked stale even if GitHub's branch rules would retain it.
  Any outstanding changes-requested review blocks the observed review gate.
- **Checks:** check runs and legacy commit statuses for the **head SHA**. Latest
  status per context is used. Failure wins; queued work is pending. Empty evidence,
  inaccessible endpoints and skipped/neutral-only checks are unknown.
- **Deployments:** the latest GitHub Deployment record for each configured environment,
  then its latest status. It counts as matching only when its SHA equals the PR head
  or, for a merged PR, GitHub's merge commit SHA. Success is an exact-commit record,
  **not a probe of what is currently running**. A newer deployment for a different
  SHA shows unknown for this PR, even if an older record once matched.
- There is **no branch ancestry inference**. A successful branch build, a PR merge,
  or a commit being on `main` is never presented as a deployment.
- **Coordination:** tickets show how many of their linked PRs have matching successful
  staging records. Link all relevant PRs yourself; dependencies aren't discovered.
- **Observed gates clear** means all linked PRs have fresh evidence, are not drafts
  or closed-unmerged, have a current-head approval, have passing observed checks,
  and all checklist steps passed against the current set of heads. It does **not**
  certify deployability or override GitHub. Deployment evidence is shown separately.
- Evidence older than ten minutes or with a failed PR sync is stale. Errors retain
  the previous snapshot for diagnosis and prevent it from clearing observed gates.
  Endpoint-specific failures make their evidence unknown. A head/merge changing
  during sync aborts that PR snapshot. There is no atomic snapshot across repositories.

Branch protection, required reviewer counts, CODEOWNERS, merge queues, conflicts,
required-check configuration, post-merge branch CI and real runtime health are
**not** inspected. Squash/rebase releases, promotion pipelines and newer descendant
commits may legitimately show deployment unknown. Pipelines must publish GitHub
Deployment records; otherwise this MVP cannot establish deployments. There is no
manual override that turns an assumption into verified evidence.

## Credentials and work data

Public repositories work without credentials, subject to GitHub's anonymous rate
limit. Private repositories require a token supplied in the server process environment
as `RR_GITHUB_TOKEN`. Nothing reads your existing `gh` credentials, generates tokens,
or writes credentials to disk. Tokens never reach the browser or database. HTTP
redirects are disabled; only `api.github.com` is used. GitHub Enterprise hosts are
not supported in this MVP.

If Gecko policy allows a personal Pi to hold this metadata, use a short-lived,
repository-scoped, read-only fine-grained token with **Pull requests, Checks,
Commit statuses and Deployments: read**, plus GitHub's required metadata access.
Organization approval/SSO may also be required. Grant access only to the repos you
intend to track. A 403/404 may indicate missing permissions, SSO or visibility;
rate limits are also shown as errors. Do not put a token in a shell command, repo,
service unit or screenshot. You can enter it without shell-history exposure:

```sh
# Bash, run interactively; the token exists only for this process tree.
read -r -s -p 'GitHub read-only token: ' RR_GITHUB_TOKEN; printf '\n'
export RR_GITHUB_TOKEN
cargo run --locked --release
unset RR_GITHUB_TOKEN
```

The default `data/radar.sqlite3` stores ticket labels, PR titles/URLs, SHA values,
reviewer login/state, check names/results, deployment records, timestamps and local
checklist text. It does not store source code, PR bodies, review bodies or raw API
responses. Data is plaintext on disk. New data directories use mode 0700 and new DB
files 0600 on Unix; ensure existing directories have suitable permissions. The
systemd example sets umask 0077. Protect device access, backups and disks according
to company policy. `.gitignore` excludes data, DB files, credentials and logs.

## Network boundary

Standalone runs default to `127.0.0.1:9977`. The Pi service is configured for
**<http://192.168.0.25:9977>**, matching the direct LAN access used by `live`. Its
unit sets `RR_BIND=0.0.0.0:9977` and `RR_ALLOWED_HOSTS=192.168.0.25:9977`; both are
needed because the app validates the HTTP Host header as well as its bind address.
Loopback health checks continue to work. If the Pi's address changes, update the
allowed host too. Add any hostname you want to use explicitly, with port 9977.

**There is no login or user authorization.** Devices that can reach the service
can read tracked metadata and change local records. This is intended for your
trusted LAN; do not forward the port to the public internet. Host/Origin checks
and the content security policy are not authentication. No CORS or third-party
assets are enabled.

If you prefer loopback-only access, set `RR_BIND=127.0.0.1:9977` in the unit and
use an SSH tunnel from your computer:

```sh
ssh -L 9977:127.0.0.1:9977 danutz@192.168.0.25
```

Then open <http://127.0.0.1:9977>.

## Configuration

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `RR_BIND` | `127.0.0.1:9977` | Literal IP and port; IPv6 supported |
| `RR_DATA_DIR` | `data` | SQLite directory, relative to working directory |
| `RR_GITHUB_TOKEN` | absent | Optional process-only GitHub token |
| `RR_SYNC_SECONDS` | `300` | Delay after a sync completes; `0` disables auto sync, minimum `60` |
| `RR_STAGING_ENV` | `staging` | Exact GitHub environment name, including case |
| `RR_PRODUCTION_ENV` | `production` | Exact GitHub environment name, including case |
| `RR_ALLOWED_HOSTS` | absent | Additional exact `host:port` values, comma-separated |

Loopback Host values and the configured bind address are allowed automatically.
Manual syncs are serialized and separated by 30 seconds. Each API request times
out after 15 seconds, each PR sync after two minutes. Sync is sequential and does
not retry upstream writes (there are none). Reviews/checks/statuses paginate up to
1,000 entries each; reaching the limit produces unknown evidence, not partial success.
A PR takes roughly nine API calls plus pagination. For private workspaces keep the
tracked set small to respect rate limits; the MVP caps tickets and PR links at 100.

## Raspberry Pi: clone, build, run

Use a 64-bit Raspberry Pi OS and a current stable Rust toolchain. Building on the
Pi avoids cross-architecture binary confusion. Debian's packaged Rust may be too
old; use an organization-approved current toolchain. SQLite is bundled; no database
server or OpenSSL development package is needed. A C compiler is needed:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config git
mkdir -p ~/Development
cd ~/Development
git clone YOUR_REPOSITORY_URL release-radar
cd release-radar
cargo build --locked --release
./target/release/release-radar --demo
```

No GitHub repo or remote is created by this project. Add this source to your own
repository on your Mac, then clone that repository on the Pi. Do not copy the
Mac-compiled executable onto the Pi.

### Pi service (same workflow as `live`)

Like the sibling `live` project, Release Radar ships a systemd unit,
`deploy/update-pi.sh` and a `make deploy` entry point. Run deployment as your normal
**danutz** user, not with `sudo`. Only the service restart uses sudo.

`deploy/release-radar.service.example` defaults to the same Pi user and project layout as
`live`: `danutz` and `/home/danutz/Development/release-radar`. It runs the installed
`bin/release-radar` executable, with automatic restart and journal logging. The
binary is separate from Cargo's build output, so rebuilding leaves the running
executable intact. SQLite stays in `/home/danutz/.local/share/release-radar`, outside
both the build output and checkout. Rebuilds and pulls do not delete it.

The service listens on **0.0.0.0:9977** and allows the Pi's LAN Host header
**192.168.0.25:9977**, so open **<http://192.168.0.25:9977>** directly from your
computer. Loopback still works for deployment health checks. It has no login.
The unit deliberately contains no token. Decide an approved credential delivery
method separately if you want an unattended service for private repositories.

#### First install on the Pi

After committing/pushing this project and cloning it onto the Pi, build and install
as `danutz`. If the checkout already exists, first run `git pull --ff-only` to get
the Makefile and deploy script. `make` is included in `build-essential`.

```sh
cd ~/Development/release-radar
cargo test --locked
cargo build --locked --release
./scripts/install-binary.sh
mkdir -p "$HOME/.local/share/release-radar"
chmod 700 "$HOME/.local/share/release-radar"
```

Inspect `deploy/release-radar.service.example` before installing it. If your user or layout
differs, copy the unit to `/tmp/release-radar.service` and change `User`,
`WorkingDirectory`, `ExecStart`, `RR_DATA_DIR` and `ReadWritePaths` there, then install
that copy instead. `deploy/release-radar.service` is ignored by Git and can hold
an existing local customized copy; use it instead of the example if it has your
settings. Keep the bind port at 9977 unless you deliberately change it.
Create the data directory before enabling the service.

```sh
sudo install -m 644 deploy/release-radar.service.example /etc/systemd/system/release-radar.service
sudo systemctl daemon-reload
sudo systemctl enable --now release-radar.service
systemctl is-active release-radar.service
curl --fail http://127.0.0.1:9977/healthz
journalctl -u release-radar.service -n 50 --no-pager
```

The server runs as the configured unprivileged user. Builds/tests also run as your
normal user. No script installs a unit, enables a service, configures credentials,
or changes the bind address automatically.

#### Enable LAN access on an already installed service

A source-file change alone does not update the installed unit. No binary rebuild
is required for this bind/Host change. On the Pi, run:

```sh
sudo systemctl edit release-radar.service
```

Put this in the override:

```ini
[Service]
Environment=RR_BIND=0.0.0.0:9977
Environment=RR_ALLOWED_HOSTS=192.168.0.25:9977
```

Then apply and check it:

```sh
sudo systemctl daemon-reload
sudo systemctl restart release-radar.service
curl --fail http://192.168.0.25:9977/healthz
```

Open <http://192.168.0.25:9977>. The override is local to the Pi and survives
ordinary app updates. If you later change the unit's networking, reconcile any
override with the base unit; `systemctl cat release-radar.service` shows both.

#### Subsequent updates

```sh
cd ~/Development/release-radar
make deploy
```

This runs `deploy/update-pi.sh`, which:

1. Requires Linux, a non-root user and a clean Git checkout.
2. Runs `git pull --ff-only`, `cargo test --locked`, then `cargo build --locked --release`.
3. Atomically installs the new executable using `scripts/install-binary.sh`, keeping
   the previous binary as `bin/release-radar.previous`.
4. Restarts **release-radar.service**, checks it is active, then checks `/healthz`
   on port **9977**, retrying up to ten times with a two-second request timeout.

A pull, test, build or install failure stops before the restart. A restart, inactive
service or failed health check makes deployment fail with a nonzero exit code;
health failures include the journal command to inspect. The update does not reinstall
the unit. If you deliberately change the unit, install it and run `daemon-reload`
separately. HTML/CSS/JavaScript are embedded in the Rust binary, so UI changes need
the rebuild too.

`RR_INSTALL_DIR` overrides the install directory; ensure your unit's `ExecStart`
uses the same destination. `RR_HEALTH_URL` overrides the updater's health URL if
you intentionally change the service bind/port; it does not change server settings.
The default remains `http://127.0.0.1:9977/healthz`.

For a rollback, copy `bin/release-radar.previous` to a temporary file in the same
directory, rename that temporary file over `bin/release-radar`, then restart the
service. This avoids truncating a running executable. Failed health checks do not
automatically roll back: inspect the journal and choose a rollback if appropriate.
Keep a data backup before future schema upgrades; binary rollback cannot undo a
schema change. The previous binary is replaced on each installation.

For a consistent SQLite backup, stop the service briefly and copy the data directory
including any WAL/SHM files, or use SQLite's online backup command. Never assume copying
only a live `.sqlite3` file includes the latest writes. No automatic backup, purge,
encryption or schema-upgrade framework is included in this initial schema.

## Development and verification

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
node --check static/app.js
python3 -m unittest discover -s tests -p 'deploy_test.py'
bash -n deploy/update-pi.sh
make -n deploy
```

Tests cover URL restrictions, commit-specific reviews, latest legacy status selection,
empty/skipped checks, checklist revisions, SQLite persistence/cascades, and API
Host/Origin protection. HTTP fixture tests run the actual reqwest client against a
local server, exercising review pagination, exact-SHA deployment evidence and HTTP
429 handling. Fixtures are synthetic examples shaped against GitHub's contract,
not Gecko production data. No live private GitHub integration has been verified.
The installed Postman CLI was too old to support `postman simulation`; these local
HTTP fixtures remain runnable without a Postman account or newer CLI.

Deployment tests use temporary checkouts and command doubles to exercise pull,
build, restart, inactivity and health failures, plus the real atomic install script.
They never run real sudo, contact the Pi, pull Git, or restart a service. Python 3
is needed only for these deployment tests. The systemd unit still needs verification
on the actual Pi; the local Mac does not run systemd.

The UI uses native forms, local assets, escaped DOM text and responsive layouts.
API mutations require JSON (except DELETE); local browser Origin checks still apply.
See `docs/api.md` for the compact local API contract.

GitHub contract references:
[PR reviews](https://docs.github.com/en/rest/pulls/reviews),
[check runs](https://docs.github.com/en/rest/checks/runs),
[commit statuses](https://docs.github.com/en/rest/commits/statuses),
[deployments](https://docs.github.com/en/rest/deployments/deployments),
[deployment statuses](https://docs.github.com/en/rest/deployments/statuses).
