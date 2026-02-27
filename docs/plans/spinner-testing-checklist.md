# Spinner Testing Checklist

Manual verification for the spinner feature added in `refactor/tui`.

## What to look for

- Spinner appears immediately after command starts
- Spinner text updates as the operation progresses (step messages)
- Spinner clears cleanly before result output (no ghost lines)
- No gap between spinner disappearing and output appearing
- Spinner does NOT appear in quiet mode (`-q`)
- Spinner does NOT appear when piping output (non-TTY)

## Commands

### Clone

- [ ] `daft clone <repo-url>` — spinner: "Cloning repository..."
- [ ] `git worktree-clone <repo-url>` — same behavior
- [ ] Clone with hooks — spinner clears before hook box appears

### Init

- [ ] `daft init` — spinner: "Initializing repository..."
- [ ] `git worktree-init` — same behavior

### Go (checkout existing)

- [ ] `daft go <branch>` — spinner: "Preparing worktree..."
- [ ] `git worktree-checkout <branch>` — same behavior

### Start (create new worktree)

- [ ] `daft start <branch>` — spinner: "Creating worktree..."
- [ ] `git worktree-checkout -b <branch>` — same behavior
- [ ] Start with hooks — spinner clears before hook box appears

### Prune

- [ ] `daft prune` — spinner: "Pruning stale branches..."
- [ ] `git worktree-prune` — same behavior
- [ ] Prune with nothing to prune — spinner clears, info message shown

### Fetch

- [ ] `daft fetch` — spinner: "Updating worktrees..."
- [ ] `git worktree-fetch` — same behavior

### Carry

- [ ] `daft carry <files>` — spinner: "Carrying changes..."

### Sync

- [ ] `daft sync` — phase 1 spinner: "Pruning stale branches..."
- [ ] `daft sync` — phase 2 spinner: "Updating worktrees..."
- [ ] `daft sync --rebase` — phase 3 spinner: "Rebasing worktrees..."

### Remove (branch delete)

- [ ] `daft remove <branch>` — spinner: "Deleting branches..."
- [ ] `daft remove -f <branch>` — same behavior
- [ ] `git worktree-branch -d <branch>` — same behavior
- [ ] `git worktree-branch -D <branch>` — same behavior
- [ ] Remove with hooks — spinner clears before hook box appears

### Rename

- [ ] `daft rename <old> <new>` — spinner: "Renaming branch..."
- [ ] `git worktree-branch -m <old> <new>` — same behavior
- [ ] `daft rename --dry-run <old> <new>` — NO spinner (instant)

### Adopt

- [ ] `daft adopt` — spinner: "Converting to worktree layout..."
- [ ] `git worktree-adopt` — same behavior
- [ ] `daft adopt --dry-run` — NO spinner (instant)

### Eject

- [ ] `daft eject` — spinner: "Converting to traditional layout..."
- [ ] `git worktree-eject` — same behavior
- [ ] `daft eject --dry-run` — NO spinner (instant)

## Edge cases

- [ ] Quiet mode (`-q`) — no spinner shown for any command
- [ ] Piped output (e.g., `daft prune | cat`) — no spinner shown
- [ ] Gitoxide notice — warning prints before spinner starts, no corruption
- [ ] Error during operation — spinner clears on early return (Drop safety net)
