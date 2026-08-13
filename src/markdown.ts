/**
 * Markdown rendering for the PR detail pane.
 *
 * GitHub stores comments and descriptions as markdown, but the app never
 * talks to GitHub's HTML endpoints — so we render with `marked`, configured
 * to approximate GitHub's flavor (line breaks and GFM), then pass the result
 * through DOMPurify. Sanitizing is non-negotiable: the body is attacker-
 * controlled text rendered into the webview, and `dangerouslySetInnerHTML`
 * bypasses React's escaping entirely.
 */

import DOMPurify from "dompurify";
import { marked } from "marked";

marked.setOptions({
  gfm: true,
  breaks: true,
});

// Every link in rendered markdown opens in the system browser, never inside
// the webview: force `target="_blank"` + `rel` in the HTML, and the Markdown
// component in PrDetail intercepts the click to route it through `openUrl`.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

/**
 * Renders raw markdown to sanitized HTML for display. The output is
 * pre-sanitized; the caller must still use `dangerouslySetInnerHTML`.
 */
export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(marked.parse(source, { async: false }) as string);
}
