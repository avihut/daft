#!/usr/bin/env bash
# Downloads and caches real repositories for realistic benchmarks.
# Repos are cached in benches/fixtures/cache/ and reused across runs.

REAL_REPO_CACHE="${BENCH_DIR}/fixtures/cache"

declare -A REAL_REPOS=(
    [git]="https://github.com/git/git.git"
    [daft]="https://github.com/avihut/daft.git"
)

# Ensure a real repo is available as a bare clone.
# Usage: ensure_real_repo <name>
# Prints the path to the cached bare repo.
ensure_real_repo() {
    local name="$1"
    local url="${REAL_REPOS[$name]:-}"
    if [[ -z "$url" ]]; then
        log_error "Unknown real repo: $name. Known: ${!REAL_REPOS[*]}"
        return 1
    fi

    local cache_path="$REAL_REPO_CACHE/${name}.git"
    mkdir -p "$REAL_REPO_CACHE"

    if [[ -d "$cache_path" ]]; then
        log "Using cached repo: $name"
        git -C "$cache_path" fetch --all --quiet 2>/dev/null || log_warn "Could not refresh $name (offline?)"
    else
        log "Cloning real repo: $name (this may take a while)..."
        git clone --bare "$url" "$cache_path"
        log_success "Cached: $cache_path"
    fi

    echo "$cache_path"
}
