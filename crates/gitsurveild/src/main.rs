//! `gitsurveild` — the gitsurveil background daemon (`specs/daemon.md`).
//!
//! Phase 1 scope: `--foreground` mode only. Service registration
//! (launchd/systemd/Windows) is Phase 9 — see `specs/architecture.md`.
//! Everything the daemon does lives behind the local API (`crate::socket`);
//! this file only wires config, storage, the poller, and the server together.

#![warn(missing_docs)]

mod config;
mod error;
mod github;
mod keychain;
mod poller;
mod socket;
mod store;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub use error::DaemonError;

use crate::config::Config;
use crate::socket::ServerState;
use crate::store::Store;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gitsurveild=info".into()),
        )
        .init();

    let foreground = std::env::args().any(|a| a == "--foreground");
    if !foreground {
        eprintln!(
            "gitsurveild: only `--foreground` is implemented so far; \
             service registration (launchd/systemd/Windows) lands in Phase 9."
        );
        std::process::exit(2);
    }

    if let Err(e) = run().await {
        tracing::error!("fatal: {e}");
        std::process::exit(1);
    }
}

async fn run() -> error::Result<()> {
    let data_dir = config::data_dir()?;
    let config_path = data_dir.join("config.toml");
    let config = Config::load(&config_path)?;
    tracing::info!(interval = config.poll_interval_secs, "config loaded");

    let db_path = data_dir.join("gitsurveil.db");
    let store = Arc::new(Store::open(&db_path)?);
    tracing::info!(path = %db_path.display(), "store opened");

    let state = Arc::new(ServerState {
        store: Arc::clone(&store),
        started_at: Instant::now(),
    });

    let poll_handle = tokio::spawn(poller::run(
        Arc::clone(&store),
        config.poll_interval_secs,
    ));

    let address = local_api_address(&data_dir);
    let serve_result = tokio::select! {
        result = socket::serve(state, &address) => result,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received shutdown signal");
            Ok(())
        }
    };

    poll_handle.abort();
    serve_result
}

/// Where the local API listens, per `specs/architecture.md`: a unix socket
/// path on macOS/Linux (Linux prefers `$XDG_RUNTIME_DIR` when set), or a
/// named pipe name on Windows.
fn local_api_address(data_dir: &std::path::Path) -> String {
    if cfg!(windows) {
        return r"\\.\pipe\gitsurveil".to_string();
    }
    if cfg!(target_os = "linux") {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            return PathBuf::from(runtime_dir)
                .join("gitsurveil.sock")
                .to_string_lossy()
                .into_owned();
        }
    }
    data_dir
        .join("daemon.sock")
        .to_string_lossy()
        .into_owned()
}
