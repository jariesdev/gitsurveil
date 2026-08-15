/**
 * The add-account form, shared by the Accounts view and the first-run
 * onboarding screen (`specs/desktop-ui.md` § Accounts, § Onboarding).
 *
 * PAT entry only for now — OAuth device flow is a later addition. The token
 * goes straight to the Rust side, which validates it against GitHub before
 * storing it in the OS keychain; it is never held in component state longer
 * than the submit, and never sent anywhere else. The provider is picked from
 * the supported list (GitHub Cloud or GitHub Enterprise Server) instead of a
 * free-form host, so the common case is one click and the Enterprise fields
 * only appear when they're actually needed. A collapsible helper explains
 * where a token comes from and which scopes are required, with a direct link
 * to the GitHub token page.
 */

import { useState } from "react";
import { addAccount, openUrl } from "../ipc";

const PROVIDERS = [
  { id: "github", label: "GitHub" },
  { id: "enterprise", label: "GitHub Enterprise Server" },
] as const;

type Provider = (typeof PROVIDERS)[number]["id"];

/** Validates the token against GitHub and registers the account. */
export function AccountForm({ onAdded }: { onAdded: () => void }) {
  const [provider, setProvider] = useState<Provider>("github");
  const [host, setHost] = useState("");
  const [token, setToken] = useState("");
  const [apiBase, setApiBase] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isEnterprise = provider === "enterprise";

  async function handleAdd(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const resolvedHost = isEnterprise ? host.trim() : "github.com";
      await addAccount(resolvedHost, token.trim(), apiBase.trim() || undefined);
      setToken("");
      onAdded();
    } catch (e) {
      // The daemon's validation error is the useful message here ("bad
      // credentials", "missing scope"), so surface it rather than a generic one.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleAdd} className="space-y-3">
      <Field label="Provider">
        <select
          value={provider}
          onChange={(e) => setProvider(e.target.value as Provider)}
          className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
        >
          {PROVIDERS.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
      </Field>

      {isEnterprise && (
        <>
          <Field label="Enterprise host">
            <input
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="github.example.com"
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          </Field>

          <Field label="API base URL">
            <input
              value={apiBase}
              onChange={(e) => setApiBase(e.target.value)}
              placeholder={`https://${host.trim() || "github.example.com"}/api/v3`}
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          </Field>
        </>
      )}

      <Field label="Personal access token">
        <input
          type="password"
          value={token}
          onChange={(e) => setToken(e.target.value)}
          placeholder="ghp_…"
          autoComplete="off"
          className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
        />
        <TokenHelp />
      </Field>

      {error && (
        <p role="alert" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}

      <button
        type="submit"
        disabled={busy || !token.trim() || (isEnterprise && !host.trim())}
        className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
      >
        {busy ? "Validating…" : "Add account"}
      </button>
    </form>
  );
}

/**
 * Collapsible guidance for the token field: where a PAT comes from, which
 * scopes are required, and a direct link to the GitHub token page. The token
 * is validated against GitHub before it is stored, then kept in the OS
 * keychain only.
 */
function TokenHelp() {
  return (
    <details className="mt-1">
      <summary className="cursor-pointer text-[11px] text-neutral-500">
        Where do I get a token?
      </summary>
      <ol className="mt-1.5 list-decimal space-y-1 pl-4 text-[11px] text-neutral-500">
        <li>
          Create a classic personal access token at{" "}
          <code>github.com/settings/tokens</code>, or a fine-grained token
          granted to the repos you care about.
        </li>
        <li>
          A classic token needs the <code>notifications</code> and{" "}
          <code>repo</code> scopes.
        </li>
        <li>
          The token is validated against GitHub before it’s stored, then kept
          in your OS keychain — never on disk.
        </li>
      </ol>
      <button
        type="button"
        onClick={() => void openUrl("https://github.com/settings/tokens")}
        className="mt-1.5 text-[11px] text-neutral-600 underline-offset-2 hover:underline dark:text-neutral-300"
      >
        Create a token on GitHub
      </button>
    </details>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs text-neutral-600 dark:text-neutral-400">
        {label}
      </span>
      {children}
    </label>
  );
}
