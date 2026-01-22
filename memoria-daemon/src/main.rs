mod config;
mod db;
mod clipboard;
mod retention;
mod ipc;

use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::net::UnixListener;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{info, warn};

/// Main daemon entrypoint.
///
/// Responsibilities:
/// - Load config from `~/.config/memoria/config.toml`
/// - Ensure data directory `~/.local/share/memoria/`
/// - Initialize SQLite at `~/.local/share/memoria/memoria.db`
/// - Bind a Unix domain socket at `/run/user/$UID/memoria.sock`
/// - Accept connections (no protocol/commands yet)
/// - Graceful shutdown on SIGTERM
#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cfg_path = config::default_config_path()?;
    let cfg = config::load_from_file(&cfg_path)?;
    info!(path=%cfg_path.display(), "loaded config");
    info!(retention_days=cfg.retention.days, "config: retention");

    let data_dir = db::default_data_dir()?;
    db::ensure_data_dir(&data_dir)?;

    let db_path = db::default_db_path()?;
    let conn = db::open_and_init(&db_path)?;
    let conn = std::sync::Arc::new(std::sync::Mutex::new(conn));
    info!(db=%db_path.display(), "database ready");

    // Start clipboard watcher in background, passing config for dedupe gating.
    let cfg_for_clipboard = cfg.clone();
    clipboard::start_watcher(conn.clone(), cfg_for_clipboard).await;
    info!("clipboard watcher started");

    // Start retention cleanup scheduler.
    let retention_policy = retention::RetentionPolicy::from_config(&cfg);
    retention::start_cleanup_scheduler(conn.clone(), retention_policy).await;
    info!("retention scheduler started");

    // Store config in Arc for IPC access.
    let cfg_arc = std::sync::Arc::new(cfg);

    let sock_path = runtime_socket_path().context("failed to build runtime socket path")?;
    let listener = bind_unix_socket(&sock_path)?;
    info!(socket=%sock_path.display(), "listening");

    run_server(listener, sock_path, conn.clone(), cfg_arc).await
}

fn init_tracing() {
    // Structured logs via `tracing`. Users can control verbosity with RUST_LOG.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .init();
}

fn runtime_socket_path() -> Result<PathBuf> {
    // systemd user services typically have XDG_RUNTIME_DIR set to `/run/user/$UID`.
    // We honor that and fall back to `/run/user/$UID` if needed.
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(dir).join("memoria.sock"));
    }

    let uid = unsafe { libc::geteuid() };
    Ok(PathBuf::from(format!("/run/user/{uid}/memoria.sock")))
}

fn bind_unix_socket(sock_path: &PathBuf) -> Result<UnixListener> {
    // If a previous instance crashed, the old socket file may exist.
    // Remove it so we can bind cleanly.
    match std::fs::remove_file(sock_path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!(error=%err, path=%sock_path.display(), "could not remove stale socket"),
    }

    let listener = UnixListener::bind(sock_path)
        .with_context(|| format!("failed to bind unix socket: {}", sock_path.display()))?;

    Ok(listener)
}

async fn run_server(listener: UnixListener, sock_path: PathBuf, conn: std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>, cfg: std::sync::Arc<config::Config>) -> Result<()> {
    let mut sigterm = signal(SignalKind::terminate()).context("failed to register SIGTERM handler")?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                info!("received SIGTERM, shutting down");
                break;
            }

            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, addr)) => {
                        info!(peer=?addr, "accepted connection");
                        let conn_clone = conn.clone();
                        let cfg_clone = cfg.clone();
                        tokio::spawn(async move {
                            ipc::handle_connection(stream, conn_clone, cfg_clone).await;
                        });
                    }
                    Err(err) => {
                        warn!(error=%err, "accept failed");
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }

    // Best-effort cleanup.
    if let Err(err) = std::fs::remove_file(&sock_path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn!(error=%err, path=%sock_path.display(), "failed to remove socket on shutdown");
        }
    }

    Ok(())
}
