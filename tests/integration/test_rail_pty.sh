#!/bin/bash

# Blessed shell tests that need a real terminal (#447 Tier A).
#
# Everything here drives daft under a PTY — the DSR-answering pty_run.py for
# the live rail, script(1) for a controlling tty — because the behaviour under
# test only exists on a TTY: the rail's planning face and receipts, the TUI
# PR column, the alias-capture trampoline. The YAML runner hands its children
# pipes and sets DAFT_TESTING, so it never materializes a region; that is the
# one hard wall the blessed shell suite exists for. Tests that need no
# terminal live in tests/manual/scenarios/.
#
# Assembled from the PTY tests that used to sit inside test_checkout.sh,
# test_branch_delete.sh, test_sync.sh, test_prune.sh and test_worktree_exec.sh
# (whose non-PTY halves are YAML scenarios now). Each section keeps the
# helpers it arrived with — the `_rail_*` / `_bd_*` namespaces are left as
# they were so the tests read the same as their history.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

PTY_RUN="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pty_run.py"

# ===========================================================================
# go / start on the rail (from test_checkout.sh)
# ===========================================================================

# Run daft with the live rail enabled (pattern from
# test_exec_verbose_toggle.sh): the framework's DAFT_TESTING hides the
# interactive region — the subject of the rail tests — so it comes off and
# the background spawns it suppressed are turned off individually. TERM is
# pinned because indicatif hides its whole draw target under an unset or
# `dumb` TERM, as on a bare CI runner.
_rail_daft() {
    env -u DAFT_TESTING \
        TERM=xterm-256color \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# Strip ANSI and split the pty's carriage-returned repaints into lines.
_rail_clean() {
    sed -e 's/\x1b\[[0-9;]*[a-zA-Z]//g' -e 's/\r/\n/g' "$1"
}

# #782: with daft.checkout.fetch=true, a cross-repo catalog hop must not
# commit a worktree-creation plan — resolution runs under the collapsed
# planning face and dissolves without a receipt. The rail only exists on a
# TTY, so this drives daft under the DSR-answering PTY (pty_run.py); the
# YAML suite runs with DAFT_TESTING set and never materializes a region.
test_go_fetch_hop_no_rail_receipt() {
    local remote_a=$(create_test_remote "test-repo-hop-a" "main")
    local remote_b=$(create_test_remote "test-repo-hop-b" "main")
    git-worktree-clone --layout contained "$remote_a" || return 1
    git-worktree-clone --layout contained "$remote_b" || return 1
    cd "test-repo-hop-a/main"
    git config daft.checkout.fetch true

    local log="$PWD/go-hop-rail.log"
    _rail_daft "$PTY_RUN" "$log" daft go test-repo-hop-b || return 1

    local clean
    clean=$(_rail_clean "$log")
    if echo "$clean" | grep -q "Failed after"; then
        log_error "hop closed a Failed rail receipt"
        return 1
    fi
    if echo "$clean" | grep -q "┌"; then
        log_error "hop committed a worktree-creation plan"
        return 1
    fi
    # The collapsed line is the region's *only* live line: the rail's shell is
    # built on a hidden draw target and attaches when a plan lands. Drop that
    # hidden() and the detached spacer/footer paint themselves beside the face
    # (`│`, `└  1ms`) — invisible to the InMemoryTerm unit tests, so this is
    # the only guard that catches it.
    if echo "$clean" | grep -q "[│└]"; then
        log_error "detached rail shell painted beside the collapsed line"
        return 1
    fi
    if echo "$clean" | grep -q "Failed to fetch"; then
        log_error "probe fetch warning leaked onto the hop"
        return 1
    fi
    if ! echo "$clean" | grep -q "Opening repository 'test-repo-hop-b'"; then
        log_error "hop line missing from the PTY output"
        return 1
    fi
    return 0
}

# #811: a real terminal Ctrl-C during `-x` must still leave the shell in the
# new worktree. Two things have to hold and only a pty can test either:
#
#   1. daft writes the cd target *before* the exec sequence, so the signal
#      cannot arrive between the command and the write; and
#   2. daft *exits* on SIGINT (130) instead of dying from it (pty_run reports
#      a signal death as 254, Python's -2). This is the load-bearing half:
#      bash and zsh abandon the enclosing function when a foreground child is
#      signal-killed, so a signal-killed daft never reaches `__daft_wrapper`'s
#      `cd` and the target it wrote is read by nobody.
#
# `--ctty` makes daft a session leader owning the pty, so writing \x03 raises
# SIGINT in the foreground process group exactly as a keyboard Ctrl-C does —
# the group-wide delivery a `kill -INT <pid>` cannot reproduce. The cue is
# emitted by the -x command itself: daft's own "Executing" step never reaches
# the pty under the rail, and pty_run splits a cue on its first colon, so a
# self-authored colon-free marker is the only reliable trigger.
#
# The cue must be ASSEMBLED AT RUNTIME, never a literal in the command text.
# Since #812 the rail plans each `-x` command as a row labelled with the
# command exactly as typed, so a literal marker is painted on screen while the
# row is still pending — the Ctrl-C then lands during planning, before the
# worktree exists, and the test interrupts the wrong thing while still
# reporting 130. `printf 'XCUE%s\n' READY` keeps `XCUEREADY` out of the label
# and puts it only in the output the command produces when it actually runs.
_X_CUE_CMD="printf 'XCUE%s\n' READY; sleep 10"
_assert_interrupted_go_kept_cd() {
    local branch="$1" log="$2" cd_file="$3" status="$4"

    if [ "$status" != "130" ]; then
        log_error "expected exit 130 (interrupted, exited cleanly), got $status"
        # 254 is pty_run relaying Python's -2: daft died *from* SIGINT, which
        # is precisely the state that strands the shell.
        [ "$status" = "254" ] && log_error "daft was signal-killed — the wrapper would abandon its cd"
        return 1
    fi
    if ! grep -q "XCUEREADY" "$log"; then
        log_error "-x never started; the interrupt proved nothing"
        return 1
    fi
    assert_directory_exists "../$branch" || return 1
    if [ ! -s "$cd_file" ]; then
        log_error "DAFT_CD_FILE empty after interrupt — the shell would stay put"
        return 1
    fi
    local want got
    want=$(cd "../$branch" && pwd -P)
    got=$(cd "$(cat "$cd_file")" && pwd -P)
    if [ "$got" != "$want" ]; then
        log_error "cd target '$got' != '$want'"
        return 1
    fi
    return 0
}

test_go_exec_interrupt_keeps_cd() {
    local remote_repo=$(create_test_remote "test-repo-x-interrupt" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-x-interrupt/main"

    local log="$PWD/go-x-interrupt.log"
    local cd_file="$PWD/go-x-interrupt.cd"
    : > "$cd_file"
    local status=0
    DAFT_CD_FILE="$cd_file" _rail_daft "$PTY_RUN" --ctty \
        --send-after 'XCUEREADY:\x03' "$log" \
        daft go develop -x "$_X_CUE_CMD" || status=$?

    _assert_interrupted_go_kept_cd develop "$log" "$cd_file" "$status"
}

# The same guarantee without a rail. Before #811's interrupt arming this was
# the case that failed even with the cd target written early: with no live
# timeline region nothing installed a SIGINT handler, daft died from the
# signal, and the wrapper abandoned its `cd`. Keep both — the rail path can
# pass on the timeline's handler alone and hide a regression here.
test_go_exec_interrupt_quiet_keeps_cd() {
    local remote_repo=$(create_test_remote "test-repo-x-interrupt-q" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-x-interrupt-q/main"

    local log="$PWD/go-x-interrupt-q.log"
    local cd_file="$PWD/go-x-interrupt-q.cd"
    : > "$cd_file"
    local status=0
    DAFT_CD_FILE="$cd_file" _rail_daft "$PTY_RUN" --ctty \
        --send-after 'XCUEREADY:\x03' "$log" \
        daft go develop -q -x "$_X_CUE_CMD" || status=$?

    _assert_interrupted_go_kept_cd develop "$log" "$cd_file" "$status"
}

# The counterpart guard: when resolution does warrant a worktree, the
# collapsed face must expand into the full rail — persisted header, the
# probe fetch as a pre-completed receipt row, and a Ready footer.
test_go_fetch_on_rail_expands() {
    local remote_repo=$(create_test_remote "test-repo-rail-expand" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-rail-expand/main"
    git config daft.checkout.fetch true

    local log="$PWD/go-rail-expand.log"
    _rail_daft "$PTY_RUN" "$log" daft go develop || return 1

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "┌  Opening develop"; then
        log_error "expanded rail header missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "✓  Fetched remote"; then
        log_error "pre-completed fetch receipt row missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready in"; then
        log_error "Ready footer missing"
        return 1
    fi
    assert_directory_exists "../develop" || return 1
    return 0
}

# #813: `daft go <full-sha>` opens a sandbox whose directory is a 12-hex
# prefix of the commit, but the sandbox rail's header was seeded with the
# spelling — forty hex characters in the slot that carries identity. A
# sandbox visit never commits a plan for the *branch* reading, so nothing
# downstream corrects it: this seed is the whole sandbox run.
#
# Scoped to the sandbox rail on purpose. `daft go <sha>` first tries the
# branch reading, and that attempt's collapsed face legitimately shows the
# spelling — daft is trying to open a branch of that name and has not probed
# yet. The "Branch '<sha>' not found; opening a detached sandbox" line between
# the two is what marks the handover. It names no sandbox: which one this
# lands in is not known until the visit resolves, and the header below it is
# where that answer belongs.
test_go_sandbox_header_names_dirname() {
    local remote_repo=$(create_test_remote "test-repo-sandbox-hdr" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sandbox-hdr/main"

    local full dirname
    full=$(git rev-parse HEAD)
    # `sandbox::derived_dirname` — DERIVED_DIRNAME_HEX = 12.
    dirname="${full:0:12}"

    local log="$PWD/go-sandbox-hdr.log"
    _rail_daft "$PTY_RUN" "$log" daft go "$full" --no-cd || return 1

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "┌  Opening $dirname"; then
        log_error "sandbox rail header did not name the sandbox '$dirname'"
        echo "$clean" | head -20
        return 1
    fi
    if echo "$clean" | grep -q "┌  Opening $full"; then
        log_error "sandbox rail header still carries the full spelling"
        return 1
    fi
    assert_directory_exists "../$dirname" || return 1
    return 0
}

# #813: one commit has one sandbox, whatever spelling summoned it. A visit
# that lands on an existing sandbox must name *that* worktree, not the name a
# fresh one would have been given — seeding the header from the spelling's
# derived name was wrong in the one case where being wrong looks most like
# being right: `<12 hex>` is exactly the shape of a real sandbox name, so the
# header asserted a directory that does not exist, four lines above the line
# naming the one that does.
test_go_sandbox_header_names_the_sandbox_it_lands_in() {
    local remote_repo=$(create_test_remote "test-repo-sandbox-land" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sandbox-land/main"

    local full derived
    full=$(git rev-parse HEAD)
    derived="${full:0:12}"
    git tag v1.0 HEAD || return 1

    # Mint the canonical sandbox for this commit under the tag's name.
    daft go v1.0 --no-cd >/dev/null 2>&1 || return 1
    assert_directory_exists "../v1.0" || return 1

    # Now reach the same commit by a spelling that derives a different name.
    local log="$PWD/go-sandbox-land.log"
    _rail_daft "$PTY_RUN" "$log" daft go "$full" --no-cd || return 1

    # Read the planning face, not a `┌` frame: a visit that *navigates*
    # commits no plan, so this rail never draws one — which is the same reason
    # the seed has to be right in the first place. Scoped past the handover
    # line for two reasons: the branch attempt above it legitimately shows the
    # spelling, and the derived name is a prefix of that spelling, so an
    # unscoped negative grep matches the branch rail and always "fails".
    local sandbox_rail
    sandbox_rail=$(_rail_clean "$log" | sed -n '/opening a detached sandbox/,$p')
    if ! echo "$sandbox_rail" | grep -q "Opening v1.0"; then
        log_error "header must name the sandbox the visit lands in ('v1.0')"
        echo "$sandbox_rail" | head -20
        return 1
    fi
    if echo "$sandbox_rail" | grep -q "Opening $derived"; then
        log_error "header named '$derived', a directory the visit never creates"
        echo "$sandbox_rail" | head -20
        return 1
    fi
    if [[ -d "../$derived" ]]; then
        log_error "the visit minted a second sandbox for one commit"
        return 1
    fi
    return 0
}

# #813: the branch journey's `Created worktree` row carried a path relative
# to the cwd, so the row's label promised a worktree and its subject was a
# location — in the path colour, which said so twice. The subject is the
# worktree: the branch it is for.
test_go_row_names_the_worktree_not_its_path() {
    local remote_repo=$(create_test_remote "test-repo-row-name" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-row-name/main"

    local log="$PWD/go-row-name.log"
    _rail_daft "$PTY_RUN" "$log" daft go develop || return 1

    local annotation
    annotation=$(_rail_clean "$log" |
        grep -o 'Created worktree  *[^ ]*' | head -1 |
        sed 's/Created worktree  *//')
    if [[ "$annotation" != "develop" ]]; then
        log_error "create row annotated '$annotation', expected 'develop'"
        _rail_clean "$log" | head -20
        return 1
    fi
    return 0
}

# #813: same row, the `daft start` journey — a separate plan builder, so a
# separate render site.
test_start_row_names_the_worktree_not_its_path() {
    local remote_repo=$(create_test_remote "test-repo-row-start" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-row-start/main"
    git config daft.checkout.push false

    # Outside the repo: `daft start` carries untracked files into the new
    # worktree, and a log written here would leave with them.
    local log="$TEMP_BASE_DIR/start-row-name.log"
    _rail_daft "$PTY_RUN" "$log" daft start feature/row-name || return 1

    local annotation
    annotation=$(_rail_clean "$log" |
        grep -o 'Created worktree  *[^ ]*' | head -1 |
        sed 's/Created worktree  *//')
    if [[ "$annotation" != "feature/row-name" ]]; then
        log_error "create row annotated '$annotation', expected 'feature/row-name'"
        _rail_clean "$log" | head -20
        return 1
    fi
    return 0
}

# #813: a sandbox pins HEAD to a commit and never touches a branch, but its
# rail reused the branch journey's stage, so the row read "Checked out
# branch" beside an annotation naming a commit — the row described something
# that does not exist in the run being watched. Its `Created worktree` row
# has the same job as the branch journey's: name the worktree, which for a
# sandbox is its directory name.
test_go_sandbox_rows_name_the_sandbox() {
    local remote_repo=$(create_test_remote "test-repo-sandbox-noun" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sandbox-noun/main"

    local full dirname
    full=$(git rev-parse HEAD)
    dirname="${full:0:12}"

    local log="$PWD/go-sandbox-noun.log"
    _rail_daft "$PTY_RUN" "$log" daft go "$full" --no-cd || return 1

    # Scoped past the handover line: the branch attempt above it is a real
    # branch checkout and keeps the branch noun.
    local sandbox_rail
    sandbox_rail=$(_rail_clean "$log" | sed -n '/opening a detached sandbox/,$p')
    if ! echo "$sandbox_rail" | grep -q "Checked out commit"; then
        log_error "sandbox rail did not name a commit"
        echo "$sandbox_rail" | head -20
        return 1
    fi
    if echo "$sandbox_rail" | grep -q "Checked out branch"; then
        log_error "sandbox rail still claims it checked out a branch"
        echo "$sandbox_rail" | head -20
        return 1
    fi

    local annotation
    annotation=$(echo "$sandbox_rail" |
        grep -o 'Created worktree  *[^ ]*' | head -1 |
        sed 's/Created worktree  *//')
    if [[ "$annotation" != "$dirname" ]]; then
        log_error "create row annotated '$annotation', expected '$dirname'"
        echo "$sandbox_rail" | head -20
        return 1
    fi
    assert_directory_exists "../$dirname" || return 1
    return 0
}

# #812: the `-x` sequence is planned onto the creation rail and runs inside
# the region's lifetime. None of that is reachable from the YAML suite —
# `commit_plan` early-returns off Interactive and the runner captures stderr,
# so every scenario there exercises the off-rail `Executing:` record instead.
# These are the only tests that execute `run_on_rail` itself: row resolution
# against the committed plan, the suspend/redraw seam around a child that owns
# the terminal, and the footer's verdict.
#
# SHELL is pinned to one daft does not alias-capture for (`ShellKind::from_path`
# knows only bash/zsh, and returns None otherwise, short-circuiting the whole
# snapshot path). The snapshot lives under `dirs::cache_dir()`, which no
# DAFT_*_DIR override redirects — capturing here would write the developer's
# real cache directory.
_exec_rail_daft() {
    _rail_daft SHELL=/bin/sh "$@"
}

# Two commands: an `exec` anchor, one row each, labelled as typed, and the
# commands' own output above a Ready footer.
test_start_exec_rows_on_rail() {
    local remote_repo=$(create_test_remote "test-repo-exec-rail" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-exec-rail/main" || return 1

    local log="$TEMP_BASE_DIR/exec-rail.log"
    _exec_rail_daft "$PTY_RUN" "$log" \
        daft start feat-exec -x 'echo first-command' -x 'echo second-command' || return 1

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "├─ exec"; then
        log_error "exec section anchor missing from the rail"
        return 1
    fi
    if ! echo "$clean" | grep -q "echo first-command"; then
        log_error "first -x row missing (label is the command as typed)"
        return 1
    fi
    if ! echo "$clean" | grep -q "echo second-command"; then
        log_error "second -x row missing — identical rows must not collapse"
        return 1
    fi
    # The commands really ran, not just got planned.
    if ! echo "$clean" | grep -q "^first-command"; then
        log_error "first command's own output missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "^second-command"; then
        log_error "second command's own output missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready in"; then
        log_error "Ready footer missing after a clean sequence"
        return 1
    fi
    assert_directory_exists "../feat-exec" || return 1
    return 0
}

# A failing `-x` fails its own row, marks the rest not-run, and still lands a
# worktree — the footer says so rather than claiming the creation failed.
test_start_exec_failure_on_rail() {
    local remote_repo=$(create_test_remote "test-repo-exec-fail" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-exec-fail/main" || return 1

    local log="$TEMP_BASE_DIR/exec-fail.log"
    # No `|| return 1`: a failing `-x` propagates its status, which is the
    # behavior under test. pty_run.py exits with the child's status.
    _exec_rail_daft "$PTY_RUN" "$log" \
        daft start feat-broken -x 'echo ran-first' -x 'false' -x 'echo never-runs'

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "exit 1"; then
        log_error "failing row missing its exit status"
        return 1
    fi
    if ! echo "$clean" | grep -q "not run"; then
        log_error "commands after the failure must resolve as not run, not vanish"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready with failures in"; then
        log_error "footer must report failures without claiming creation failed"
        return 1
    fi
    if echo "$clean" | grep -q "^never-runs"; then
        log_error "the sequence continued past a failure"
        return 1
    fi
    # The worktree is the point: a failed command must not undo it.
    assert_directory_exists "../feat-broken" || return 1
    return 0
}

# A pasted multi-line command is ordinary input for `-x`. Its label is
# flattened to one line so the region's line accounting and its shared
# annotation column both survive it (#751's failure mode, different door).
test_start_exec_multiline_label_stays_one_row() {
    local remote_repo=$(create_test_remote "test-repo-exec-multiline" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-exec-multiline/main" || return 1

    local log="$TEMP_BASE_DIR/exec-multiline.log"
    _exec_rail_daft "$PTY_RUN" "$log" \
        daft start feat-multiline -x $'echo alpha\necho beta' || return 1

    local clean
    clean=$(_rail_clean "$log")
    # Both halves of the label on one line, in order.
    if ! echo "$clean" | grep -q "echo alpha echo beta"; then
        log_error "multi-line -x label was not flattened onto one row"
        return 1
    fi
    # One command is one row, with no section to anchor.
    if echo "$clean" | grep -q "├─ exec"; then
        log_error "a lone -x command must not get a section anchor"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready in"; then
        log_error "rail did not close cleanly after a multi-line label"
        return 1
    fi
    assert_directory_exists "../feat-multiline" || return 1
    return 0
}

# ===========================================================================
# remove on the rail (from test_branch_delete.sh)
# ===========================================================================

# --- #813: the rail header names the worktree, not the path argument ---

# Run daft with the live rail enabled (pattern from test_checkout.sh): the
# framework's DAFT_TESTING hides the interactive region — the subject of
# these tests — so it comes off and the background spawns it suppresses are
# turned off individually. TERM is pinned because indicatif hides its whole
# draw target under an unset or `dumb` TERM, as on a bare CI runner.
_bd_rail_daft() {
    env -u DAFT_TESTING \
        TERM=xterm-256color \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# Strip ANSI and split the pty's carriage-returned repaints into lines.
_bd_rail_clean() {
    sed -e 's/\x1b\[[0-9;]*[a-zA-Z]//g' -e 's/\r/\n/g' "$1"
}

# The planning face is the *first* thing painted, before any plan commits.
# Grepping the whole log would pass on the committed header alone (which was
# always right), so the assertion has to be pinned to the first `Removing`
# line the terminal ever saw.
_bd_first_removing_line() {
    _bd_rail_clean "$1" | grep -o 'Removing [^ ]*' | head -1
}

# #813: `daft remove .` announced "Removing ." — the raw argument in the
# slot that carries identity. The committed header resolved it, but the
# planning face is what the user reads first, and on the runs that never
# commit a plan it is all they read.
test_remove_dot_header_names_branch() {
    local remote_repo=$(create_test_remote "test-repo-bd-hdr" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-bd-hdr"
    local project_root
    project_root=$(pwd -P)

    git-worktree-checkout -b feature/header || return 1
    cd "feature/header"

    local log="$project_root/remove-dot-rail.log"
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-hdr.XXXXXX")
    DAFT_CD_FILE="$cd_file" _bd_rail_daft "$PTY_RUN" "$log" daft remove . >/dev/null 2>&1
    rm -f "$cd_file"

    local first
    first=$(_bd_first_removing_line "$log")
    if [[ "$first" != "Removing feature/header" ]]; then
        log_error "first header line was '$first', expected 'Removing feature/header'"
        _bd_rail_clean "$log" | head -20
        return 1
    fi
    return 0
}

# #813: the header must agree with the error underneath it. A dirty worktree
# aborts during validation, so no plan ever commits and the seed is the only
# header the run will ever have — the case the PlanCommit replacement cannot
# reach.
test_remove_dot_header_survives_validation_failure() {
    local remote_repo=$(create_test_remote "test-repo-bd-hdr-fail" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-bd-hdr-fail"
    local project_root
    project_root=$(pwd -P)

    git-worktree-checkout -b feature/dirty-header || return 1
    cd "feature/dirty-header"
    echo "uncommitted" > scratch.txt

    local log="$project_root/remove-dirty-rail.log"
    _bd_rail_daft "$PTY_RUN" "$log" daft remove . >/dev/null 2>&1

    local first
    first=$(_bd_first_removing_line "$log")
    if [[ "$first" != "Removing feature/dirty-header" ]]; then
        log_error "first header line was '$first', expected 'Removing feature/dirty-header'"
        _bd_rail_clean "$log" | head -20
        return 1
    fi
    # The worktree must survive — this is a display change only.
    if [[ ! -d "$project_root/feature/dirty-header" ]]; then
        log_error "dirty worktree was removed"
        return 1
    fi
    return 0
}

# #813: the row's label promises a worktree, so its subject is the worktree —
# the branch it is for. It used to carry a path relative to the cwd, which
# for every `daft remove .` (the way you delete the worktree you are standing
# in) rendered as a bare `.`: the argument echoed back, naming nothing.
test_remove_dot_row_names_the_worktree() {
    local remote_repo=$(create_test_remote "test-repo-bd-row" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-bd-row"
    local project_root
    project_root=$(pwd -P)

    git-worktree-checkout -b feature/annotation || return 1
    cd "feature/annotation"

    local log="$project_root/remove-dot-row.log"
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-row.XXXXXX")
    DAFT_CD_FILE="$cd_file" _bd_rail_daft "$PTY_RUN" "$log" daft remove . >/dev/null 2>&1
    rm -f "$cd_file"

    # `✓  Removed worktree   <annotation>  (0.1s)`. Only the Done face spells
    # "Removed"; the pending/active faces say "Remove"/"Removing".
    local annotation
    annotation=$(_bd_rail_clean "$log" |
        grep -o 'Removed worktree  *[^ ]*' | head -1 |
        sed 's/Removed worktree  *//')
    if [[ "$annotation" != "feature/annotation" ]]; then
        log_error "removal row annotated '$annotation', expected 'feature/annotation'"
        _bd_rail_clean "$log" | head -20
        return 1
    fi
    return 0
}

# #813 do-not-regress: an argument that resolves to nothing must still be
# echoed exactly as typed. Replacing an unresolvable path with a guess is
# worse than showing the path.
test_remove_unresolvable_path_echoes_verbatim() {
    local remote_repo=$(create_test_remote "test-repo-bd-hdr-miss" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-bd-hdr-miss/main"
    local log="$PWD/remove-miss-rail.log"

    _bd_rail_daft "$PTY_RUN" "$log" daft remove ../feature/nope >/dev/null 2>&1

    local first
    first=$(_bd_first_removing_line "$log")
    if [[ "$first" != "Removing ../feature/nope" ]]; then
        log_error "first header line was '$first', expected 'Removing ../feature/nope'"
        _bd_rail_clean "$log" | head -20
        return 1
    fi
    return 0
}

# ===========================================================================
# the PR column on the sync and prune TUIs (from test_sync.sh / test_prune.sh)
# ===========================================================================

# The PR-column tests drive the *TUI* path (columns only render there), so
# daft runs under the DSR-answering PTY (pty_run.py; bare script(1) leaves
# crossterm's cursor query unanswered).

# The PR column is default on the sync TUI (#127), with the same silent
# visibility gate as `daft list`: decorated from the forge-PR cache while
# healthy, removable with `--columns -pr`, and silently hidden once a
# refresh dies an auth death.
test_sync_pr_column_default_gated() {
    local remote_repo=$(create_test_remote "test-repo-sync-prcol" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-prcol"
    git-worktree-checkout -b feature-x || return 1

    # A GitHub-shaped remote makes the repo forge-capable; a fake gh seeds
    # the cache with PR #5 heading feature-x. Loud failure on unexpected calls.
    (cd main && git remote add forge https://github.com/acme/widget.git) || return 1
    local bin="$TEMP_BASE_DIR/sync-prcol-bin"
    mkdir -p "$bin"
    cat > "$bin/gh" <<'GH'
#!/usr/bin/env bash
state=""; prev=""
for a in "$@"; do
  if [ "$prev" = "--state" ]; then state="$a"; fi
  prev="$a"
done
if [ "$1" = "pr" ] && [ "$2" = "list" ] && [ "$state" = "open" ]; then
  printf '%s' '[{"number": 5, "title": "Add feature five", "state": "OPEN", "headRefName": "feature-x", "isCrossRepository": false, "url": "https://github.com/acme/widget/pull/5", "author": {"login": "octocat"}, "statusCheckRollup": [{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"}]}]'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "list" ] && [ "$state" = "merged" ]; then printf '[]'; exit 0; fi
echo "unexpected gh call: $*" >&2
exit 3
GH
    chmod +x "$bin/gh"
    (cd main && PATH="$bin:$PATH" daft __refresh-forge) || return 1

    # Default: the PR column decorates feature-x from the cache.
    local log="$TEMP_BASE_DIR/sync-prcol.log"
    (cd main && PATH="$bin:$PATH" python3 "$PTY_RUN" "$log" git-worktree-sync) || return 1
    if ! grep -q "#5" "$log"; then
        log_error "default sync TUI must decorate feature-x with its PR (#5)"
        return 1
    fi

    # --columns -pr removes the column and its cells together.
    local log2="$TEMP_BASE_DIR/sync-prcol-minus.log"
    (cd main && PATH="$bin:$PATH" python3 "$PTY_RUN" "$log2" git-worktree-sync --columns=-pr) || return 1
    if grep -q "#5" "$log2"; then
        log_error "--columns -pr must drop the PR cells from the sync TUI"
        return 1
    fi

    # An auth-dead refresh flips persisted health: the default-sourced
    # column silently hides on the next run.
    cat > "$bin/gh" <<'GH'
#!/usr/bin/env bash
echo "To get started with GitHub CLI, please run:  gh auth login" >&2
exit 4
GH
    chmod +x "$bin/gh"
    (cd main && PATH="$bin:$PATH" daft __refresh-forge)
    local log3="$TEMP_BASE_DIR/sync-prcol-unhealthy.log"
    (cd main && PATH="$bin:$PATH" python3 "$PTY_RUN" "$log3" git-worktree-sync) || return 1
    if grep -q "#5" "$log3"; then
        log_error "an unhealthy forge must silently hide the default PR column"
        return 1
    fi

    return 0
}

# The PR column is default on the prune TUI (#127), decorated from the
# forge-PR cache. The full gate arc (removal, auth-death hiding) is covered
# on sync — prune shares the same resolution code path.
test_prune_pr_column_default() {
    local remote_repo=$(create_test_remote "test-repo-prune-prcol" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-prune-prcol"
    git-worktree-checkout -b feature-x || return 1

    (cd main && git remote add forge https://github.com/acme/widget.git) || return 1
    local bin="$TEMP_BASE_DIR/prune-prcol-bin"
    mkdir -p "$bin"
    cat > "$bin/gh" <<'GH'
#!/usr/bin/env bash
state=""; prev=""
for a in "$@"; do
  if [ "$prev" = "--state" ]; then state="$a"; fi
  prev="$a"
done
if [ "$1" = "pr" ] && [ "$2" = "list" ] && [ "$state" = "open" ]; then
  printf '%s' '[{"number": 5, "title": "Add feature five", "state": "OPEN", "headRefName": "feature-x", "isCrossRepository": false, "url": "https://github.com/acme/widget/pull/5", "author": {"login": "octocat"}, "statusCheckRollup": [{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"}]}]'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "list" ] && [ "$state" = "merged" ]; then printf '[]'; exit 0; fi
echo "unexpected gh call: $*" >&2
exit 3
GH
    chmod +x "$bin/gh"
    (cd main && PATH="$bin:$PATH" daft __refresh-forge) || return 1

    local log="$TEMP_BASE_DIR/prune-prcol.log"
    (cd main && PATH="$bin:$PATH" python3 "$PTY_RUN" "$log" git-worktree-prune) || return 1
    if ! grep -q "#5" "$log"; then
        log_error "default prune TUI must decorate feature-x with its PR (#5)"
        return 1
    fi

    return 0
}

# ===========================================================================
# alias capture under a controlling terminal (from test_worktree_exec.sh)
# ===========================================================================

# Run a command line with a freshly allocated pty as its controlling
# terminal. macOS/BSD and util-linux script(1) disagree on argv shape.
# The command line should tee its own output/exit code to files — BSD
# script's status propagation isn't relied on.
run_under_pty() {
    local cmdline="$1"
    if [[ "$(uname)" == "Darwin" ]]; then
        script -q /dev/null sh -c "$cmdline" < /dev/null > /dev/null 2>&1
    else
        script -q -e -c "$cmdline" /dev/null < /dev/null > /dev/null 2>&1
    fi
}

# Alias capture must survive a controlling terminal (#663 regression):
# an interactive capture shell whose session still holds daft's tty
# job-stops itself with the SIGTTIN foreground dance (bash -i
# force-opens /dev/tty; zsh likewise) unless capture detaches into its
# own session via the `daft __capture-aliases` setsid trampoline.
# Without the trampoline this burns the full 10s capture deadline and
# loses the aliases. CI runners have no tty, so the terminal is
# fabricated with script(1) — this is the only automated coverage of
# the production trampoline dispatch (unit tests substitute perl).
test_exec_alias_capture_under_tty() {
    if ! command -v script >/dev/null 2>&1; then
        log_success "script(1) unavailable — skipped"
        return 0
    fi

    local remote_repo
    remote_repo=$(create_test_remote "exec-alias-tty" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-alias-tty/main" || return 1

    # Fixture: isolated HOME with a .bashrc alias, plus a $SHELL wrapper
    # NAMED bash (ShellKind sniffs the basename) that pins rc lookup to
    # the fixture home even if the environment leaks.
    local fix="$TEMP_BASE_DIR/exec-alias-tty-fixture"
    rm -rf "$fix"
    mkdir -p "$fix/bin" "$fix/home"
    # The alias drops a marker in the worktree it runs in — file
    # assertions don't depend on renderer output layout.
    echo "alias daft_tty_probe='echo TTY_ALIAS_EXPANDED > \$PWD/tty-marker'" \
        > "$fix/home/.bashrc"
    cat > "$fix/bin/bash" <<EOF
#!/bin/sh
export HOME=$fix/home
exec bash "\$@"
EOF
    chmod +x "$fix/bin/bash"

    local out_file="$fix/exec.out" rc_file="$fix/exec.rc"
    # HOME/XDG_CACHE_HOME are scoped to the fixture so the capture cache
    # can't touch the real user cache; stdout goes to a file (keeps the
    # renderer plain) while the pty stays the controlling terminal —
    # which is all the foreground dance needs.
    local cmdline="env HOME=$fix/home XDG_CACHE_HOME=$fix/home/.cache SHELL=$fix/bin/bash \
git-worktree-exec --all -- daft_tty_probe > $out_file 2>&1; echo \$? > $rc_file"

    local t0=$SECONDS
    run_under_pty "$cmdline"
    local elapsed=$((SECONDS - t0))

    local rc
    rc=$(cat "$rc_file" 2>/dev/null)
    if [[ "$rc" != "0" ]]; then
        log_error "exec under pty exited ${rc:-<none>} (alias likely didn't resolve): $(cat "$out_file" 2>/dev/null)"
        return 1
    fi
    local repo_root
    repo_root="$(cd .. && pwd)"
    if ! grep -q "TTY_ALIAS_EXPANDED" "$repo_root/main/tty-marker" 2>/dev/null; then
        log_error "alias did not expand under a controlling tty: $(cat "$out_file" 2>/dev/null)"
        return 1
    fi
    # A stopped capture shell burns the whole 10s deadline before the
    # rc-less fallback runs — a healthy capture finishes in ~1s.
    if [[ $elapsed -gt 8 ]]; then
        log_error "exec under pty took ${elapsed}s — capture deadline burned (tty stop?)"
        return 1
    fi

    log_success "alias capture survived a controlling terminal (${elapsed}s)"
    return 0
}

# Run all PTY-bound tests
run_rail_pty_tests() {
    log "Running blessed PTY integration tests..."

    # Rail behavior under a PTY (#782)
    run_test "go_fetch_hop_no_rail_receipt" "test_go_fetch_hop_no_rail_receipt"
    run_test "go_fetch_on_rail_expands" "test_go_fetch_on_rail_expands"

    # Rail header names the sandbox, not the spelling (#813)
    run_test "go_sandbox_header_names_dirname" "test_go_sandbox_header_names_dirname"
    run_test "go_sandbox_header_names_the_sandbox_it_lands_in" "test_go_sandbox_header_names_the_sandbox_it_lands_in"
    run_test "go_sandbox_rows_name_the_sandbox" "test_go_sandbox_rows_name_the_sandbox"
    run_test "go_row_names_the_worktree_not_its_path" "test_go_row_names_the_worktree_not_its_path"
    run_test "start_row_names_the_worktree_not_its_path" "test_start_row_names_the_worktree_not_its_path"

    # `-x` rows on the creation rail under a PTY (#812)
    run_test "start_exec_rows_on_rail" "test_start_exec_rows_on_rail"
    run_test "start_exec_failure_on_rail" "test_start_exec_failure_on_rail"
    run_test "start_exec_multiline_label_stays_one_row" "test_start_exec_multiline_label_stays_one_row"

    # Interrupted -x keeps the cd redirect (#811)
    run_test "go_exec_interrupt_keeps_cd" "test_go_exec_interrupt_keeps_cd"
    run_test "go_exec_interrupt_quiet_keeps_cd" "test_go_exec_interrupt_quiet_keeps_cd"

    # `daft remove` rail header names the worktree, not the argument (#813)
    run_test "remove_dot_header_names_branch" "test_remove_dot_header_names_branch"
    run_test "remove_dot_header_survives_validation_failure" "test_remove_dot_header_survives_validation_failure"
    run_test "remove_dot_row_names_the_worktree" "test_remove_dot_row_names_the_worktree"
    run_test "remove_unresolvable_path_echoes_verbatim" "test_remove_unresolvable_path_echoes_verbatim"

    # The forge PR column on the TUIs (#127)
    run_test "sync_pr_column_default_gated" "test_sync_pr_column_default_gated"
    run_test "prune_pr_column_default" "test_prune_pr_column_default"

    # Alias capture under a controlling terminal (#663)
    run_test "exec_alias_capture_under_tty" "test_exec_alias_capture_under_tty"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_rail_pty_tests
    print_summary
    exit $?
fi
