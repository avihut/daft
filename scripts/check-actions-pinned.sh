#!/usr/bin/env bash
# Guard: every `uses:` in .github/workflows/ must reference a full 40-hex
# commit SHA. Tag and branch refs are mutable — whoever controls the upstream
# repo (or takes it over) can move them — and Dependabot keeps SHA pins current
# by itself, which is what makes auto-merging its actions bumps reasonable.
# The repository setting "Require actions to be pinned to a full-length commit
# SHA" refuses to *run* an unpinned workflow; this check fails the PR that
# would introduce one, before it reaches master and silently breaks the next
# release run. `dist generate` rewriting release.yml with tag refs is the
# usual way one appears — `scripts/pin-actions.sh` re-pins.
#
# Local (`./path`) and `docker://` actions are outside the rule.
set -euo pipefail

dir=".github/workflows"
status=0

while IFS= read -r line; do
  file="${line%%:*}"
  rest="${line#*:}"
  lineno="${rest%%:*}"
  spec=$(printf '%s' "${rest#*:}" | sed -E 's/^[[:space:]]*-?[[:space:]]*uses:[[:space:]]*//; s/[[:space:]]*(#.*)?$//')
  case "$spec" in
    ./*|docker://*) continue ;;
  esac
  ref="${spec##*@}"
  if [ "$ref" = "$spec" ]; then
    echo "${file}:${lineno}: '${spec}' has no @ref at all" >&2
    status=1
  elif ! printf '%s' "$ref" | grep -qE '^[0-9a-f]{40}$'; then
    echo "${file}:${lineno}: '${spec}' is not pinned to a 40-hex commit SHA" >&2
    status=1
  fi
done < <(grep -nE '^[[:space:]]*-?[[:space:]]*uses:' "$dir"/*.yml "$dir"/*.yaml 2>/dev/null || true)

if [ "$status" -ne 0 ]; then
  echo >&2
  echo "error: unpinned GitHub Actions in ${dir}. Run scripts/pin-actions.sh to pin" >&2
  echo "them to commit SHAs (the tag stays as a trailing comment for Dependabot)." >&2
  exit 1
fi

echo "ok: every action in ${dir} is pinned to a commit SHA"
