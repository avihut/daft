---
title: Python/uv with mise + sops secrets
description:
  Threading declarative-envs, toolchain-bootstrap, and env-vars-&-secrets into a
  real Python project — mise for tool versions, uv for deps, sops for secrets,
  layered via direnv.
pillars: [worktrees, hooks]
---

# Python/uv with mise + sops secrets

## Starting state

An ML pipeline project:

```
ml-pipeline/
├── pyproject.toml         # uv-driven
├── uv.lock
├── mise.toml              # python = "3.11" pin (just the one tool)
├── src/ml_pipeline/
├── notebooks/             # exploratory work, hits the same dev DB
└── tests/
```

The current setup ritual:

1. `mise install` — fine, everyone has mise.
2. `uv sync` — works, but occasionally drifts the lockfile because nobody used
   `--frozen` in the README's setup line.
3. `cp .env.example .env`, then ask in Slack who has the staging DB password
   this week.
4. Tomorrow you forget you exported anything; your notebook talks to your
   laptop's empty Postgres; you spend 20 minutes wondering why a query returns
   zero rows.

When the team last rotated their DB password, someone kept committing notebook
output that included the old password in a connection error stacktrace; the team
spent an hour scrubbing git history.

The reach for daft: declarative tool versions and committed env defaults are
half-solved by mise already. The other half — the secrets — needs an automatic,
per-worktree fetch. Add sops + age, layer it underneath direnv, and "what
password is this week's" stops being a Slack question.

This walkthrough threads three patterns:

- **[Declarative envs](/recipes/declarative-envs)** — mise pins Python, ruff, uv
  versions and ships non-secret env defaults via `[env]`.
- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — `uv sync --frozen`
  per worktree creates `.venv` from `uv.lock`.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — sops decrypts to
  `.env`, direnv loads it on cd.

The interesting bit is the **layering**: declarative tool config and committed
env defaults co-exist with hook-fetched secrets, all loaded the moment you cd
into a worktree.

## Prerequisite: team sops + age setup (one-time)

sops is a per-team configuration, not a per-worktree daft step. Do it once
during onboarding; the daft hooks below assume it's done.

Each developer generates an age key and shares the public half:

```bash
brew install sops age
mkdir -p ~/.config/sops/age
age-keygen -o ~/.config/sops/age/keys.txt
grep 'public key' ~/.config/sops/age/keys.txt
# → public key: age1abc...
```

Whoever maintains the secret store collects everyone's public keys and writes
`.sops.yaml`:

```yaml
# .sops.yaml  (committed)
creation_rules:
  - path_regex: secrets\.enc\.env$
    age: >-
      age1abc..., age1def..., age1ghi...
```

…then re-encrypts `secrets.enc.env` with all current recipients:

```bash
echo 'API_KEY=real-key-here' > secrets.env
echo 'DATABASE_URL=postgres://prod-readonly:hunter2@db/app' >> secrets.env
sops --encrypt secrets.env > secrets.enc.env
rm secrets.env
git add .sops.yaml secrets.enc.env
git commit -m "chore: rotate secrets, add new dev"
```

The encrypted file is committed; the plaintext never is. From here on, the daft
hooks below decrypt automatically per-worktree.

## Step 1: declarative tool versions

Apply [Declarative envs](/recipes/declarative-envs) — pin Python, ruff, and uv
via mise, and add committed non-secret defaults:

```toml
# mise.toml
[tools]
python = "3.13"
ruff = "0.9"
uv = "0.5"

[env]
PYTHONUNBUFFERED = "1"
PYTHONDONTWRITEBYTECODE = "1"
ML_DATA_DIR = "{{ config_root }}/data"
```

```bash
git add mise.toml
git commit -m "chore: pin python, ruff, uv via mise; add env defaults"
```

mise's shell hook handles the cd-time activation; no daft involvement needed for
that. The `[env]` block exports non-secret defaults (buffering flags, the data
directory) — anything that's fine to commit. Real secrets stay out of
`mise.toml` entirely.

## Step 2: install Python deps with uv

Apply [Toolchain bootstrap](/recipes/toolchain-bootstrap):

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-mise-versions
        run: mise install

      - name: sync-deps
        run: uv sync --frozen
        needs: [install-mise-versions]
```

`mise install` materializes any missing Python/uv/ruff versions for this
worktree (idempotent — already-installed versions are skipped).
`uv sync --frozen` then creates `.venv/` inside the worktree and installs deps
from `uv.lock`. `--frozen` refuses to update the lockfile, which is exactly what
stops the lockfile drift the team had been hitting.

```bash
git add daft.yml
git commit -m "chore(daft): install python deps on worktree create"
git daft-hooks trust

daft start feature/scratch
python --version              # 3.13.x — mise activated it
.venv/bin/python -c "import sys; print(sys.path)"
```

## Step 3: decrypt secrets at hook time

Apply
[Env vars & secrets → sops + age](/recipes/env-vars-and-secrets#sops-age-encrypted-secrets-in-the-repo).
Add a decrypt job:

```yaml
# daft.yml — add to worktree-post-create
- name: decrypt-secrets
  run: |
    sops --decrypt secrets.enc.env > .env
    chmod 600 .env
  needs: [install-mise-versions]
  env:
    SOPS_AGE_KEY_FILE: ${HOME}/.config/sops/age/keys.txt
```

`chmod 600` restricts `.env` to the owning user. Add `.env` to `.gitignore` once
(never commit decrypted secrets).

Each worktree decrypts independently. Rotating secrets is "re-encrypt
`secrets.enc.env`, commit." Existing worktrees pick up the new values on the
next `daft hooks run worktree-post-create`; new worktrees get them
automatically.

::: warning Don't decrypt to a file readable by other users

`chmod 600` is not optional. Without it, anyone else on a shared machine can
read the worktree's secrets. See
[Anti-pattern: secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks)
for the broader security story.

:::

## Step 4: load `.env` into the shell with direnv

The decrypted `.env` is sitting on disk; you want its contents exported when you
cd into the worktree. Add a final hook job that wires direnv:

```yaml
- name: setup-direnv
  run: |
    if [ ! -f .envrc ]; then
      cat > .envrc <<'EOF'
      # Auto-loaded by direnv when entering this worktree
      dotenv .env
      EOF
    fi
    direnv allow .
  needs: [decrypt-secrets]
```

`dotenv .env` is a direnv directive that loads the file into the shell
environment. Combined with mise's `[env]`, the layered result on cd is:

| Source                                 | Contents                                  |
| -------------------------------------- | ----------------------------------------- |
| `mise.toml` `[env]`                    | Non-secret defaults (PYTHONUNBUFFERED, …) |
| `.env` (sops-decrypted, dotenv-loaded) | Real secrets (API_KEY, DATABASE_URL)      |

mise activates first (PATH + tool-version env), then direnv loads `.env`. The
two don't compete for the same keys, so order is fine.

## Final `daft.yml`

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: install-mise-versions
        run: mise install

      - name: sync-deps
        run: uv sync --frozen
        needs: [install-mise-versions]

      - name: decrypt-secrets
        run: |
          sops --decrypt secrets.enc.env > .env
          chmod 600 .env
        needs: [install-mise-versions]
        env:
          SOPS_AGE_KEY_FILE: ${HOME}/.config/sops/age/keys.txt

      - name: setup-direnv
        run: |
          if [ ! -f .envrc ]; then
            cat > .envrc <<'EOF'
            dotenv .env
            EOF
          fi
          direnv allow .
        needs: [decrypt-secrets]
```

Plus `mise.toml` for tool versions and committed defaults; plus `.sops.yaml` and
`secrets.enc.env` for encrypted secrets. All four files are committed; only the
decrypted `.env` is per-worktree and gitignored.

## What you got

Before:

- "Run this script with these env vars" — manual every time.
- Secrets shared via Slack DMs and `.env` files passed around.
- Each password rotation = a thread in #engineering and a stack trace in
  someone's notebook output that needed a history scrub.
- Onboarding meant hitting different bugs per machine because Python versions
  and env defaults drifted.

After:

- New dev sends their age public key, gets added to `.sops.yaml`, re-encrypts.
  The next `daft start` materializes a fully-configured worktree with the right
  Python, the right deps, and the right secrets — automatically.
- Rotating a secret is `sops --encrypt` and a commit. Existing worktrees re-run
  the hook to pick up; new ones get them on creation.
- "Only repros on Alex's machine" stops being a Python or env-default mismatch.

## Where to next

- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — variants for vault
  lookups (1Password, Bitwarden), per-job env, and the derived-value pattern.
- **[Anti-pattern: secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks)**
  — the failure modes when this pattern gets wired wrong (committed decrypts,
  secrets in `ps -e ww`, baked-into-image env vars).
- **[CI parity](/recipes/ci-parity)** — running these same hooks in CI, with
  CI-side secret injection (CI doesn't have age keys; the decrypt step gets
  skipped, secrets come from the CI provider's store).
