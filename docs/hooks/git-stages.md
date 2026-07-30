---
title: Git stages
description:
  Run daft as your repository's git hooks manager — pre-commit, commit-msg,
  pre-push and the rest — from the same daft.yml your lifecycle hooks use.
---

# Git stages

Daft can be the thing git calls. Install once, and `pre-commit`, `commit-msg`,
`pre-push` and thirteen other stages run from the same `hooks:` block as your
worktree lifecycle hooks — same jobs, same parallelism, same trust model, same
`daft hooks jobs` history.

```yaml
hooks:
  pre-commit:
    parallel: true
    jobs:
      - name: format
        run: prettier --write {staged_files}
        glob: "*.{ts,tsx,md}"
        stage_fixed: true
      - name: lint
        run: eslint {staged_files}
        glob: "*.{ts,tsx}"

  pre-push:
    jobs:
      - name: test
        run: npm test
```

```
daft hooks install
```

## What installing does

Shims — ten-line `sh` scripts — go into the repository's hooks directory. Each
one does nothing but call `daft __hook <stage>`; no part of your config is baked
in, so editing `daft.yml` takes effect on the next commit with no reinstall.

Everything lives under `.git/`. Nothing is added to the tracked tree, which is
what makes trying daft as your hooks manager a reversible decision:

```
daft hooks uninstall
```

restores the repository byte for byte, including any hook the install displaced.

Shims are installed for **every** supported stage, not only the ones you have
defined. A stage with no definition costs a fast no-op (about 10 ms); the
alternative is a config change that silently does nothing until someone
remembers to reinstall.

### What install will not do

- **Overwrite a hook it does not recognise.** That file may be the only copy of
  somebody's work, so the stage is skipped and reported. `--force` backs it up
  and installs anyway.
- **Claim `core.hooksPath`.** That setting is winner-take-all, so pointing it at
  daft would silently shadow every hook daft does not manage. If something
  _else_ has already claimed it, install refuses — writing shims git will never
  look at is worse than refusing, because `hooks status` would look right and no
  gate would ever fire. `--force` unsets it, and uninstall restores it.

A hook from a manager daft recognises is moved aside to `<stage>.pre-daft` and
restored on uninstall — its own config regenerates it, so nothing is lost.

## Stages

| Stage                | Fires                                  | Default file list | Blocks on failure |
| -------------------- | -------------------------------------- | ----------------- | ----------------- |
| `pre-commit`         | Before a commit is created             | staged            | yes               |
| `pre-merge-commit`   | Before a merge commit                  | staged            | yes               |
| `prepare-commit-msg` | Before the message editor opens        | staged            | yes               |
| `commit-msg`         | After the message is written           | staged            | yes               |
| `post-commit`        | After the commit lands                 | —                 | no                |
| `applypatch-msg`     | `git am`: message check                | staged            | yes               |
| `pre-applypatch`     | `git am`: before applying              | staged            | yes               |
| `post-applypatch`    | `git am`: after applying               | —                 | no                |
| `pre-rebase`         | Before a rebase starts                 | —                 | yes               |
| `post-checkout`      | After a checkout                       | —                 | no                |
| `git-post-merge`     | After a merge completes                | —                 | no                |
| `pre-push`           | Before refs are sent                   | pushed            | yes               |
| `pre-auto-gc`        | Before automatic gc                    | —                 | yes               |
| `post-rewrite`       | After amend or rebase rewrites commits | —                 | no                |
| `sendemail-validate` | `git send-email`                       | —                 | yes               |
| `post-index-change`  | After the index changes                | —                 | no                |

"Blocks on failure" is what _git_ does with the exit code, and it is what the
`failMode` default follows — a stage git ignores defaults to `warn`, because
aborting there would invent a gate that does not exist. Override per stage:

```
git config daft.hooks.preCommit.failMode warn
```

**Why `git-post-merge`.** Daft already has a `post-merge` lifecycle hook that
fires after `daft merge`. One YAML key cannot mean two events, so git's stage
takes the qualified spelling. The file on disk is still `post-merge` — that name
is git's to choose.

## File lists

A lifecycle hook has one notion of "the files this operation touched". A git
stage has several, so a job says which it means:

| Placeholder       | Expands to                                                                        |
| ----------------- | --------------------------------------------------------------------------------- |
| `{files}`         | whatever the stage is about — staged for the commit family, pushed for `pre-push` |
| `{changed_files}` | exact synonym of `{files}`                                                        |
| `{staged_files}`  | what is staged for the commit in progress                                         |
| `{push_files}`    | what a push would send                                                            |
| `{all_files}`     | every tracked file                                                                |

Stages with no natural answer (`post-checkout`, `pre-rebase`) have no default: a
file-aware job there must name a source or bring its own `files:` command. "Lint
the whole tree on every checkout" has to be a choice, not something a config
falls into by omission.

Quoting is part of the placeholder:

```yaml
run: eslint {files} # each path shell-quoted, space-joined
run: grep -l TODO "{files}" # each path double-quoted
run: printf '%s\n' '{files}' # each path single-quoted
```

An expansion too long for one `exec` is split across several runs of the same
command, under one job row and one log stream, stopping at the first failure.
Without that, a `pre-commit` gate over a few thousand staged files fails with
`E2BIG` — which reads as "the gate stopped working".

## What git passes the hook

Every stage gets `DAFT_GIT_STAGE`. Beyond that, the arguments git supplies are
available two ways — as named variables, and as `{1}`, `{2}`, … positionals
matching git's own numbering, so a job written against any other hook manager
translates directly (`$1` becomes `{1}`).

| Stage                          | Variables                                                               |
| ------------------------------ | ----------------------------------------------------------------------- |
| `commit-msg`, `applypatch-msg` | `DAFT_COMMIT_MSG_FILE`                                                  |
| `prepare-commit-msg`           | `DAFT_COMMIT_MSG_FILE`, `DAFT_COMMIT_MSG_SOURCE`, `DAFT_COMMIT_MSG_SHA` |
| `pre-rebase`                   | `DAFT_REBASE_UPSTREAM`, `DAFT_REBASE_BRANCH`                            |
| `post-checkout`                | `DAFT_CHECKOUT_PREV_SHA`, `DAFT_CHECKOUT_NEW_SHA`, `DAFT_CHECKOUT_FLAG` |
| `git-post-merge`               | `DAFT_GIT_MERGE_SQUASH`                                                 |
| `pre-push`                     | `DAFT_PUSH_REMOTE`, `DAFT_PUSH_REMOTE_URL`, `DAFT_PUSH_REFS`            |
| `post-rewrite`                 | `DAFT_REWRITE_COMMAND`                                                  |

`pre-push` and `post-rewrite` also receive a payload on stdin. Daft has to read
it before any job can (a process cannot read its stdin twice), so it is
republished as a variable — and jobs that want it as a stream declare
`use_stdin: true`.

## Trust

The same gate as every other hook: a repository is untrusted until you say
otherwise, and an untrusted repository's stages do not run.

Untrusted stages **do not block**. A gate that refuses every commit in an
unfamiliar clone would be an obstacle, not a safeguard — so the skip is reported
and the operation proceeds. `daft hooks trust` turns them on, and
`daft hooks status` says which state you are in.

See [Trust & security](/hooks/trust-and-security).

## `daft push`

When daft manages `pre-push`, `daft push` runs the stage itself — as real job
rows, one per job, with the full output treatment — and then pushes with git's
own hook dispatch suppressed so nothing fires twice. The refs it hands the stage
are the exact block git would have written, so a definition behaves identically
whether daft or git invoked it.

Plain `git push` still works: the shim runs the same stage the same way.

## git-lfs

Installing displaces git-lfs's hook files, so daft calls `git lfs <stage>`
itself before running your jobs on the four stages LFS installs for. Without
that, large files silently stop uploading and the failure surfaces on somebody
else's clone days later. `skip_lfs: true` at the top level opts out for
repositories that wire LFS some other way.

## Escape hatches

- `git commit --no-verify` / `git push --no-verify` — git's own, unchanged.
- `DAFT_HOOKS=0` — disables daft's dispatch for every stage, for when a script
  invokes git several times and `--no-verify` cannot reach through it.
- `daft hooks run <stage>` — fire a stage by hand, without making a commit.

## Coming from another hooks manager

Daft can run your existing config as-is, and convert it when you are ready. See
[Migrating from lefthook](/hooks/lefthook-migration).

## Checking the state

```
daft hooks status
```

reports how many shims are installed, which config the stages come from, what
was displaced, and any hook present that daft does not manage.
