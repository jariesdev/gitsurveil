/**
 * Typed wrappers around the Tauri commands exposed by the Rust shell
 * (`crates/gitsurveil-app/src/main.rs`).
 *
 * The webview never talks to GitHub or to the daemon socket directly — it
 * calls these, and the Rust side does the work. Keeping that boundary strict
 * is what lets the webview be destroyed at any moment without losing anything.
 */

import { invoke } from "@tauri-apps/api/core";
import type { ScoredItem, StatusResult } from "./types";

/**
 * Fetches every currently open action item, already scored and sorted
 * most-urgent-first by the daemon's priority engine.
 */
export function listItems(): Promise<ScoredItem[]> {
  return invoke<ScoredItem[]>("list_items");
}

/** Fetches the daemon's status summary. */
export function daemonStatus(): Promise<StatusResult> {
  return invoke<StatusResult>("daemon_status");
}

/** Opens `url` in the default browser and dismisses the popover. */
export function openUrl(url: string): Promise<void> {
  return invoke<void>("open_url", { url });
}

/** Closes (and destroys) the popover window. */
export function closePopover(): Promise<void> {
  return invoke<void>("close_popover");
}
