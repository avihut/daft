---
branch: daft-875/transform-carry-any-tree-state
---

# Layout transform carries any tree state (#875)

The YAML harness has no terminal, so the three prompts below can only be driven
by hand. Everything else is covered by `tests/manual/scenarios/layout/`.

## Prompts (interactive terminal, no `-y`)

- [ ] `daft layout transform sibling` in a bare repo whose default branch has no
      worktree and several candidates shows the picker ("Which worktree becomes
      the main working tree?") with branch / path / tree summary rows; `j`/`k`
      and arrows move, Enter picks, Esc prints "Transform cancelled; nothing was
      changed." and exits 130 with nothing moved.
- [ ] The same with `-y` does **not** show the picker — it refuses naming
      `--pivot <branch>`.
- [ ] `daft layout transform contained` on a detached main working tree prompts
      "Directory name for the detached main working tree" pre-filled with the
      12-hex derived name; a bad name (`a/b`, `..x`) is rejected inline; Enter
      nests under the typed name, still detached.
- [ ] The same with `-y` takes the derived name without prompting.
- [ ] `daft layout transform centralized` with `daft_data_dir` on another volume
      asks "Copy '<branch>' (<size>) to …?" with the copy-not-rename notice; `n`
      prints "Transform cancelled; nothing was changed." and exits 0; `y`
      copies, repairs, verifies, removes the source; `-y` skips the question.

## The verification bed

- [ ] `~/Projects/tax-analyzer` (branch `task/local-docker`, 5 modified, 1
      untracked, 0 staged, `node_modules/` + `.next/` ignored):
      `daft layout transform contained` prints exactly
      `main working tree on 'task/local-docker' → task/local-docker/ · 5 modified, 1 untracked carried along · 'master': no worktree`,
      asks nothing, and afterwards `git -C task/local-docker status --porcelain`
      is byte-identical to the capture in the issue, `git diff --cached` is
      empty, `refs/stash` is absent, `node_modules/` and `.next/` are inside
      `task/local-docker/` with the same inodes, `.git/index` is absent at the
      root and present under `.git/worktrees/<id>/`, `daft list` shows one
      worktree, and the shell is in `task/local-docker/`.
- [ ] `daft layout transform sibling` from there restores the identical status
      block at the repository root.
