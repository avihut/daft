---
title: Layouts
description: Choose how worktrees are organized on disk
---

# Layouts

A layout controls where daft places worktrees relative to the repository. When
you clone or adopt a repository, the layout determines the directory structure
you work in.

daft ships with four built-in layouts. You can also define custom layouts using
templates.

## Built-in Layouts

### Contained

```
my-project/
├── .git/                    # Shared Git metadata
├── main/                    # Worktree for the default branch
├── feature/auth/            # Worktree for a feature branch
└── bugfix/login/            # Worktree for a bugfix branch
```

Worktrees live inside the repository directory as subdirectories. This is the
layout shown in the [Quick Start](/getting-started/quick-start) guide.

**Template:** `{{ repo_path }}/{{ branch }}`

**Best for:** Teams that want all branches grouped under one directory. Clean
and self-contained — the entire project is one folder.

### Sibling (default)

```
~/code/
├── my-project/              # Main checkout (default branch)
├── my-project.feature-auth/ # Worktree for a feature branch
└── my-project.bugfix-login/ # Worktree for a bugfix branch
```

Worktrees are placed next to the repository directory, named `<repo>.<branch>`.
The main checkout is a regular Git repository.

**Template:** `{{ repo }}.{{ branch | sanitize }}`

**Best for:** Gradual adoption. The main checkout looks and behaves like a
normal `git clone`. You get the benefits of worktrees without changing how the
primary directory works.

### Nested

```
my-project/
├── .git/                    # Regular Git directory
├── src/                     # Working files (default branch)
├── package.json
└── .worktrees/              # Hidden worktree directory
    ├── feature-auth/        # Worktree for a feature branch
    └── bugfix-login/        # Worktree for a bugfix branch
```

Worktrees are placed in a hidden `.worktrees/` subdirectory inside the
repository. The main checkout is a regular Git repository. daft automatically
adds `.worktrees/` to `.gitignore`.

**Template:** `{{ repo }}/.worktrees/{{ branch | sanitize }}`

**Best for:** Keeping worktrees out of sight. The repository looks normal from
the outside, and worktrees are tucked away in a hidden directory.

### Centralized

```
~/code/my-project/           # Main checkout (default branch)
├── .git/
├── src/
└── package.json

~/.local/share/daft/worktrees/my-project/
├── feature-auth/            # Worktree stored centrally
└── bugfix-login/            # Worktree stored centrally
```

Worktrees are stored in a central location under daft's
[XDG data directory](https://specifications.freedesktop.org/basedir-spec/latest/),
separate from the repository. The main checkout is a regular Git repository.

**Template:** `{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}`

**Best for:** Keeping your source directories clean. The repository itself has
no extra directories — worktrees live elsewhere entirely.

## Comparison

| Layout          | Where worktrees go               | Main checkout                   | Worktrees visible?          |
| --------------- | -------------------------------- | ------------------------------- | --------------------------- |
| **contained**   | Inside the repo directory        | Bare (no working files at root) | Yes, as subdirectories      |
| **sibling**     | Next to the repo directory       | Regular Git repo                | Yes, as sibling directories |
| **nested**      | Hidden `.worktrees/` inside repo | Regular Git repo                | Hidden (in `.worktrees/`)   |
| **centralized** | daft data directory (XDG)        | Regular Git repo                | No (stored elsewhere)       |

## Choosing a Layout

If you're new to daft, start with the default (**sibling**). It works like a
normal Git clone and adds worktrees alongside it — no surprises.

Switch to **contained** if you prefer all branches grouped under one directory,
or if your team uses it (check for a `layout` field in the repository's
`daft.yml`).

Use **nested** or **centralized** if you want worktrees out of the way.

You can change layouts at any time with
[`daft layout transform`](/cli/daft-layout#transform).

## Using a Layout

### At Clone Time

Pass `--layout` to choose a layout when cloning:

```bash
daft clone --layout contained git@github.com:user/my-project.git
daft clone --layout sibling git@github.com:user/my-project.git
```

Without `--layout`, daft uses the first available from:

1. The repository's `daft.yml` `layout` field (team convention)
2. Your global default
3. The built-in default (sibling)

On your very first clone, if none of these are configured, daft prompts you to
choose between sibling and contained. This prompt appears only once — subsequent
clones use whichever source is available, or fall back to sibling.

### Setting a Global Default

Set a default layout so you don't need `--layout` every time:

```bash
daft layout default contained
```

This writes to `~/.config/daft/config.toml`. To reset back to the built-in
default:

```bash
daft layout default --reset
```

### Team Convention via daft.yml

Add a `layout` field to your repository's `daft.yml` to recommend a layout for
everyone who clones:

```yaml
layout: contained

hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
```

This is checked during `daft clone`. If the repository specifies a layout, it's
used automatically — no prompt, no need for `--layout`.

## Checking Your Current Layout

See the resolved layout for the current repository:

```bash
daft layout show
```

Output shows the layout name, template, and where the setting came from:

```
contained  {{ repo_path }}/{{ branch }}  (daft.yml)
```

List all available layouts (built-in and custom):

```bash
daft layout list
```

## Switching Layouts

Convert an existing repository to a different layout:

```bash
daft layout transform contained
daft layout transform sibling
daft layout transform nested
```

daft handles everything automatically — moving worktrees to their new locations,
converting the internal Git structure when needed, and updating its records.
Your shell is moved to the new location of whatever branch you were on.

::: tip If any worktrees have uncommitted changes, the transform will warn you.
Use `--force` to proceed anyway. :::

## Custom Layouts

If the built-in layouts don't fit your workflow, define custom ones in
`~/.config/daft/config.toml`:

```toml
[layouts.my-team]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
```

Then use it by name:

```bash
daft clone --layout my-team git@github.com:user/my-project.git
daft layout default my-team
```

### Template Variables

| Variable              | Description                          | Example                                                                     |
| --------------------- | ------------------------------------ | --------------------------------------------------------------------------- |
| `{{ repo_path }}`     | Absolute path to the repository root | `/home/user/my-project`                                                     |
| `{{ repo }}`          | Repository directory name            | `my-project`                                                                |
| `{{ branch }}`        | Branch name (as-is)                  | `feature/auth`                                                              |
| `{{ daft_data_dir }}` | daft's XDG data directory            | `~/.local/share/daft` (Linux), `~/Library/Application Support/daft` (macOS) |

### Filters

| Filter     | Description                   | Example                               |
| ---------- | ----------------------------- | ------------------------------------- |
| `sanitize` | Replaces `/` and `\` with `-` | `feature/auth` becomes `feature-auth` |

Use `{{ branch | sanitize }}` when the branch name appears in a file path to
avoid creating nested directories from branch names like `feature/auth`.

### Path Resolution

- **Absolute paths** (`/home/user/...`) are used as-is
- **Home-relative paths** (`~/...`) expand to your home directory
- **Relative paths** are resolved from the parent of the repository directory

### Bare Override

Templates that start with `{{ repo_path }}/` automatically use a bare repository
structure (worktrees placed inside the repo need this). To override this for
custom layouts:

```toml
[layouts.my-layout]
template = "{{ repo_path }}/branches/{{ branch | sanitize }}"
bare = false  # or true
```

## How Layout Resolution Works

When daft needs to determine which layout to use, it checks these sources in
order:

| Priority | Source                          | Set by                  |
| -------- | ------------------------------- | ----------------------- |
| 1        | `--layout` CLI flag             | You, at clone/init time |
| 2        | Per-repo setting (`repos.json`) | `daft layout transform` |
| 3        | `daft.yml` `layout` field       | Repository maintainer   |
| 4        | Global default (`config.toml`)  | `daft layout default`   |
| 5        | Built-in default                | Always `sibling`        |

The first match wins. This means a team convention in `daft.yml` is respected
unless you explicitly override it with `--layout` or have already transformed
the repo to a different layout.

::: tip On your very first clone, if no layout is configured at any level, daft
prompts you to choose. The chosen layout is applied as if you had passed
`--layout`. This prompt appears only once. :::

## See Also

- [daft layout](/cli/daft-layout) — CLI reference for layout commands
- [Configuration](/guide/configuration#layout-settings) — Layout configuration
  options
- [Adopting Existing Repos](/guide/adopting-existing-repos) — Convert an
  existing repository
