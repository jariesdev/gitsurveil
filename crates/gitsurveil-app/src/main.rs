//! `gitsurveil` — the desktop app (`specs/menubar-ui.md`).
//!
//! A thin client over `gitsurveild`. This binary owns only presentation: the
//! tray icon and the popover window. All state lives in the daemon, which
//! keeps polling and notifying whether or not this process is running.
//!
//! The defining constraint is idle footprint. The popover webview is
//! **hidden** (not destroyed) between uses so a tray click shows it instantly,
//! and a background task in `popover` destroys it once it has sat hidden past
//! an idle timeout — an abandoned popover eventually costs nothing again, and
//! a frequently used one never pays the recreate cost.

// Hides the console window that would otherwise appear alongside the GUI on
// Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![warn(missing_docs)]

mod daemon;
mod popover;
mod tray;

use std::path::PathBuf;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};

/// Id of the tray icon, so the severity watcher can find it to recolor.
const TRAY_ID: &str = "main";

/// Label of the main desktop window (`specs/desktop-ui.md`).
const MAIN_LABEL: &str = "main";

/// Main window dimensions — big enough for the dashboard's two-column layout
/// without assuming a large display.
const MAIN_WIDTH: f64 = 1000.0;
const MAIN_HEIGHT: f64 = 680.0;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_items,
            commands::daemon_status,
            commands::open_url,
            commands::browsers_list,
            commands::open_url_with_browser,
            commands::close_popover,
            commands::open_main_window,
            commands::list_history,
            commands::clear_history,
            commands::dismiss_item,
            commands::undismiss_item,
            commands::add_account,
            commands::remove_account,
            commands::update_account_token,
            commands::list_accounts,
            commands::list_rules,
            commands::notifications_prefs,
            commands::notifications_set_pref,
            commands::repos_list,
            commands::repos_set,
            commands::repos_set_notify,
            commands::repos_remove,
            commands::repos_new,
            commands::repos_ack_new,
            commands::repos_refresh,
            commands::repos_clone,
            commands::repos_clone_status,
            commands::repos_worktrees,
            commands::repos_worktree_add,
            commands::repos_worktree_remove,
            commands::poll_now,
            commands::pr_detail,
            commands::pr_create,
            commands::pr_update,
            commands::pr_close,
            commands::pr_merge,
            commands::pr_comments,
            commands::pr_comment,
            commands::pr_comment_reply,
            commands::pr_resolve,
            commands::pr_branches,
            commands::pr_labels,
            commands::prs_list,
            commands::apps_list,
            commands::apps_add,
            commands::apps_remove,
            commands::apps_open,
            commands::reveal_in_file_manager,
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
            // Idle-clock + teardown for the hidden-but-warm popover: hide/show
            // is cheap, but a popover nobody clicks must give its webview back.
            app.manage(popover::IdleClock::default());
            popover::spawn_idle_teardown(app.handle().clone());
            // Fire-and-forget: gets the daemon running (and registered to
            // survive future logins) on every platform, without delaying the
            // tray icon.
            tauri::async_runtime::spawn(ensure_daemon_running());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the gitsurveil app")
        .run(|_app, event| {
            // Keep the process alive when the last window closes. Without
            // this, destroying the popover (the idle-teardown path) would quit
            // the app and take the tray icon with it.
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
    let open_full_ui = MenuItem::with_id(app, "open_full_ui", "Open GitSurveil", true, None::<&str>)?;
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
                let _ = popover::toggle(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Builds the macOS application menu (the one next to the Apple logo).
///
/// Standard items use `PredefinedMenuItem` so the OS handles About, Hide,
/// and Quit natively — no custom event wiring needed. The menu is set on
/// the main window; it only appears when that window is focused.
fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let about = PredefinedMenuItem::about(
        app,
        Some("About GitSurveil"),
        Some(tauri::menu::AboutMetadata {
            version: Some(env!("CARGO_PKG_VERSION").into()),
            copyright: Some("Copyright 2026 Jay Aries Flores".into()),
            ..Default::default()
        }),
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let services = PredefinedMenuItem::services(app, Some("Services"))?;
    let hide = PredefinedMenuItem::hide(app, Some("Hide GitSurveil"))?;
    let hide_others = PredefinedMenuItem::hide_others(app, None)?;
    let show_all = PredefinedMenuItem::show_all(app, None)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit GitSurveil"))?;
    let website = MenuItem::with_id(app, "website", "GitSurveil on GitHub", true, None::<&str>)?;

    let app_menu = Submenu::with_items(
        app,
        "GitSurveil",
        true,
        &[
            &about,
            &separator,
            &website,
            &separator,
            &services,
            &separator,
            &hide,
            &hide_others,
            &show_all,
            &separator,
            &quit,
        ],
    )?;

    Menu::with_items(app, &[&app_menu])
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
        .title("GitSurveil")
        .inner_size(MAIN_WIDTH, MAIN_HEIGHT)
        .min_inner_size(760.0, 520.0)
        .resizable(true)
        .build()?;

    window.set_menu(build_app_menu(app)?)?;
    {
        let app = app.clone();
        window.on_menu_event(move |_window, event| {
            if event.id.as_ref() == "website" {
                use tauri_plugin_opener::OpenerExt;
                let _ = app.opener().open_url(
                    "https://github.com/jariesdev/gitsurveil",
                    None::<&str>,
                );
            }
        });
    }

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

/// Makes sure `gitsurveild` is up, registering and starting it if this is a
/// fresh install (or a previous run's registration went missing).
///
/// Runs on every launch rather than only "first run": that makes it
/// self-healing for the same failure mode (registration lost, binary moved)
/// without a separate first-run flag to get out of sync with reality.
async fn ensure_daemon_running() {
    if daemon::status().await.is_ok() {
        return;
    }
    let Some(sidecar) = daemon_sidecar_path() else {
        // Dev builds run the daemon by hand (`cargo run -p gitsurveild`); a
        // packaged app always has the sidecar next to it (`bundle-daemon.sh`).
        return;
    };

    // Idempotent: writes/repoints the login registration. On macOS and Linux
    // this also starts the daemon; on Windows it only registers the Run-key
    // entry (`service.rs`), so the status check below still applies there.
    //
    // `install` blocks on `launchctl`/`systemctl`/`reg`, so it runs on a
    // blocking thread rather than pulling tokio's `process` feature in just
    // for this one startup call.
    let install_sidecar = sidecar.clone();
    let install_result =
        tokio::task::spawn_blocking(move || std::process::Command::new(&install_sidecar).arg("install").output())
            .await;
    if let Ok(Err(e)) = install_result {
        eprintln!("gitsurveil: could not run `gitsurveild install`: {e}");
    }

    // Give a just-started daemon a moment to bind its socket/pipe before
    // deciding it needs a direct spawn too.
    for _ in 0..5 {
        if daemon::status().await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    if let Err(e) = spawn_daemon_detached(&sidecar) {
        eprintln!("gitsurveil: could not start gitsurveild directly: {e}");
    }
}

/// Path to the bundled `gitsurveild` sidecar, which Tauri places next to the
/// main executable in every packaged build. `None` if it isn't there (dev
/// builds, or a build that skipped `scripts/bundle-daemon.sh`).
fn daemon_sidecar_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let name = format!("gitsurveild{}", std::env::consts::EXE_SUFFIX);
    let path = dir.join(name);
    path.exists().then_some(path)
}

/// Starts the daemon directly, detached from this process — the fallback for
/// Windows, where `install` only writes the autostart registration and never
/// starts anything itself.
#[cfg(windows)]
fn spawn_daemon_detached(sidecar: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // DETACHED_PROCESS: survives this process exiting. CREATE_NO_WINDOW: no
    // console flash, matching the `windows_subsystem = "windows"` app itself.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(sidecar)
        .arg("--foreground")
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()?;
    Ok(())
}

/// macOS/Linux fallback, for the rare case `install` wrote the registration
/// but its own start-now step failed (e.g. `launchctl` reachable neither via
/// `bootstrap` nor `load`, per `service.rs`).
#[cfg(not(windows))]
fn spawn_daemon_detached(sidecar: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new(sidecar)
        .arg("--foreground")
        .spawn()?;
    Ok(())
}

/// Commands callable from the webview. Each is a thin pass-through to the
/// daemon: the frontend never talks to GitHub, and never holds state the
/// daemon doesn't have.
mod commands {
    use gitsurveil_proto::{AccountRef, ScoredItem, StatusResult};
    use tauri::Emitter;

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

    /// Opens `url` in the user's browser, then hides the popover — clicking
    /// an item should get you to GitHub and leave no window behind.
    #[tauri::command]
    pub async fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())?;
        close_popover(app).await
    }

    /// Lists installed system browsers by checking for known `.app` bundles in
    /// standard Application directories. Used by context-menu submenus to let
    /// the user pick a specific browser instead of the OS default.
    #[tauri::command]
    pub async fn browsers_list() -> Result<Vec<String>, String> {
        let browsers = [
            "Safari",
            "Google Chrome",
            "Firefox",
            "Microsoft Edge",
            "Brave Browser",
            "Arc",
            "Vivaldi",
            "Opera",
            "Chromium",
        ];
        let app_dirs: Vec<std::path::PathBuf> = [
            Some(std::path::PathBuf::from("/Applications")),
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join("Applications")),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut found = Vec::new();
        for browser in browsers {
            for dir in &app_dirs {
                if dir.join(format!("{browser}.app")).is_dir() {
                    found.push(browser.to_string());
                    break;
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Opens `url` in the specified browser using `open -a` on macOS. The
    /// browser name must match the `.app` bundle name (e.g. "Google Chrome").
    #[tauri::command]
    pub async fn open_url_with_browser(url: String, browser: String) -> Result<(), String> {
        std::process::Command::new("open")
            .args(["-a", &browser, &url])
            .spawn()
            .map_err(|e| format!("failed to open {browser}: {e}"))?;
        Ok(())
    }

    /// Dismisses the popover: hides it (webview stays warm for the next tray
    /// click) rather than destroying it. The idle teardown in `popover`
    /// reclaims it if the user never comes back.
    #[tauri::command]
    pub async fn close_popover(app: tauri::AppHandle) -> Result<(), String> {
        crate::popover::hide(&app);
        Ok(())
    }

    /// Opens the full desktop window, and hides the popover behind it.
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

    /// Archives every resolved and dismissed item permanently; the UI
    /// confirms with the user first.
    #[tauri::command]
    pub async fn clear_history() -> Result<(), String> {
        crate::daemon::clear_history()
            .await
            .map_err(|e| e.to_string())
    }

    /// Hides an item locally. GitHub activity on it will bring it back.
    #[tauri::command]
    pub async fn dismiss_item(app: tauri::AppHandle, id: String) -> Result<(), String> {
        crate::daemon::set_dismissed(&id, true)
            .await
            .map_err(|e| e.to_string())?;
        notify_items_changed(&app);
        Ok(())
    }

    /// Restores a dismissed item.
    #[tauri::command]
    pub async fn undismiss_item(app: tauri::AppHandle, id: String) -> Result<(), String> {
        crate::daemon::set_dismissed(&id, false)
            .await
            .map_err(|e| e.to_string())?;
        notify_items_changed(&app);
        Ok(())
    }

    /// Tells every open window that an item's local state changed, so each
    /// refetches instead of showing a stale list. Emitted from the shared
    /// `dismiss_item`/`undismiss_item` commands — a dismissal in the popover
    /// must immediately remove the item from an open Dashboard, and a
    /// restore in History must bring it back in the popover too. Windows
    /// never refetch on their own action; the event is the one refresh path.
    fn notify_items_changed(app: &tauri::AppHandle) {
        let _ = app.emit("items-changed", ());
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

    /// Validates a new token against the existing account's GitHub instance,
    /// then replaces the old token in the OS keychain.
    #[tauri::command]
    pub async fn update_account_token(id: String, token: String) -> Result<AccountRef, String> {
        crate::daemon::update_account_token(&id, &token)
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

    /// Lists every item kind's current notification preference.
    #[tauri::command]
    pub async fn notifications_prefs() -> Result<Vec<gitsurveil_proto::KindPref>, String> {
        crate::daemon::notifications_prefs().await.map_err(|e| e.to_string())
    }

    /// Sets whether `kind` may produce a notification.
    #[tauri::command]
    pub async fn notifications_set_pref(kind: String, enabled: bool) -> Result<(), String> {
        crate::daemon::notifications_set_pref(&kind, enabled)
            .await
            .map_err(|e| e.to_string())
    }

    /// Lists the repository catalog (`specs/desktop-ui.md`): every discovered
    /// repo plus the orgs each account can filter by.
    #[tauri::command]
    pub async fn repos_list() -> Result<gitsurveil_proto::RepoCatalog, String> {
        crate::daemon::repos_list().await.map_err(|e| e.to_string())
    }

    /// Registers a local clone path for one repo (daemon-validated) and marks
    /// it tracked.
    #[tauri::command]
    pub async fn repos_set(repo: String, path: String) -> Result<gitsurveil_proto::Repository, String> {
        crate::daemon::repos_set(&repo, &path).await.map_err(|e| e.to_string())
    }

    /// Sets whether a repo's items feed notifications and the Pull Requests
    /// view, independent of its clone-tracking state.
    #[tauri::command]
    pub async fn repos_set_notify(
        account_id: String,
        repo: String,
        enabled: bool,
    ) -> Result<gitsurveil_proto::Repository, String> {
        crate::daemon::repos_set_notify(&account_id, &repo, enabled)
            .await
            .map_err(|e| e.to_string())
    }

    /// Removes a repo's local clone path; idempotent. The catalog row survives.
    #[tauri::command]
    pub async fn repos_remove(repo: String) -> Result<(), String> {
        crate::daemon::repos_remove(&repo).await.map_err(|e| e.to_string())
    }

    /// Repositories discovered but never acknowledged, newest-first.
    #[tauri::command]
    pub async fn repos_new() -> Result<Vec<gitsurveil_proto::Repository>, String> {
        crate::daemon::repos_new().await.map_err(|e| e.to_string())
    }

    /// Dismisses every currently-new repository; returns how many were acked.
    #[tauri::command]
    pub async fn repos_ack_new(first_seen_at: String) -> Result<u64, String> {
        crate::daemon::repos_ack_new(&first_seen_at)
            .await
            .map_err(|e| e.to_string())
    }

    /// Forces a discovery cycle for every account; returns the fresh catalog.
    #[tauri::command]
    pub async fn repos_refresh() -> Result<gitsurveil_proto::RepoCatalog, String> {
        crate::daemon::repos_refresh().await.map_err(|e| e.to_string())
    }

    /// Starts a background clone; returns a `job_id` to poll via
    /// `repos_clone_status`.
    #[tauri::command]
    pub async fn repos_clone(repo: String, target: String) -> Result<String, String> {
        crate::daemon::repos_clone(&repo, &target).await.map_err(|e| e.to_string())
    }

    /// One clone job's current status, or `null` for an unknown id.
    #[tauri::command]
    pub async fn repos_clone_status(job_id: String) -> Result<Option<gitsurveil_proto::CloneStatus>, String> {
        crate::daemon::repos_clone_status(&job_id)
            .await
            .map_err(|e| e.to_string())
    }

    /// A repo's user-created worktrees plus the branches a new one can use.
    #[tauri::command]
    pub async fn repos_worktrees(repo: String) -> Result<gitsurveil_proto::WorktreesResult, String> {
        crate::daemon::repos_worktrees(&repo).await.map_err(|e| e.to_string())
    }

    /// Creates a worktree for `branch` at `path`; `branch` may be new.
    #[tauri::command]
    pub async fn repos_worktree_add(
        repo: String,
        branch: String,
        path: String,
    ) -> Result<gitsurveil_proto::WorktreeInfo, String> {
        crate::daemon::repos_worktree_add(&repo, &branch, &path)
            .await
            .map_err(|e| e.to_string())
    }

    /// Removes a worktree (keeping its branch); refuses dirty worktrees
    /// unless `force` is true.
    #[tauri::command]
    pub async fn repos_worktree_remove(
        repo: String,
        name: String,
        force: bool,
    ) -> Result<(), String> {
        crate::daemon::repos_worktree_remove(&repo, &name, force)
            .await
            .map_err(|e| e.to_string())
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

    /// Replies inside a review thread; `in_reply_to` is the last comment's id.
    #[tauri::command]
    pub async fn pr_comment_reply(
        repo: String,
        number: u64,
        in_reply_to: u64,
        body: String,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.comment_reply",
            serde_json::json!({
                "repo": repo,
                "number": number,
                "in_reply_to": in_reply_to,
                "body": body,
            }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Resolves or unresolves a review thread by its GraphQL id.
    #[tauri::command]
    pub async fn pr_resolve(
        repo: String,
        thread_id: String,
        resolved: bool,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "pr.resolve",
            serde_json::json!({ "repo": repo, "thread_id": thread_id, "resolved": resolved }),
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

    /// Label names defined on a repository, for the edit form's picker.
    #[tauri::command]
    pub async fn pr_labels(repo: String) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call("pr.labels", serde_json::json!({ "repo": repo }))
            .await
            .map_err(|e| e.to_string())
    }

    /// Rows for the Pull Requests view (`specs/desktop-ui.md`). `state` is
    /// `open`/`closed`/`merged` or `None` for all; it re-queries the daemon
    /// because it changes the GraphQL search qualifier.
    #[tauri::command]
    pub async fn prs_list(
        account_id: Option<String>,
        state: Option<String>,
    ) -> Result<serde_json::Value, String> {
        crate::daemon::pr_call(
            "prs.list",
            serde_json::json!({ "account_id": account_id, "state": state }),
        )
        .await
        .map_err(|e| e.to_string())
    }

    // ---- registered apps (`specs/desktop-ui.md`) ------------------------
    //
    // The "Open with" apps for worktree context menus. The daemon stores the
    // registry and does the actual process spawn; these just forward.

    /// Lists the registered "Open with" applications, sorted by display name.
    #[tauri::command]
    pub async fn apps_list() -> Result<Vec<gitsurveil_proto::RegisteredApp>, String> {
        crate::daemon::apps_list().await.map_err(|e| e.to_string())
    }

    /// Registers an application (`name` shown in the menu, `command` is the
    /// bare executable on `PATH`).
    #[tauri::command]
    pub async fn apps_add(name: String, command: String) -> Result<gitsurveil_proto::RegisteredApp, String> {
        crate::daemon::apps_add(&name, &command).await.map_err(|e| e.to_string())
    }

    /// Forgets a registered application; idempotent.
    #[tauri::command]
    pub async fn apps_remove(command: String) -> Result<(), String> {
        crate::daemon::apps_remove(&command).await.map_err(|e| e.to_string())
    }

    /// Opens `path` with a registered application. The daemon spawns
    /// `command <path>` (never through a shell) and errors if the app is not
    /// installed or registered.
    #[tauri::command]
    pub async fn apps_open(command: String, path: String) -> Result<(), String> {
        crate::daemon::apps_open(&command, &path).await.map_err(|e| e.to_string())
    }

    /// Reveals `path` in the native file manager (Finder on macOS, Explorer on
    /// Windows). No-op on other platforms — the frontend should never call this
    /// on Linux.
    #[tauri::command]
    pub fn reveal_in_file_manager(path: String) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .args(["-R", &path])
                .spawn()
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("explorer")
                .args(["/select,", &path])
                .spawn()
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        #[allow(unreachable_code)]
        Err("reveal_in_file_manager is not supported on this platform".into())
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
