---
title: Merge gate parity
description:
  Make your pre-merge jobs the same checks your forge requires on a pull
  request, so a local merge predicts the PR result instead of guessing at it.
pillars: [worktrees, hooks]
---

# Merge gate parity

## Starting state

The repo's required checks live in a workflow file, and branch protection makes
them mandatory before anything reaches the default branch.

```yaml
# .github/workflows/pr.yml — abridged
jobs:
  fmt:
    steps:
      - run: cargo fmt --check
  clippy:
    steps:
      - run: cargo clippy --all-targets -- -D warnings
  test:
    steps:
      - run: cargo test --workspace
  integration:
    steps:
      - run: ./scripts/integration.sh
```

The ritual is push, open the PR, switch to something else, come back to a red X
on `clippy` for one unused import. Fix, push, wait again. Locally you run some
subset of those four commands from memory — whichever ones you remember, in
whatever form you remember them, which is rarely the form the workflow uses.

So the checks are real but they are not _here_. They exist only in a file that
runs somewhere else, and the only way to ask "would this pass?" is to push and
find out. Sooner or later someone lands a merge locally, sees green, and gets
the failure on the shared branch instead.

The reach for daft: make the answer available before the push, from the same
list of checks the forge will apply.

## What changes

The four checks become `pre-merge` jobs, and a `merge:` policy block makes their
result predictive rather than advisory: the merge must be a fast-forward, so the
tree the jobs tested is the tree that lands.

The workflow file keeps its checks. It has to — contributors who don't use daft
still need the forge to stop them, and branch protection is the only thing that
holds for everyone. What changes is that the local merge stops being a guess.

## Recipe

Both files run the same commands. Route each check through a task runner
(`mise run`, `make`, `just`, an npm script) so the two lists can be compared by
name instead of by reading two dialects of shell.

```toml
# mise.toml
[tasks.fmt]
run = "cargo fmt --check"
[tasks.clippy]
run = "cargo clippy --all-targets -- -D warnings"
[tasks.test]
run = "cargo test --workspace"
[tasks.integration]
run = "./scripts/integration.sh"
```

```yaml
# daft.yml
merge:
  ff: only # source must already contain the target's tip
  source_worktree: clean # and its worktree must exist, with no dirty files

hooks:
  pre-merge:
    jobs:
      - name: fmt
        run: mise run fmt
        root: "{merge_source_path}"
      - name: clippy
        run: mise run clippy
        root: "{merge_source_path}"
      - name: test
        run: mise run test
        root: "{merge_source_path}"
      - name: integration
        run: mise run integration
        root: "{merge_source_path}"
        tags: [deep]
```

```yaml
# .github/workflows/pr.yml
jobs:
  fmt:
    steps:
      - uses: actions/checkout@v4
      - uses: jdx/mise-action@v2
      - run: mise run fmt
  clippy:
    steps:
      - uses: actions/checkout@v4
      - uses: jdx/mise-action@v2
      - run: mise run clippy
  test:
    steps:
      - uses: actions/checkout@v4
      - uses: jdx/mise-action@v2
      - run: mise run test
  integration:
    steps:
      - uses: actions/checkout@v4
      - uses: jdx/mise-action@v2
      - run: mise run integration
```

One job per required check, named the same, invoking the same task. A check that
gains a flag gains it once, in `mise.toml`, and both sides pick it up.

`root: "{merge_source_path}"` runs each job in the source worktree, so the jobs
hit its warm build caches instead of rebuilding in the target.

`daft merge feature/api` now runs the same four checks the PR will, and refuses
to land while any of them is red. When one fails it names the invocation:

```bash
daft hooks jobs --last --hook pre-merge  # what ran, what failed, output inline
daft hooks jobs logs clippy              # the full log
```

The rule that keeps this true over time: **when you add, remove, or rename a
required check, change both files in the same commit.** A check that lives only
in the workflow makes the local merge a false green; one that lives only in the
gate blocks the people who merge through daft and no one else.

## Variants

By **forge** — the daft side is identical; what differs is where the required
list is declared.

### GitHub

Required checks are branch protection rules on the default branch, named after
the workflow jobs. The names in `daft.yml` should match the names in the
required-checks list, so a red job locally tells you which PR check would fail.

### GitLab

Merge request pipelines with `rules: [{ when: always }]` on each job, plus
"Pipelines must succeed" in the merge request settings. Same mapping: one
`pre-merge` job per pipeline job, same task-runner command.

## Keeping the gate fast

A gate that takes as long as CI gets skipped, and a skipped gate is worse than
no gate. Two levers, neither of which changes what the forge enforces:

Tag the slow checks and drop them for a quick iteration:

```bash
daft merge feature/api --skip-tag deep
```

Or scope a job to the files that actually changed, which CI cannot safely do
because it validates the whole tree:

```yaml
- name: integration
  run: mise run integration
  root: "{merge_source_path}"
  glob: ["src/**/*.rs", "scripts/integration.sh"]
  tags: [deep]
```

A job whose changed-file set comes back empty is skipped and recorded as such.
See
[Changed-file filters](/hooks/yaml-reference#changed-file-filters-glob-exclude-files)
for the pattern dialect.

## Idempotency & safety

**The gate does not replace branch protection.** It runs on the machine of
whoever is merging, and only when they merge through daft. Keep the forge checks
required; the gate is the local mirror, not the enforcement point.

**Skipping jobs never relaxes policy.** `--skip-tag` and `--skip-hooks` drop
jobs; the fast-forward and clean-source requirements still hold. Relaxing those
takes an explicit `--no-ff-only` or `--source-worktree any` on the invocation,
and daft announces it when you do.

**The gate re-checks itself before it lands.** If the source branch advances
while the jobs are running, the merge refuses rather than landing a tree no job
saw. Rebase and run it again.

::: warning Don't let the gate grow checks the forge doesn't have

A check that exists only in `pre-merge` blocks daft users and lets everyone else
through — the asymmetry reads as "daft is broken on my machine." If a check is
worth blocking a merge, add it to the required list too.

:::

## Where to next

- **[CI parity](/recipes/ci-parity)** — the other parity axis: running the same
  `daft.yml` setup hooks in CI, so "how this project builds" has one definition.
- **[Merging across worktrees](/worktrees/merging)** — merge styles, conflicts,
  cleanup, and the rest of the merge surface.
- **[Merge gate policy](/hooks/yaml-reference#merge-gate-policy)** — the
  `merge:` block schema and what each value refuses.
