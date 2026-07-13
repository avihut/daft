---
branch: daft-664/feat/skill-install-and-freshness
---

# Agent Skill Distribution & Freshness

Inside the daft sandbox (`mise run sandbox`, a dev shell that sourced
`shared-env.sh`, or the YAML/bash test harnesses) the agent-skill root is
redirected to a sandbox dir via `DAFT_SKILLS_DIR`, so `daft skill install`,
`daft skill uninstall`, and `daft doctor --fix` stay off the real `~/.claude`
automatically — the default target below resolves to `$DAFT_SKILLS_DIR`, not
`~/.claude/skills`. Only when exercising a binary with no sandbox env (a release
build, or a system `daft` on `PATH` that ignores `DAFT_SKILLS_DIR`) do you need
to isolate `HOME` yourself (`HOME=$(mktemp -d) daft ...`).

## Install

- [ ] `daft skill` (bare) prints the group help with the embedded version in the
      header; `daft skill bogus` suggests the real verbs and exits 1
- [ ] `daft skill install` in the sandbox creates
      `$DAFT_SKILLS_DIR/daft-worktree-workflow/SKILL.md` (the default target,
      redirected off `~/.claude`) and reports
      `Installed agent skill (vX.Y.Z) → ...`
- [ ] Re-running reports `Agent skill already up to date (vX.Y.Z) at ...` and
      leaves the file byte-identical
- [ ] After editing the stamp to an older version, re-running reports
      `Updated agent skill (vOLD → vNEW) → ...`
- [ ] After appending a line (same stamp), re-running reports
      `Refreshed agent skill (vX.Y.Z) → ...`
- [ ] `-q` suppresses the result line; exit code stays 0
- [ ] `daft skill install --project` from a worktree subdirectory writes to the
      worktree root's `.claude/skills/` and prints the commit-to-share notice on
      stderr
- [ ] `daft skill install --project` at the bare container root of a
      contained-layout repo errors with the cd-into-a-worktree tip
- [ ] `daft skill install --project` outside any repo errors; `--project --dir`
      together is a clap error (exit 2)
- [ ] `daft skill install --dir <path>` creates
      `<path>/daft-worktree-workflow/SKILL.md`
- [ ] `git daft skill install` works and renders hints in `git daft` form

## Show

- [ ] `daft skill show` in a terminal renders the skill (orange headers) and
      pages it; `q` quits
- [ ] `daft skill show --no-pager` in a terminal renders but does not page
- [ ] `daft skill show | head` prints raw markdown (frontmatter first) and exits
      0 (no broken-pipe error) — the pipe is not a TTY, so no rendering, no
      pager
- [ ] `daft skill show > <root>/daft-worktree-workflow/SKILL.md` writes raw
      bytes; a following `daft skill install --dir <root>` reports already up to
      date (redirect is byte-identical to the embedded skill)

## Uninstall

- [ ] `daft skill uninstall --dir <root>` after an install removes SKILL.md and
      the now-empty daft-worktree-workflow directory, reporting
      `Removed agent skill (vX.Y.Z) → ...`
- [ ] Re-running reports `No agent skill installed at ... (nothing to remove)`
      and exits 0
- [ ] With a user file left beside the skill, uninstall removes SKILL.md but
      keeps the directory and that file, and prints a stderr notice that it kept
      the directory because other files remain
- [ ] A foreign SKILL.md (frontmatter name is not daft-worktree-workflow) is
      refused with an error and left in place
- [ ] `daft skill uninstall --project` removes the current worktree's copy;
      `--project --dir` together is a clap error (exit 2)
- [ ] Bare `daft skill` help lists `uninstall`; completions offer it
      (bash/zsh/fish); `daft skill install`'s `show` completion offers
      `--no-pager`

## Doctor freshness

- [ ] Fresh sandbox (empty `$DAFT_SKILLS_DIR`): `daft doctor` shows no Agent
      skill row; `daft doctor -v` shows `[−] Agent skill — not installed`
- [ ] After install: `[✓] Agent skill (installed, vX.Y.Z)`
- [ ] Stamp aged: warning that the installed copy is stale (vOLD vs the binary's
      vNEW), with the `daft skill install` suggestion
- [ ] Stamp removed entirely: warning (`no daft_version stamp`)
- [ ] Stamp newer than the binary (e.g. 999.0.0): `[✓]` with the
      consider-upgrading-daft note, and `--fix` does NOT touch it
- [ ] `daft doctor --fix --dry-run` previews the rewrite with the target path;
      `daft doctor --fix` rewrites the file and the next run passes
- [ ] Project-level copy: `Agent skill (project)` row appears in the Repository
      section only when `<worktree>/.claude/skills/...` exists, and `--fix`
      repairs a stale one

## Content (rewritten SKILL.md)

- [ ] Frontmatter carries `daft_version` matching `daft --version`
- [ ] No `worktree-*` command teaching outside the Running daft recognition
      table (`rg 'daft worktree-' SKILL.md` hits only that section)
- [ ] The rejected-command fallback section names `daft --help` and
      `daft skill install`
- [ ] Spot-check must-not-lose content: all 8 merge pitfalls, job-fields YAML,
      `--skip-hooks` cascade, untrusted-replay workflow, `daft list` JSON field
      list, 14-row env-tool table

## Release stamping

- [ ] `cargo run -p xtask -- stamp-skill --version 9.9.9 --file <copy>` rewrites
      the stamp; re-running is idempotent; a file without frontmatter errors
- [ ] `cargo test --lib skill` fails if SKILL.md's stamp is hand-edited away
      from the Cargo.toml version (drift guard has teeth)

## Tripwire

- [ ] `mise run test:manual tests/manual/scenarios/skill` passes with the real
      `~/.claude` untouched (the suite fails loudly if any test writes it —
      verified by the state-guard wrapper)
