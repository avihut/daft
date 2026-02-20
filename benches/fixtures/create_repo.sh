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

# Create a bare repo that has daft hooks for post-clone and worktree-post-create.
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
