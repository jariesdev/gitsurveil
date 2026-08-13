#!/usr/bin/env bash
#
# Live verification against real GitHub.
#
# Closes the standing gap that no pull-request operation has ever run against
# the real API: every wire shape in crates/gitsurveild/src/github/pr.rs was
# written from GitHub's documentation, not from an observed response. The
# conflict resolver is built on top of pr.detail's mergeability mapping, so a
# mistake there is invisible until something is actually merged.
#
# A mock server cannot close this gap — it would only confirm our assumptions
# against themselves.
#
# Usage:
#   GITSURVEIL_TOKEN=ghp_... ./scripts/verify-live.sh [owner/scratch-repo]
#
# The token needs `repo` and `notifications` scopes. If no repo is given, one
# named gitsurveil-verify is created under the authenticated user and reused
# on later runs.
#
# This script CREATES a repository, branches, and a pull request on the
# authenticated account, and pushes commits that deliberately conflict. Point
# it at a scratch account or accept the noise on your own.

set -euo pipefail

TOKEN="${GITSURVEIL_TOKEN:-}"
if [[ -z "$TOKEN" ]]; then
  echo "error: set GITSURVEIL_TOKEN to a personal access token" >&2
  exit 2
fi

API="https://api.github.com"
WORK="${TMPDIR:-/tmp}/gitsurveil-verify"
SOCK_MAC="$HOME/Library/Application Support/io.gitsurveil.gitsurveil/daemon.sock"
SOCK="${XDG_RUNTIME_DIR:-}/gitsurveil.sock"
[[ -S "$SOCK_MAC" ]] && SOCK="$SOCK_MAC"

pass=0; fail=0
ok()   { printf "  \033[32mPASS\033[0m %s\n" "$1"; pass=$((pass+1)); }
bad()  { printf "  \033[31mFAIL\033[0m %s\n" "$1"; fail=$((fail+1)); }
note() { printf "  ---- %s\n" "$1"; }

api() { # api METHOD PATH [BODY]
  local method="$1" path="$2" body="${3:-}"
  if [[ -n "$body" ]]; then
    curl -sS -X "$method" -H "Authorization: Bearer $TOKEN" \
      -H "Accept: application/vnd.github+json" -d "$body" "$API$path"
  else
    curl -sS -X "$method" -H "Authorization: Bearer $TOKEN" \
      -H "Accept: application/vnd.github+json" "$API$path"
  fi
}

rpc() { # rpc METHOD PARAMS_JSON
  printf '{"id":1,"method":"%s","params":%s}\n' "$1" "$2" | nc -U "$SOCK"
}

# ---- 0. preconditions ----------------------------------------------------

echo "== preconditions =="
LOGIN=$(api GET /user | python3 -c 'import json,sys; print(json.load(sys.stdin).get("login",""))')
[[ -n "$LOGIN" ]] && ok "token valid (authenticated as $LOGIN)" || { bad "token rejected"; exit 1; }
[[ -S "$SOCK" ]] && ok "daemon socket present" || { bad "daemon not running — start gitsurveild --foreground"; exit 1; }

REPO="${1:-$LOGIN/gitsurveil-verify}"
OWNER="${REPO%%/*}"; NAME="${REPO##*/}"
note "scratch repo: $REPO"

# ---- 1. scratch repo with a real conflict --------------------------------

echo "== scratch repo =="
if api GET "/repos/$REPO" | grep -q '"full_name"'; then
  ok "repo exists"
else
  api POST /user/repos "{\"name\":\"$NAME\",\"private\":true,\"auto_init\":true}" >/dev/null
  ok "repo created (private)"
  sleep 2
fi

rm -rf "$WORK"; mkdir -p "$WORK"
git clone -q "https://$TOKEN@github.com/$REPO.git" "$WORK/clone"
cd "$WORK/clone"
git config user.email "verify@example.com"; git config user.name "gitsurveil verify"

STAMP=$(date +%s)
BRANCH="verify/conflict-$STAMP"

# Both branches edit the same line, which is what produces the conflict.
printf 'line one\nshared line\nline three\n' > conflict.txt
git add -A && git commit -qm "verify: baseline $STAMP"
git push -q origin HEAD:main
BASE_SHA=$(git rev-parse HEAD)

git checkout -qb "$BRANCH"
printf 'line one\nBRANCH EDIT %s\nline three\n' "$STAMP" > conflict.txt
git commit -qam "verify: branch edit"
git push -q origin "$BRANCH"

git checkout -q main
printf 'line one\nMAIN EDIT %s\nline three\n' "$STAMP" > conflict.txt
git commit -qam "verify: main edit"
git push -q origin main
ok "conflicting branches pushed"

PR_NUM=$(api POST "/repos/$REPO/pulls" \
  "{\"title\":\"verify conflict $STAMP\",\"head\":\"$BRANCH\",\"base\":\"main\",\"body\":\"Automated verification. Safe to close.\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin).get("number",""))')
[[ -n "$PR_NUM" ]] && ok "pull request #$PR_NUM opened" || { bad "could not open PR"; exit 1; }

note "waiting for GitHub to compute mergeability (it is asynchronous)"
sleep 8

# ---- 2. the actual gap: our wire shapes vs real responses ----------------

echo "== daemon against real GitHub =="

DETAIL=$(rpc pr.detail "{\"repo\":\"$REPO\",\"number\":$PR_NUM}")
if echo "$DETAIL" | grep -q '"error"'; then
  bad "pr.detail: $(echo "$DETAIL" | head -c 200)"
else
  ok "pr.detail decodes a real response"
  MERGEABLE=$(echo "$DETAIL" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["mergeability"])')
  note "mergeability = $MERGEABLE"
  # THE critical assertion: the conflict resolver keys off this field.
  [[ "$MERGEABLE" == "conflicted" ]] \
    && ok "mergeability mapping is correct on a genuinely conflicted PR" \
    || bad "expected 'conflicted', got '$MERGEABLE' — the conflict resolver's entry point is wrong"
fi

for m in pr.comments pr.branches; do
  R=$(rpc "$m" "{\"repo\":\"$REPO\",\"number\":$PR_NUM}")
  echo "$R" | grep -q '"error"' && bad "$m: $(echo "$R" | head -c 160)" || ok "$m decodes"
done

R=$(rpc pr.comment "{\"repo\":\"$REPO\",\"number\":$PR_NUM,\"body\":\"verification comment\"}")
echo "$R" | grep -q '"error"' && bad "pr.comment: $(echo "$R" | head -c 160)" || ok "pr.comment posts"

R=$(rpc prs.list '{"state":"open"}')
if echo "$R" | grep -q '"error"'; then
  bad "prs.list: $(echo "$R" | head -c 200)"
else
  ok "prs.list decodes a real response"
  echo "$R" | python3 -c '
import json,sys
rows = json.load(sys.stdin)["result"]
print(f"  ---- {len(rows)} open PRs; roles seen: " +
      ", ".join(sorted({r for row in rows for r in row["roles"]})))
dupes = [k for k in {} ]
seen = {}
for row in rows:
    key = (row["repo"], row["number"])
    seen[key] = seen.get(key, 0) + 1
dupes = [k for k, v in seen.items() if v > 1]
print("  \033[31mFAIL\033[0m duplicate rows: " + str(dupes) if dupes
      else "  \033[32mPASS\033[0m one row per pull request")
'
fi

# ---- 3. conflict resolver end to end -------------------------------------

echo "== conflict resolver =="
note "requires a configured clone: repos.set $REPO -> $WORK/clone"
R=$(rpc repos.set "{\"repo\":\"$REPO\",\"path\":\"$WORK/clone\"}")
echo "$R" | grep -q '"error"' && bad "repos.set: $(echo "$R" | head -c 160)" || ok "repos.set accepts the clone"

R=$(rpc conflicts.prepare "{\"repo\":\"$REPO\",\"number\":$PR_NUM}")
if echo "$R" | grep -q '"error"'; then
  bad "conflicts.prepare: $(echo "$R" | head -c 200)"
else
  ok "conflicts.prepare creates a session"
  echo "$R" | python3 -c '
import json,sys
s = json.load(sys.stdin)["result"]
print(f"  ---- worktree: {s[\"worktree_path\"]}")
print(f"  ---- conflicted files: {[f[\"path\"] for f in s[\"files\"]]}")
'
  # The safety property that matters most: the user's clone is untouched.
  cd "$WORK/clone"
  DIRTY=$(git status --porcelain)
  [[ -z "$DIRTY" ]] && ok "user's clone still clean during a session" || bad "clone was modified: $DIRTY"

  rpc conflicts.abort "{\"session_id\":\"$REPO\"}" >/dev/null
  LEFT=$(git worktree list --porcelain | grep -c gitsurveil || true)
  [[ "$LEFT" == "0" ]] && ok "abort leaves no worktree behind" || bad "$LEFT worktree(s) survived abort"
fi

# ---- summary --------------------------------------------------------------

echo
echo "== summary =="
printf "  %d passed, %d failed\n" "$pass" "$fail"
echo
echo "  PR left open for manual UI checks: https://github.com/$REPO/pull/$PR_NUM"
echo "  Clone at: $WORK/clone"
echo "  Clean up when done: close the PR, and rm -rf $WORK"
[[ "$fail" -eq 0 ]] || exit 1
