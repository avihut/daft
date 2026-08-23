#!/usr/bin/env bash
# Compare the live rulesets of the GitHub repository with the intent committed
# under .github/rulesets/*.json. Exits non-zero on drift, with a diff. Read-only;
# needs `gh` authenticated with read access to the repository (rulesets are
# visible to anyone who can read the repo). Not a CI job — a maintainer's tool
# for confirming that what the repository says about its own policy is true.
set -euo pipefail

repo="${1:-${GH_REPO:-avihut/daft}}"
dir=".github/rulesets"

for cmd in gh jq diff; do
  command -v "$cmd" >/dev/null || { echo "missing required tool: $cmd" >&2; exit 2; }
done

# Project both sides onto the fields the API accepts on write, in a canonical
# key and element order (GitHub stores rules in its own order), so incidental
# read-only fields (ids, timestamps, links) and serialization differences do
# not read as drift.
normalize() {
  jq -S '{
    name, target, enforcement,
    conditions,
    rules: ([ .rules[] | { type } + (if .parameters then { parameters } else {} end) ] | sort_by(.type)),
    bypass_actors: ([ (.bypass_actors // [])[] | { actor_id, actor_type, bypass_mode } ] | sort_by(.actor_type, .actor_id))
  }
  | .rules |= map(if .parameters and .parameters.required_reviewers == [] then del(.parameters.required_reviewers) else . end)'
}

live_index=$(gh api "repos/${repo}/rulesets" --paginate)
status=0
for f in "$dir"/*.json; do
  name=$(jq -r .name "$f")
  id=$(printf '%s' "$live_index" | jq -r --arg n "$name" '.[] | select(.name == $n) | .id' | head -1)
  if [ -z "$id" ]; then
    echo "DRIFT: no live ruleset named '${name}' (${f})" >&2
    status=1
    continue
  fi
  if diff -u <(normalize < "$f") <(gh api "repos/${repo}/rulesets/${id}" | normalize) >/tmp/ruleset-diff.$$ 2>&1; then
    echo "ok: ruleset '${name}' (id ${id}) matches ${f}"
  else
    echo "DRIFT: ruleset '${name}' (id ${id}) differs from ${f}:" >&2
    sed 's/^/    /' /tmp/ruleset-diff.$$ >&2
    status=1
  fi
  rm -f /tmp/ruleset-diff.$$
done
exit $status
