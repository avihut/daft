---
title: Env vars & secrets
description:
  Provision env vars and secrets per worktree — direnv, sops, mise env, vault
  lookups — and how daft hooks compose with them.
pillars: [worktrees, hooks]
---

# Env vars & secrets

> Different worktrees often need different env vars: a different `DATABASE_URL`
> per feature branch, a different `API_KEY` for staging vs prod scratch, ports
> that don't collide. Some of these are values that belong in version control;
> some are secrets that absolutely don't. Either way, you want them set
> automatically when you `cd` into a fresh worktree.

## When to reach for this

- Each worktree should run against its own DB, queue, or external service (so
  feature-A and feature-B don't fight over a shared dev environment).
- You have secrets (API keys, tokens) that need to be available to local
  processes but **never** committed to git.
- You want `cd ../feature-x` to "just work" — no manual export step, no
  forgotten variables.

## Minimal recipe

Seed an ignored `.envrc` from a committed template, then let
[direnv](https://direnv.net) auto-load it on `cd`:

```yaml
# daft.yml
hooks:
  worktree-post-create:
    jobs:
      - name: seed-envrc
        run: |
          if [ ! -f .envrc ] && [ -f .envrc.example ]; then
            cp .envrc.example .envrc
            direnv allow .
          fi
```

Repository setup:

```bash
# .envrc.example  (committed)
export DATABASE_URL="postgres://localhost/myapp_dev"
export API_KEY="set-me"

# .gitignore  (add)
.envrc
```

What happens: a fresh worktree gets a `.envrc` copied from the template, direnv
is told to trust it, and the next time you `cd` into the worktree the vars
export. You edit `.envrc` if you need per-worktree overrides; those edits stay
local to that worktree.

Prerequisites: direnv installed and its shell hook loaded
(`eval "$(direnv hook zsh)"` or equivalent in your shell rc).

## Variants

### direnv with a vault lookup

For real secrets, don't put them in `.envrc.example`. Have the hook fetch them
at create-time from a vault you trust:

```yaml
- name: seed-envrc-from-1password
  run: |
    op inject -i .envrc.tpl -o .envrc
    direnv allow .
```

`.envrc.tpl` is committed and contains `op://` references; `op inject` resolves
them. Same idea works with Bitwarden CLI (`bw`), AWS Secrets Manager
(`aws secretsmanager`), HashiCorp Vault, etc.

The advantage over committing encrypted secrets: revocation is centralized
(rotate in the vault, every new worktree gets the new value), and there's no
decryption key to manage locally.

### sops + age — encrypted secrets in the repo

[sops](https://github.com/getsops/sops) encrypts files with
[age](https://github.com/FiloSottile/age) or KMS keys. The encrypted file is
committed; decryption happens per-worktree at create time:

```yaml
- name: decrypt-secrets
  run: |
    sops -d secrets.enc.env > .env
  env:
    SOPS_AGE_KEY_FILE: ${HOME}/.config/sops/age/keys.txt
```

Repository:

```
secrets.enc.env       # committed, encrypted
.env                  # gitignored, decrypted per worktree
.sops.yaml            # routes encryption rules
```

This works well when the team shares an age recipient list — onboarding a new
dev means adding their public key to `.sops.yaml` and re-encrypting, not handing
over a vault token.

### mise's `[env]` section — declarative, no hook

mise (and asdf with the mise-compatible plugin) has a built-in env-var mechanism
that doesn't need a daft hook at all:

```toml
# mise.toml  (committed)
[env]
DATABASE_URL = "postgres://localhost/myapp_dev"
NODE_ENV = "development"

[env.development.SOPS_FILE]
file = ".env.sops"
```

When mise's shell hook activates the worktree, the env exports automatically.
The trade-off: `[env]` values are committed, so they're fine for non-secret
defaults but not for actual secrets. Combine with sops for the secret half.

See [Declarative envs](/recipes/declarative-envs) for the broader mise/direnv
comparison.

### Per-job env (no shell loading)

Sometimes you don't want vars in your shell — only in a specific hook job. Use
the job's `env:` field:

```yaml
- name: migrate
  run: ./scripts/migrate.sh
  env:
    DATABASE_URL: postgres://localhost/myapp_dev_${DAFT_BRANCH_NAME}
    LOG_LEVEL: debug
```

Variables in `env:` are exported only for that job's process, never to the
parent shell. Useful when a hook needs ad-hoc context that shouldn't leak.

### Per-worktree port via the branch name

Need every worktree to pick its own port without a config file? Derive it
deterministically from the branch:

```yaml
- name: seed-port
  run: |
    # Hash the branch name to a port in 30000-39999
    PORT=$((30000 + $(echo -n "$DAFT_BRANCH_NAME" | cksum | cut -d' ' -f1) % 10000))
    echo "export PORT=$PORT" >> .envrc
    direnv allow .
```

`$DAFT_BRANCH_NAME` is set by the lifecycle env (see
[Lifecycle hooks → Worktree](/hooks/lifecycle#worktree-creation-and-removal-hooks)).
This gives every worktree a stable, collision-free port without a central
registry.

For full service orchestration with port allocation and host wiring, see
[Services with ports](/recipes/services-with-ports).

## Idempotency & safety

The seed-from-template and decrypt-from-sops patterns are idempotent because
they check for the destination file before writing. Be careful with patterns
that **don't** check:

```yaml
# Bad: will overwrite per-worktree edits on every hook run
- name: seed-envrc
  run: cp .envrc.example .envrc
```

```yaml
# Better: only seed if missing
- name: seed-envrc
  run: |
    [ -f .envrc ] || cp .envrc.example .envrc
```

For vault-fetched secrets, decide whether to refresh on every hook run or only
when missing. Usually missing-only is right — secrets don't change frequently,
and a network call on every worktree create is slow.

## Composes well with

- **[Toolchain bootstrap](/recipes/toolchain-bootstrap)** — install comes first,
  env vars follow. Some installers (poetry, uv) read env vars themselves, so
  order matters: seed env, then install.
- **[Services with ports](/recipes/services-with-ports)** — port allocation
  lives here when it's the only env-var concern; in services with ports it's
  part of a richer compose-aware story.
- **[Declarative envs](/recipes/declarative-envs)** — the alternative or
  complement: mise/asdf for tool versions and committed env defaults, daft hooks
  for installs and secret fetching.
- **[CI parity](/recipes/ci-parity)** — running the same `daft.yml` in CI
  requires CI-side secret injection (different vault, different keys).

## Anti-patterns

- **[Secrets in version-controlled hooks](/recipes/anti-patterns/secrets-in-hooks)**
  — committing API keys to `daft.yml`, embedding tokens in a `.envrc.example`,
  or echoing secrets in hook output. Secrets get into the repo via easy
  mistakes; this page lists them.
- **`cp .envrc.example .envrc`** without a `[ -f .envrc ] ||` guard — destroys
  per-worktree edits on the next hook run.
- **Exposing secrets in `env:`** of a long-running background job — visible in
  `ps`, leaks into child processes. Decrypt to a file with restrictive
  permissions instead.

## See also

- **[Lifecycle hooks](/hooks/lifecycle)** — env vars passed to hooks, including
  `DAFT_BRANCH_NAME`, `DAFT_WORKTREE_PATH`
- **[Job orchestration](/hooks/job-orchestration)** — `env:`, `tags`, `only` /
  `skip` for conditional env-var work
- **[Trust & security](/hooks/trust-and-security)** — why `daft.yml` needs trust
  before secrets-touching jobs run
