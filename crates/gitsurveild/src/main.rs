//! `gitsurveild` — the gitsurveil background daemon (`specs/daemon.md`).
//!
//! Runs attached to a terminal (`--foreground`) or registered to start at
//! login (`install`, see `crate::service`). Everything the daemon does lives
//! behind the local API (`crate::socket`); this file only parses the four
//! subcommands and wires config, storage, the poller, and the server
//! together.

#![warn(missing_docs)]

mod config;
mod conflicts;
mod discovery;
mod error;
mod gitops;
mod github;
mod keychain;
mod notifications;
mod poller;
mod priority;
mod service;
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

    // One positional subcommand, or `--foreground`. Hand-parsed rather than
    // pulling in an argument crate: there are four verbs and no options.
    let arg = std::env::args().nth(1).unwrap_or_default();
    let result = match arg.as_str() {
        "--foreground" | "run" => run().await,
        "install" => install_service(),
        "uninstall" => uninstall_service(),
        "status" => print_status(),
        "--help" | "-h" | "help" => {
            print_usage();
            return;
        }
        other => {
            if !other.is_empty() {
                eprintln!("gitsurveild: unknown argument `{other}`\n");
            }
            print_usage();
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        tracing::error!("fatal: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!(
        "gitsurveild — the gitsurveil background service\n\
         \n\
         USAGE:\n\
         \x20   gitsurveild --foreground   Run attached to this terminal\n\
         \x20   gitsurveild install        Start automatically at login\n\
         \x20   gitsurveild uninstall      Stop starting at login\n\
         \x20   gitsurveild status         Show registration and health\n"
    );
}

/// Registers the daemon to start at login, reporting where the registration
/// landed so the user can inspect or remove it by hand.
fn install_service() -> error::Result<()> {
    let location = service::install()?;
    println!("gitsurveil will now start at login.");
    println!("  registration: {location}");
    println!("  binary:       {}", std::env::current_exe()?.display());
    println!("\nMoving or rebuilding that binary breaks the registration;");
    println!("re-run `gitsurveild install` to repoint it.");
    Ok(())
}

fn uninstall_service() -> error::Result<()> {
    let location = service::uninstall()?;
    println!("gitsurveil will no longer start at login.");
    println!("  removed: {location}");
    Ok(())
}

/// Prints registration state and whether the daemon is actually answering.
///
/// The two are independent — a registration can exist while the daemon is
/// dead, and vice versa — so both are reported rather than inferring one from
/// the other.
fn print_status() -> error::Result<()> {
    let status = service::status()?;
    println!(
        "Start at login: {}",
        if status.registered { "yes" } else { "no" }
    );
    println!("  registration: {}", status.location);
    if let Some(program) = &status.program {
        println!("  binary:       {program}");
        // A registration pointing at a binary that no longer exists is the
        // most common reason "it's installed but nothing happens".
        if !std::path::Path::new(program).exists() {
            println!("  WARNING: that binary no longer exists — re-run `install`.");
        }
    }

    let data_dir = config::data_dir()?;
    let socket = local_api_address(&data_dir);
    // Connect rather than stat: a crash leaves the socket *file* behind, so
    // its mere existence would report a dead daemon as healthy — the one
    // answer that would send someone debugging in the wrong direction.
    let running = probe_socket(&socket);
    println!("\nRunning now:    {}", if running { "yes" } else { "no" });
    println!("  socket:       {socket}");
    if !running && std::path::Path::new(&socket).exists() {
        println!("  (socket file is stale; it is cleaned up on next start)");
    }
    Ok(())
}

/// Whether something is actually accepting on the local API.
#[cfg(unix)]
fn probe_socket(address: &str) -> bool {
    std::os::unix::net::UnixStream::connect(address).is_ok()
}

#[cfg(windows)]
fn probe_socket(address: &str) -> bool {
    // Opening the pipe is the equivalent check; a stale name simply fails.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(address)
        .is_ok()
}

async fn run() -> error::Result<()> {
    let data_dir = config::data_dir()?;
    let config_path = data_dir.join("config.toml");
    let config = Config::load(&config_path)?;
    tracing::info!(interval = config.poll_interval_secs, "config loaded");

    let db_path = data_dir.join("gitsurveil.db");
    let store = Arc::new(Store::open(&db_path)?);
    tracing::info!(path = %db_path.display(), "store opened");

    // One-time migration of the pre-catalog `repos` block into the catalog.
    // The import itself is a no-op once `repositories` already has rows, and
    // the file is rewritten without the stale key so the block is read once.
    // (specs/desktop-ui.md, "Migration")
    if let Some(legacy) = Config::load_legacy_repos(&config_path) {
        let entries: Vec<(String, String)> = legacy
            .into_iter()
            .map(|r| (r.repo, r.path.to_string_lossy().into_owned()))
            .collect();
        let imported = store.import_legacy_repos(&entries, &crate::poller::now_rfc3339())?;
        if imported > 0 {
            tracing::info!(count = imported, "imported legacy clone paths into the catalog");
            config.save(&config_path)?;
        }
    }

    // A crash can leave a clone job `running` with a half-fetched checkout on
    // disk. Drop the partial targets and their job rows so a retry into the
    // same folder starts clean (specs/desktop-ui.md). Only targets the daemon
    // itself created are removed; a pre-existing path is left untouched, no
    // matter what state the job died in.
    for (job_id, target, target_owned) in store.stale_running_jobs()? {
        if target_owned {
            match std::fs::remove_dir_all(&target) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(job = %job_id, "could not remove partial clone {target}: {e}")
                }
            }
        }
        store.delete_clone_job(&job_id)?;
        tracing::warn!(job = %job_id, "cleaned up stale clone job");
    }

    let state = Arc::new(ServerState {
        store: Arc::clone(&store),
        started_at: Instant::now(),
        rules: config.rules.clone(),
        sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        data_dir: data_dir.clone(),
    });

    // Sessions are in-memory only, so a daemon restart orphans any worktrees
    // a previous run created. Prune them before the server accepts requests
    // (specs/conflict-resolver.md AC-2.5). Sources are the catalog's tracked
    // clones rather than the old config block.
    let catalog = store.list_catalog()?;
    for repo in catalog.repos.iter().filter(|r| r.tracked) {
        let Some(path) = &repo.clone_path else { continue };
        match conflicts::session::prune_orphaned(std::path::Path::new(path)) {
            Ok(()) => tracing::debug!(repo = %repo.full_name, "pruned orphaned conflict worktrees"),
            Err(e) => tracing::warn!(repo = %repo.full_name, "could not prune orphaned conflict worktrees: {e}"),
        }
    }

    let poll_handle = tokio::spawn(poller::run(
        Arc::clone(&store),
        config.rules,
        config.poll_interval_secs,
    ));

    // Background catalog refresh (`specs/desktop-ui.md`): six-hour cadence
    // plus an on-demand cycle through `repos.refresh`.
    let discovery_handle = tokio::spawn(discovery::run(Arc::clone(&store)));

    let address = local_api_address(&data_dir);
    let serve_result = tokio::select! {
        result = socket::serve(state, &address) => result,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received shutdown signal");
            Ok(())
        }
    };

    poll_handle.abort();
    discovery_handle.abort();
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
