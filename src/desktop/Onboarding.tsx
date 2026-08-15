/**
 * First-run orientation for a brand-new user (`specs/desktop-ui.md` §
 * Onboarding). Shown in place of the normal window shell while no account is
 * configured: a short pitch of what GitSurveil does with the add-account form
 * front and center.
 *
 * "Skip for now" hides it for the session only — it returns on the next
 * window open until at least one account exists, because a user who never
 * added an account still needs this. Once an account is added the App data
 * reloads and this screen stops rendering, landing the window on the
 * Dashboard.
 */

import { AccountForm } from "./AccountForm";

const PITCH = [
  "Review requests, assignments, mentions, and failing CI land in one prioritized list.",
  "It runs entirely on your machine; nothing is hosted, and your token never leaves your OS keychain.",
  "You’re only interrupted when something outranks what was already at the top of your list.",
] as const;

export function Onboarding({
  onAdded,
  onSkip,
}: {
  onAdded: () => void;
  onSkip: () => void;
}) {
  return (
    <div className="flex h-screen overflow-hidden bg-white text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <div className="m-auto w-full max-w-xl p-8">
        <h1 className="text-xl font-semibold">Welcome to GitSurveil</h1>
        <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-400">
          GitSurveil watches GitHub in the background and tells you only what
          actually needs your attention.
        </p>

        <ul className="mt-4 space-y-2 text-sm text-neutral-600 dark:text-neutral-400">
          {PITCH.map((point) => (
            <li key={point} className="flex gap-2">
              <span aria-hidden="true" className="text-neutral-400">
                •
              </span>
              {point}
            </li>
          ))}
        </ul>

        <div className="mt-6 rounded border border-neutral-200 p-4 dark:border-neutral-800">
          <h2 className="text-sm font-medium">Add your GitHub account</h2>
          <p className="mt-1 text-xs text-neutral-500">
            Paste a personal access token to start monitoring. You can add
            more accounts — including Enterprise — later.
          </p>
          <div className="mt-3">
            <AccountForm onAdded={onAdded} />
          </div>
        </div>

        <button
          type="button"
          onClick={onSkip}
          className="mt-4 text-xs text-neutral-500 underline-offset-2 hover:underline"
        >
          Skip for now
        </button>
      </div>
    </div>
  );
}
