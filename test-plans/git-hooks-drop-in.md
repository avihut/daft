---
branch: daft-468/git-hooks-drop-in
---

# Git hooks drop-in

Manual checks for daft as a git hooks manager. Every step runs in a scratch
repository — never this one.

Setup for each section:

```bash
WORK=$(mktemp -d /tmp/daft-hooks-XXXXXX)
export DAFT_CONFIG_DIR="$WORK/cfg" DAFT_STATE_DIR="$WORK/state" DAFT_DATA_DIR="$WORK/data"
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@test.com \
       GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@test.com
git init -q -b main "$WORK/repo" && cd "$WORK/repo"
```

## Native stages

- [ ] `daft hooks install` reports 16 stages and names the hooks directory
- [ ] The hooks directory has one file per stage, each executable
- [ ] A failing `pre-commit` job blocks `git commit`; the commit is absent from
      `git log`
- [ ] Fixing the cause lets the same commit through
- [ ] `git commit --no-verify` bypasses the gate
- [ ] `DAFT_HOOKS=0 git commit` bypasses it too
- [ ] Editing `daft.yml` changes behaviour on the next commit with no reinstall
- [ ] `daft hooks run pre-commit` fires the stage by hand without committing
- [ ] A `commit-msg` job sees the message file as `{1}` and as
      `$DAFT_COMMIT_MSG_FILE`
- [ ] A **failing `pre-commit` job marked `background: true` still blocks the
      commit**, and the run names the job it ran inline
- [ ] The same job under `post-commit` detaches instead — the commit returns
      immediately and the job shows up in `daft hooks jobs`
- [ ] `daft hooks run pre-commit` honors `background: true` (nothing is waiting
      on the verdict, so it detaches)
- [ ] `git-post-merge` in `daft.yml` fires on a real `git merge`; a plain
      `post-merge` key does **not** (it is daft's own lifecycle hook)

## Trust

- [ ] In an untrusted repository, a commit **succeeds** and the skip is reported
- [ ] `daft hooks trust` turns the gates on
- [ ] `daft hooks status` reports trust level, installed shims, and the source

## Install safety

- [ ] A hand-written `pre-push` hook survives install untouched and is reported
      as skipped
- [ ] `--force` backs it up to `pre-push.pre-daft` and installs
- [ ] A lefthook-style hook is backed up without `--force`
- [ ] `core.hooksPath` set elsewhere refuses the install with an explanation
- [ ] `--force` unsets it; `daft hooks uninstall` restores it
- [ ] Reinstalling twice reports every stage unchanged
- [ ] `daft hooks uninstall` restores every displaced hook byte for byte (`diff`
      the backups against the restored files)
- [ ] A shim replaced by hand after install is reported and left alone by
      uninstall

## File lists

- [ ] `{staged_files}` receives only staged paths, not the whole tree
- [ ] `git commit -a` (temporary index) still sees the right staged set — this
      is the `GIT_INDEX_FILE` case
- [ ] A deleted file does not reach a formatter's argv
- [ ] `glob:` narrows the list; a non-matching change skips the job
- [ ] `file_types: text` keeps a binary out of a formatter's argv
- [ ] `stage_fixed: true` puts a formatter's edits into the commit
- [ ] 5,000+ staged files run without `E2BIG` (chunked)

## pre-push

- [ ] `git push` fires `pre-push` with `$DAFT_PUSH_REFS` populated
- [ ] `daft push` runs the stage as first-class job rows and pushes once
- [ ] The stage does not fire twice under `daft push` (check job history)
- [ ] A failing `pre-push` stops the push; the remote is unchanged
- [ ] A delete-only push (`git push origin :branch`) does not fail on an empty
      file list
- [ ] `daft sync` with `daft.sync.pushHookStrategy=batched` falls back to
      per-branch and says so

## Takeover

- [ ] A repository with only a `lefthook.yml` takes over on install, and the
      output names the file as the source
- [ ] Its `pre-commit` gate blocks a bad commit
- [ ] A `glob:` in the older `commands:` map form is honoured — a job scoped to
      `*.rs` does **not** run on a `.md`-only commit
- [ ] `remotes:` is reported on install and on each firing run
- [ ] `min_version:` is reported once, not per stage
- [ ] `LEFTHOOK=0` disables the run; `LEFTHOOK_EXCLUDE=job` skips that job, and
      the skip is attributed to `LEFTHOOK_EXCLUDE` rather than `--skip-hooks`
- [ ] `daft hooks status` names the file its stages come from, and lists them
- [ ] `daft hooks trust` names that file before asking
- [ ] Adding any git stage to `daft.yml` flips the source to native
- [ ] A `lefthook.toml` reports the format rather than "no config found"

## Import

- [ ] `daft hooks import --dry-run` previews without writing
- [ ] `import` into an existing `daft.yml` preserves every comment and blank
      line
- [ ] A `daft.yml` that already has a `hooks:` block receives the entries
      **inside** it, at its own indentation — one `hooks:` key, not two
- [ ] Every field survives the conversion — `glob`, `stage_fixed`, `root` and
      the rest, not just `run`
- [ ] Custom hook names land in `tasks:` and run via `daft run <name>`
- [ ] `daft hooks validate` passes on the result — including when the source
      used the `commands:` map form (tasks reject it, so import rewrites it)
- [ ] `lefthook.yml` is left on disk, and the `git rm` is printed
- [ ] A shape the editor cannot read (`hooks: {…}` as a flow mapping) is
      declined with a pasteable snippet

## git-lfs

In a repository with LFS configured:

- [ ] `git lfs pre-push` still runs on push (large files upload)
- [ ] `skip_lfs: true` stops the chaining

## Performance

- [ ] A commit in a repository with shims but no matching stage adds under ~20
      ms per stage (`time daft __hook post-commit`)

## Cleanup

```bash
cd - && rm -rf /tmp/daft-hooks-*
```
