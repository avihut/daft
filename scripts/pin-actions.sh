#!/usr/bin/env bash
# Pin every `uses: owner/repo@<tag>` in .github/workflows/*.yml to the full
# commit SHA the tag resolves to, keeping the tag as a trailing comment:
#
#   uses: actions/checkout@v7.0.1
#     -> uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
#
# A floating major tag (`@v2`) is pinned to its current commit and the comment
# names the most specific tag at that commit (`# v2.8.1`), which is the form
# Dependabot reads to keep the pin and the comment moving together.
#
# Refs that already are 40-hex SHAs are left alone. Refs that resolve only to a
# *branch* are refused: a branch pin needs a human to decide what it means
# (dtolnay/rust-toolchain, for instance, is pinned to its `v1` tag with an
# explicit `toolchain:` input because its `stable`/`1.xx` refs are branches
# that name the toolchain, not versions of the action).
#
# Pairs with scripts/check-actions-pinned.sh, which fails CI when a workflow
# carries an unpinned action — the typical way one sneaks in is `dist generate`
# rewriting release.yml with tag refs; re-run this script to re-pin.
#
# Usage: scripts/pin-actions.sh [workflow.yml ...]   (default: all workflows)
set -euo pipefail

files=("$@")
if [ ${#files[@]} -eq 0 ]; then
  files=(.github/workflows/*.yml)
fi

for cmd in git sed grep sort; do
  command -v "$cmd" >/dev/null || { echo "missing required tool: $cmd" >&2; exit 2; }
done

# Collect the distinct unpinned `owner/repo@ref` pairs across the given files.
# `uses:` lines for local (`./`) and docker:// actions are not pinnable here.
refs=$(grep -hoE '^[[:space:]]*-?[[:space:]]*uses:[[:space:]]*[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[A-Za-z0-9_./-]+)?@[^[:space:]#]+' "${files[@]}" \
  | sed -E 's/^[[:space:]]*-?[[:space:]]*uses:[[:space:]]*//' \
  | grep -vE '@[0-9a-f]{40}$' \
  | sort -u || true)

if [ -z "$refs" ]; then
  echo "ok: every action in ${files[*]} is already pinned to a commit SHA"
  exit 0
fi

status=0
while IFS= read -r spec; do
  [ -n "$spec" ] || continue
  action="${spec%@*}"   # owner/repo or owner/repo/path
  ref="${spec##*@}"
  repo=$(printf '%s' "$action" | cut -d/ -f1-2)

  # One ls-remote per repo: tags (peeled where annotated) and heads.
  remote=$(git ls-remote --tags --heads "https://github.com/${repo}" 2>/dev/null || true)
  if [ -z "$remote" ]; then
    echo "error: could not list refs for ${repo}" >&2
    status=1
    continue
  fi

  # Commit for this ref. Prefer the peeled `^{}` line (annotated tag), then the
  # plain tag, then a branch head.
  sha=$(printf '%s\n' "$remote" | awk -v r="refs/tags/${ref}^{}" '$2 == r { print $1; exit }')
  if [ -z "$sha" ]; then
    sha=$(printf '%s\n' "$remote" | awk -v r="refs/tags/${ref}" '$2 == r { print $1; exit }')
  fi
  if [ -z "$sha" ]; then
    head=$(printf '%s\n' "$remote" | awk -v r="refs/heads/${ref}" '$2 == r { print $1; exit }')
    if [ -n "$head" ]; then
      echo "refusing: ${spec} — '${ref}' is a branch of ${repo}, not a tag; pin it by hand" >&2
    else
      echo "error: ${spec} — no tag or branch '${ref}' in ${repo}" >&2
    fi
    status=1
    continue
  fi

  # Most specific tag at that commit for the comment: among all tags pointing
  # at `sha`, pick the one with the most dot-separated components (then the
  # longest). A bare `v2` becomes `v2.8.1`; a full `v7.0.1` stays itself.
  comment=$(printf '%s\n' "$remote" \
    | awk -v s="$sha" '$1 == s && $2 ~ /^refs\/tags\// { t=$2; sub(/^refs\/tags\//, "", t); sub(/\^\{\}$/, "", t); print t }' \
    | sort -u \
    | awk '{ n = gsub(/\./, ".", $0); print n, length($0), $0 }' \
    | sort -k1,1nr -k2,2nr \
    | head -1 | awk '{ print $3 }')
  [ -n "$comment" ] || comment="$ref"

  for f in "${files[@]}"; do
    # Replace `@ref` only when followed by end-of-line, whitespace, or a
    # comment, so `@v2` never rewrites `@v2.8.1`.
    sed -i.bak -E "s|(uses:[[:space:]]*${action}@)${ref}([[:space:]]*(#.*)?)?\$|\1${sha} # ${comment}|" "$f"
    rm -f "$f.bak"
  done
  echo "pinned: ${spec} -> ${sha} # ${comment}"
done <<< "$refs"

exit $status
