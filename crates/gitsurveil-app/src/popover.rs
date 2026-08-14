//! Popover window lifecycle (`specs/menubar-ui.md`).
//!
//! The popover is the tray's instant surface, and "instant" is a webview
//! tradeoff: recreating the window on every click costs a visible open
//! latency, while keeping it alive forever costs an idle renderer process.
//! This module picks the middle: the popover is **hidden** (not destroyed)
//! when dismissed, so a tray click shows it in ~0 ms, and a background task
//! **destroys** it after it has sat hidden past [`IDLE_TEARDOWN_TIMEOUT`],
//! reclaiming the webview for users who have stopped clicking.
//!
//! Idle policy:
//! - dismiss = hide; the webview stays warm for the next toggle;
//! - every hide stamps [`IdleClock::last_hidden`];
//! - every show clears the stamp;
//! - [`spawn_idle_teardown`] closes the window once it has been hidden past
//!   the timeout, so a never-reopened popover eventually costs nothing.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Label of the popover window. Used to find (or confirm the absence of) the
/// window on every tray click.
pub(crate) const LABEL: &str = "popover";

/// Popover dimensions. Sized for a scannable list without becoming a second
/// main window — the full UI is a separate surface (`specs/desktop-ui.md`).
const WIDTH: f64 = 400.0;
const HEIGHT: f64 = 520.0;

/// How long a hidden popover may keep its webview alive before the teardown
/// task destroys it. Long enough that ordinary back-and-forth with the tray
/// never tears the webview down; short enough that an abandoned popover stops
/// costing a renderer process.
const IDLE_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How often the teardown task checks whether the timeout has elapsed. A
/// 30-second wakeup keeps teardown within half a minute of the deadline for
/// the cost of a negligible tick.
const TEARDOWN_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// When the popover last went idle (hidden), so the teardown task can decide
/// whether a hidden webview has outlived its welcome.
#[derive(Default)]
pub(crate) struct IdleClock {
    last_hidden: Mutex<Option<Instant>>,
}

impl IdleClock {
    /// Records that the popover just went hidden.
    pub(crate) fn hidden_now(&self) {
        *self.last_hidden.lock().unwrap() = Some(Instant::now());
    }

    /// Records that the popover is back in use; cancels any pending teardown.
    pub(crate) fn shown(&self) {
        *self.last_hidden.lock().unwrap() = None;
    }

    /// True once the popover has stayed hidden past the idle timeout.
    pub(crate) fn hidden_long_enough(&self) -> bool {
        self.last_hidden
            .lock()
            .unwrap()
            .is_some_and(|hidden_at| hidden_at.elapsed() >= IDLE_TEARDOWN_TIMEOUT)
    }

    /// Clears the idle record after a teardown, so the task does not loop.
    pub(crate) fn clear(&self) {
        *self.last_hidden.lock().unwrap() = None;
    }
}

/// Shows the popover, creating it on first use, and hides it if it is already
/// visible — the tray's left-click toggle.
pub(crate) fn toggle(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        if window.is_visible().unwrap_or(false) {
            hide(app);
            return Ok(());
        }
    }
    show(app)
}

/// Dismisses the popover: **hides** it rather than destroying it, so the next
/// tray click reuses the warm webview. No-op when the popover is already gone
/// or hidden.
pub(crate) fn hide(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        if let Some(clock) = app.try_state::<IdleClock>() {
            clock.hidden_now();
        }
        let _ = window.hide();
    }
}

/// Shows and focuses the popover at the tray, reusing the hidden webview or
/// creating it on first use.
fn show(app: &AppHandle) -> tauri::Result<()> {
    let popover = match app.get_webview_window(LABEL) {
        Some(existing) => existing,
        None => create(app)?,
    };

    // In use again; no pending teardown.
    if let Some(clock) = app.try_state::<IdleClock>() {
        clock.shown();
    }

    use tauri_plugin_positioner::{Position, WindowExt};
    // TrayCenter needs the tray position captured by `on_tray_event` in
    // `crate::build_tray` on every click, before this is called.
    popover.move_window(Position::TrayCenter).ok();
    popover.show()?;
    popover.set_focus()?;
    Ok(())
}

/// Builds the popover window and its dismiss-on-blur handler.
fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let popover = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
        .title("gitsurveil")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        // Keep the popover out of the dock/taskbar and app switcher: it's a
        // menubar surface, not a window users alt-tab to.
        .skip_taskbar(true)
        .visible(false)
        .build()?;

    // Belt and braces on the frameless look: the builder flag alone has been
    // observed to leave a title bar on macOS, so assert it on the built window
    // too. A menubar popover with window chrome looks like a stray dialog.
    popover.set_decorations(false)?;

    // Dismiss-on-blur, the expected behavior for a menubar popover. Hiding
    // rather than closing is what keeps the reopen instant.
    let handle = app.clone();
    popover.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(false) = event {
            hide(&handle);
        }
    });

    Ok(popover)
}

/// Starts the background task that destroys the popover once it has been
/// hidden past [`IDLE_TEARDOWN_TIMEOUT`], so an abandoned popover eventually
/// gives its renderer process back.
pub(crate) fn spawn_idle_teardown(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TEARDOWN_CHECK_INTERVAL).await;
            let Some(clock) = app.try_state::<IdleClock>() else {
                continue;
            };
            if !clock.hidden_long_enough() {
                continue;
            }
            if let Some(window) = app.get_webview_window(LABEL) {
                // Destroy, not hide: this is the idle budget's pressure valve.
                let _ = window.close();
            }
            clock.clear();
        }
    });
}
