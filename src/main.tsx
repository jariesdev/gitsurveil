/** Entry point for the popover webview. */

import React from "react";
import ReactDOM from "react-dom/client";
import { Popover } from "./Popover";
import "./styles.css";

// Suppress the webview's native context menu ("Reload", "Back", …). It exposes
// browser affordances that make no sense in a menubar popover and can navigate
// the window somewhere it can't recover from. Kept in dev, where inspecting the
// page is useful.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (event) => event.preventDefault());
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Popover />
  </React.StrictMode>,
);
