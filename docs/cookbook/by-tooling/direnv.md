---
title: daft + direnv
description:
  Per-worktree env vars and secrets via direnv, automated by daft hooks.
pillars: [worktrees, hooks]
tooling: [direnv]
languages: []
---

# daft + direnv

> **Goal:** Each worktree exports its own env vars (DB URLs, API keys, feature
> flags) automatically when you `cd` in.

## Context

[direnv](https://direnv.net) reads `.envrc` per directory and exports its
contents into your shell. With daft, each worktree is a directory, so `.envrc`
is per-branch. A worktree-post-create hook seeds the `.envrc` from a template;
direnv's shell hook loads it on `cd`.

## Prerequisites

- daft installed and shell integration enabled
- direnv installed (`brew install direnv` on macOS)
- direnv's shell hook in your shell profile (`eval "$(direnv hook bash)"` or
  equivalent)

## Steps

### 1. Add a `.envrc.example` to the repo

```bash
cat > .envrc.example <<'EOF'
export DATABASE_URL="postgres://localhost/myapp_dev"
export API_KEY="set-me"
EOF
git add .envrc.example
git commit -m "chore: add .envrc template"
```

### 2. Add `.envrc` to `.gitignore`

```bash
echo ".envrc" >> .gitignore
git add .gitignore
git commit -m "chore: gitignore .envrc"
```

### 3. Add a `daft.yml` to seed `.envrc` per worktree

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: seed envrc
      run: |
        if [ ! -f .envrc ] && [ -f .envrc.example ]; then
          cp .envrc.example .envrc
          direnv allow .
        fi
```

Trust:

```bash
git add daft.yml
git commit -m "chore(daft): seed .envrc on worktree create"
git daft-hooks trust
```

### 4. Create a worktree

```bash
daft start feat/billing
```

`.envrc` is seeded from the template; direnv loads it on `cd`.

## Verifying it works

```bash
echo $DATABASE_URL    # postgres://localhost/myapp_dev
```

## Variations

### Per-branch overrides

After seeding, edit `.envrc` in the worktree to override values. The change
persists for that worktree (until you remove and recreate it).

### Sourcing secrets from a vault

Replace the static seed with a vault lookup in the post-create job. Example with
`1password`:

```yaml
- name: seed envrc from 1password
  run: |
    op inject -i .envrc.tpl -o .envrc
    direnv allow .
```

## Troubleshooting

- **direnv complains "blocked"** — direnv requires `direnv allow` per directory.
  The post-create job runs it automatically; if you edit `.envrc` later, run
  `direnv allow` again.
- **`.envrc` was not seeded** — check the worktree-post-create logs:
  `git daft-hooks log show`.

## Where to next

- **[mise](/cookbook/by-tooling/mise)** — tool versions per worktree (mise
  handles tools; direnv handles env vars)
- **[Hooks](/hooks/)** — what else can fire on worktree create
- **[copy_paths](https://github.com/avihut/daft/issues/387)** (planned) —
  replicate `.envrc` automatically across worktrees without a hook
