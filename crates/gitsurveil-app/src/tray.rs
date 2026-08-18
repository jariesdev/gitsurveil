//! Tray icon severity coloring (`specs/priority-engine.md`, "Severity mapping").
//!
//! The tray is the app's ambient channel: it answers "does anything need me?"
//! without a click and without an interruption. Its color therefore has to
//! stay current even when the popover is closed and no webview exists — so
//! this runs in the Rust shell, not the frontend.
//!
//! ponytail: polls `status` on a timer rather than subscribing to a push
//! event. `specs/daemon.md` describes a `severity.changed` event stream, which
//! is the right long-term answer; a 30-second poll of an existing method is a
//! fraction of the code and the daemon is a unix socket away. Swap it for the
//! event stream when that stream exists for other reasons.

use std::time::Duration;

use gitsurveil_proto::Severity;
use tauri::image::Image;
use tauri::AppHandle;

/// How often to refresh the tray color. Comfortably faster than the default
/// 60-second poll interval, so the icon reflects a change within one cycle,
/// and cheap enough to be irrelevant at idle (one unix-socket round trip).
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// PNG bytes for each severity, embedded at compile time so the icon can be
/// swapped without touching the filesystem at runtime.
const IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
const INFO: &[u8] = include_bytes!("../icons/tray-info.png");
const NORMAL: &[u8] = include_bytes!("../icons/tray-normal.png");
const HIGH: &[u8] = include_bytes!("../icons/tray-high.png");
const CRITICAL: &[u8] = include_bytes!("../icons/tray-critical.png");

fn icon_bytes(severity: Severity) -> &'static [u8] {
    match severity {
        Severity::Idle => IDLE,
        Severity::Info => INFO,
        Severity::Normal => NORMAL,
        Severity::High => HIGH,
        Severity::Critical => CRITICAL,
    }
}

/// Applies `severity`'s icon to the tray.
///
/// None of the icons are macOS template images: each carries explicit colour
/// so the severity is legible regardless of menu-bar theme.
pub fn apply(app: &AppHandle, severity: Severity) {
    let Some(tray) = app.tray_by_id(crate::TRAY_ID) else {
        return;
    };
    match Image::from_bytes(icon_bytes(severity)) {
        Ok(image) => {
            let _ = tray.set_icon(Some(image));
            let _ = tray.set_icon_as_template(false);
        }
        Err(e) => tracing_warn(&format!("could not decode tray icon: {e}")),
    }
}

/// Spawns the background task that keeps the tray color in sync with the
/// daemon's top severity.
pub fn spawn_severity_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            // An unreachable daemon leaves the icon as-is rather than showing
            // "idle": claiming all-clear when we simply can't see is the one
            // failure mode that would actively mislead.
            if let Ok(status) = crate::daemon::status().await {
                let app = app.clone();
                let severity = status.top_severity;
                let _ = app
                    .clone()
                    .run_on_main_thread(move || apply(&app, severity));
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

/// The app has no logging stack of its own; failures here are cosmetic, so
/// they go to stderr rather than justifying a `tracing` dependency.
fn tracing_warn(message: &str) {
    eprintln!("gitsurveil: {message}");
}
