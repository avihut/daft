---
title: Unilateral daft adoption
description:
  Use daft hooks and worktree automation on any repo without committing a
  daft.yml — personal visitor configuration that survives the full development
  lifecycle.
pillars: [worktrees, hooks]
---

# Unilateral daft adoption

## Starting state

You're working on a team repo. Your workflow:

```
git clone git@github.com/org/api-service.git
cd api-service
nvm use
npm install
cp .env.example .env
# edit .env by hand
```

The README has a "First time setup" section. You follow it on every clone. When
you check out a feature branch and install comes up clean but something is
missing, it's because you forgot a step.

The team has discussed `daft.yml` and decided not to adopt it — too much process
overhead for a team this size, or you're a contractor without write access, or
the repo is public and you'd rather not put your personal hooks into the project
history.

The reach for daft: run the automation without committing anything. You want
hooks that set up each worktree for you, without asking for anyone's permission.

## What changes

- A `daft.yml` is created at the repo root — untracked, ignored via
  `.git/info/exclude` (the per-clone, never-committed mechanism).
- Daft treats the file as a **visitor configuration**: same schema, same loader,
  same hook execution. The only difference is the git tracking state.
- When you branch out, daft copies the file into the new worktree before your
  hooks run. When you merge, daft resolves visitor configs atomically. The file
  travels with you without ever touching the commit graph.

## Recipe

**1. Install the starter config.**

```bash
daft install
```

This writes a commented `daft.yml` skeleton at the repo root and exits. It
refuses if `daft.yml` already exists (the repo already has a team config — use
`daft.local.yml` for personal overrides instead).

**2. Add `daft.yml` to your per-clone ignore list.**

`.git/info/exclude` works like `.gitignore` but is never committed — right for a
personal file that should never appear in `git status` for teammates:

```bash
echo "daft.yml" >> .git/info/exclude
```

If you prefer an entry in `.gitignore`, that works too, but the addition would
show up as a tracked change that teammates see. `.git/info/exclude` is the
cleaner choice for a file you're deliberately keeping personal.

**3. Write your hooks.**

Open `daft.yml` and add the automation you want. A minimal setup hook:

```yaml
# daft.yml (untracked — visitor configuration)
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
      - name: copy-env
        run: "[ -f .env ] || cp .env.example .env"
```

The `copy-env` job guards with `[ -f .env ]` so re-running on an existing
worktree doesn't clobber environment edits.

**4. Trust the file and verify.**

```bash
daft hooks trust
daft hooks validate
```

`validate` parses the YAML and reports schema errors before you hit them at hook
time.

**5. Branch out and watch propagation.**

```bash
daft start feat/my-feature
```

Before the `worktree-post-create` hook fires, daft copies `daft.yml` (and
`daft.local.yml` if present) from your current worktree into the new one. The
new worktree's hooks run from the propagated copy — no manual copy step needed.

```
api-service.feat/my-feature/
├── daft.yml          # propagated from source worktree — same content
├── node_modules/     # populated by install-deps job
└── .env              # created by copy-env job
```

The file never appears in `git status` in either worktree. From the team's
perspective, nothing happened.

## Variants

By **ignore mechanism** — which file receives the `daft.yml` exclusion rule.

### `.git/info/exclude` (recommended)

```bash
echo "daft.yml" >> .git/info/exclude
```

Per-clone, never committed, never visible to teammates. Works for any git repo
without touching tracked content.

### `.gitignore`

```bash
echo "daft.yml" >> .gitignore
```

Committed if tracked, so the exclusion rule is visible in history and to
teammates who pull. Some teams are fine with this; others prefer to keep
personal tooling entirely invisible.

### `core.excludesFile` (global)

```bash
echo "daft.yml" >> ~/.gitignore_global
# (if not already set)
git config --global core.excludesFile ~/.gitignore_global
```

One ignore entry covers every repo on the machine. Use this if you plan to use
visitor configs across many repos and don't want per-clone setup.

## Promoting to a team baseline

If the team later decides to adopt daft, your visitor config is the natural
starting point. Merge it into a fresh tracked file:

```bash
# Create an empty tracked file
touch daft.yml.team
# Merge your visitor config into it
daft file merge daft.yml.team daft.yml --keep-source
mv daft.yml.team daft.yml
git add daft.yml
git commit -m "chore(daft): adopt team daft configuration"
```

`daft file merge` performs a recursive YAML merge where the source wins on
conflicts. `--keep-source` preserves your original visitor file so you can diff
and verify before committing. After the commit, `daft.yml` is tracked — daft
reclassifies it automatically on the next hook run.

::: warning Collision when pulling a tracked `daft.yml` into an existing visitor

If your visitor `daft.yml` is in place and someone else commits a tracked
`daft.yml` to the repo, a plain `git pull` will overwrite your file. Active
collision resolution (detection, prompts, and `daft pull`) is tracked in
[#493](https://github.com/avihut/daft/issues/493). Until that ships, run
`daft doctor` before pulling — it surfaces visitor-vs-tracked status so you know
to back up your file first.

:::

## Idempotency & safety

- `daft install` is idempotent — running it twice on a repo that already has
  `daft.yml` refuses and exits cleanly rather than overwriting.
- Visitor propagation copies are `merge_configs`-resolved, not blind copies. If
  the target worktree already has an untracked `daft.yml` with local edits, the
  source wins on conflicts but the target's content is the base — edits unique
  to the target are preserved.
- Before removing a worktree whose visitor config differs from the merge
  target's, daft prompts and suggests `daft file merge`. Pass `--force` to
  bypass if you genuinely don't care about the divergence.

## Where to next

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — the standard
  `worktree-post-create` pattern for dependency installation, equally usable
  with a visitor config.
- **[Trust & security](/hooks/trust-and-security)** — why `daft hooks trust` is
  required before hooks run and what the trust model guarantees.
- **[Hooks reference](/hooks/yaml-reference)** — full `daft.yml` schema for
  building out your visitor automation.
