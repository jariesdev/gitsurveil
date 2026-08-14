/**
 * Priority rules, read-only (`specs/priority-engine.md`).
 *
 * Editing writes through a `rules.set` API method that lands with config
 * hot-reloading; until then this explains *why* items score the way they do,
 * which is the part users actually need first. The config file path is shown
 * so the rules can be edited by hand in the meantime.
 */

import { KIND_LABELS, SEVERITY_LABELS, type Rule } from "../types";

/** Base score per kind, mirroring `crates/gitsurveild/src/priority.rs`. */
const BASE_SCORES: [string, number][] = [
  ["CI failed", 100],
  ["Review requested", 80],
  ["Changes requested", 70],
  ["Mentioned", 50],
  ["Assigned", 40],
  ["Participating", 20],
];

export function Rules({ rules }: { rules: Rule[] }) {
  return (
    <div className="mx-auto max-w-2xl p-6">
      <h2 className="text-base font-semibold">Priority rules</h2>
      <p className="mt-1 text-sm text-neutral-500">
        Each item scores its base value for its type, plus any matching rules,
        plus one point per four hours it stays open (up to 30). You are only
        interrupted when something outranks what was already at the top — and
        a failing build always interrupts.
      </p>

      <h3 className="mt-6 text-sm font-medium">Base scores</h3>
      <ul className="mt-2 divide-y divide-neutral-200 text-sm dark:divide-neutral-800">
        {BASE_SCORES.map(([label, score]) => (
          <li key={label} className="flex justify-between py-1.5">
            <span>{label}</span>
            <span className="tabular-nums text-neutral-500">{score}</span>
          </li>
        ))}
      </ul>

      <h3 className="mt-6 text-sm font-medium">
        Your rules{" "}
        <span className="font-normal text-neutral-500">
          ({rules.length})
        </span>
      </h3>
      {rules.length === 0 ? (
        <p className="mt-2 text-sm text-neutral-500">No rules configured.</p>
      ) : (
        <ul className="mt-2 space-y-2">
          {rules.map((rule) => (
            <li
              key={rule.id}
              className="rounded border border-neutral-200 p-3 text-sm dark:border-neutral-800"
            >
              <div className="flex items-center justify-between">
                <code className="text-xs">{rule.id}</code>
                {!rule.enabled && (
                  <span className="text-[11px] text-neutral-500">disabled</span>
                )}
              </div>
              <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-400">
                {describeRule(rule)}
              </p>
            </li>
          ))}
        </ul>
      )}

      <p className="mt-6 text-[11px] text-neutral-500">
        Rules live in <code>config.toml</code> in the GitSurveil data directory.
        A graphical editor is coming; edits to the file take effect on restart.
      </p>
    </div>
  );
}

/** Renders a rule as a sentence, so it reads without knowing the schema. */
export function describeRule(rule: Rule): string {
  const conditions: string[] = [];
  if (rule.match.kind?.length) {
    conditions.push(
      `type is ${rule.match.kind.map((k) => KIND_LABELS[k]).join(" or ")}`,
    );
  }
  if (rule.match.repo) conditions.push(`repository matches ${rule.match.repo}`);
  if (rule.match.author?.length) {
    conditions.push(`author is ${rule.match.author.join(" or ")}`);
  }

  const effects: string[] = [];
  if (typeof rule.effect.add === "number") {
    const add = rule.effect.add;
    effects.push(`${add >= 0 ? "add" : "subtract"} ${Math.abs(add)}`);
  }
  if (rule.effect.pin_severity) {
    effects.push(`pin severity to ${SEVERITY_LABELS[rule.effect.pin_severity]}`);
  }
  if (rule.effect.mute_notifications) effects.push("mute notifications");

  const when = conditions.length ? `When ${conditions.join(" and ")}` : "Always";
  const then = effects.length ? effects.join(", ") : "do nothing";
  return `${when}, ${then}.`;
}
