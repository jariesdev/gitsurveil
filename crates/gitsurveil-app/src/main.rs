//! `gitsurveil` — the desktop app (`specs/menubar-ui.md`).
//!
//! A thin client over `gitsurveild`. This binary owns only presentation: the
//! tray icon and the popover window. All state lives in the daemon, which
//! keeps polling and notifying whether or not this process is running.
//!
//! The defining constraint is idle footprint: the popover webview is
//! **destroyed** when it closes and rebuilt when the tray is clicked, so an
//! idle menubar app costs a tray icon and nothing else. Everything here is
//! written to preserve that — notably, no long-lived webview and no state
//! cached on the JS side that would need one.

// Hides the console window that would otherwise appear alongside the GUI on
// Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![warn(missing_docs)]

mod daemon;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};

/// Label of the popover window. Used to find (or confirm the absence of) the
/// window on every tray click.
const POPOVER_LABEL: &str = "popover";

/// Popover dimensions. Sized for a scannable list without becoming a second
/// main window — the full UI is a separate surface (`specs/desktop-ui.md`).
const POPOVER_WIDTH: f64 = 400.0;
const POPOVER_HEIGHT: f64 = 520.0;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_items,
            commands::daemon_status,
            commands::open_url,
            commands::close_popover,
        ])
        .setup(|app| {
            // Menubar app, not a dock app: no dock icon, no app-switcher
            // entry, and crucially no "app quit" semantics when the last
            // window closes. Set at runtime rather than via an Info.plist
            // key so it applies during `tauri dev` too.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the gitsurveil app")
        .run(|_app, event| {
            // Keep the process alive when the popover closes. Without this,
            // destroying the last window would quit the app and take the tray
            // icon with it — but destroying that window is exactly how the
            // idle-footprint budget is met.
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}

/// Builds the tray icon and its right-click menu.
///
/// Deliberately the *only* place a tray icon is created: declaring one in
/// `tauri.conf.json` as well produces a second, inert icon in the menu bar,
/// since the config-declared icon carries none of the handlers below.
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let open_full_ui = MenuItem::with_id(app, "open_full_ui", "Open gitsurveil", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_full_ui, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("bundled icon"))
        // Let the OS tint the icon for light/dark menu bars instead of
        // shipping two assets. Severity coloring replaces this in Phase 4.
        .icon_as_template(true)
        .menu(&menu)
        // Without this, a left-click opens the menu and we never see the
        // click event that should toggle the popover.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "open_full_ui" => {
                // The full desktop UI is Phase 5; until it exists this at
                // least surfaces the same list rather than doing nothing.
                let _ = toggle_popover(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = toggle_popover(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Opens the popover, or closes it if it's already open.
///
/// "Close" here means [`tauri::WebviewWindow::close`], which **destroys** the
/// window and its webview rather than hiding it. That is deliberate and is the
/// single most important line in this file for the memory budget: a hidden
/// webview keeps its renderer process alive, a destroyed one does not.
fn toggle_popover(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(POPOVER_LABEL) {
        existing.close()?;
        return Ok(());
    }

    let popover = WebviewWindowBuilder::new(app, POPOVER_LABEL, WebviewUrl::default())
        .title("gitsurveil")
        .inner_size(POPOVER_WIDTH, POPOVER_HEIGHT)
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

    use tauri_plugin_positioner::{Position, WindowExt};
    // TrayCenter needs the tray position captured by `on_tray_event` above.
    popover.move_window(Position::TrayCenter).ok();
    popover.show()?;
    popover.set_focus()?;

    // Dismiss-on-blur, the expected behavior for a menubar popover. Also the
    // main path by which the webview gets destroyed, so it directly serves
    // the idle-footprint budget.
    let handle = app.clone();
    popover.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(false) = event {
            if let Some(window) = handle.get_webview_window(POPOVER_LABEL) {
                let _ = window.close();
            }
        }
    });

    Ok(())
}

/// Commands callable from the webview. Each is a thin pass-through to the
/// daemon: the frontend never talks to GitHub, and never holds state the
/// daemon doesn't have.
mod commands {
    use gitsurveil_proto::{ActionItem, StatusResult};

    /// Returns every open action item, or an error string the UI renders as a
    /// "service unreachable" state.
    #[tauri::command]
    pub async fn list_items() -> Result<Vec<ActionItem>, String> {
        crate::daemon::list_items().await.map_err(|e| e.to_string())
    }

    /// Returns the daemon's status summary.
    #[tauri::command]
    pub async fn daemon_status() -> Result<StatusResult, String> {
        crate::daemon::status().await.map_err(|e| e.to_string())
    }

    /// Opens `url` in the user's browser, then closes the popover — clicking
    /// an item should get you to GitHub and leave no window behind.
    #[tauri::command]
    pub async fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())?;
        close_popover(app).await
    }

    /// Closes (and destroys) the popover window.
    #[tauri::command]
    pub async fn close_popover(app: tauri::AppHandle) -> Result<(), String> {
        use tauri::Manager;
        if let Some(window) = app.get_webview_window(crate::POPOVER_LABEL) {
            window.close().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
