#!/usr/bin/env bash
# Fail if the current branch introduces dep entries (in Cargo.lock or bun.lock)
# whose release date is younger than MIN_AGE_DAYS (default: 7) compared to the
# given base ref (default: origin/master).
#
# Allowlist: .dep-age-allowlist (one per line, format `<ecosystem>:<name>` or
# `<ecosystem>:<name>@<version>`; lines starting with `#` are comments).
# Bypass via env: ALLOW_FRESH_DEPS=1.
set -euo pipefail

base_ref="${1:-${BASE_REF:-origin/master}}"
min_age_days="${MIN_AGE_DAYS:-7}"
min_age_secs=$((min_age_days * 86400))
allowlist_file=".dep-age-allowlist"

if [[ "${ALLOW_FRESH_DEPS:-0}" == "1" ]]; then
  echo "ALLOW_FRESH_DEPS=1 set; skipping lockfile age check." >&2
  exit 0
fi

for cmd in jq curl git perl; do
  command -v "$cmd" >/dev/null || { echo "missing required tool: $cmd" >&2; exit 2; }
done

if ! git rev-parse --verify "$base_ref" >/dev/null 2>&1; then
  echo "base ref not found: $base_ref" >&2
  echo "in CI, check out with fetch-depth: 0 and fetch the base branch first." >&2
  exit 2
fi

now_secs=$(date -u +%s)
violations=()

is_allowlisted() {
  local entry="$1" name="${1#*:}" name_only="${name%@*}" eco="${1%%:*}"
  [[ -f "$allowlist_file" ]] || return 1
  grep -vE '^\s*(#|$)' "$allowlist_file" | grep -Fxq "$entry" && return 0
  grep -vE '^\s*(#|$)' "$allowlist_file" | grep -Fxq "${eco}:${name_only}"
}

extract_cargo_pkgs() {
  # Emit `name<TAB>version` lines from the [[package]] entries of a Cargo.lock
  # blob on stdin. Skips workspace members (entries with no `source` field).
  awk '
    /^\[\[package\]\]/ { in_pkg = 1; name = ""; version = ""; has_src = 0; next }
    in_pkg && /^name = / { gsub(/"/, "", $3); name = $3; next }
    in_pkg && /^version = / { gsub(/"/, "", $3); version = $3; next }
    in_pkg && /^source = / { has_src = 1; next }
    /^$/ {
      if (in_pkg && has_src && name != "" && version != "") print name "\t" version
      in_pkg = 0
    }
    END {
      if (in_pkg && has_src && name != "" && version != "") print name "\t" version
    }
  '
}

extract_bun_pkgs() {
  # bun.lock is JSON5-ish (trailing commas across newlines). Slurp the whole
  # file with perl -0777 to strip them, then parse with jq. Each
  # .packages[<key>] is an array; element 0 is "<name>@<version>".
  perl -0777 -pe 's/,(\s*[}\]])/$1/g' \
    | jq -r '.packages // {} | to_entries[] | .value[0]' \
    | awk -F'@' 'NF >= 2 {
        # rejoin in case of scoped names like @scope/foo@1.2.3
        n = NF; v = $n; n_name = ""
        for (i = 1; i < n; i++) n_name = n_name (i > 1 ? "@" : "") $i
        print n_name "\t" v
      }'
}

diff_added() {
  # Print added/changed `name<TAB>version` lines (after - before).
  local before="$1" after="$2"
  comm -13 <(printf '%s\n' "$before" | sort -u) \
           <(printf '%s\n' "$after" | sort -u)
}

check_crates_io() {
  local name="$1" version="$2"
  local url="https://crates.io/api/v1/crates/${name}/${version}"
  curl -fsSL -A "daft-dep-age-check" "$url" | jq -r '.version.created_at // empty'
}

check_npm() {
  local name="$1" version="$2"
  # https://registry.npmjs.org/<name> returns time map under .time
  local url="https://registry.npmjs.org/${name}"
  curl -fsSL "$url" | jq -r --arg v "$version" '.time[$v] // empty'
}

age_secs() {
  # macOS and GNU date both accept ISO-8601 with -d/-jf via different flags.
  local iso="$1" published_secs
  if date -u -d "$iso" +%s >/dev/null 2>&1; then
    published_secs=$(date -u -d "$iso" +%s)
  else
    published_secs=$(date -u -jf "%Y-%m-%dT%H:%M:%S" "${iso%.*}" +%s 2>/dev/null \
      || date -u -jf "%Y-%m-%dT%H:%M:%SZ" "${iso%.*}Z" +%s)
  fi
  echo $((now_secs - published_secs))
}

check_lockfile() {
  local eco="$1" path="$2" extractor="$3" registry_check="$4"
  [[ -f "$path" ]] || return 0

  local before after added
  before=$(git show "${base_ref}:${path}" 2>/dev/null | "$extractor" || true)
  after=$(<"$path" "$extractor")
  added=$(diff_added "$before" "$after" || true)
  [[ -z "$added" ]] && return 0

  echo "Checking ${#} new/changed entries in ${path}..." >&2
  while IFS=$'\t' read -r name version; do
    [[ -z "$name" || -z "$version" ]] && continue
    local key="${eco}:${name}@${version}"
    if is_allowlisted "$key"; then
      echo "  allow: $key" >&2
      continue
    fi
    local published age days
    published=$("$registry_check" "$name" "$version" 2>/dev/null || true)
    if [[ -z "$published" ]]; then
      echo "  skip:  $key (no pubtime from registry)" >&2
      continue
    fi
    age=$(age_secs "$published")
    days=$((age / 86400))
    if (( age < min_age_secs )); then
      violations+=("$key  age=${days}d  published=${published}")
      echo "  FRESH: $key (age ${days}d, threshold ${min_age_days}d)" >&2
    else
      echo "  ok:    $key (age ${days}d)" >&2
    fi
  done <<< "$added"
}

check_lockfile cargo Cargo.lock extract_cargo_pkgs check_crates_io
check_lockfile npm bun.lock extract_bun_pkgs check_npm
check_lockfile npm docs/bun.lock extract_bun_pkgs check_npm

if (( ${#violations[@]} > 0 )); then
  echo >&2
  echo "Lockfile-age check failed: ${#violations[@]} entry(ies) younger than ${min_age_days} days." >&2
  printf '  - %s\n' "${violations[@]}" >&2
  echo >&2
  echo "Options:" >&2
  echo "  1. Wait for the package to age past the threshold." >&2
  echo "  2. Add an entry to ${allowlist_file} with rationale (e.g. security fix)." >&2
  echo "  3. Set ALLOW_FRESH_DEPS=1 in the environment for an emergency override." >&2
  exit 1
fi

echo "Lockfile-age check passed." >&2
