---
title: daft-env
description: Print deterministic per-worktree env values (ports, names)
---

# daft env

Print deterministic per-worktree env values (ports, names)

## Description

Print deterministic per-worktree env values derived from the worktree's name.

Every value is a pure function of (salt, worktree slug, declaration) — no allocation, no registry, no daemon. The same inputs give the same answer on every machine, from every directory, even for a worktree that does not exist yet, so services in different worktrees (and different repos) can find each other with zero coordination: compute, don't communicate.

Ports hash the worktree to a contiguous block (default 16 ports in 20000-32767) and declared names take small offsets inside it, so one worktree's ports are consecutive and collisions between worktrees require whole-block hash collisions, which daft warns about. Undeclared names resolve in a disjoint ad-hoc region when the repo declares no env: schema; declaring one makes unknown names an error (--ad-hoc overrides).

The positional address is [repo:]VAR[@worktree] — 'daft env API_PORT', 'daft env backend:API_PORT' (that repo's worktree matching this one's name), 'daft env API_PORT@feat-x'. --repo/--worktree are the canonical spellings of the same qualifiers. Unlike other commands' bare --repo (which targets the default-branch worktree), env's cross-repo default is the worktree matching the current one's name: an address names a value's coordinate, not an execution target, and the matching-name convention is how one feature spans repos.

Derivation knobs (--salt, --range, --block-size, --offset) answer "what would it be": overriding them computes a hypothetical — useful to preview a salt change before committing it — and daft notes on stderr when the answer differs from the configured one.

Declare values under env: in daft.yml (see the yaml reference). Injection: hooks, tasks, and daft exec receive declared values automatically; shells via eval "$(daft env --export)" (direnv/mise); file-reading tools via daft env --write (dotenv). Injected values never override variables already set in the parent environment.

## Usage

```
daft env [OPTIONS] [VAR]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<VAR>` | Value address: VAR, repo:VAR, VAR@worktree, or repo:VAR@worktree. Omit to list every declared value for the addressed worktree | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--repo <NAME>` | Read another cataloged repository's values (canonical form of the `repo:` address prefix) |  |
| `--worktree <NAME>` | Address a specific worktree by name (canonical form of the `@worktree` address suffix). Works for worktrees that do not exist yet — the value is already determined |  |
| `--salt <SALT>` | Override the hash salt (default: `env.salt`, else the repo directory name). Answers "what would my values be under this salt" |  |
| `--range <START-END>` | Override the port range (default: `env.range`, else 20000-32767) |  |
| `--block-size <N>` | Override the ports-per-worktree block size (default: `env.block_size`, else 16) |  |
| `--offset <N>` | With VAR: compute the port at this offset in the worktree's block, regardless of what the schema declares |  |
| `--export` | Emit `export NAME='value'` lines for the shell (`eval "$(daft env --export)"`) |  |
| `--write <PATH>` | Write the declared values as a dotenv file. PATH defaults to `env.write` from daft.yml; `-` writes the dotenv text to stdout |  |
| `--ad-hoc` | Resolve an undeclared name to an ad-hoc port even though this repo declares an env: schema |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

