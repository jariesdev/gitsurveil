/** Independent XSS audit of the markdown pipeline. */
import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";

/** Payloads a hostile PR body or comment could plausibly carry. */
const VECTORS: [string, string][] = [
  ["raw script tag", "<script>alert(1)</script>"],
  ["img onerror", '<img src=x onerror="alert(1)">'],
  ["svg onload", '<svg onload="alert(1)"></svg>'],
  ["iframe", '<iframe src="https://evil.test"></iframe>'],
  ["markdown link js: url", "[click me](javascript:alert(1))"],
  ["raw anchor js: url", '<a href="javascript:alert(1)">x</a>'],

  ["body onload", '<body onload="alert(1)">'],
  ["object tag", '<object data="data:text/html,<script>alert(1)</script>"></object>'],
  ["form action", '<form action="https://evil.test"><input name=a></form>'],

  ["meta refresh", '<meta http-equiv="refresh" content="0;url=https://evil.test">'],
  ["nested encoding", "<scr<script>ipt>alert(1)</scr</script>ipt>"],
  ["case variation", '<ImG SrC=x OnErRoR="alert(1)">'],
];

describe("renderMarkdown sanitization", () => {
  it.each(VECTORS)("neutralizes %s", (_label, payload) => {
    const html = renderMarkdown(payload).toLowerCase();
    expect(html).not.toContain("<script");
    expect(html).not.toContain("onerror");
    expect(html).not.toContain("onload");
    // Only href/src matter: CSS url(javascript:) has not executed in a
    // browser since old IE, so assert on the attributes that do.
    expect(html).not.toMatch(/href\s*=\s*["']?javascript:/);
    expect(html).not.toMatch(/src\s*=\s*["']?javascript:/);
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain("<object");
    expect(html).not.toContain("http-equiv");
  });

  it("still renders legitimate markdown", () => {
    const html = renderMarkdown("**bold** and `code`\n\n- a\n- b");
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<code>code</code>");
    expect(html).toContain("<li>");
  });

  it("keeps safe links intact", () => {
    const html = renderMarkdown("[docs](https://github.com/acme/api)");
    expect(html).toContain('href="https://github.com/acme/api"');
  });

  it("does not throw on empty or odd input", () => {
    expect(() => renderMarkdown("")).not.toThrow();
    expect(() => renderMarkdown("<<<<<<< HEAD")).not.toThrow();
  });
});
