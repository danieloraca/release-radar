use anyhow::{Context, Result, bail};
use release_radar::{
    db::Store,
    github::GitHub,
    model::*,
    web::{self, App},
};
use std::{env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Release Radar\nUsage: release-radar [--demo]\n\n--demo: ephemeral synthetic data; never contacts GitHub.\nRR_BIND=127.0.0.1:9977\nRR_DATA_DIR=data\nRR_SYNC_SECONDS=300 (0 disables automatic sync)\nRR_STAGING_ENV=staging\nRR_PRODUCTION_ENV=production\nRR_ALLOWED_HOSTS=comma-separated exact host:port values (additional)\nRR_GITHUB_TOKEN=optional read-only token in process environment"
        );
        return Ok(());
    }
    if args.iter().any(|a| a != "--demo") {
        bail!("Unknown option; use --help");
    }
    let demo = args.iter().any(|a| a == "--demo");
    let bind: SocketAddr = env::var("RR_BIND")
        .unwrap_or_else(|_| "127.0.0.1:9977".into())
        .parse()
        .context("RR_BIND must be an IP address and port")?;
    let interval: u64 = env::var("RR_SYNC_SECONDS")
        .unwrap_or_else(|_| "300".into())
        .parse()?;
    if interval != 0 && interval < 60 {
        bail!("RR_SYNC_SECONDS must be 0 or at least 60");
    }
    let environments = [
        env::var("RR_STAGING_ENV").unwrap_or_else(|_| "staging".into()),
        env::var("RR_PRODUCTION_ENV").unwrap_or_else(|_| "production".into()),
    ];
    let store = if demo {
        Store::open(std::path::Path::new(":memory:"))?
    } else {
        let dir = PathBuf::from(env::var("RR_DATA_DIR").unwrap_or_else(|_| "data".into()));
        if !dir.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&dir)?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir_all(&dir)?;
        }
        let path = dir.join("radar.sqlite3");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path).context("Cannot create database")?;
        Store::open(&path)?
    };
    if demo {
        seed(&store, &environments)?;
    }
    let mut hosts = vec![
        bind.to_string(),
        format!("localhost:{}", bind.port()),
        format!("127.0.0.1:{}", bind.port()),
        format!("[::1]:{}", bind.port()),
    ];
    hosts.extend(
        env::var("RR_ALLOWED_HOSTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    );
    let app = Arc::new(App {
        store,
        github: GitHub::new(
            if demo {
                None
            } else {
                env::var("RR_GITHUB_TOKEN").ok()
            },
            environments.clone(),
        )?,
        demo,
        hosts,
        environments,
        sync: tokio::sync::Mutex::new(None),
    });
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .context("Cannot bind listening address")?;
    if !bind.ip().is_loopback() {
        eprintln!("No authentication: use a trusted network or an authenticated reverse proxy.");
    }
    println!(
        "Release Radar at http://{bind}{}",
        if demo { " · DEMO (ephemeral)" } else { "" }
    );
    let worker = if !demo && interval > 0 {
        let state = app.clone();
        Some(tokio::spawn(async move {
            loop {
                if let Err(e) = web::sync_all(&state).await {
                    eprintln!("Sync: {e}");
                }
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
        }))
    } else {
        None
    };
    axum::serve(listener, web::router(app))
        .with_graceful_shutdown(async {
            let ctrl_c = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("signal handler");
                tokio::select! {_=ctrl_c=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            ctrl_c.await;
        })
        .await?;
    if let Some(worker) = worker {
        worker.abort();
    }
    Ok(())
}
fn seed(store: &Store, environments: &[String; 2]) -> Result<()> {
    let id = store.add_ticket("DEMO-42", "Coordinate API + widget release")?;
    for repo in ["api", "widget"] {
        store.add_pr(
            id,
            &PrRef::parse(&format!("https://github.com/example/{repo}/pull/42")).unwrap(),
        )?;
    }
    for (index, pr) in store.tickets()?[0].prs.iter().enumerate() {
        let head = if index == 0 { "a" } else { "b" }.repeat(40);
        store.save_snapshot(
            pr.id,
            &Snapshot {
                title: if index == 0 {
                    "Add integration endpoint"
                } else {
                    "Wire up the widget"
                }
                .into(),
                head: head.clone(),
                merge_commit: None,
                base: "main".into(),
                state: "open".into(),
                draft: false,
                merged: false,
                reviews: vec![Review {
                    user: "demo-reviewer".into(),
                    state: "APPROVED".into(),
                    commit: if index == 0 {
                        head.clone()
                    } else {
                        "c".repeat(40)
                    },
                }],
                checks: vec![Check {
                    name: "build & test".into(),
                    state: "success".into(),
                }],
                deployments: if index == 0 {
                    vec![Deployment {
                        environment: environments[0].clone(),
                        sha: head,
                        state: "success".into(),
                        matches: true,
                        evidence_url: "https://github.com/example/api/deployments".into(),
                        recorded_at: chrono::Utc::now().to_rfc3339(),
                    }]
                } else {
                    vec![]
                },
                warnings: vec![],
                observed_at: chrono::Utc::now(),
            },
        )?;
    }
    Ok(())
}
