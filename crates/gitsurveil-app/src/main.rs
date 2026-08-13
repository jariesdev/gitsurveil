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
mod tray;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};

/// Label of the popover window. Used to find (or confirm the absence of) the
/// window on every tray click.
const POPOVER_LABEL: &str = "popover";

/// Id of the tray icon, so the severity watcher can find it to recolor.
const TRAY_ID: &str = "main";

/// Label of the main desktop window (`specs/desktop-ui.md`).
const MAIN_LABEL: &str = "main";

/// Main window dimensions — big enough for the dashboard's two-column layout
/// without assuming a large display.
const MAIN_WIDTH: f64 = 1000.0;
const MAIN_HEIGHT: f64 = 680.0;

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
            commands::open_main_window,
            commands::list_history,
            commands::dismiss_item,
            commands::undismiss_item,
            commands::add_account,
            commands::remove_account,
            commands::list_accounts,
            commands::list_rules,
            commands::list_repos,
            commands::set_repo,
            commands::remove_repo,
            commands::poll_now,
            commands::pr_detail,
            commands::pr_create,
            commands::pr_update,
            commands::pr_close,
            commands::pr_merge,
            commands::pr_comments,
            commands::pr_comment,
            commands::pr_branches,
            commands::conflict_prepare,
            commands::conflict_file,
            commands::conflict_save,
            commands::conflict_commit,
            commands::conflict_push,
            commands::conflict_abort,
        ])
        .setup(|app| {
            // Menubar app, not a dock app: no dock icon, no app-switcher
            // entry, and crucially no "app quit" semantics when the last
            // window closes. Set at runtime rather than via an Info.plist
            // key so it applies during `tauri dev` too.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;
            // Keeps the tray color current with no popover open.
            tray::spawn_severity_watcher(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the gitsurveil app")
        .run(|_app, event| {
            // Keep the process alive when the last window closes. Without
            // this, destroying the popover would quit the app and take the
            // tray icon with it — but destroying that window is exactly how
            // the idle-footprint budget is met.
            //
            // `code` distinguishes the two ways an exit is requested: `None`
            // means the windows went away, `Some` means something called
            // `app.exit()` — which is what the tray's Quit does. Vetoing both
            // makes Quit do nothing at all.
            if let tauri::RunEvent::ExitRequested { api, code: None, .. } = event {
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

    TrayIconBuilder::with_id(TRAY_ID)
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
                let _ = open_main(app);
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

/// Opens the full desktop window, focusing it if it already exists.
///
/// Unlike the popover this is an ordinary window: it stays where the user put
/// it, appears in the app switcher, and does not vanish on blur. Its webview
/// is still dropped when the window closes, so a closed main window costs
/// nothing.
fn open_main(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(MAIN_LABEL) {
        existing.show()?;
        existing.unminimize().ok();
        existing.set_focus()?;
        return Ok(());
    }

    // `#main` routes the shared bundle to the desktop UI; the popover loads
    // the same index.html with no fragment.
    let window = WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App("index.html#main".into()))
        .title("gitsurveil")
        .inner_size(MAIN_WIDTH, MAIN_HEIGHT)
        .min_inner_size(760.0, 520.0)
        .resizable(true)
        .build()?;

    // The app runs as an accessory (no dock icon) for the tray's sake, which
    // also means a new window can open behind whatever is in front. Become a
    // regular app while a real window is up, so "Open gitsurveil" actually
    // shows you something and the window is reachable in the app switcher.
    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy;
        let _ = app.set_activation_policy(ActivationPolicy::Regular);

        // ...and go back to accessory when it closes, or the dock icon
        // outlives the window that justified it.
        let handle = app.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::Destroyed = event {
                let _ = handle.set_activation_policy(ActivationPolicy::Accessory);
            }
        });
    }
    window.set_focus()?;
    Ok(())
}

/// Commands callable from the webview. Each is a thin pass-through to the
/// daemon: the frontend never talks to GitHub, and never holds state the
/// daemon doesn't have.
mod commands {
    use gitsurveil_proto::{AccountRef, ScoredItem, StatusResult};

    /// Returns every open action item, scored and sorted by the daemon's
    /// priority engine, or an error string the UI renders as a "service
    /// unreachable" state.
    #[tauri::command]
    pub async fn list_items() -> Result<Vec<ScoredItem>, String> {
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

    /// Opens the full desktop window, and closes the popover behind it.
    #[tauri::command]
    pub async fn open_main_window(app: tauri::AppHandle) -> Result<(), String> {
        crate::open_main(&app).map_err(|e| e.to_string())?;
        close_popover(app).await
    }

    /// Resolved and dismissed items, for the history view.
    #[tauri::command]
    pub async fn list_history(limit: Option<usize>) -> Result<Vec<ScoredItem>, String> {
        crate::daemon::list_history(limit)
            .await
            .map_err(|e| e.to_string())
    }

    /// Hides an item locally. GitHub activity on it will bring it back.
    #[tauri::command]
    pub async fn dismiss_item(id: String) -> Result<(), String> {
        crate::daemon::set_dismissed(&id, true)
            .await
            .map_err(|e| e.to_string())
    }

    /// Restores a dismissed item.
    #[tauri::command]
    pub async fn undismiss_item(id: String) -> Result<(), String> {
        crate::daemon::set_dismissed(&id, false)
            .await
            .map_err(|e| e.to_string())
    }

    /// Validates a token and registers an account.
    #[tauri::command]
    pub async fn add_account(
        host: String,
        token: String,
        api_base: Option<String>,
    ) -> Result<AccountRef, String> {
        crate::daemon::add_account(&host, &token, api_base.as_deref())
            .await
            .map_err(|e| e.to_string())
    }

    /// Removes an account, its items, and its stored token.
    #[tauri::command]
    pub async fn remove_account(id: String) -> Result<(), String> {
        crate::daemon::remove_account(&id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Lists configured accounts. Never includes tokens.
    #[tauri::command]
    pub async fn list_accounts() -> Result<Vec<AccountRef>, String> {
        crate::daemon::list_accounts().await.map_err(|e| e.to_string())
    }

    /// Lists the active priority rules, so the UI can explain scores.
    #[tauri::command]
    pub async fn list_rules() -> Result<serde_json::Value, String> {
        crate::daemon::list_rules().await.map_err(|e| e.to_string())
    }

    /// Lists configured local clone paths for the conflict resolver.
    #[tauri::command]
    pub async fn list_repos() -> Result<serde_json::Value, String> {
        crate::daemon::repos_list().await.map_err(|e| e.to_string())
    }

    /// Registers a local clone path for one repo (daemon-validated).
    #[tauri::command]
    pub async fn set_repo(repo: String, path: String) -> Result<serde_json::Value, String> {
        crate::daemon::repos_set(&repo, &path).await.map_err(|e| e.to_string())
    }

    /// Removes a repo's local clone path.
    #[tauri::command]
    pub async fn remove_repo(repo: String) -> Result<serde_json::Value, String> {
        crate::daemon::repos_remove(&repo).await.map_err(|e| e.to_string())
    }

    /// Triggers an immediate poll instead of waiting for the next cycle.
    #[tauri::command]
    pub async fn poll_now() -> Result<(), String> {
        crate::daemon::poll_now().await.map_err(|e| e.to_string())
    }

    // ---- pull requests (`specs/pr-management.md`) --------------------
    //
    // Every mutating command below runs only from an explicit click in the
    // UI. The daemon does the GitHub call; these just forward.

    /// Full detail for one pull request.
    #[tauri::command]
    pub async fn pr_detail(repo: String, number: u64) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call("pr.detail", serde_json::json!({ "repo": repo, "number": number }))
            .await
            .map_err(|e| e.to_string())
    }

    /// Creates a pull request.
    #[tauri::command]
    pub async fn pr_create(
        repo: String,
        base: String,
        head: String,
        title: String,
        body: String,
        draft: bool,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.create",
            serde_json::json!({
                "repo": repo, "base": base, "head": head,
                "title": title, "body": body, "draft": draft,
            }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Applies a partial update to a pull request.
    #[tauri::command]
    pub async fn pr_update(
        repo: String,
        number: u64,
        patch: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.update",
            serde_json::json!({ "repo": repo, "number": number, "patch": patch }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Closes a pull request without merging.
    #[tauri::command]
    pub async fn pr_close(
        repo: String,
        number: u64,
        comment: Option<String>,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.close",
            serde_json::json!({ "repo": repo, "number": number, "comment": comment }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Merges a pull request. `head_sha` guards against merging a PR that
    /// moved since the UI loaded it.
    #[tauri::command]
    pub async fn pr_merge(
        repo: String,
        number: u64,
        method: String,
        head_sha: String,
        title: Option<String>,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.merge",
            serde_json::json!({
                "repo": repo, "number": number, "method": method,
                "head_sha": head_sha, "title": title,
            }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// The conversation on a pull request.
    #[tauri::command]
    pub async fn pr_comments(repo: String, number: u64) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call("pr.comments", serde_json::json!({ "repo": repo, "number": number }))
            .await
            .map_err(|e| e.to_string())
    }

    /// Posts a comment on a pull request.
    #[tauri::command]
    pub async fn pr_comment(
        repo: String,
        number: u64,
        body: String,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.comment",
            serde_json::json!({ "repo": repo, "number": number, "body": body }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Branch names in a repository, for the create-PR form.
    #[tauri::command]
    pub async fn pr_branches(repo: String) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call("pr.branches", serde_json::json!({ "repo": repo }))
            .await
            .map_err(|e| e.to_string())
    }

    // ---- conflict resolution (`specs/conflict-resolver.md`) --------------
    //
    // Six thin pass-throughs. The daemon owns the temp worktree, the merge,
    // the commit, and the push; the UI only previews and sends intent.

    /// Starts a resolution session for `repo#number` on a temp worktree.
    #[tauri::command]
    pub async fn conflict_prepare(
        repo: String,
        number: u64,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::conflicts_call(
            "conflicts.prepare",
            serde_json::json!({ "repo": repo, "number": number }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// The conflict regions of one file in the session's worktree.
    #[tauri::command]
    pub async fn conflict_file(
        session_id: String,
        path: String,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::conflicts_call(
            "conflicts.file",
            serde_json::json!({ "session_id": session_id, "path": path }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Writes a resolution: full `content`, or a whole-file `pick` of a side.
    #[tauri::command]
    pub async fn conflict_save(
        session_id: String,
        path: String,
        content: Option<String>,
        pick: Option<String>,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::conflicts_call(
            "conflicts.save",
            serde_json::json!({
                "session_id": session_id, "path": path,
                "content": content, "pick": pick,
            }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Stages the resolved files and creates the merge commit in the worktree.
    #[tauri::command]
    pub async fn conflict_commit(
        session_id: String,
        message: String,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::conflicts_call(
            "conflicts.commit",
            serde_json::json!({ "session_id": session_id, "message": message }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Pushes the resolution branch to the PR's head and tears the session down.
    #[tauri::command]
    pub async fn conflict_push(session_id: String) -> Result<serde_json::Value, String> {
        crate::daemon::conflicts_call(
            "conflicts.push",
            serde_json::json!({ "session_id": session_id }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Abandons the session: prunes the worktree, deletes the branch, keeps the
    /// user's clone and the remote untouched. Idempotent.
    #[tauri::command]
    pub async fn conflict_abort(session_id: String) -> Result<serde_json::Value, String> {
        crate::daemon::conflicts_call(
            "conflicts.abort",
            serde_json::json!({ "session_id": session_id }),
        )
        .await
        .map_err(|e| e.to_string())
    }
}
