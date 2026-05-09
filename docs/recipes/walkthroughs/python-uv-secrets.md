---
title: Python/uv with mise + sops secrets
description:
  End-to-end daft setup for a Python project using uv for deps, mise for tool
  versions, and sops for secrets — declarative env layered with imperative
  hooks.
pillars: [worktrees, hooks]
---

# Python/uv with mise + sops secrets

This walkthrough sets up a modern Python project where each daft worktree gets:

1. The exact Python version and tool versions declared in `mise.toml`.
2. A per-worktree `.venv` populated by `uv sync --frozen`.
3. Sensitive env vars (API keys, DB credentials) decrypted from a committed sops
   file using the team's age keys.
4. Non-secret defaults declared in `mise.toml` `[env]`, layered with the
   sops-decrypted secrets via direnv.

This walkthrough is the middle ground between the two extremes: the heavy
infrastructure of
[Node monorepo with services](/recipes/walkthroughs/node-monorepo-services)
isn't here, but it's more involved than the
[Rust binary](/recipes/walkthroughs/rust-binary). The interesting bit is the
**layering**: declarative tool config and committed env defaults co-exist with
hook-fetched secrets.

## What you're building

A Python project with shape:

```
ml-pipeline/
├── pyproject.toml         # uv, drives `uv sync`
├── uv.lock
├── mise.toml              # Python version + committed env defaults
├── secrets.enc.env        # encrypted with sops
├── .sops.yaml             # routes encryption rules
├── src/ml_pipeline/
└── tests/
```

The team uses sops with [age](https://github.com/FiloSottile/age) keys — each
developer adds their public key to `.sops.yaml`, the encrypted file is
re-encrypted, and from then on every dev's worktrees can decrypt the secrets
locally.

## Patterns used

- **[Declarative envs](/recipes/declarative-envs)** — mise pins Python and ruff
  versions, and provides committed env defaults.
- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — `uv sync --frozen`
  per worktree creates `.venv` with locked deps.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — sops decrypts to
  `.env`, direnv loads it into the shell.

## Step 1: declarative tool versions

In the default-branch worktree, pin Python and any other CLI tools you need:

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

Commit:

```bash
git add mise.toml
git commit -m "chore: pin python, ruff, uv via mise"
```

mise's shell hook (`eval "$(mise activate zsh)"` in your shell rc) takes care of
activating these on `cd`. No daft involvement is needed for activation — only
for the install step in step 2.

The `[env]` block adds non-secret defaults: things you want every dev to have,
no need to encrypt or hide.

## Step 2: install Python deps with uv

Add the install hook:

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

`mise install` materializes any missing Python/uv/ruff versions for the worktree
(idempotent — already-installed versions are skipped). `uv sync --frozen` then
creates `.venv/` inside the worktree and installs deps from `uv.lock`.
`--frozen` refuses to update the lockfile, so worktrees can't drift.

```bash
git add daft.yml
git commit -m "chore(daft): install Python deps on worktree create"
git daft-hooks trust
```

Verify:

```bash
daft start feature/scratch
# In the new worktree:
python --version              # 3.13.x
.venv/bin/python -c "import sys; print(sys.path)"
```

## Step 3: encrypt the team's shared secrets with sops

Install sops and age (one-time per dev):

```bash
brew install sops age
```

Generate a personal age key (one-time per dev):

```bash
mkdir -p ~/.config/sops/age
age-keygen -o ~/.config/sops/age/keys.txt
# Print your public key to share with the team:
grep 'public key' ~/.config/sops/age/keys.txt
# → public key: age1...
```

Each dev sends their public key to whoever holds the secret store. That person
updates `.sops.yaml`:

```yaml
# .sops.yaml
creation_rules:
  - path_regex: secrets\.enc\.env$
    age: >-
      age1abc..., age1def..., age1ghi...
```

And re-encrypts the secrets file with all current age recipients:

```bash
echo 'API_KEY=real-key-here' > secrets.env
echo 'DATABASE_URL=postgres://prod-readonly:hunter2@db/app' >> secrets.env
sops --encrypt secrets.env > secrets.enc.env
rm secrets.env
git add .sops.yaml secrets.enc.env
git commit -m "chore: rotate secrets, add new dev"
```

The encrypted file is committed; the plaintext never is.

## Step 4: decrypt at hook time

Add the decryption job:

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

`chmod 600` restricts `.env` to the owner — visible to the local user only. Add
`.env` to `.gitignore` (never commit decrypted secrets).

Each worktree decrypts independently. If you rotate secrets, re-creating the
worktree (or re-running the hook with `daft hooks run worktree-post-create`)
picks up the new values.

::: warning Don't decrypt to a file readable by other users `chmod 600` is not
optional. Without it, anyone else on the machine can read the worktree's
secrets. See
[Anti-pattern: secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks)
for the broader security story. :::

## Step 5: load secrets into the shell with direnv

The decrypted `.env` is sitting on disk; you want its contents loaded into the
shell whenever you `cd` into the worktree. Add a final hook job that wires
direnv:

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

direnv's `dotenv` directive loads `.env` into the shell environment. Combined
with mise's `[env]` block, the layered result is:

| Source                                 | Contents                                            |
| -------------------------------------- | --------------------------------------------------- |
| `mise.toml` `[env]`                    | Non-secret defaults (PYTHONUNBUFFERED, ML_DATA_DIR) |
| `.env` (sops-decrypted, dotenv-loaded) | Real secrets                                        |

When you `cd` in:

1. mise activates Python 3.13 and exports the `[env]` defaults.
2. direnv loads `.env`, exporting the secrets.

Order is fine — `[env]` defaults are non-secret, and `.env` doesn't override
them.

## Step 6: verify it works

```bash
daft start feature/measure-secrets

# In the new worktree:
python --version              # 3.13.x — mise
echo $PYTHONUNBUFFERED        # 1 — mise [env]
echo $API_KEY                 # real-key-here — sops + direnv
ls -la .env                   # -rw------- (owner-only)

# Confirm Python sees the API key:
python -c "import os; print(os.environ['API_KEY'])"
```

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

Plus `mise.toml` for tool versions and committed env defaults; plus `.sops.yaml`
and `secrets.enc.env` for encrypted secrets. All four files are committed; only
the decrypted `.env` is per-worktree and gitignored.

## What you got

Before:

- "Run this script with these env vars" — manual every time.
- Secrets shared via Slack DMs and `.env` files passed around.
- New devs spent half a day getting onboarded, hitting different bugs per
  machine because Python versions or env defaults were slightly different.

After:

- New dev sends their age public key, gets added to `.sops.yaml`. The next
  `daft start` materializes a fully-configured worktree with the right Python,
  the right deps, and the right secrets.
- Secrets are committed (encrypted) — no separate dropbox to keep in sync.
- Rotating a secret is `sops --encrypt` and a commit, not a Slack thread.
- The same `daft.yml` applies to every worktree, every dev.

## Where to next

- **[Declarative envs](/recipes/declarative-envs)** — full reference for mise,
  asdf, nvm, pyenv and the division-of-labor table.
- **[Env vars & secrets](/recipes/env-vars-and-secrets)** — variants for vault
  lookups (1Password, Bitwarden), per-job env, and alternative secret stores.
- **[Anti-pattern: secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks)**
  — how secrets accidentally end up in repos, and how to avoid it.
- **[CI parity](/recipes/ci-parity)** — running these same hooks in CI, with
  CI-side secret injection (CI doesn't have age keys; secrets come from the CI
  provider's secret store).
