#!/usr/bin/env bash
# Benchmark: the copy: engine (via daft warm) on a pnpm-shaped cache tree,
# against the platform's own fast copy — the regression bench #801's
# acceptance asked for.
#
# Two arms, because the copier has two paths with different scaling:
#   copy-warm        the default path (whole-tree clonefile on macOS/APFS,
#                    walking copy elsewhere), vs `cp -cR` / `cp -R --reflink=auto`
#   copy-warm-walk   the portable walking path, pinned via DAFT_COPY_FORCE_WALK,
#                    vs plain byte-copy `cp -R` — what Linux/ext4 users run
#
# SCALE=small|medium|large (default small) sizes the fixture; see
# fixtures/gen_pnpm_tree.py for what each generates.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"

setup_bench_env

SCALE="${SCALE:-small}"
PYTHON="$(command -v python3 || true)"
if [[ -z "$PYTHON" ]]; then
    log_warn "python3 not found — skipping copy benchmark (fixture generator needs it)"
    exit 0
fi

log "=== Copy benchmark: $SCALE pnpm-shaped tree ==="

ROOT="$TEMP_BASE/copy-warm-$SCALE"
rm -rf "$ROOT"
mkdir -p "$ROOT"

# Isolate daft's state/config/data — warm records nothing anyone wants in
# the real dirs, and the bench must never touch them.
export DAFT_CONFIG_DIR="$ROOT/dcfg" DAFT_STATE_DIR="$ROOT/dstate" DAFT_DATA_DIR="$ROOT/ddata"
export DAFT_TESTING=1

# A daft-layout repo pair: main holds the cache, work receives it.
seed="$ROOT/seed"
mkdir -p "$seed"
git -C "$seed" init -q -b main
echo node_modules > "$seed/.gitignore"
printf 'copy:\n  - node_modules\n' > "$seed/daft.yml"
echo bench > "$seed/README.md"
git -C "$seed" add -A
git -C "$seed" commit -qm init
git clone --bare -q "$seed" "$ROOT/bench/.git"
git -C "$ROOT/bench/.git" worktree add -q ../main main
git -C "$ROOT/bench/.git" worktree add -q -b work ../work main
rm -rf "$seed"

"$PYTHON" "$SCRIPT_DIR/../fixtures/gen_pnpm_tree.py" "$ROOT/fixture" --scale "$SCALE"
mv "$ROOT/fixture/node_modules" "$ROOT/bench/main/node_modules"

WORK="$ROOT/bench/work"
PREPARE="rm -rf $WORK/node_modules"

# The platform's own fast copy is the honest competitor.
case "$(uname)" in
    Darwin) CP_FAST="cp -cR" ;;
    *)      CP_FAST="cp -R --reflink=auto" ;;
esac

bench_compare --two-way \
    "copy-warm-${SCALE}" \
    "$PREPARE" \
    "cd $WORK && daft warm" \
    "$CP_FAST $ROOT/bench/main/node_modules $WORK/node_modules"

# The portable walking path, measurable on any filesystem via the pin.
bench_compare --two-way \
    "copy-warm-walk-${SCALE}" \
    "$PREPARE" \
    "cd $WORK && DAFT_COPY_FORCE_WALK=1 daft warm" \
    "cp -R $ROOT/bench/main/node_modules $WORK/node_modules"

log_success "Copy benchmark ($SCALE) done"
cleanup_bench
