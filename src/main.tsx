/** Entry point for the popover webview. */

import React from "react";
import ReactDOM from "react-dom/client";
import { Popover } from "./Popover";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Popover />
  </React.StrictMode>,
);
