use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use wikicraft::app::AppState;
use wikicraft::config::Config;

#[derive(Parser)]
#[command(name = "wikicraft", version, about = "WikiCraft — a self-hosted team wiki")]
struct Cli {
    #[command(flatten)]
    config: Config,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the web server (default).
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Analyze a Confluence export (zip or directory) without importing; prints a JSON report.
    AnalyzeConfluence { path: PathBuf },
    /// Import a Confluence export as the given admin user (by e-mail).
    ImportConfluence {
        path: PathBuf,
        #[arg(long)]
        as_user: String,
        #[arg(long)]
        space_key: Option<String>,
        #[arg(long)]
        skip_history: bool,
    },
    /// Rebuild the search index.
    Reindex,
    /// Probe the running server's `/api/health` (for container HEALTHCHECKs); exits non-zero when unhealthy.
    Healthcheck,
}

/// Plain-TCP GET of `/api/health` on the configured bind address (no curl/shell needed in the image).
fn healthcheck(bind: std::net::SocketAddr) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::time::Duration;
    let mut addr = bind;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() { std::net::Ipv4Addr::LOCALHOST.into() } else { std::net::Ipv6Addr::LOCALHOST.into() });
    }
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.write_all(format!("GET /api/health HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes())?;
    let mut body = String::new();
    s.take(64 * 1024).read_to_string(&mut body)?;
    anyhow::ensure!(body.starts_with("HTTP/1.") && body.contains(" 200 ") && body.contains("\"ok\""), "unhealthy: {}", body.lines().next().unwrap_or(""));
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(Command::Healthcheck) = cli.command {
        return healthcheck(cli.config.bind);
    }
    // logs go to stdout only (12-factor): the platform (docker, kubernetes, systemd) collects them
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=info,sqlx=warn,tantivy=warn"));
    if cli.config.log_format.eq_ignore_ascii_case("json") {
        tracing_subscriber::fmt().json().flatten_event(true).with_current_span(true).with_span_list(false).with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    match cli.command.unwrap_or(Command::Serve) {
        Command::Healthcheck => unreachable!(),
        Command::AnalyzeConfluence { path } => {
            let report = tokio::task::spawn_blocking(move || wikicraft::importer::analyze(&path)).await??;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Migrate => {
            AppState::new(cli.config).await?;
            println!("migrations applied");
        }
        Command::Reindex => {
            let state = AppState::new(cli.config).await?;
            let n = state.search.reindex_all(&state.db).await?;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            println!("indexed {n} pages");
        }
        Command::ImportConfluence { path, as_user, space_key, skip_history } => {
            let state = AppState::new(cli.config).await?;
            let user = sqlx::query_as::<_, wikicraft::auth::User>(sqlx::AssertSqlSafe(format!(
                "SELECT {} FROM users WHERE lower(email) = lower($1) AND role IN ('owner','admin')",
                wikicraft::auth::USER_COLUMNS
            )))
            .bind(&as_user)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no admin user with e-mail {as_user}"))?;
            // copy: the importer deletes its input when done
            let tmp = state.config.upload_dir().join(format!("{}.cli", wikicraft::app::new_id()));
            if path.is_dir() {
                anyhow::bail!("please pass the export .zip (directories are supported by analyze-confluence only)");
            }
            std::fs::copy(&path, &tmp)?;
            let opts = wikicraft::importer::ImportOptions { space_key, skip_history, ..Default::default() };
            let job = wikicraft::app::new_id();
            sqlx::query("INSERT INTO jobs (id, workspace_id, kind, created_by) VALUES ($1, $2, 'import.confluence', $3)")
                .bind(job)
                .bind(user.workspace_id)
                .bind(user.id)
                .execute(&state.db)
                .await?;
            wikicraft::importer::run(state.clone(), job, user.workspace_id, user.id, tmp, opts).await;
            let (status, result, error): (String, Option<serde_json::Value>, Option<String>) =
                sqlx::query_as("SELECT status, result, error FROM jobs WHERE id = $1").bind(job).fetch_one(&state.db).await?;
            println!("{status}");
            if let Some(r) = result {
                println!("{}", serde_json::to_string_pretty(&r)?);
            }
            if let Some(e) = error {
                eprintln!("{e}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await; // let the search indexer flush
        }
        Command::Serve => {
            let bind = cli.config.bind;
            let state = AppState::new(cli.config).await?;
            tokio::spawn(wikicraft::collab::persist_loop(state.clone()));
            wikicraft::cluster::spawn(&state);
            wikicraft::jobs::spawn_worker(&state);
            wikicraft::schema_upgrade::spawn(&state);
            wikicraft::mail::spawn_worker(&state);
            wikicraft::webhooks::spawn_worker(&state);
            let app = wikicraft::routes::router(state.clone());
            let listener = tokio::net::TcpListener::bind(bind).await?;
            tracing::info!("WikiCraft listening on http://{bind}; web app: {}", wikicraft::web_assets::source(&state.config.static_dir));
            axum::serve(listener, app).with_graceful_shutdown(shutdown(state)).await?;
        }
    }
    Ok(())
}

/// Waits for Ctrl-C or SIGTERM (what Kubernetes/docker send), then flushes live documents.
async fn shutdown(state: AppState) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down, flushing live documents");
    state.shutting_down.store(true, std::sync::atomic::Ordering::SeqCst);
    wikicraft::collab::flush_all(&state).await;
}
