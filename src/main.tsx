/**
 * Entry point for both webviews.
 *
 * One bundle serves the popover and the desktop window; the Rust shell picks
 * between them with a URL fragment (`index.html#main`). Sharing a bundle keeps
 * the types, IPC client, and row rendering in one place — and the popover
 * still only mounts its own small tree.
 */

import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./desktop/App";
import { Popover } from "./Popover";
import "./styles.css";

// Suppress the webview's native context menu ("Reload", "Inspect Element",
// …). It exposes browser affordances that make no sense in a desktop app and
// can navigate the window somewhere it can't recover from. Applies in dev
// too; the custom context menus rendered by the app's own `ContextMenu`
// component are unaffected (they listen for the same event and render their
// own menu).
document.addEventListener("contextmenu", (event) => event.preventDefault());

const isMainWindow = window.location.hash === "#main";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{isMainWindow ? <App /> : <Popover />}</React.StrictMode>,
);
