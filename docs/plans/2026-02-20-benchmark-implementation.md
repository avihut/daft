# Benchmark Suite Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Build a hyperfine-based benchmark suite that compares every daft
command against equivalent git scripting (with maximum parallelism on the git
side), tracks results over time, and publishes them to the docs site.

**Architecture:** Shell-native suite in `benches/` mirroring the existing
`tests/integration/` patterns. Shared framework script, fixture generators,
per-command scenario scripts, and a runner that aggregates JSON into markdown.
Mise tasks expose individual and aggregate runs. A GitHub Actions workflow
commits results on each master push.

**Tech Stack:** hyperfine, bash, mise tasks, GitHub Actions, VitePress

---

### Task 1: Delete test:perf

**Files:**

- Delete: `mise-tasks/test/perf`

**Step 1: Delete the file**

```bash
rm mise-tasks/test/perf
```

**Step 2: Verify it's gone**

Run: `mise task | grep perf` Expected: no output

**Step 3: Commit**

```bash
git add mise-tasks/test/perf
git commit -m "chore: remove test:perf (replaced by proper benchmark suite)"
```

---

### Task 2: Create bench framework

**Files:**

- Create: `benches/bench_framework.sh`

**Step 1: Write the shared framework**

```bash
#!/usr/bin/env bash
# Shared framework for daft benchmark suite.
# Source this from scenario scripts.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="$BENCH_DIR/results"
HISTORY_DIR="$BENCH_DIR/history"
TEMP_BASE="/tmp/daft-bench"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log()         { echo -e "${BLUE}[bench]${NC} $*"; }
log_success() { echo -e "${GREEN}[+]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $*"; }
log_error()   { echo -e "${RED}[-]${NC} $*"; }

require_hyperfine() {
    if ! command -v hyperfine >/dev/null 2>&1; then
        log_error "hyperfine not found. Install: brew install hyperfine"
        exit 1
    fi
}

require_daft() {
    if ! command -v git-worktree-clone >/dev/null 2>&1; then
        log_error "daft not in PATH. Run: mise run dev"
        exit 1
    fi
}

setup_bench_env() {
    require_hyperfine
    require_daft
    mkdir -p "$RESULTS_DIR" "$HISTORY_DIR" "$TEMP_BASE"

    export GIT_AUTHOR_NAME="Bench User"
    export GIT_AUTHOR_EMAIL="bench@example.com"
    export GIT_COMMITTER_NAME="Bench User"
    export GIT_COMMITTER_EMAIL="bench@example.com"

    # Isolated git config — never touch global
    export GIT_CONFIG_GLOBAL="$TEMP_BASE/.gitconfig"
    touch "$GIT_CONFIG_GLOBAL"
}

cleanup_bench() {
    rm -rf "$TEMP_BASE"
}

# Run a hyperfine comparison.
# Usage: bench_compare <name> <prepare_cmd> <daft_cmd> <git_cmd> [extra hyperfine flags...]
bench_compare() {
    local name="$1"
    local prepare_cmd="$2"
    local daft_cmd="$3"
    local git_cmd="$4"
    shift 4

    local json_out="$RESULTS_DIR/${name}.json"
    local md_out="$RESULTS_DIR/${name}.md"

    log "Running: $name"

    local prepare_args=()
    if [[ -n "$prepare_cmd" ]]; then
        prepare_args=(--prepare "$prepare_cmd")
    fi

    hyperfine \
        --warmup 3 \
        --min-runs 10 \
        "${prepare_args[@]}" \
        --export-json "$json_out" \
        --export-markdown "$md_out" \
        "$@" \
        --command-name "daft" "$daft_cmd" \
        --command-name "git" "$git_cmd"

    log_success "Saved: $json_out"
}

trap cleanup_bench EXIT
```

**Step 2: Make executable**

```bash
chmod +x benches/bench_framework.sh
```

**Step 3: Verify it sources cleanly**

Run: `bash -c 'source benches/bench_framework.sh && echo OK'` Expected: `OK`

**Step 4: Commit**

```bash
git add benches/bench_framework.sh
git commit -m "chore(bench): add shared benchmark framework"
```

---

### Task 3: Create synthetic repo fixture generator

**Files:**

- Create: `benches/fixtures/create_repo.sh`

**Step 1: Write the fixture generator**

```bash
#!/usr/bin/env bash
# Creates synthetic git repos of configurable sizes for benchmarking.
# Source this file, then call create_bare_repo or create_bare_repo_with_hooks.
#
# Sizes:
#   small:  100 files, 50 commits, 2 branches
#   medium: 1000 files, 500 commits, 5 branches
#   large:  10000 files, 2000 commits, 10 branches

create_bare_repo() {
    local dest="$1"
    local size="${2:-small}"

    local num_files num_commits num_branches
    case "$size" in
        small)  num_files=100;   num_commits=50;   num_branches=2 ;;
        medium) num_files=1000;  num_commits=500;  num_branches=5 ;;
        large)  num_files=10000; num_commits=2000; num_branches=10 ;;
        *) echo "Unknown size: $size"; return 1 ;;
    esac

    local work="$TEMP_BASE/create-repo-$$-$RANDOM"
    mkdir -p "$work"
    git -C "$work" init -b main >/dev/null 2>&1

    # Create initial file tree
    for i in $(seq 1 "$num_files"); do
        local subdir="$work/dir-$((i % 20))"
        mkdir -p "$subdir"
        printf 'file %d\nsome content line\n' "$i" > "$subdir/file-${i}.txt"
    done

    (
        cd "$work"
        git add -A
        git commit -m "Initial commit: $num_files files" >/dev/null

        for i in $(seq 2 "$num_commits"); do
            local target="dir-$((RANDOM % 20))/file-$((RANDOM % num_files + 1)).txt"
            printf 'update %d\n' "$i" >> "$target" 2>/dev/null \
                || printf 'update %d\n' "$i" > "dir-0/extra-${i}.txt"
            git add -A
            git commit -m "Commit $i" >/dev/null
        done

        for b in $(seq 1 "$num_branches"); do
            git checkout -b "feature/branch-$b" >/dev/null 2>&1
            printf 'branch %d\n' "$b" > "branch-${b}.txt"
            git add -A
            git commit -m "Branch $b" >/dev/null
            git checkout main >/dev/null 2>&1
        done
    )

    git clone --bare "$work" "$dest" >/dev/null 2>&1
    rm -rf "$work"
}

# Create a bare repo that has a daft post-clone hook.
create_bare_repo_with_hooks() {
    local dest="$1"
    local size="${2:-small}"

    create_bare_repo "$dest" "$size"

    local inject="$TEMP_BASE/inject-hooks-$$-$RANDOM"
    git clone "$dest" "$inject" >/dev/null 2>&1
    (
        cd "$inject"
        mkdir -p .daft/hooks

        cat > .daft/hooks/post-clone <<'HOOK'
#!/usr/bin/env bash
# Simulates typical post-clone setup
echo "export PROJECT_ROOT=$(pwd)" > .envrc
touch .tool-versions
# Simulate moderate setup workload
sleep 0.05
HOOK
        chmod +x .daft/hooks/post-clone

        cat > .daft/hooks/worktree-post-create <<'HOOK'
#!/usr/bin/env bash
# Simulates per-worktree setup
echo "export WORKTREE=$(pwd)" > .envrc
touch .mise.local.toml
sleep 0.03
HOOK
        chmod +x .daft/hooks/worktree-post-create

        git add .daft
        git commit -m "Add daft hooks" >/dev/null
        git push >/dev/null 2>&1
    )
    rm -rf "$inject"
}
```

**Step 2: Make executable**

```bash
chmod +x benches/fixtures/create_repo.sh
```

**Step 3: Smoke test**

Run:

```bash
bash -c '
  export TEMP_BASE=/tmp/daft-bench-test
  mkdir -p $TEMP_BASE
  export GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="t@t.com"
  export GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="t@t.com"
  export GIT_CONFIG_GLOBAL=$TEMP_BASE/.gitconfig; touch $GIT_CONFIG_GLOBAL
  source benches/fixtures/create_repo.sh
  create_bare_repo /tmp/daft-bench-test/repo small
  echo "Commits: $(git -C /tmp/daft-bench-test/repo log --oneline | wc -l)"
  echo "Branches: $(git -C /tmp/daft-bench-test/repo branch | wc -l)"
  rm -rf /tmp/daft-bench-test
'
```

Expected: ~52 commits (50 + 2 branch commits), 3 branches (main + 2 feature)

**Step 4: Commit**

```bash
git add benches/fixtures/create_repo.sh
git commit -m "chore(bench): add synthetic repo fixture generator"
```

---

### Task 4: Create real repo fixture downloader

**Files:**

- Create: `benches/fixtures/real_repos.sh`

**Step 1: Write the downloader/cache script**

```bash
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
```

**Step 2: Make executable + add cache to .gitignore**

```bash
chmod +x benches/fixtures/real_repos.sh
```

Append to `.gitignore`:

```
# Benchmark artifacts
benches/results/
benches/fixtures/cache/
```

Also create `benches/history/.gitkeep`:

```bash
mkdir -p benches/history
touch benches/history/.gitkeep
```

**Step 3: Commit**

```bash
git add benches/fixtures/real_repos.sh benches/history/.gitkeep .gitignore
git commit -m "chore(bench): add real repo fixture downloader and gitignore entries"
```

---

### Task 5: Create clone scenario

**Files:**

- Create: `benches/scenarios/clone.sh`

**Step 1: Write the scenario**

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

DEST="$TEMP_BASE/clone-dest"

for size in small medium large; do
    REPO="$TEMP_BASE/repo-clone-$size"
    log "Preparing $size fixture..."
    create_bare_repo "$REPO" "$size"

    bench_compare "clone-$size" \
        "rm -rf $DEST" \
        "git-worktree-clone file://$REPO $DEST/daft" \
        "bash -c 'git clone --bare file://$REPO $DEST/git.git && git -C $DEST/git.git worktree add $DEST/git/main main'"
done

log_success "Clone benchmarks complete."
```

**Step 2: Make executable**

```bash
chmod +x benches/scenarios/clone.sh
```

**Step 3: Run it to verify**

Run: `bash benches/scenarios/clone.sh` Expected: hyperfine output with timing
table for each size, JSON files in `benches/results/`

**Step 4: Commit**

```bash
git add benches/scenarios/clone.sh
git commit -m "chore(bench): add clone benchmark scenario"
```

---

### Task 6: Create clone-with-hooks scenario

**Files:**

- Create: `benches/scenarios/clone_with_hooks.sh`

**Step 1: Write the scenario**

The git side scripts the same setup the hook does, using parallelism where
possible.

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

DEST="$TEMP_BASE/clone-hooks-dest"

# Git equivalent: clone bare + worktree + run hook work manually (parallelized)
GIT_SCRIPT="$TEMP_BASE/git-clone-with-hooks.sh"
cat > "$GIT_SCRIPT" <<'GITSCRIPT'
#!/usr/bin/env bash
set -euo pipefail
REPO="$1"; DEST="$2"
git clone --bare "file://$REPO" "$DEST/git.git"
git -C "$DEST/git.git" worktree add "$DEST/git/main" main
# Simulate post-clone hook work — parallelized where possible
cd "$DEST/git/main"
echo "export PROJECT_ROOT=$(pwd)" > .envrc &
touch .tool-versions &
wait
sleep 0.05
GITSCRIPT
chmod +x "$GIT_SCRIPT"

for size in small medium large; do
    REPO="$TEMP_BASE/repo-clone-hooks-$size"
    log "Preparing $size fixture with hooks..."
    create_bare_repo_with_hooks "$REPO" "$size"

    bench_compare "clone-hooks-$size" \
        "rm -rf $DEST" \
        "git-worktree-clone file://$REPO $DEST/daft" \
        "bash $GIT_SCRIPT $REPO $DEST"
done

log_success "Clone-with-hooks benchmarks complete."
```

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/clone_with_hooks.sh
git add benches/scenarios/clone_with_hooks.sh
git commit -m "chore(bench): add clone-with-hooks benchmark scenario"
```

---

### Task 7: Create checkout scenario

**Files:**

- Create: `benches/scenarios/checkout.sh`

**Step 1: Write the scenario**

Tests both `checkout` (existing branch) and `checkout-branch` (new branch).

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    BARE_SRC="$TEMP_BASE/bare-co-$size"
    ROOT="$TEMP_BASE/root-co-$size"
    DEST="$TEMP_BASE/co-dest-$size"

    log "Preparing $size fixture..."
    create_bare_repo "$BARE_SRC" "$size"

    # Set up a worktree root (same starting point for both sides)
    git clone --bare "file://$BARE_SRC" "$ROOT/.git" >/dev/null 2>&1
    git -C "$ROOT/.git" worktree add "$ROOT/main" main >/dev/null 2>&1

    # Checkout existing branch
    bench_compare "checkout-$size" \
        "rm -rf $DEST; git -C $ROOT/.git worktree prune 2>/dev/null || true" \
        "git-worktree-checkout feature/branch-1 --path $DEST/daft --bare-dir $ROOT/.git" \
        "git -C $ROOT/.git worktree add $DEST/git feature/branch-1"

    # Checkout new branch
    bench_compare "checkout-branch-$size" \
        "rm -rf $DEST; git -C $ROOT/.git worktree prune 2>/dev/null || true; git -C $ROOT/.git branch -D bench-new 2>/dev/null || true" \
        "git-worktree-checkout-branch bench-new --path $DEST/daft --bare-dir $ROOT/.git" \
        "git -C $ROOT/.git worktree add -b bench-new $DEST/git"
done

log_success "Checkout benchmarks complete."
```

Note: The exact daft CLI flags for `--path` and `--bare-dir` may differ. The
implementer should check `git-worktree-checkout --help` and
`git-worktree-checkout-branch --help` and adjust the invocation to match the
actual CLI interface. The key requirement is that both sides create the worktree
in a comparable destination.

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/checkout.sh
git add benches/scenarios/checkout.sh
git commit -m "chore(bench): add checkout benchmark scenario"
```

---

### Task 8: Create checkout-with-hooks scenario

**Files:**

- Create: `benches/scenarios/checkout_with_hooks.sh`

**Step 1: Write the scenario**

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

# Git side: create worktree + run hook work manually (parallelized)
GIT_SCRIPT="$TEMP_BASE/git-checkout-hooks.sh"
cat > "$GIT_SCRIPT" <<'GITSCRIPT'
#!/usr/bin/env bash
set -euo pipefail
BARE="$1"; BRANCH="$2"; DEST="$3"
git -C "$BARE" worktree add -b "$BRANCH" "$DEST"
cd "$DEST"
echo "export WORKTREE=$(pwd)" > .envrc &
touch .mise.local.toml &
wait
sleep 0.03
GITSCRIPT
chmod +x "$GIT_SCRIPT"

for size in small medium large; do
    BARE_SRC="$TEMP_BASE/bare-coh-$size"
    ROOT="$TEMP_BASE/root-coh-$size"
    DEST="$TEMP_BASE/coh-dest-$size"

    log "Preparing $size fixture with hooks..."
    create_bare_repo_with_hooks "$BARE_SRC" "$size"
    git clone --bare "file://$BARE_SRC" "$ROOT/.git" >/dev/null 2>&1
    git -C "$ROOT/.git" worktree add "$ROOT/main" main >/dev/null 2>&1

    bench_compare "checkout-hooks-$size" \
        "rm -rf $DEST; git -C $ROOT/.git worktree prune 2>/dev/null || true; git -C $ROOT/.git branch -D bench-hook 2>/dev/null || true" \
        "git-worktree-checkout-branch bench-hook --path $DEST/daft --bare-dir $ROOT/.git" \
        "bash $GIT_SCRIPT $ROOT/.git bench-hook-git $DEST/git"
done

log_success "Checkout-with-hooks benchmarks complete."
```

Same note as Task 7: verify the exact daft CLI flags at implementation time.

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/checkout_with_hooks.sh
git add benches/scenarios/checkout_with_hooks.sh
git commit -m "chore(bench): add checkout-with-hooks benchmark scenario"
```

---

### Task 9: Create init scenario

**Files:**

- Create: `benches/scenarios/init.sh`

**Step 1: Write the scenario**

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"

setup_bench_env

DEST="$TEMP_BASE/init-dest"

bench_compare "init" \
    "rm -rf $DEST" \
    "git-worktree-init $DEST/daft" \
    "bash -c 'git init --bare $DEST/git/.git && git -C $DEST/git/.git worktree add $DEST/git/main --orphan main'"

log_success "Init benchmark complete."
```

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/init.sh
git add benches/scenarios/init.sh
git commit -m "chore(bench): add init benchmark scenario"
```

---

### Task 10: Create prune scenario

**Files:**

- Create: `benches/scenarios/prune.sh`

**Step 1: Write the scenario**

Sets up stale worktrees (directories deleted but not deregistered), then
benchmarks cleanup.

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

NUM_STALE=10
ROOT="$TEMP_BASE/prune-root"
BARE_SRC="$TEMP_BASE/prune-bare-src"

# Helper script for the git side
GIT_SCRIPT="$TEMP_BASE/git-prune.sh"
cat > "$GIT_SCRIPT" <<'GITSCRIPT'
#!/usr/bin/env bash
set -euo pipefail
BARE="$1"
git -C "$BARE" worktree prune
GITSCRIPT
chmod +x "$GIT_SCRIPT"

# Setup function that creates a fresh repo with stale worktrees.
# Written as a standalone script so hyperfine --prepare can call it.
SETUP_SCRIPT="$TEMP_BASE/prune-setup.sh"
cat > "$SETUP_SCRIPT" <<SETUPSCRIPT
#!/usr/bin/env bash
set -euo pipefail
export GIT_AUTHOR_NAME="Bench User" GIT_AUTHOR_EMAIL="bench@example.com"
export GIT_COMMITTER_NAME="Bench User" GIT_COMMITTER_EMAIL="bench@example.com"
export GIT_CONFIG_GLOBAL="$TEMP_BASE/.gitconfig"
export TEMP_BASE="$TEMP_BASE"
source "$BENCH_DIR/fixtures/create_repo.sh"

rm -rf "$ROOT"
create_bare_repo "$BARE_SRC" small 2>/dev/null || true
git clone --bare "file://$BARE_SRC" "$ROOT/.git" >/dev/null 2>&1
git -C "$ROOT/.git" worktree add "$ROOT/main" main >/dev/null 2>&1

for i in \$(seq 1 $NUM_STALE); do
    git -C "$ROOT/.git" worktree add "$ROOT/stale-\$i" -b "stale-\$i" >/dev/null 2>&1
    rm -rf "$ROOT/stale-\$i"
done
SETUPSCRIPT
chmod +x "$SETUP_SCRIPT"

# Run setup once first so bare repo exists
bash "$SETUP_SCRIPT"

bench_compare "prune" \
    "bash $SETUP_SCRIPT" \
    "git-worktree-prune --bare-dir $ROOT/.git" \
    "bash $GIT_SCRIPT $ROOT/.git"

log_success "Prune benchmark complete."
```

Note: verify the exact daft prune CLI flags at implementation time (`--bare-dir`
or positional arg).

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/prune.sh
git add benches/scenarios/prune.sh
git commit -m "chore(bench): add prune benchmark scenario"
```

---

### Task 11: Create fetch scenario

**Files:**

- Create: `benches/scenarios/fetch.sh`

**Step 1: Write the scenario**

The git side parallelizes fetch across remotes with `xargs -P`.

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

REMOTE1="$TEMP_BASE/fetch-remote1"
REMOTE2="$TEMP_BASE/fetch-remote2"
ROOT="$TEMP_BASE/fetch-root"

create_bare_repo "$REMOTE1" small
create_bare_repo "$REMOTE2" small

git clone --bare "file://$REMOTE1" "$ROOT/.git" >/dev/null 2>&1
git -C "$ROOT/.git" remote add second "file://$REMOTE2"
git -C "$ROOT/.git" worktree add "$ROOT/main" main >/dev/null 2>&1

# Git side: parallel fetch across all remotes
GIT_SCRIPT="$TEMP_BASE/git-fetch-parallel.sh"
cat > "$GIT_SCRIPT" <<GITSCRIPT
#!/usr/bin/env bash
set -euo pipefail
BARE="\$1"
git -C "\$BARE" remote | xargs -P 0 -I{} git -C "\$BARE" fetch {} 2>/dev/null
GITSCRIPT
chmod +x "$GIT_SCRIPT"

bench_compare "fetch" \
    "" \
    "git-worktree-fetch --bare-dir $ROOT/.git" \
    "bash $GIT_SCRIPT $ROOT/.git"

log_success "Fetch benchmark complete."
```

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/fetch.sh
git add benches/scenarios/fetch.sh
git commit -m "chore(bench): add fetch benchmark scenario"
```

---

### Task 12: Create branch-delete scenario

**Files:**

- Create: `benches/scenarios/branch_delete.sh`

**Step 1: Write the scenario**

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

ROOT="$TEMP_BASE/bd-root"
BARE_SRC="$TEMP_BASE/bd-bare-src"

create_bare_repo "$BARE_SRC" small
git clone --bare "file://$BARE_SRC" "$ROOT/.git" >/dev/null 2>&1
git -C "$ROOT/.git" worktree add "$ROOT/main" main >/dev/null 2>&1

GIT_SCRIPT="$TEMP_BASE/git-branch-delete.sh"
cat > "$GIT_SCRIPT" <<'GITSCRIPT'
#!/usr/bin/env bash
set -euo pipefail
BARE="$1"; WT="$2"; BRANCH="$3"
git -C "$BARE" worktree remove "$WT" 2>/dev/null || rm -rf "$WT"
git -C "$BARE" worktree prune
git -C "$BARE" branch -D "$BRANCH" 2>/dev/null || true
GITSCRIPT
chmod +x "$GIT_SCRIPT"

# Prepare: recreate the worktree before each run
PREPARE_SCRIPT="$TEMP_BASE/bd-prepare.sh"
cat > "$PREPARE_SCRIPT" <<PREPSCRIPT
#!/usr/bin/env bash
set -euo pipefail
git -C "$ROOT/.git" worktree remove "$ROOT/to-delete" 2>/dev/null || rm -rf "$ROOT/to-delete"
git -C "$ROOT/.git" worktree prune 2>/dev/null || true
git -C "$ROOT/.git" branch -D to-delete 2>/dev/null || true
git -C "$ROOT/.git" worktree add -b to-delete "$ROOT/to-delete" >/dev/null 2>&1
PREPSCRIPT
chmod +x "$PREPARE_SCRIPT"

# Run prepare once so the initial state exists
bash "$PREPARE_SCRIPT"

bench_compare "branch-delete" \
    "bash $PREPARE_SCRIPT" \
    "git-worktree-branch-delete to-delete --bare-dir $ROOT/.git" \
    "bash $GIT_SCRIPT $ROOT/.git $ROOT/to-delete to-delete"

log_success "Branch-delete benchmark complete."
```

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/branch_delete.sh
git add benches/scenarios/branch_delete.sh
git commit -m "chore(bench): add branch-delete benchmark scenario"
```

---

### Task 13: Create full workflow scenario

**Files:**

- Create: `benches/scenarios/workflow_full.sh`

**Step 1: Write the scenario**

End-to-end: clone, create 3 worktrees, run hooks in each, prune 2. The git side
maximizes parallelism.

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    REPO="$TEMP_BASE/wf-repo-$size"
    DEST="$TEMP_BASE/wf-dest-$size"

    log "Preparing $size fixture with hooks..."
    create_bare_repo_with_hooks "$REPO" "$size"

    # daft workflow (sequential — reflects actual daft behavior)
    DAFT_SCRIPT="$TEMP_BASE/daft-wf-$size.sh"
    cat > "$DAFT_SCRIPT" <<DSCRIPT
#!/usr/bin/env bash
set -euo pipefail
git-worktree-clone file://$REPO $DEST/daft
cd $DEST/daft
git-worktree-checkout-branch feature-a
git-worktree-checkout-branch feature-b
git-worktree-checkout-branch feature-c
DSCRIPT
    chmod +x "$DAFT_SCRIPT"

    # git workflow (maximum parallelism)
    GIT_SCRIPT="$TEMP_BASE/git-wf-$size.sh"
    cat > "$GIT_SCRIPT" <<GSCRIPT
#!/usr/bin/env bash
set -euo pipefail

git clone --bare "file://$REPO" "$DEST/git/.git"
git -C "$DEST/git/.git" worktree add "$DEST/git/main" main

# Parallel: create 3 worktrees
git -C "$DEST/git/.git" worktree add -b feature-a "$DEST/git/feature-a" &
git -C "$DEST/git/.git" worktree add -b feature-b "$DEST/git/feature-b" &
git -C "$DEST/git/.git" worktree add -b feature-c "$DEST/git/feature-c" &
wait

# Parallel: run post-create hook equivalent in each
run_hook() {
    local wt="\$1"
    cd "\$wt"
    echo "export WORKTREE=\$(pwd)" > .envrc
    touch .mise.local.toml
    sleep 0.03
}
run_hook "$DEST/git/main" &
run_hook "$DEST/git/feature-a" &
run_hook "$DEST/git/feature-b" &
run_hook "$DEST/git/feature-c" &
wait

# Parallel: prune 2 worktrees
git -C "$DEST/git/.git" worktree remove "$DEST/git/feature-a" &
git -C "$DEST/git/.git" worktree remove "$DEST/git/feature-b" &
wait
GSCRIPT
    chmod +x "$GIT_SCRIPT"

    bench_compare "workflow-full-$size" \
        "rm -rf $DEST" \
        "bash $DAFT_SCRIPT" \
        "bash $GIT_SCRIPT"
done

log_success "Full workflow benchmarks complete."
```

Note: the daft side invocations (especially `git-worktree-checkout-branch`
inside a daft-managed repo) need to match the actual CLI behavior. The
implementer should verify by running `git-worktree-checkout-branch --help` and
testing inside a daft-cloned repo. Adjust the script to use the correct
arguments (e.g., it may just take a branch name when run from within a daft
worktree).

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/workflow_full.sh
git add benches/scenarios/workflow_full.sh
git commit -m "chore(bench): add full workflow benchmark scenario"
```

---

### Task 14: Create competition scenario (opt-in)

**Files:**

- Create: `benches/scenarios/vs_competition.sh`

**Step 1: Write the scenario**

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$BENCH_DIR/bench_framework.sh"
source "$BENCH_DIR/fixtures/create_repo.sh"

setup_bench_env

check_competitor() {
    local name="$1"; local cmd="$2"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        log_warn "$name ($cmd) not found — skipping"
        return 1
    fi
    return 0
}

REPO="$TEMP_BASE/comp-repo"
DEST="$TEMP_BASE/comp-dest"
create_bare_repo "$REPO" medium

# vs plain shell alias pattern (always runs)
SHELL_SCRIPT="$TEMP_BASE/shell-alias-clone.sh"
cat > "$SHELL_SCRIPT" <<'SHELLSCRIPT'
#!/usr/bin/env bash
set -euo pipefail
REPO="$1"; DEST="$2"
git clone --bare "file://$REPO" "$DEST/shell/.git"
default_branch=$(git -C "$DEST/shell/.git" symbolic-ref --short HEAD 2>/dev/null || echo main)
git -C "$DEST/shell/.git" worktree add "$DEST/shell/$default_branch" "$default_branch"
SHELLSCRIPT
chmod +x "$SHELL_SCRIPT"

bench_compare "vs-shell-alias-clone" \
    "rm -rf $DEST" \
    "git-worktree-clone file://$REPO $DEST/daft" \
    "bash $SHELL_SCRIPT $REPO $DEST"

# vs git-town (if installed)
if check_competitor "git-town" "git-town"; then
    log "git-town benchmarks would go here — needs repo-specific setup"
    log_warn "git-town integration not yet implemented"
fi

log_success "Competition benchmarks complete."
```

**Step 2: Make executable + commit**

```bash
chmod +x benches/scenarios/vs_competition.sh
git add benches/scenarios/vs_competition.sh
git commit -m "chore(bench): add competition comparison scenario (opt-in)"
```

---

### Task 15: Create run_all.sh orchestrator

**Files:**

- Create: `benches/run_all.sh`

**Step 1: Write the orchestrator**

```bash
#!/usr/bin/env bash
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$BENCH_DIR/bench_framework.sh"

SCENARIOS=(
    clone
    clone_with_hooks
    checkout
    checkout_with_hooks
    init
    prune
    fetch
    branch_delete
    workflow_full
)

FAILED=()

for scenario in "${SCENARIOS[@]}"; do
    script="$BENCH_DIR/scenarios/${scenario}.sh"
    if [[ ! -f "$script" ]]; then
        log_warn "Not found: $script — skipping"
        continue
    fi
    log "=== $scenario ==="
    if ! bash "$script"; then
        log_error "FAILED: $scenario"
        FAILED+=("$scenario")
    fi
done

# Aggregate markdown results into summary
SUMMARY="$RESULTS_DIR/summary.md"
{
    echo "# daft Benchmark Results"
    echo
    echo "Generated: $(date -u '+%Y-%m-%d %H:%M UTC')"
    echo
    for md in "$RESULTS_DIR"/*.md; do
        [[ -f "$md" ]] || continue
        [[ "$(basename "$md")" == "summary.md" ]] && continue
        name="$(basename "$md" .md)"
        echo "## $name"
        echo
        cat "$md"
        echo
    done
} > "$SUMMARY"

log_success "Summary: $SUMMARY"

if [[ ${#FAILED[@]} -gt 0 ]]; then
    log_error "Failed: ${FAILED[*]}"
    exit 1
fi

log_success "All benchmarks complete."
```

**Step 2: Make executable + commit**

```bash
chmod +x benches/run_all.sh
git add benches/run_all.sh
git commit -m "chore(bench): add benchmark orchestrator"
```

---

### Task 16: Add mise tasks

**Files:**

- Create: `mise-tasks/bench/_default`
- Create: `mise-tasks/bench/clone`
- Create: `mise-tasks/bench/clone-hooks`
- Create: `mise-tasks/bench/checkout`
- Create: `mise-tasks/bench/checkout-hooks`
- Create: `mise-tasks/bench/init`
- Create: `mise-tasks/bench/prune`
- Create: `mise-tasks/bench/fetch`
- Create: `mise-tasks/bench/branch-delete`
- Create: `mise-tasks/bench/workflow`
- Create: `mise-tasks/bench/competition`
- Create: `mise-tasks/bench/compare`
- Create: `mise-tasks/bench/baseline`

**Step 1: Create the task files**

Follow the pattern from `mise-tasks/test/`. Each task file is a bash script with
`#MISE` directives. Use `$(git rev-parse --show-toplevel)` to find the project
root.

`mise-tasks/bench/_default`:

```bash
#!/usr/bin/env bash
#MISE description="Run all benchmark scenarios"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/run_all.sh"
```

`mise-tasks/bench/clone`:

```bash
#!/usr/bin/env bash
#MISE description="Run clone benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/clone.sh"
```

`mise-tasks/bench/clone-hooks`:

```bash
#!/usr/bin/env bash
#MISE description="Run clone-with-hooks benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/clone_with_hooks.sh"
```

`mise-tasks/bench/checkout`:

```bash
#!/usr/bin/env bash
#MISE description="Run checkout benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/checkout.sh"
```

`mise-tasks/bench/checkout-hooks`:

```bash
#!/usr/bin/env bash
#MISE description="Run checkout-with-hooks benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/checkout_with_hooks.sh"
```

`mise-tasks/bench/init`:

```bash
#!/usr/bin/env bash
#MISE description="Run init benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/init.sh"
```

`mise-tasks/bench/prune`:

```bash
#!/usr/bin/env bash
#MISE description="Run prune benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/prune.sh"
```

`mise-tasks/bench/fetch`:

```bash
#!/usr/bin/env bash
#MISE description="Run fetch benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/fetch.sh"
```

`mise-tasks/bench/branch-delete`:

```bash
#!/usr/bin/env bash
#MISE description="Run branch-delete benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/branch_delete.sh"
```

`mise-tasks/bench/workflow`:

```bash
#!/usr/bin/env bash
#MISE description="Run full workflow benchmark"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/workflow_full.sh"
```

`mise-tasks/bench/competition`:

```bash
#!/usr/bin/env bash
#MISE description="Run competitor comparison benchmarks (opt-in)"
#MISE depends=["build"]
set -euo pipefail

bash "$(git rev-parse --show-toplevel)/benches/scenarios/vs_competition.sh"
```

`mise-tasks/bench/compare`:

```bash
#!/usr/bin/env bash
#MISE description="Compare latest results against baseline"
set -euo pipefail

BENCH_DIR="$(git rev-parse --show-toplevel)/benches"

if [[ ! -f "$BENCH_DIR/results/summary.md" ]]; then
    echo "No results found. Run: mise run bench"
    exit 1
fi

if [[ ! -f "$BENCH_DIR/results/baseline.md" ]]; then
    echo "No baseline found. Run: mise run bench:baseline"
    echo
    cat "$BENCH_DIR/results/summary.md"
    exit 0
fi

diff --color "$BENCH_DIR/results/baseline.md" "$BENCH_DIR/results/summary.md" || true
```

`mise-tasks/bench/baseline`:

```bash
#!/usr/bin/env bash
#MISE description="Pin current results as the new baseline"
set -euo pipefail

BENCH_DIR="$(git rev-parse --show-toplevel)/benches"

if [[ ! -f "$BENCH_DIR/results/summary.md" ]]; then
    echo "No results to baseline. Run: mise run bench"
    exit 1
fi

cp "$BENCH_DIR/results/summary.md" "$BENCH_DIR/results/baseline.md"
echo "Baseline updated from current results."
```

**Step 2: Make all task files executable**

```bash
chmod +x mise-tasks/bench/*
```

**Step 3: Verify**

Run: `mise task | grep bench` Expected: lines for `bench`, `bench:clone`,
`bench:checkout`, etc.

**Step 4: Commit**

```bash
git add mise-tasks/bench/
git commit -m "chore(bench): add mise tasks for all benchmark scenarios"
```

---

### Task 17: Add CI workflow

**Files:**

- Create: `.github/workflows/bench.yml`

**Step 1: Write the workflow**

```yaml
name: Benchmarks

on:
  push:
    branches: [master]
    paths-ignore:
      - "*.md"
      - "docs/**"
  workflow_dispatch:

jobs:
  bench:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v6

      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust dependencies
        uses: Swatinem/rust-cache@v2

      - name: Install mise
        uses: jdx/mise-action@v3

      - name: Install hyperfine
        run: cargo install hyperfine

      - name: Build daft and set up symlinks
        run: mise run dev

      - name: Run benchmarks
        run: mise run bench

      - name: Copy summary to docs
        run: |
          mkdir -p docs/benchmarks
          VERSION=$(./target/release/daft --version 2>/dev/null | awk '{print $2}' || echo "unknown")
          {
            echo "---"
            echo "title: Benchmarks"
            echo "description: daft performance benchmarks vs. equivalent git scripting"
            echo "---"
            echo
            cat benches/results/summary.md
            echo
            echo "---"
            echo
            echo "Version: v${VERSION}"
          } > docs/benchmarks/index.md

      - name: Save to history
        run: |
          VERSION=$(./target/release/daft --version 2>/dev/null | awk '{print $2}' || echo "unknown")
          DATE=$(date -u '+%Y-%m-%d')
          mkdir -p benches/history
          cp benches/results/summary.md "benches/history/${DATE}-v${VERSION}.md"

      - name: Commit results
        run: |
          git config --local user.name "github-actions[bot]"
          git config --local user.email "github-actions[bot]@users.noreply.github.com"
          git add docs/benchmarks/index.md benches/history/
          if git diff --staged --quiet; then
            echo "No changes to commit"
          else
            git commit -m "chore: update benchmark results [skip ci]"
            git push
          fi

      - name: Upload raw results
        uses: actions/upload-artifact@v6
        if: always()
        with:
          name: benchmark-results
          path: benches/results/
          retention-days: 90
```

**Step 2: Commit**

```bash
git add .github/workflows/bench.yml
git commit -m "ci: add benchmark workflow"
```

---

### Task 18: Add benchmarks page to docs site

**Files:**

- Create: `docs/benchmarks/index.md`
- Modify: `docs/.vitepress/config.ts` (lines 214-218 for nav, lines 307-313 for
  sidebar)

**Step 1: Create placeholder page**

```markdown
---
title: Benchmarks
description: daft performance benchmarks vs. equivalent git scripting
---

# Benchmarks

Results are generated automatically on each push to master. Run `mise run bench`
locally to generate your own.

This page will be replaced with actual benchmark data after the first CI run.
```

**Step 2: Add to VitePress nav**

In `docs/.vitepress/config.ts`, add a Benchmarks link to the `nav` array (line
~217, before the GitHub link):

```typescript
{ text: "Benchmarks", link: "/benchmarks/" },
```

So the nav becomes:

```typescript
nav: [
  { text: "Guide", link: "/getting-started/installation" },
  { text: "CLI Reference", link: "/cli/git-worktree-clone" },
  { text: "Benchmarks", link: "/benchmarks/" },
  { text: `v${version}`, link: "/changelog" },
  { text: "GitHub", link: "https://github.com/avihut/daft" },
],
```

**Step 3: Add to sidebar**

Add a Benchmarks section to the sidebar (line ~307, in the "Project" section):

```typescript
{
  text: "Project",
  items: [
    { text: "Benchmarks", link: "/benchmarks/" },
    { text: "Contributing", link: "/contributing" },
    { text: "Changelog", link: "/changelog" },
  ],
},
```

**Step 4: Commit**

```bash
git add docs/benchmarks/index.md docs/.vitepress/config.ts
git commit -m "docs: add benchmarks page to docs site"
```

---

### Task 19: Verify everything works end-to-end

**Step 1: Run `mise task | grep bench`**

Expected: all bench tasks listed

**Step 2: Run a single scenario**

Run: `mise run bench:init` Expected: hyperfine output, JSON and markdown in
`benches/results/`

**Step 3: Run all scenarios**

Run: `mise run bench` Expected: all scenarios pass, `benches/results/summary.md`
generated

**Step 4: Check docs build**

Run: `mise run docs:site:build` Expected: builds successfully, benchmarks page
included

**Step 5: Commit any fixes**

If any scenario needed CLI flag adjustments, commit them:

```bash
git add -A
git commit -m "fix(bench): adjust CLI invocations for actual daft interface"
```
