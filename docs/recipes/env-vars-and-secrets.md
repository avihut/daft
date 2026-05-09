---
title: Env vars & secrets
description:
  Per-worktree env vars and secrets — vault lookups, sops, per-job env —
  populated automatically on worktree create.
pillars: [worktrees, hooks]
---

# Env vars & secrets

## Starting state

Your team's "how do I get a working `.env`" process is verbal. Someone keeps
`.env.example` committed with placeholder values; new devs fill in the real ones
from a 1Password note that gets shared during onboarding. A working `.env` once
got DM'd to a contractor in Slack; that channel is still in scrollback and the
`DATABASE_URL` in it points at the (since-rotated) production read replica.

A dev once edited `.envrc` to flip `DATABASE_URL` to their local Postgres, then
committed it with a benign-sounding message. GitHub's secret scanner caught the
staging password they didn't realize was in the surrounding lines. Rotation took
an afternoon.

The reach for daft: env vars and secrets should be **automatic per-worktree**.
Populated when the worktree is created, gone when it's removed, never typed by
hand, never committed.

## What changes

A `worktree-post-create` job seeds the worktree's `.env` (or `.envrc`) from a
trusted source — a vault, a sops-encrypted file in the repo, or a committed
template that contains nothing sensitive. The on-disk artifact is per-worktree,
gitignored, and never touched manually.

What this page is **not** about: non-secret env defaults (a placeholder
`DATABASE_URL`, `LOG_LEVEL`, `NODE_ENV`). Those belong in
[Declarative envs → mise `[env]`](/recipes/declarative-envs#committed-env-defaults-mise-toml-env).
This page covers what you can't commit, plus per-worktree dynamic values.

## Recipe

The simplest case: a committed `.envrc.example` with **placeholder** values,
seeded into a per-worktree `.envrc` and trusted via direnv. This is the right
starting point when most of your env vars are non-secret defaults the dev edits
locally.

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: seed-envrc
        run: |
          if [ ! -f .envrc ]; then
            cp .envrc.example .envrc
            direnv allow .
          fi
```

Repository setup:

```bash
# .envrc.example  (committed — placeholders only, no real secrets)
export DATABASE_URL="postgres://localhost/myapp_dev"
export API_KEY="set-me"
```

```
# .gitignore  (add)
.envrc
```

What happens: a fresh worktree gets `.envrc` copied from the template, direnv is
told to trust it, and the next `cd` into the worktree exports the vars. The
`[ -f .envrc ]` guard makes the job idempotent — re-running the hook never
overwrites local edits.

Prerequisites: [direnv](https://direnv.net) installed and its shell hook loaded
(`eval "$(direnv hook zsh)"` or your shell's equivalent).

## Variants

By **source** — where the secret value comes from. The hook shape stays the
same; only how it gets the bytes differs.

### Vault lookup at hook time (1Password, Vault, Bitwarden)

For real secrets, don't put them in `.envrc.example`. Fetch them at
worktree-create time from a vault you trust:

```yaml
- name: seed-envrc
  run: |
    op inject -i .envrc.tpl -o .envrc
    direnv allow .
```

`.envrc.tpl` is the committed template — it contains `op://` references that
`op inject` resolves, so the vault path is in version control but the secret
value is not:

```bash
# .envrc.tpl  (committed)
export DATABASE_URL="$(op read 'op://daft-dev/staging-db/url')"
export API_KEY="$(op read 'op://daft-dev/staging-api/key')"
```

The same shape works with HashiCorp Vault (`vault kv get`), Bitwarden
(`bw get`), AWS Secrets Manager — anything with a CLI that reads a named secret.

The advantage over committing encrypted files: revocation is centralized. Rotate
in the vault, every new worktree picks up the new value automatically. Old
worktrees can re-run the hook to refresh.

### sops + age — encrypted secrets in the repo

[sops](https://github.com/getsops/sops) encrypts a file with
[age](https://github.com/FiloSottile/age) (or KMS) keys. The encrypted file is
committed; decryption happens per-worktree at create time:

```yaml
- name: decrypt-secrets
  run: |
    sops --decrypt secrets.enc.env > .env
    chmod 600 .env
  env:
    SOPS_AGE_KEY_FILE: ${HOME}/.config/sops/age/keys.txt
```

Repository:

```
secrets.enc.env       # committed, encrypted
.env                  # gitignored, decrypted per worktree
.sops.yaml            # routes encryption rules
```

`chmod 600` restricts the decrypted file to the owning user — a basic mitigation
against shared-machine readers.

This works well when the team shares an age recipient list. Onboarding a new dev
is "add their public key to `.sops.yaml`, re-encrypt, commit" — no separate
vault token to hand over.

### Per-job `env:` — no shell loading

Sometimes you don't want vars in your shell at all — only available to a
specific hook job. Use the job's `env:` field:

```yaml
- name: migrate
  run: ./scripts/migrate.sh
  env:
    DATABASE_URL: postgres://localhost/myapp_dev_${DAFT_BRANCH_NAME}
    LOG_LEVEL: debug
```

`env:` values export only to that job's process. They never reach the parent
shell, never appear in `.envrc`, never persist past the hook run. Useful when a
hook needs ad-hoc context that shouldn't leak to your interactive shell.

::: warning Don't expose secrets via `env:` on backgrounded jobs A long-running
background job has its env vars visible in `ps -e ww` to anyone on the machine.
Read-only env files (`chmod 600`) are fine; env vars on a backgrounded process
aren't. See
[Anti-pattern: secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks).
:::

## Per-worktree derived values

Some "env vars" aren't fetched, they're **computed** — per-worktree ports,
branch-derived database names, run IDs. The pattern is the same as the
seed-from-template recipe, but the source is `bash`, not a vault:

```yaml
- name: allocate-port
  run: |
    PORT=$((30000 + $(echo -n "$DAFT_BRANCH_NAME" | cksum | cut -d' ' -f1) % 10000))
    echo "export PORT=$PORT" >> .envrc
    direnv allow .
```

`$DAFT_BRANCH_NAME` is set by the lifecycle env (see
[Lifecycle hooks → Worktree](/hooks/lifecycle#worktree-creation-and-removal-hooks)).
Hashing it gives every branch a stable, collision-free port — no central
registry, no race conditions, and the same branch always lands on the same port.

For full service orchestration with port allocation and host wiring, see
[Services with ports](/recipes/services-with-ports), which builds on this
pattern.

## Idempotency & safety

Idempotent seeding requires a guard against overwriting local edits:

```yaml
# Wrong — overwrites .envrc on every hook run, destroys per-worktree edits
- name: seed-envrc
  run: cp .envrc.example .envrc

# Right — only seed if missing
- name: seed-envrc
  run: |
    [ -f .envrc ] || cp .envrc.example .envrc
```

For vault-fetched secrets, the same applies: refresh-on-every-run is slow (a
network call per worktree), and overwrites local debugging tweaks. Default to
seed-if-missing; force-refresh only when you deliberately want to pick up
rotated values (`daft hooks run worktree-post-create`).

## Where to next

- **[Declarative envs](/recipes/declarative-envs)** — for **non-secret**
  defaults (mise `[env]`), which is the half of "what gets exported on cd" that
  doesn't belong in this recipe.
- **[Services with ports](/recipes/services-with-ports)** — the next step when
  "per-worktree env" includes booting compose stacks on derived ports.
- **[Anti-pattern: secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks)**
  — the failure modes when a recipe like this gets wired wrong.
