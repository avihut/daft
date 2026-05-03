# Docs IA Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure daft's docs from a flat, narrative-less catchall into a
pillar-driven information architecture with Diátaxis quadrants honored inside
each pillar, including a top-level Cookbook section and the foundation for
future Networking and full git-hooks drop-in pillars.

**Architecture:** Reshape `docs/.vitepress/config.ts` (sidebar + top-nav + URL
rewrites), reorganize `docs/*.md` files into pillar directories (`worktrees/`,
`hooks/`, `reference/`, `cookbook/`, `about/`), split mixed-purpose pages into
single-purpose Diátaxis pages, write new Explanation pages (Worktrees Overview,
Hooks Overview with the boundaries thesis, Why daft, Comparison), add Cloudflare
`_redirects` to keep legacy URLs alive, and seed the Cookbook with structure
plus an initial set of recipes.

**Tech Stack:** VitePress 1.6.4, Bun (docs package manager), Biome (lint),
Cloudflare Pages (deploy), `mise run docs:site` (dev),
`mise run docs:site:build` (build).

**Spec:** `docs/superpowers/specs/2026-05-03-docs-ia-restructure-design.md`

**Tracking issue:** #398

**Out of scope (deferred to feature-paired tickets):** Merge hooks docs (#330),
full git-hooks drop-in docs (#468), Networking pillar content (#357), visual
rebrand (#467).

---

## How to use this plan

The plan is divided into 8 phases. Each phase is independently committable; some
phases include multiple commits. **Run `mise run docs:site:build` at the end of
every task** to confirm the build still passes. If a build fails, fix it before
committing.

**Verify once at the start of work** by running `mise run docs:site` and
confirming the docs site renders at `http://localhost:5173/`. Leave it running
in a terminal — VitePress hot-reloads on most changes.

**Recipe content (Phase 6) authoring strategy:** the plan creates the cookbook
_structure_ (sidebar entries, index page, recipe templates, frontmatter
contract) plus three anchor recipes (`mise`, `direnv`, `monorepo`). The
remaining recipes are stubbed with frontmatter + outline only — they're written
as follow-up commits on this branch, not in this plan. This keeps the plan
focused on the IA and lets recipes accrete naturally.

---

## File structure (post-migration)

```
docs/
├── index.md                          # Landing (light hero copy update)
├── getting-started/
│   ├── installation.md               # Kept
│   ├── quick-start.md                # Kept (worktree-arc Tutorial — light edits)
│   └── shell-integration.md          # Kept
├── worktrees/                        # ★ pillar
│   ├── index.md                      # NEW (Overview — replaces guide/worktree-workflow.md)
│   ├── layouts.md                    # MOVED + EDITED (was guide/layouts.md)
│   ├── adopting-existing-repos.md    # MOVED (was guide/adopting-existing-repos.md)
│   ├── multi-remote.md               # MOVED + EDITED (was guide/multi-remote.md)
│   ├── running-commands.md           # MOVED + RENAMED (was guide/running-commands-across-worktrees.md)
│   └── shortcuts.md                  # MOVED (was guide/shortcuts.md)
├── hooks/                            # ★ pillar
│   ├── index.md                      # NEW (Overview — boundaries thesis)
│   ├── lifecycle.md                  # NEW Reference (extracted from guide/hooks.md)
│   ├── yaml-reference.md             # NEW Reference (extracted from guide/hooks.md)
│   ├── job-orchestration.md          # NEW Explanation
│   ├── trust-and-security.md         # NEW Explanation
│   └── roadmap.md                    # NEW (stub linking #330, #468, future work)
├── cookbook/
│   ├── index.md                      # NEW (taxonomy + skeleton)
│   ├── by-tooling/
│   │   ├── mise.md                   # NEW (anchor recipe)
│   │   ├── direnv.md                 # NEW (anchor recipe)
│   │   ├── nvm.md                    # STUB (frontmatter + outline)
│   │   ├── pyenv.md                  # STUB
│   │   └── asdf.md                   # STUB
│   ├── by-language/
│   │   ├── node.md                   # STUB
│   │   ├── python.md                 # STUB
│   │   ├── rust.md                   # STUB
│   │   └── go.md                     # STUB
│   └── by-scenario/
│       ├── monorepo.md               # NEW (anchor recipe)
│       ├── fork-workflow.md          # STUB
│       └── ci-integration.md         # STUB
├── reference/
│   ├── configuration.md              # MOVED (was guide/configuration.md)
│   ├── output-formats.md             # MOVED (was guide/output-formats.md)
│   ├── agent-skill.md                # MOVED + RENAMED (was guide/claude-skill.md)
│   └── cli/                          # NOT MOVED on disk — vitepress rewrites surface docs/cli/* at /reference/cli/*
├── about/
│   ├── index.md                      # NEW (about hub, links to children)
│   ├── why-daft.md                   # NEW (parallel-dev thesis)
│   ├── glossary.md                   # NEW
│   ├── faq.md                        # NEW
│   ├── troubleshooting.md            # NEW
│   ├── comparison.md                 # NEW (vs git worktree, vs lefthook, vs gitup)
│   ├── networking-roadmap.md         # NEW (stub linking #357)
│   ├── contributing.md               # MOVED (was contributing.md at root)
│   └── changelog.md                  # MOVED (was changelog.md at root)
├── public/
│   └── _redirects                    # NEW (Cloudflare URL redirects for legacy paths)
└── .vitepress/
    └── config.ts                     # MAJOR EDIT (sidebar + nav + rewrites)
```

`docs/cli/` stays where it is on disk — xtask still generates there. VitePress
`rewrites` surfaces them under `/reference/cli/*` URLs.

`docs/guide/` is fully drained by the end of Phase 5; the empty directory is
then removed.

---

## Conventions used by every task

**Verify build:** every task ends with `mise run docs:site:build`. Output should
end with `build complete in ...s`. If you see "dead links," fix them before
committing.

**Commits:** conventional commits (`docs:`, `docs(scope):`). One commit per task
unless the task says otherwise.

**No emoji** anywhere per `CLAUDE.md`.

**Cross-link rule when moving a page:** if you move `docs/guide/X.md` to
`docs/Y/X.md`, also `grep -rln '/guide/X' docs/` and update every reference to
point at `/Y/X` (without the `.md` extension — VitePress uses clean URLs).

**Frontmatter rule** for every page: keep the existing `title:` and
`description:` fields. Pages without them get a one-line `description:` added.
(VitePress uses these for `<title>` and meta tags.)

---

## Phase 1 — Foundation

Sets up the new IA shape (sidebar, nav, URL rewrites, redirects) before any
content moves. After this phase, the new sidebar groups are visible but most
pages 404 — that's fine, the next phases populate them.

### Task 1.1: Create empty placeholder index pages so sidebar links resolve

**Files:**

- Create: `docs/worktrees/index.md`
- Create: `docs/hooks/index.md`
- Create: `docs/cookbook/index.md`
- Create: `docs/reference/index.md`
- Create: `docs/about/index.md`

- [ ] **Step 1: Create the five placeholder pages**

Each placeholder is a one-liner pointing at the upcoming real content. Replace
`<pillar>` with the actual pillar name (e.g., "Worktrees").

```markdown
---
title: <pillar>
description: <pillar> overview (under construction)
---

# <pillar>

This page is being built. See
[issue #398](https://github.com/avihut/daft/issues/398) for status.
```

For each of the five files, write the markdown above with the pillar/section
name substituted:

- `docs/worktrees/index.md`: title "Worktrees"
- `docs/hooks/index.md`: title "Hooks"
- `docs/cookbook/index.md`: title "Cookbook"
- `docs/reference/index.md`: title "Reference"
- `docs/about/index.md`: title "About"

- [ ] **Step 2: Verify build**

Run: `mise run docs:site:build` Expected: build succeeds.

- [ ] **Step 3: Commit**

```bash
git add docs/worktrees/index.md docs/hooks/index.md docs/cookbook/index.md docs/reference/index.md docs/about/index.md
git commit -m "docs: scaffold pillar directory placeholders"
```

### Task 1.2: Update VitePress config — top nav + sidebar + rewrites

**Files:**

- Modify: `docs/.vitepress/config.ts:210-358`

This is the largest single edit in the plan. The `themeConfig` block is replaced
wholesale. Other config keys (markdown, head, transformPageData) stay as-is.

- [ ] **Step 1: Add `rewrites` to the top-level config**

Add this key after `cleanUrls: true,` (around line 22). It surfaces
autogenerated CLI docs at the new URLs without moving files on disk.

```typescript
  rewrites: {
    'cli/:command.md': 'reference/cli/:command.md',
  },
```

- [ ] **Step 2: Replace the `nav` array**

Find `nav: [` (around line 214) and replace through the closing `],` with:

```typescript
    nav: [
      { text: "Worktrees", link: "/worktrees/" },
      { text: "Hooks", link: "/hooks/" },
      { text: "Cookbook", link: "/cookbook/" },
      { text: `v${version}`, link: "/about/changelog" },
      { text: "GitHub", link: "https://github.com/avihut/daft" },
    ],
```

- [ ] **Step 3: Replace the `sidebar` array**

Find `sidebar: [` (around line 225) and replace through the closing `],` with:

```typescript
    sidebar: [
      {
        text: "Getting Started",
        items: [
          { text: "Installation", link: "/getting-started/installation" },
          { text: "Quick Start", link: "/getting-started/quick-start" },
          {
            text: "Shell Integration",
            link: "/getting-started/shell-integration",
          },
        ],
      },
      {
        text: "Worktrees",
        items: [
          { text: "Overview", link: "/worktrees/" },
          { text: "Layouts", link: "/worktrees/layouts" },
          {
            text: "Adopting existing repos",
            link: "/worktrees/adopting-existing-repos",
          },
          { text: "Multi-remote", link: "/worktrees/multi-remote" },
          {
            text: "Running commands across worktrees",
            link: "/worktrees/running-commands",
          },
          { text: "Shortcuts", link: "/worktrees/shortcuts" },
          {
            text: "Recipes",
            link: "/cookbook/?pillar=worktrees",
          },
        ],
      },
      {
        text: "Hooks",
        items: [
          { text: "Overview", link: "/hooks/" },
          { text: "Lifecycle hooks", link: "/hooks/lifecycle" },
          { text: "Job orchestration", link: "/hooks/job-orchestration" },
          { text: "YAML reference", link: "/hooks/yaml-reference" },
          { text: "Trust & security", link: "/hooks/trust-and-security" },
          { text: "Roadmap", link: "/hooks/roadmap" },
          {
            text: "Recipes",
            link: "/cookbook/?pillar=hooks",
          },
        ],
      },
      {
        text: "Cookbook",
        items: [
          { text: "Overview", link: "/cookbook/" },
          {
            text: "By tooling",
            collapsed: false,
            items: [
              { text: "mise", link: "/cookbook/by-tooling/mise" },
              { text: "direnv", link: "/cookbook/by-tooling/direnv" },
              { text: "nvm", link: "/cookbook/by-tooling/nvm" },
              { text: "pyenv", link: "/cookbook/by-tooling/pyenv" },
              { text: "asdf", link: "/cookbook/by-tooling/asdf" },
            ],
          },
          {
            text: "By language",
            collapsed: false,
            items: [
              { text: "Node.js", link: "/cookbook/by-language/node" },
              { text: "Python", link: "/cookbook/by-language/python" },
              { text: "Rust", link: "/cookbook/by-language/rust" },
              { text: "Go", link: "/cookbook/by-language/go" },
            ],
          },
          {
            text: "By scenario",
            collapsed: false,
            items: [
              { text: "Monorepo", link: "/cookbook/by-scenario/monorepo" },
              { text: "Fork workflow", link: "/cookbook/by-scenario/fork-workflow" },
              { text: "CI integration", link: "/cookbook/by-scenario/ci-integration" },
            ],
          },
        ],
      },
      {
        text: "Reference",
        items: [
          { text: "Overview", link: "/reference/" },
          { text: "Configuration", link: "/reference/configuration" },
          { text: "Output formats", link: "/reference/output-formats" },
          { text: "Agent skill", link: "/reference/agent-skill" },
          {
            text: "CLI",
            collapsed: true,
            items: [
              {
                text: "Setup",
                items: [
                  { text: "clone", link: "/reference/cli/daft-clone" },
                  { text: "init", link: "/reference/cli/daft-init" },
                  { text: "adopt", link: "/reference/cli/daft-adopt" },
                ],
              },
              {
                text: "Branching",
                items: [
                  { text: "go", link: "/reference/cli/daft-go" },
                  { text: "start", link: "/reference/cli/daft-start" },
                  { text: "rename", link: "/reference/cli/daft-rename" },
                  { text: "remove", link: "/reference/cli/daft-remove" },
                ],
              },
              {
                text: "Maintenance",
                items: [
                  { text: "sync", link: "/reference/cli/daft-sync" },
                  { text: "prune", link: "/reference/cli/daft-prune" },
                  { text: "update", link: "/reference/cli/daft-update" },
                  { text: "carry", link: "/reference/cli/daft-carry" },
                  { text: "exec", link: "/reference/cli/daft-exec" },
                  { text: "eject", link: "/reference/cli/daft-eject" },
                  { text: "repo remove", link: "/reference/cli/daft-repo-remove" },
                ],
              },
              {
                text: "Utilities",
                items: [
                  { text: "list", link: "/reference/cli/daft-list" },
                  { text: "doctor", link: "/reference/cli/daft-doctor" },
                  { text: "release-notes", link: "/reference/cli/daft-release-notes" },
                  { text: "shell-init", link: "/reference/cli/daft-shell-init" },
                  { text: "completions", link: "/reference/cli/daft-completions" },
                  { text: "setup", link: "/reference/cli/daft-setup" },
                ],
              },
              {
                text: "Configuration",
                items: [
                  { text: "config", link: "/reference/cli/daft-config" },
                  { text: "hooks", link: "/reference/cli/git-daft-hooks" },
                  { text: "layout", link: "/reference/cli/daft-layout" },
                  { text: "multi-remote", link: "/reference/cli/daft-multi-remote" },
                ],
              },
              {
                text: "Git Commands",
                collapsed: true,
                items: [
                  {
                    text: "Setup",
                    items: [
                      { text: "worktree-clone", link: "/reference/cli/git-worktree-clone" },
                      { text: "worktree-init", link: "/reference/cli/git-worktree-init" },
                      { text: "flow-adopt", link: "/reference/cli/git-worktree-flow-adopt" },
                    ],
                  },
                  {
                    text: "Branching",
                    items: [
                      { text: "worktree-checkout", link: "/reference/cli/git-worktree-checkout" },
                      { text: "worktree-branch", link: "/reference/cli/git-worktree-branch" },
                      {
                        text: "worktree-branch-delete (deprecated)",
                        link: "/reference/cli/git-worktree-branch-delete",
                      },
                    ],
                  },
                  {
                    text: "Maintenance",
                    items: [
                      { text: "worktree-sync", link: "/reference/cli/git-worktree-sync" },
                      { text: "worktree-list", link: "/reference/cli/git-worktree-list" },
                      { text: "worktree-prune", link: "/reference/cli/git-worktree-prune" },
                      { text: "worktree-fetch", link: "/reference/cli/git-worktree-fetch" },
                      { text: "worktree-carry", link: "/reference/cli/git-worktree-carry" },
                      { text: "worktree-exec", link: "/reference/cli/git-worktree-exec" },
                      { text: "flow-eject", link: "/reference/cli/git-worktree-flow-eject" },
                    ],
                  },
                ],
              },
            ],
          },
        ],
      },
      {
        text: "About",
        items: [
          { text: "Why daft", link: "/about/why-daft" },
          { text: "Glossary", link: "/about/glossary" },
          { text: "FAQ", link: "/about/faq" },
          { text: "Troubleshooting", link: "/about/troubleshooting" },
          { text: "Comparison", link: "/about/comparison" },
          { text: "Networking roadmap", link: "/about/networking-roadmap" },
          { text: "Contributing", link: "/about/contributing" },
          { text: "Changelog", link: "/about/changelog" },
        ],
      },
    ],
```

- [ ] **Step 4: Build with allow-dead-links so it succeeds during migration**

Temporarily set `ignoreDeadLinks: true` (around line 21, change from `false` to
`true`). Add a comment:
`// TODO: revert to false in Task 8.4 once migration is complete`.

- [ ] **Step 5: Verify build**

Run: `mise run docs:site:build` Expected: build succeeds with warnings about
dead links (that's fine).

- [ ] **Step 6: Commit**

```bash
git add docs/.vitepress/config.ts
git commit -m "docs(vitepress): adopt pillar IA in sidebar + nav + URL rewrites

Replaces the legacy 'Getting Started / Guide / daft Commands / Git
Commands / Project' sidebar with the new pillar IA from #398:

  Getting Started | Worktrees | Hooks | Cookbook | Reference | About

Top nav exposes the three product surfaces (Worktrees / Hooks /
Cookbook) plus the version + GitHub link.

Adds vitepress rewrites so docs/cli/*.md (still autogenerated by
xtask at the same path) surface at /reference/cli/* URLs without
needing to move generated files.

ignoreDeadLinks is temporarily true; will revert in the final
migration cleanup task."
```

### Task 1.3: Add Cloudflare `_redirects` for legacy URLs

**Files:**

- Create: `docs/public/_redirects`

VitePress copies everything in `docs/public/` to the build root verbatim.
Cloudflare Pages reads `_redirects` from the build root.

- [ ] **Step 1: Create `docs/public/_redirects`**

```
# Legacy URLs from pre-IA-restructure (#398)
# Format: source dest [status]

# Guide pages → pillar pages
/guide/worktree-workflow                  /worktrees/                            301
/guide/layouts                            /worktrees/layouts                     301
/guide/adopting-existing-repos            /worktrees/adopting-existing-repos     301
/guide/multi-remote                       /worktrees/multi-remote                301
/guide/running-commands-across-worktrees  /worktrees/running-commands            301
/guide/shortcuts                          /worktrees/shortcuts                   301
/guide/hooks                              /hooks/                                301

# Guide pages → reference pages
/guide/configuration                      /reference/configuration               301
/guide/output-formats                     /reference/output-formats              301
/guide/claude-skill                       /reference/agent-skill                 301

# CLI ref → reference/cli (also handled by vitepress rewrites; this catches direct hits)
/cli/*                                    /reference/cli/:splat                  301

# Root meta → about
/contributing                             /about/contributing                    301
/changelog                                /about/changelog                       301
```

- [ ] **Step 2: Verify build**

Run: `mise run docs:site:build` Expected: build succeeds. Confirm `_redirects`
is in the build output: `ls docs/.vitepress/dist/_redirects`.

- [ ] **Step 3: Commit**

```bash
git add docs/public/_redirects
git commit -m "docs: add Cloudflare redirects for legacy URLs

Maps pre-restructure URLs (/guide/*, /cli/*, /contributing,
/changelog) to their new homes under the pillar IA. 301 permanent
redirects so search engine results flow to the new locations."
```

---

## Phase 2 — Worktrees pillar

Migrates and writes the Worktrees pillar. The pillar's Overview is the linchpin
— it's the gradient narrative scoped to worktrees (code → env → automation), and
it's the spine of the pillar's onboarding story.

### Task 2.1: Write `worktrees/index.md` (the pillar Overview)

**Files:**

- Modify: `docs/worktrees/index.md` (was placeholder from Task 1.1)
- Modify: `docs/guide/worktree-workflow.md` (delete after content migrated —
  done in this task's commit)

Replaces the placeholder with the real pillar Overview. Content is sourced from
the existing `guide/worktree-workflow.md` plus the gradient narrative (code →
env → automation) as the pillar's adoption arc, plus a "Where to next" section
that links to the rest of the pillar's pages.

- [ ] **Step 1: Read the source content**

Read `docs/guide/worktree-workflow.md` in full. Note the section structure:
"What Is a Git Worktree?", "The daft Directory Layout", and any later sections.

- [ ] **Step 2: Write `docs/worktrees/index.md`**

Replace the placeholder content with this structure:

```markdown
---
title: Worktrees
description:
  Code isolation through per-branch directories — daft's Worktrees pillar.
---

# Worktrees

The Worktrees pillar gives every Git branch its own directory on disk. Run
different branches in different terminals with full isolation — no stashing, no
context switching, no waiting for builds to restart.

## The adoption arc

Worktree adoption deepens in three stages. You don't need all three to get
value; each stage stands on its own.

1. **Code isolation.** Each branch lives in its own directory. The Git metadata
   is shared (one `.git/`), but the working files are separate. You can edit
   `feature-A` and `feature-B` simultaneously without `git stash` or branch
   swapping.
2. **Environment isolation.** Different branches often need different runtime
   versions, env vars, or secrets. With per-worktree env management
   ([mise](/cookbook/by-tooling/mise), [direnv](/cookbook/by-tooling/direnv),
   nvm, pyenv), each worktree boots with the right environment.
3. **Automation.** Setting up env per worktree gets repetitive. The
   [Hooks pillar](/hooks/) automates it: declarative jobs that run when
   worktrees are created, removed, or merged.

This page covers stage 1. Stages 2 and 3 are covered by linked pages.

## What is a Git worktree?

(Migrate the existing "What Is a Git Worktree?" section from
`guide/worktree-workflow.md` here. Keep the explanation at the same level —
clarify that Git 2.5+ supports multiple worktrees sharing one `.git`, each
branch can have its own files, daft structures this into a consistent layout.)

## The daft directory layout

(Migrate the "daft Directory Layout" section. Keep the directory tree examples.)

## Where to next

- **Geometry on disk:** [Layouts](/worktrees/layouts) — sibling, contained,
  nested, custom
- **Existing repos:**
  [Adopting existing repos](/worktrees/adopting-existing-repos) — convert a
  traditional repo to the worktree layout
- **Forks and mirrors:** [Multi-remote](/worktrees/multi-remote) — organize
  worktrees by remote
- **Run commands across worktrees:**
  [Running commands across worktrees](/worktrees/running-commands) — `daft exec`
- **Faster typing:** [Shortcuts](/worktrees/shortcuts) — `gwt*` symlink aliases
- **Recipes:** [Cookbook recipes for Worktrees](/cookbook/?pillar=worktrees)
- **Next pillar:** [Hooks](/hooks/) — automate the env-setup-per-worktree
  problem
```

When migrating sections, copy the existing markdown content verbatim from
`guide/worktree-workflow.md` for the "What is" and "Layout" sections. Don't
rewrite the prose — just relocate it.

- [ ] **Step 3: Delete the legacy file**

```bash
git rm docs/guide/worktree-workflow.md
```

- [ ] **Step 4: Update cross-links**

Find every reference to `/guide/worktree-workflow` in the docs and update:

```bash
grep -rln '/guide/worktree-workflow' docs/ | xargs sed -i.bak 's|/guide/worktree-workflow|/worktrees/|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 5: Verify build**

Run: `mise run docs:site:build` Expected: build succeeds.

- [ ] **Step 6: Commit**

```bash
git add docs/worktrees/index.md docs/guide/worktree-workflow.md docs/
git commit -m "docs(worktrees): write pillar Overview, retire guide/worktree-workflow

Establishes the Worktrees pillar's spine page. Content sourced from
the existing guide/worktree-workflow.md, plus a new 'adoption arc'
section that names the three stages (code → env → automation) and
links forward into the cookbook and the Hooks pillar.

Closes the worktree-workflow legacy URL via the redirect added in
Task 1.3."
```

### Task 2.2: Migrate `guide/layouts.md` → `worktrees/layouts.md`

**Files:**

- Create: `docs/worktrees/layouts.md`
- Delete: `docs/guide/layouts.md`

`guide/layouts.md` is a mixed Explanation+Reference page. The split: keep the
page as Explanation primary, but inline the small reference table of layout
names. The deep `daft layout list` reference lives at
`/reference/cli/daft-layout`.

- [ ] **Step 1: Read the source**

Read `docs/guide/layouts.md` in full. Identify the "What is a layout" sections
(Explanation) vs the per-layout reference details (Reference).

- [ ] **Step 2: Write the new file**

Move the entire content of `docs/guide/layouts.md` to
`docs/worktrees/layouts.md`. Then:

- Add a `## Where to next` section at the bottom linking to
  `/reference/cli/daft-layout` (full CLI reference) and
  `/cookbook/by-scenario/monorepo` (real-world usage).
- Keep the existing examples in place — they're useful for both explanation and
  quick-reference.

- [ ] **Step 3: Delete the legacy file and update cross-links**

```bash
git rm docs/guide/layouts.md
grep -rln '/guide/layouts' docs/ | xargs sed -i.bak 's|/guide/layouts|/worktrees/layouts|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 4: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 5: Commit**

```bash
git add docs/worktrees/layouts.md docs/guide/layouts.md docs/
git commit -m "docs(worktrees): migrate layouts page to pillar"
```

### Task 2.3: Migrate `guide/adopting-existing-repos.md` → `worktrees/adopting-existing-repos.md`

**Files:**

- Move: `docs/guide/adopting-existing-repos.md` →
  `docs/worktrees/adopting-existing-repos.md`

Pure How-to, single-purpose. Pure file move + cross-link updates.

- [ ] **Step 1: Move the file**

```bash
git mv docs/guide/adopting-existing-repos.md docs/worktrees/adopting-existing-repos.md
```

- [ ] **Step 2: Update cross-links**

```bash
grep -rln '/guide/adopting-existing-repos' docs/ | xargs sed -i.bak 's|/guide/adopting-existing-repos|/worktrees/adopting-existing-repos|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs(worktrees): move adopting-existing-repos to pillar"
```

### Task 2.4: Migrate `guide/running-commands-across-worktrees.md` → `worktrees/running-commands.md`

**Files:**

- Move: `docs/guide/running-commands-across-worktrees.md` →
  `docs/worktrees/running-commands.md`

Pure How-to. File move + rename (shorter path) + cross-link updates.

- [ ] **Step 1: Move + rename the file**

```bash
git mv docs/guide/running-commands-across-worktrees.md docs/worktrees/running-commands.md
```

- [ ] **Step 2: Update cross-links**

```bash
grep -rln '/guide/running-commands-across-worktrees' docs/ | xargs sed -i.bak 's|/guide/running-commands-across-worktrees|/worktrees/running-commands|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs(worktrees): rename running-commands-across-worktrees to running-commands"
```

### Task 2.5: Migrate `guide/shortcuts.md` → `worktrees/shortcuts.md`

**Files:**

- Move: `docs/guide/shortcuts.md` → `docs/worktrees/shortcuts.md`

- [ ] **Step 1: Move**

```bash
git mv docs/guide/shortcuts.md docs/worktrees/shortcuts.md
```

- [ ] **Step 2: Update cross-links**

```bash
grep -rln '/guide/shortcuts' docs/ | xargs sed -i.bak 's|/guide/shortcuts|/worktrees/shortcuts|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs(worktrees): move shortcuts to pillar"
```

### Task 2.6: Migrate `guide/multi-remote.md` → `worktrees/multi-remote.md`

**Files:**

- Move: `docs/guide/multi-remote.md` → `docs/worktrees/multi-remote.md`

The original is a mixed Explanation + How-to + Reference page. The split is
light: keep one page that flows Explanation → How-to (rare to need a hard split
for this content). The per-flag CLI reference lives at
`/reference/cli/daft-multi-remote`.

- [ ] **Step 1: Move**

```bash
git mv docs/guide/multi-remote.md docs/worktrees/multi-remote.md
```

- [ ] **Step 2: Light edit — add a `Where to next` section**

Append at the bottom of `docs/worktrees/multi-remote.md`:

```markdown
## Where to next

- **CLI reference:** [`daft multi-remote`](/reference/cli/daft-multi-remote)
- **Recipe:** [Fork workflow cookbook](/cookbook/by-scenario/fork-workflow)
```

- [ ] **Step 3: Update cross-links**

```bash
grep -rln '/guide/multi-remote' docs/ | xargs sed -i.bak 's|/guide/multi-remote|/worktrees/multi-remote|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 4: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 5: Commit**

```bash
git add docs/
git commit -m "docs(worktrees): move multi-remote to pillar with cross-links"
```

---

## Phase 3 — Hooks pillar

Splits the existing `guide/hooks.md` (a kitchen-sink page) into purpose-specific
Diátaxis pages, and writes the new Overview that opens with the **boundaries
thesis**: hooks as a local parallel to GitHub Actions, with each code-evolution
stage getting its own gate.

### Task 3.1: Write `hooks/index.md` (Overview with boundaries thesis)

**Files:**

- Modify: `docs/hooks/index.md` (was placeholder from Task 1.1)

The thesis is the most important conceptual contribution of this restructure.
The page must do four things: name the thesis, map it to today's hook types,
honestly mark which stages are shipped vs roadmap, and link forward.

- [ ] **Step 1: Write the new index**

Replace the placeholder with:

```markdown
---
title: Hooks
description:
  daft hooks define clear boundaries as your code evolves — a local, parallel
  approach to GitHub Actions.
---

# Hooks

> **daft hooks define clear boundaries as your code evolves. They are a local,
> parallel approach to GitHub Actions — every stage of code's lifecycle gets a
> well-defined gate.**

Each stage in your code's journey through development has different needs:

- When you start isolated work, you want the right env booted up
- When you commit a change, you want format/lint/fast-tests to gate it
- When you merge, you want the equivalent of PR checks to gate it
- When you tear down a worktree, you want artifacts persisted and state
  reclaimed

daft models each of these as a hook stage. They share one configuration system,
one trust model, one job orchestrator.

## The boundaries

| Stage                             | Hook type                                      | Boundary semantics                                                                             | Status                                                      |
| --------------------------------- | ---------------------------------------------- | ---------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| End of clone setup                | `post-clone`                                   | One-shot bootstrap of a fresh repo                                                             | Shipped                                                     |
| Start of isolated dev             | Worktree hooks (`worktree-pre/post-create`)    | Set up local dev env (deps, services)                                                          | Shipped                                                     |
| Sealing a unit of change          | Commit hooks                                   | Progressive code-replication boundary — format, lint, fast tests before the change is recorded | Roadmap ([#468](https://github.com/avihut/daft/issues/468)) |
| Letting a change escape isolation | Merge hooks (`pre-merge`, `post-merge`)        | PR-check parity — full tests, integration, security gates before code leaves the branch        | Roadmap ([#330](https://github.com/avihut/daft/issues/330)) |
| Reclaiming an isolated env        | Worktree teardown (`worktree-pre/post-remove`) | Teardown, persist artifacts, sync state                                                        | Shipped                                                     |

## How daft hooks differ from lefthook

Two distinctions:

1. **Lefthook is commit-time-only.** daft covers the full code-evolution
   lifecycle. Commit hooks are one stage among many — they share the trust
   model, the YAML schema, and the job orchestrator with worktree-lifecycle
   hooks. (See [#468](https://github.com/avihut/daft/issues/468) for the
   lefthook drop-in plan.)
2. **Boundaries before changes leave your machine.** CI traditionally runs
   _after_ code reaches the central repo; daft hooks run _before_. CI shifts
   left.

## Where to next

- **Reference:** [Lifecycle hooks](/hooks/lifecycle) — types, triggers, env
  vars, exit-code semantics
- **Reference:** [YAML reference](/hooks/yaml-reference) — the full `daft.yml`
  schema
- **Concept:** [Job orchestration](/hooks/job-orchestration) — parallelism,
  dependencies, conditions, OS/arch gating
- **Concept:** [Trust & security](/hooks/trust-and-security) — why hooks need
  trust and how the model works
- **Status:** [Roadmap](/hooks/roadmap) — what's coming for commit and merge
  stages
- **Recipes:** [Cookbook recipes for Hooks](/cookbook/?pillar=hooks)
```

- [ ] **Step 2: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 3: Commit**

```bash
git add docs/hooks/index.md
git commit -m "docs(hooks): write pillar Overview with boundaries thesis

Frames daft hooks as a local-parallel-to-GitHub-Actions surface,
with each code-evolution stage (worktree create / commit / merge /
worktree teardown) getting its own gate. Honestly marks which
stages are shipped vs roadmap (#330, #468)."
```

### Task 3.2: Extract `hooks/lifecycle.md` (Reference)

**Files:**

- Create: `docs/hooks/lifecycle.md`

Distill the Reference content from `guide/hooks.md` — the hook types table,
triggers, "runs from" directory, env vars exposed by each hook type. This is the
per-hook-type reference page.

- [ ] **Step 1: Read the source**

Read `docs/guide/hooks.md` end-to-end. Identify the Reference-shaped content:
the "Hook Types" table, the "Hook Context" section if present, exit-code
semantics.

- [ ] **Step 2: Write `docs/hooks/lifecycle.md`**

```markdown
---
title: Lifecycle hooks reference
description:
  Reference for daft's worktree-lifecycle hooks — types, triggers, environment,
  exit codes.
---

# Lifecycle hooks

This page is a complete reference for the **worktree-lifecycle hook types** —
the stages that fire when worktrees are created or removed, and when a clone
finishes. For commit-stage and merge-stage hooks, see the
[roadmap](/hooks/roadmap).

For the conceptual framing, see the [Hooks Overview](/hooks/).

For the YAML schema, see [YAML reference](/hooks/yaml-reference).

## Hook types

(Copy the Hook Types table verbatim from `guide/hooks.md`. Schema:)

| Hook                   | Trigger                       | Runs from                            |
| ---------------------- | ----------------------------- | ------------------------------------ |
| `post-clone`           | After `daft clone` completes  | New default branch worktree          |
| `worktree-pre-create`  | Before new worktree is added  | Source worktree (where command runs) |
| `worktree-post-create` | After new worktree is created | New worktree                         |
| `worktree-pre-remove`  | Before worktree is removed    | Worktree being removed               |
| `worktree-post-remove` | After worktree is removed     | Current worktree (where prune runs)  |

## Environment provided to hooks

(Migrate the env-var reference content from `guide/hooks.md`. List each env var,
what it contains, when it's set.)

## Exit-code semantics

(Migrate exit-code semantics from `guide/hooks.md`. Cover: 0 = success, non-zero
= abort, fail_mode for pre-remove, etc.)

## Hooks vs jobs

`daft.yml` lets a single hook fire **multiple jobs** in parallel or sequenced.
The hook is the trigger; the job is the unit of work. See
[Job orchestration](/hooks/job-orchestration) for parallelism, dependencies, and
conditions.
```

When migrating, copy the actual content verbatim from `guide/hooks.md`'s
reference sections. Don't paraphrase.

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 4: Commit**

```bash
git add docs/hooks/lifecycle.md
git commit -m "docs(hooks): extract lifecycle reference from guide/hooks.md"
```

### Task 3.3: Extract `hooks/yaml-reference.md` (Reference)

**Files:**

- Create: `docs/hooks/yaml-reference.md`

Distill the daft.yml schema reference content from `guide/hooks.md`. This is the
page someone opens to remember "what fields go in `daft.yml`."

- [ ] **Step 1: Read the source**

Identify the daft.yml schema content in `guide/hooks.md` — top-level keys, hook
entries, job entries, allowed values for
`parallel`/`if`/`needs`/`run`/`shell`/etc.

- [ ] **Step 2: Write `docs/hooks/yaml-reference.md`**

```markdown
---
title: daft.yml YAML reference
description: Complete reference for daft.yml hook configuration schema.
---

# `daft.yml` YAML reference

Complete reference for the `daft.yml` schema. For the conceptual framing, see
[Hooks Overview](/hooks/). For lifecycle-specific behavior, see
[Lifecycle hooks](/hooks/lifecycle).

## Top-level keys

(List each top-level key with type, default, semantics. Migrate verbatim from
`guide/hooks.md`.)

## Hook entries

(Schema for each hook entry — `jobs:`, etc. Migrate verbatim.)

## Job entries

(Schema for each job — `name:`, `run:`, `shell:`, `parallel:`, `if:`, `needs:`,
OS/arch gating, env vars, working directory, etc. Migrate verbatim.)

## Examples

(Migrate one or two end-to-end examples from `guide/hooks.md` if present.)
```

Migrate the existing content verbatim. The schema is the authoritative reference
and the _one place_ schema details live now.

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 4: Commit**

```bash
git add docs/hooks/yaml-reference.md
git commit -m "docs(hooks): extract daft.yml schema reference from guide/hooks.md"
```

### Task 3.4: Write `hooks/job-orchestration.md` (Explanation)

**Files:**

- Create: `docs/hooks/job-orchestration.md`

Explanation of how multiple jobs run together — parallelism, dependencies,
conditional skipping, OS/arch gating. Distill the conceptual content from
`guide/hooks.md` into a single Explanation page.

- [ ] **Step 1: Write `docs/hooks/job-orchestration.md`**

```markdown
---
title: Job orchestration
description:
  How daft hooks orchestrate multiple jobs — parallelism, dependencies,
  conditions.
---

# Job orchestration

A single hook can fire multiple jobs. This page explains how those jobs are
orchestrated.

## Why jobs, not just scripts

Worktree setup often has independent steps that should run in parallel (install
Node deps, install Python deps, start a Postgres container) and steps that
depend on others (install deps before running migrations). Modeling this as a
list of jobs with parallelism and dependencies is more honest than serializing
them in a shell script.

## Parallelism

(Explanation of how `parallel: true` works, the default behavior, when parallel
is safe vs unsafe. Reference `daft.yml` syntax with one example. Refer to YAML
reference for full schema.)

## Dependencies (`needs:`)

(Explanation of `needs:`, how DAG resolution works, how a failed dependency
skips downstream jobs.)

## Conditional skipping (`if:`)

(Explanation of `if:` expressions, how OS/arch gating works, when to use vs
branch matching.)

## OS and architecture gating

(Explanation of OS/arch matchers — when they fire, why they're a feature.)

## Trust and side effects

Job orchestration only runs once a hook is **trusted**. See
[Trust & security](/hooks/trust-and-security) for the model.

## Where to next

- **Schema:** [YAML reference](/hooks/yaml-reference) — every field
- **Trust:** [Trust & security](/hooks/trust-and-security)
- **Recipes:** [Cookbook recipes for Hooks](/cookbook/?pillar=hooks)
```

For each `(Explanation of ...)` placeholder, write 2-4 paragraphs explaining the
concept. Source material: existing `guide/hooks.md` plus any references in
`src/hooks/` if implementation details are needed for accurate explanation.

- [ ] **Step 2: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 3: Commit**

```bash
git add docs/hooks/job-orchestration.md
git commit -m "docs(hooks): write job-orchestration explanation page"
```

### Task 3.5: Write `hooks/trust-and-security.md` (Explanation)

**Files:**

- Create: `docs/hooks/trust-and-security.md`

Explanation of the trust model. Distill from `guide/hooks.md` and any
`git daft hooks` reference content.

- [ ] **Step 1: Write `docs/hooks/trust-and-security.md`**

```markdown
---
title: Trust & security
description:
  How daft hooks balance team-shared automation with security against malicious
  .daft/ directories.
---

# Trust & security

daft hooks are committed to the repo and run on developer machines — same shape
as a `package.json` `postinstall`, with the same risk: a malicious `daft.yml`
can run arbitrary code. daft mitigates this with a **trust-on-first-use** model.

## The threat

(Brief explanation: someone clones a hostile repo, daft runs hooks before
they've reviewed `daft.yml`. Without mitigation, this is a code-exec on clone.)

## The model

(Explanation of trust-on-first-use, where trust is stored, how invalidation
works on `daft.yml` change, how to revoke. Reference the `git daft-hooks` CLI
commands.)

## Trust granularity

(Explanation of repo-level trust vs per-hook trust if applicable.)

## Why not signing

(Brief discussion of why a signing model would be heavier than the value it adds
for the audience daft serves.)

## Where to next

- **CLI:** [`git daft-hooks`](/reference/cli/git-daft-hooks)
- **Reference:** [Lifecycle hooks](/hooks/lifecycle),
  [YAML reference](/hooks/yaml-reference)
```

For each `(Explanation of ...)` block, write actual prose distilled from the
implementation in `src/hooks/trust.rs` (or wherever trust lives — verify before
writing).

- [ ] **Step 2: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 3: Commit**

```bash
git add docs/hooks/trust-and-security.md
git commit -m "docs(hooks): write trust-and-security explanation page"
```

### Task 3.6: Write `hooks/roadmap.md` (Explanation stub)

**Files:**

- Create: `docs/hooks/roadmap.md`

Honest accounting of what's not yet shipped in the Hooks pillar.

- [ ] **Step 1: Write `docs/hooks/roadmap.md`**

```markdown
---
title: Hooks roadmap
description: Hook stages that are designed but not yet shipped.
---

# Hooks roadmap

Two hook stages are part of the [boundaries thesis](/hooks/) but not yet
shipped. They are tracked as feature issues, with their docs landing in the same
PR as the feature (per "docs and features enter together").

## Commit hooks (full git-hooks drop-in)

**Tracking:** [#468](https://github.com/avihut/daft/issues/468)

Lefthook-style drop-in: `pre-commit`, `commit-msg`, `prepare-commit-msg`,
`pre-push`, `post-commit`, `pre-rebase`. The "progressive code-replication
boundary" — format, lint, fast tests gate every commit.

When this ships, daft becomes a viable lefthook replacement. Recipes for the
migration will live under
[Cookbook → By tooling → lefthook → daft](/cookbook/by-tooling/) once written.

## Merge hooks

**Tracking:** [#330](https://github.com/avihut/daft/issues/330)

`pre-merge` and `post-merge` hooks fire around `daft merge` /
`daft worktree-merge`. The "PR-check-parity" boundary — full tests, integration,
security gates before code leaves an isolated branch.

This is the merge feature itself, currently in flight. Hook docs land in the
same PR.

## Why these aren't shipped yet

The IA exists today (this pillar, this Overview, this roadmap page) so the
conceptual frame can be in place. The features are sequenced after the IA itself
stabilizes — see [#398](https://github.com/avihut/daft/issues/398) for context.
```

- [ ] **Step 2: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 3: Commit**

```bash
git add docs/hooks/roadmap.md
git commit -m "docs(hooks): write roadmap stub for #330 (merge) and #468 (commit drop-in)"
```

### Task 3.7: Retire `guide/hooks.md`

**Files:**

- Delete: `docs/guide/hooks.md`

All content has been migrated into `hooks/index.md`, `hooks/lifecycle.md`,
`hooks/yaml-reference.md`, `hooks/job-orchestration.md`,
`hooks/trust-and-security.md`.

- [ ] **Step 1: Delete the legacy file**

```bash
git rm docs/guide/hooks.md
```

- [ ] **Step 2: Update cross-links**

```bash
grep -rln '/guide/hooks' docs/ | xargs sed -i.bak 's|/guide/hooks|/hooks/|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`

- [ ] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs(hooks): retire guide/hooks.md (content migrated to pillar pages)"
```

---

## Phase 4 — Reference moves

Three small file moves. Pure relocation, no content rewrites.

### Task 4.1: Move `guide/configuration.md` → `reference/configuration.md`

**Files:**

- Move: `docs/guide/configuration.md` → `docs/reference/configuration.md`

- [ ] **Step 1: Move + update cross-links**

```bash
git mv docs/guide/configuration.md docs/reference/configuration.md
grep -rln '/guide/configuration' docs/ | xargs sed -i.bak 's|/guide/configuration|/reference/configuration|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/
git commit -m "docs(reference): move configuration page to reference/"
```

### Task 4.2: Move `guide/output-formats.md` → `reference/output-formats.md`

**Files:**

- Move: `docs/guide/output-formats.md` → `docs/reference/output-formats.md`

- [ ] **Step 1: Move + update cross-links**

```bash
git mv docs/guide/output-formats.md docs/reference/output-formats.md
grep -rln '/guide/output-formats' docs/ | xargs sed -i.bak 's|/guide/output-formats|/reference/output-formats|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/
git commit -m "docs(reference): move output-formats page to reference/"
```

### Task 4.3: Move `guide/claude-skill.md` → `reference/agent-skill.md`

**Files:**

- Move + rename: `docs/guide/claude-skill.md` → `docs/reference/agent-skill.md`

The rename matches the page's actual title ("Agent Skill" — applies to multiple
agents, not just Claude).

- [ ] **Step 1: Move + rename + update cross-links**

```bash
git mv docs/guide/claude-skill.md docs/reference/agent-skill.md
grep -rln '/guide/claude-skill' docs/ | xargs sed -i.bak 's|/guide/claude-skill|/reference/agent-skill|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/
git commit -m "docs(reference): move agent skill page to reference/ with multi-agent name"
```

### Task 4.4: Write `reference/index.md`

**Files:**

- Modify: `docs/reference/index.md` (was placeholder from Task 1.1)

- [ ] **Step 1: Replace placeholder with a real overview**

```markdown
---
title: Reference
description:
  Configuration keys, CLI commands, output formats, and the agent skill.
---

# Reference

Authoritative descriptions of daft's machinery. For task-oriented guidance, see
the [Cookbook](/cookbook/) or the per-pillar pages.

- **[Configuration](/reference/configuration)** — every `git config daft.*` key
- **[Output formats](/reference/output-formats)** — `--format` and `--template`
  across daft commands
- **[Agent skill](/reference/agent-skill)** — the `daft-worktree-workflow` skill
  that teaches AI coding agents how to use daft
- **CLI** — every `daft *` and `git-worktree-*` command (in the sidebar;
  collapsed by default)
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/reference/index.md
git commit -m "docs(reference): write reference hub overview"
```

---

## Phase 5 — About pages

Six new pages plus two relocations (contributing, changelog). The most
content-heavy phase.

### Task 5.1: Move `contributing.md` and `changelog.md` to `about/`

**Files:**

- Move: `docs/contributing.md` → `docs/about/contributing.md`
- Move: `docs/changelog.md` → `docs/about/changelog.md`

- [ ] **Step 1: Move both files**

```bash
git mv docs/contributing.md docs/about/contributing.md
git mv docs/changelog.md docs/about/changelog.md
```

- [ ] **Step 2: Update cross-links**

```bash
grep -rln '/contributing\b' docs/ | xargs sed -i.bak 's|/contributing\b|/about/contributing|g'
grep -rln '/changelog\b' docs/ | xargs sed -i.bak 's|/changelog\b|/about/changelog|g'
find docs/ -name '*.bak' -delete
```

- [ ] **Step 3: Update vitepress config — version-link in nav already points at
      `/about/changelog` (set in Task 1.2). Confirm.**

- [ ] **Step 4: Verify + commit**

```bash
mise run docs:site:build
git add docs/
git commit -m "docs(about): move contributing and changelog under about/"
```

### Task 5.2: Write `about/why-daft.md`

**Files:**

- Create: `docs/about/why-daft.md`

The unifying thesis page. This is the answer to "what is daft" for a newcomer
who lands on the docs site cold.

- [ ] **Step 1: Write the page**

```markdown
---
title: Why daft
description:
  daft helps you parallelize development through isolation, and (eventually)
  coordinate changes across repos.
---

# Why daft

daft is built on one thesis:

> **Parallelize development through isolation; coordinate across repos via
> networking.**

The first half — parallelize through isolation — is what daft does today. The
second half — coordinate across repos — is in design
([#357](https://github.com/avihut/daft/issues/357)).

## The problem

Modern dev work is often blocked by serialization that doesn't have to exist:

- You can't work on feature A and feature B simultaneously because they share a
  working tree
- Switching branches restarts builds, dev servers, file watchers
- `git stash` is a sharp tool that loses work when used carelessly
- A bug fix can't share a working tree with the feature you were working on
- Different branches need different env vars, runtime versions, secrets — and
  `.envrc` doesn't follow your branch

These are all symptoms of one root cause: a single working directory that flips
between branches.

## The shape of the solution

Three pillars, each idempotent — you can adopt one without the others.

- **[Worktrees](/worktrees/)**: every branch gets its own directory. No
  flipping. No stashing. Run `feature-A` and `feature-B` in different terminals
  at the same time.
- **[Hooks](/hooks/)**: declarative automation at every code-evolution boundary.
  Local equivalent of GitHub Actions, but enforced before code leaves your
  machine.
- **Networking** ([#357](https://github.com/avihut/daft/issues/357)): coordinate
  changes across repos. A repo catalog plus a manifest of cross-repo
  relationships, so a change that touches three services can be propagated
  coherently.

The pillars are loosely coupled. A user who only wants worktrees never has to
learn hooks. A user who only wants hooks doesn't need to adopt worktrees (once
the [full git-hooks drop-in](https://github.com/avihut/daft/issues/468) ships).

## When daft is the right tool

- You frequently switch contexts and lose flow because of it
- Your branches need different env vars, runtime versions, or services running
  locally
- You want CI-style gates running before code leaves your machine, not after
- You work in a polyrepo where changes naturally span multiple repos

## When daft is not the right tool

- You only ever work on one branch at a time and never context-switch (rare in
  practice — but if it's you, daft adds setup cost without value)
- You need worktree-aware features inside an IDE that doesn't support multi-root
  projects (technically still usable but rougher)
- You need to deploy on Windows-only environments where shell-integration is
  awkward (works, but the ergonomics are rougher)

## Where to start

- **[Quick Start](/getting-started/quick-start)** — a Tutorial that walks the
  worktree adoption arc
- **[Worktrees](/worktrees/)** — the foundation pillar
- **[Cookbook](/cookbook/)** — recipes for adopting daft alongside your existing
  tooling
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/why-daft.md
git commit -m "docs(about): write the why-daft thesis page"
```

### Task 5.3: Write `about/glossary.md`

**Files:**

- Create: `docs/about/glossary.md`

A reference glossary of daft-specific terms. Each term: one-line definition +
link to the pillar/page that explains it.

- [ ] **Step 1: Write the page**

```markdown
---
title: Glossary
description: daft-specific terminology in one place.
---

# Glossary

| Term                  | Definition                                                                                                         | More                                                          |
| --------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------- |
| **Worktree**          | A working directory linked to a Git repo's branch. daft places one worktree per branch on disk.                    | [Worktrees](/worktrees/)                                      |
| **Layout**            | The geometry of where worktrees go on disk relative to the repo (sibling, contained, nested, custom).              | [Layouts](/worktrees/layouts)                                 |
| **Adopt**             | Convert a traditional Git repo (single working tree) into the daft worktree layout.                                | [Adopting existing repos](/worktrees/adopting-existing-repos) |
| **Hook**              | A unit of automation that fires on a code-evolution boundary (worktree create, commit, merge, etc.).               | [Hooks](/hooks/)                                              |
| **Job**               | A unit of work _inside_ a hook. One hook can fire multiple jobs in parallel or sequenced.                          | [Job orchestration](/hooks/job-orchestration)                 |
| **Trust**             | A confirmation that a `daft.yml` is authorized to run on this machine. Trust is invalidated when the file changes. | [Trust & security](/hooks/trust-and-security)                 |
| **Multi-remote**      | A worktree layout that organizes worktrees by their remote (e.g., separate folders for `origin` and `upstream`).   | [Multi-remote](/worktrees/multi-remote)                       |
| **Shortcut**          | A short symlink alias for a longer command (e.g., `gwtco` for `git worktree-checkout`).                            | [Shortcuts](/worktrees/shortcuts)                             |
| **Networking**        | (Planned) cross-repo coordination via a catalog and relations manifest.                                            | [Networking roadmap](/about/networking-roadmap)               |
| **Repo catalog**      | (Planned) the registry that tracks repos and their relations to each other.                                        | [Networking roadmap](/about/networking-roadmap)               |
| **Boundaries thesis** | The framing that daft hooks are gates at each code-evolution stage — a local parallel to GitHub Actions.           | [Hooks](/hooks/)                                              |
| **Adoption arc**      | The three stages of worktree adoption depth: code isolation → environment isolation → automation.                  | [Worktrees](/worktrees/)                                      |
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/glossary.md
git commit -m "docs(about): write glossary"
```

### Task 5.4: Write `about/faq.md`

**Files:**

- Create: `docs/about/faq.md`

Common questions distilled from GitHub discussions, Discord (if any), and the
existing `guide/` pages where they touch on FAQs.

- [ ] **Step 1: Write the page**

```markdown
---
title: FAQ
description: Frequently asked questions about daft.
---

# FAQ

## Does daft replace `git`?

No. daft sits next to git. Every daft command either calls into git or wraps a
git operation. You can mix `git` and `daft` commands freely in the same repo.

## Does daft work with monorepos?

Yes. See [Cookbook → By scenario → Monorepo](/cookbook/by-scenario/monorepo) for
the recommended pattern.

## Does daft work on Windows?

Yes. The binary is shipped for Windows and tested in CI. Shell integration works
in PowerShell, Git Bash, WSL, and Cmd (limited). See
[Shell integration](/getting-started/shell-integration).

## Does daft replace lefthook?

Today: no — daft hooks are scoped to worktree lifecycle. The lefthook drop-in is
on the roadmap ([#468](https://github.com/avihut/daft/issues/468)).

## Does daft replace GitHub Actions?

No. daft hooks are _local_ CI — they run on developer machines, before code
reaches the central repo. GitHub Actions runs _centrally_, after code arrives.
They're complementary: shift fast checks left into daft hooks; keep
slow/secrets-bound checks in Actions.

## How do I migrate an existing repo to daft?

`daft adopt`. See [Adopting existing repos](/worktrees/adopting-existing-repos).

## How do I uninstall daft from a repo?

`daft eject`. The repo is restored to a single-working-tree layout.

## Is daft safe for collaborators who don't use it?

Yes. daft writes to `.git/` and a single `daft.yml` (if you use hooks).
Collaborators using plain `git` see normal behavior; they don't need to adopt
daft.

## How does daft handle uncommitted changes when removing a worktree?

`daft remove` prompts before destroying uncommitted work. Use `--force` to
bypass.

## Does daft modify global git config?

No, ever. daft only writes repo-local config and its own files. Your global git
config is untouched.

## Where are hooks trusted?

In your XDG state directory — by default `~/.local/state/daft/trust.toml` on
Linux, `~/Library/Application Support/daft/trust.toml` on macOS,
`%LOCALAPPDATA%\daft\trust.toml` on Windows.
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/faq.md
git commit -m "docs(about): write FAQ"
```

### Task 5.5: Write `about/troubleshooting.md`

**Files:**

- Create: `docs/about/troubleshooting.md`

Symptom → cause → fix entries for common issues.

- [ ] **Step 1: Write the page**

````markdown
---
title: Troubleshooting
description: Common issues and how to fix them.
---

# Troubleshooting

If your problem isn't listed here, run `daft doctor` first — it diagnoses common
configuration issues automatically.

## "command not found: daft"

`daft` is installed but not on `PATH`. Verify the install location
(`brew prefix avihut/tap/daft` on macOS) is in your shell's `PATH`.

## My shell doesn't `cd` into the new worktree

Shell integration isn't installed. See
[Shell integration](/getting-started/shell-integration) for the eval line to add
to your shell config.

## "daft.yml is untrusted; refusing to run hooks"

A `daft.yml` was added or changed. Trust the new contents:

```bash
git daft-hooks trust
```
````

This is intentional — see [Trust & security](/hooks/trust-and-security) for why.

## Hooks fire but I don't see their output

Job stdout/stderr is captured to log files in `~/.local/state/daft/logs/` (XDG
state dir). Inspect with:

```bash
git daft-hooks log show
```

## Worktree creation fails with "fatal: <branch> is already checked out"

The branch is checked out in a different worktree. Either remove the other
worktree first (`daft remove <branch>`), or use a different branch name.

## `daft adopt` says my repo "looks like it's already adopted"

The repo already has the daft layout. Use `daft list` to see existing worktrees,
or `daft eject` to restore a single-working-tree layout if you want to start
over.

## I can't tell which worktree is which

`daft list` prints all worktrees. With `--format json` you get machine-readable
output.

## When in doubt

Run `daft doctor`. It diagnoses install, shell integration, layout health, and
hook trust state.

````

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/troubleshooting.md
git commit -m "docs(about): write troubleshooting"
````

### Task 5.6: Write `about/comparison.md`

**Files:**

- Create: `docs/about/comparison.md`

Honest framing of daft vs nearby tools. Each comparison: one-line positioning,
what daft adds, what the other tool does better, when to pick which.

- [ ] **Step 1: Write the page**

```markdown
---
title: Comparison
description: daft vs nearby tools — git worktree, lefthook, gitup, gh worktree.
---

# Comparison

How daft relates to nearby tools.

## vs plain `git worktree`

`git worktree` is the foundation daft is built on. daft adds:

- **Layout management.** `git worktree` makes you place worktrees manually; daft
  enforces a chosen geometry (sibling, contained, nested, custom).
- **Lifecycle automation.** `daft.yml` hooks fire on create/remove; plain
  `git worktree` has no hook surface.
- **Shell integration.** daft's shell wrapper auto-`cd`s into new worktrees;
  plain `git worktree` leaves you in the source.
- **Maintenance commands.** `daft prune`, `daft sync`, `daft list`,
  `daft doctor` — orchestrated workflows that you'd otherwise script yourself.

When to pick plain `git worktree`: occasional, one-off worktree usage where the
daft layout would be overkill.

## vs lefthook

[Lefthook](https://github.com/evilmartians/lefthook) is a popular git hook
manager focused on commit-stage hooks (pre-commit, commit-msg, pre-push).

Today, daft hooks are scoped to worktree-lifecycle stages — they don't replace
lefthook. The full git-hooks drop-in
([#468](https://github.com/avihut/daft/issues/468)) is on the roadmap; once
shipped, daft will be a viable lefthook replacement.

When that ships, the comparison will be:

- **daft** covers the full code-evolution lifecycle (worktree → commit → merge →
  teardown) under one config and one trust model.
- **lefthook** covers commit-stage only, but is mature and battle-tested.

When to pick lefthook today: you only need commit-stage hooks. Revisit when #468
ships.

## vs gitup / `gh worktree` / `git-town`

These are smaller-scope tools targeting specific workflow gaps:

- **[gitup](https://github.com/jonas/gitup)** is a TUI for `git worktree`. daft
  is a CLI with a richer feature set (layouts, hooks, multi-remote).
- **[`gh worktree`](https://github.com/cli/cli)** (planned in github/cli) is a
  thin GitHub CLI extension over `git worktree`. daft is broader (not
  GitHub-specific).
- **[git-town](https://www.git-town.com/)** automates branch sync workflows on a
  single working tree. daft solves the parallel-branches problem instead.

When to pick one of those: you have a narrow workflow gap that one of them fills
better than daft, or you don't need worktrees at all.

## vs GitHub Actions PR checks

(Speculative — fully realized once
[#330](https://github.com/avihut/daft/issues/330) and
[#468](https://github.com/avihut/daft/issues/468) ship.)

GitHub Actions runs PR checks **after** code reaches the central repo. daft
hooks (when the full set is shipped) run **before** code leaves your machine.

These are complementary: fast checks shift left to daft hooks (faster feedback,
no minutes consumed); slow/secrets-bound checks stay in Actions.

When to lean on Actions over daft hooks: deployment, release pipelines, artifact
publishing, integration with external secret stores.
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/comparison.md
git commit -m "docs(about): write comparison page (vs git worktree, lefthook, gitup, Actions)"
```

### Task 5.7: Write `about/networking-roadmap.md`

**Files:**

- Create: `docs/about/networking-roadmap.md`

Stub for the future Networking pillar. Once #357 ships, this page goes away (its
content migrates to `networking/index.md`).

- [ ] **Step 1: Write the page**

```markdown
---
title: Networking roadmap
description: Cross-repo coordination is daft's third pillar — in design.
---

# Networking roadmap

> **Status: in design.** Tracking issue:
> [#357](https://github.com/avihut/daft/issues/357).

The Networking pillar is the second half of daft's thesis (the first half —
parallel dev via isolation — is what worktrees + hooks deliver today):

> **Coordinate changes across repos.**

## The problem

Polyrepo development means a change often spans multiple repos: a service and
its client, a library and its consumers, a monorepo of microservices. Today, the
coordination is manual:

- You clone N repos by hand
- You apply N versions of a related change by hand
- You track N PRs across N repos in a spreadsheet
- You cherry-pick a refactor across N repos because there's no shared
  abstraction

Networking is daft's surface for that.

## The shape of the solution

Two pieces:

1. **A repo catalog** — a daft-managed registry of repos on your machine, with
   their layout, default branch, and identity.
2. **A relations manifest** — a per-repo declaration of "this repo depends on
   these others" / "this repo is a sibling of those others." Stored in
   `daft.yml` (or similar; design pending).

With those, daft can:

- Clone the closure of a repo and its declared dependencies
- Propagate a related change across repos (start matched feature branches, run
  merge gates across the closure)
- Surface "stale" repos in the catalog (haven't synced in N days)
- Coordinate releases across a service+client pair

## When this ships

This page goes away. Its content migrates to `networking/index.md` as the third
pillar's Overview, and the top nav adds a "Networking" entry between "Hooks" and
"Cookbook." See
[#398's coordination notes](https://github.com/avihut/daft/issues/398) for how
docs land alongside the feature.
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/networking-roadmap.md
git commit -m "docs(about): write networking-roadmap stub linking #357"
```

### Task 5.8: Write `about/index.md`

**Files:**

- Modify: `docs/about/index.md` (was placeholder from Task 1.1)

- [ ] **Step 1: Replace placeholder**

```markdown
---
title: About
description: Background, FAQ, troubleshooting, comparison, and project meta.
---

# About

- **[Why daft](/about/why-daft)** — the thesis
- **[Glossary](/about/glossary)** — daft-specific terminology
- **[FAQ](/about/faq)** — common questions
- **[Troubleshooting](/about/troubleshooting)** — symptom → fix
- **[Comparison](/about/comparison)** — daft vs nearby tools
- **[Networking roadmap](/about/networking-roadmap)** — the third pillar, in
  design
- **[Contributing](/about/contributing)** — how to help
- **[Changelog](/about/changelog)** — release notes
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/about/index.md
git commit -m "docs(about): write about hub overview"
```

---

## Phase 6 — Cookbook

The Cookbook is daft's adoption gateway. It opens with a taxonomy + recipe
template, ships with three anchor recipes written in full, and stubs the
remaining 10 recipes as frontmatter-only files. Stub recipes are filled in as
follow-up commits on this branch (or in the PR review iteration).

### Task 6.1: Define the recipe template (used by every recipe task)

**Recipe template** — every cookbook recipe follows this skeleton:

```markdown
---
title: <Recipe Title>
description: <One-line elevator pitch — what this recipe accomplishes.>
pillars: [<list of touched pillars: worktrees / hooks / networking>]
tooling: [<list of tools — mise / direnv / nvm / pyenv / asdf / docker / etc.>]
languages:
  [<list — node / python / rust / go / etc., empty if language-agnostic>]
---

# <Recipe Title>

> **Goal:** <One sentence — what the reader has after following this recipe.>

## Context

<2-3 sentences — when this recipe applies, what problem it solves.>

## Prerequisites

- <Bullet list of installed tools / repo state needed>

## Steps

### 1. <First action>

<Explanation + commands.>

### 2. <Second action>

<Explanation + commands.>

(Continue with as many numbered steps as needed.)

## Verifying it works

<How the reader confirms the recipe took. Concrete commands + expected output.>

## Variations

<Optional. Common variations on the recipe — e.g., for a different shell, OS, or
tool combination.>

## Troubleshooting

<Optional. Symptom → fix entries specific to this recipe.>

## Where to next

- <Cross-link to related recipes>
- <Cross-link to relevant pillar pages>
```

This template is referenced by every cookbook recipe task below. Each task fills
in the template with recipe-specific content.

(No commit for this task — it's reference for the next tasks.)

### Task 6.2: Write `cookbook/index.md`

**Files:**

- Modify: `docs/cookbook/index.md` (was placeholder from Task 1.1)

- [ ] **Step 1: Replace placeholder**

```markdown
---
title: Cookbook
description:
  Recipes for adopting daft alongside your existing tooling, language, or
  scenario.
---

# Cookbook

Recipes for putting daft into practice. Each recipe is task-oriented (here's how
to do X), tagged with which **pillar(s)** it touches and which **tooling** /
**language** / **scenario** it's about.

## Find a recipe

### By tooling

How daft fits with the env-manager you already use.

- **[mise](/cookbook/by-tooling/mise)** — per-worktree tool versions and tasks
  via `mise.toml`
- **[direnv](/cookbook/by-tooling/direnv)** — per-worktree env vars via `.envrc`
- **[nvm](/cookbook/by-tooling/nvm)** — per-worktree Node versions via `.nvmrc`
- **[pyenv](/cookbook/by-tooling/pyenv)** — per-worktree Python versions via
  `.python-version`
- **[asdf](/cookbook/by-tooling/asdf)** — multi-language version management via
  `.tool-versions`

### By language

Per-language patterns for daft adoption.

- **[Node.js](/cookbook/by-language/node)** — `package.json`, `node_modules`,
  npm/pnpm/yarn
- **[Python](/cookbook/by-language/python)** — virtualenvs, requirements, `pip`
  vs `uv`
- **[Rust](/cookbook/by-language/rust)** — `target/` per worktree, `cargo`
  caches
- **[Go](/cookbook/by-language/go)** — `GOPATH`, modules, build cache

### By scenario

Patterns for specific workflow shapes.

- **[Monorepo](/cookbook/by-scenario/monorepo)** — daft in a multi-package
  monorepo
- **[Fork workflow](/cookbook/by-scenario/fork-workflow)** — daft + multi-remote
  for forks
- **[CI integration](/cookbook/by-scenario/ci-integration)** — running daft
  hooks in CI for parity

## Pillar tags

Every recipe lists the **pillar(s)** it touches in its frontmatter:

- `pillars: [worktrees]` — worktree workflow only
- `pillars: [worktrees, hooks]` — worktrees + automation via daft hooks
- `pillars: [hooks]` — hooks-only (rare today; will be common once
  [#468](https://github.com/avihut/daft/issues/468) ships)

## Contributing a recipe

Spot a missing recipe? See [Contributing](/about/contributing).
```

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/cookbook/index.md
git commit -m "docs(cookbook): write cookbook hub with by-tooling/by-language/by-scenario taxonomy"
```

### Task 6.3: Write `cookbook/by-tooling/mise.md` (anchor recipe)

**Files:**

- Create: `docs/cookbook/by-tooling/mise.md`

mise is daft's own tool manager — this recipe is dogfooding territory and will
be referenced by many other recipes, so it's an anchor.

- [ ] **Step 1: Write the page**

Use the recipe template from Task 6.1. Fill in:

````markdown
---
title: daft + mise
description:
  Per-worktree tool versions and tasks via mise, automated by daft hooks.
pillars: [worktrees, hooks]
tooling: [mise]
languages: []
---

# daft + mise

> **Goal:** Each worktree boots with the exact tool versions declared in its
> `mise.toml`, automatically — no manual activation.

## Context

[mise](https://mise.jdx.dev) reads `mise.toml` to pin tool versions per
directory. With daft, each branch is a directory, so `mise.toml` becomes a
per-branch tool manifest. A worktree-post-create hook installs missing versions
on first creation; mise's shell hook activates them on `cd`.

## Prerequisites

- daft installed and shell integration enabled
- mise installed (`brew install mise` on macOS)
- mise's shell activation in your shell profile (`eval "$(mise activate bash)"`
  or equivalent)

## Steps

### 1. Add `mise.toml` to the repo

In the default-branch worktree:

```bash
cd ~/work/my-project/main
mise use node@22 python@3.13
git add mise.toml
git commit -m "chore: pin mise versions"
```
````

### 2. Add a `daft.yml` to install missing versions on worktree create

In the same worktree:

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: install mise versions
      run: mise install
```

Trust the new `daft.yml`:

```bash
git add daft.yml
git commit -m "chore(daft): install mise versions on worktree create"
git daft-hooks trust
```

### 3. Create a worktree

```bash
daft start feat/upgrade-react
```

The hook fires; mise installs any missing versions. Your shell `cd`s into the
new worktree, and `mise activate` exposes the pinned tools on `PATH`.

## Verifying it works

```bash
node --version    # 22.x.x
python --version  # 3.13.x
which node        # ~/.local/share/mise/installs/node/22/bin/node (or similar)
```

## Variations

### Per-branch divergence

A feature branch can pin different versions. Edit `mise.toml` in that worktree,
commit, and the next time someone creates a worktree from that branch, the
post-create hook installs the new versions.

### mise tasks instead of `package.json` scripts

`mise.toml` `[tasks.*]` blocks let you run tasks via `mise run <name>`. This
works inside daft worktrees the same as anywhere else; no daft-specific
configuration needed.

## Troubleshooting

- **`mise install` fails with "no plugin found"** — run
  `mise plugin install <tool>` once to install the plugin globally; subsequent
  worktrees will reuse it.
- **`mise activate` not exposing tools** — confirm the shell hook is in your
  profile (after `eval "$(daft shell-init bash)"` is fine; before it works too).

## Where to next

- **[direnv](/cookbook/by-tooling/direnv)** — env vars per worktree (mise
  handles tool versions; direnv handles secrets)
- **[Hooks](/hooks/)** — what else can fire on worktree create
- **[Job orchestration](/hooks/job-orchestration)** — run `mise install` in
  parallel with other setup jobs

````

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/cookbook/by-tooling/mise.md
git commit -m "docs(cookbook): write mise recipe (anchor)"
````

### Task 6.4: Write `cookbook/by-tooling/direnv.md` (anchor recipe)

**Files:**

- Create: `docs/cookbook/by-tooling/direnv.md`

- [ ] **Step 1: Write using the recipe template**

````markdown
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
````

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

````

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/cookbook/by-tooling/direnv.md
git commit -m "docs(cookbook): write direnv recipe (anchor)"
````

### Task 6.5: Write `cookbook/by-scenario/monorepo.md` (anchor recipe)

**Files:**

- Create: `docs/cookbook/by-scenario/monorepo.md`

- [ ] **Step 1: Write using the recipe template**

````markdown
---
title: daft in a monorepo
description:
  Pattern for using daft inside a multi-package monorepo (Nx, Turborepo, pnpm
  workspaces, etc.).
pillars: [worktrees, hooks]
tooling: []
languages: []
---

# daft in a monorepo

> **Goal:** Multiple feature branches active simultaneously inside a monorepo,
> each with its own caches and node_modules / target / venv per branch.

## Context

Monorepos amplify daft's value: a typical "feature" touches one or two packages
out of dozens, and switching branches in a single working tree triggers full
re-installs and cache invalidations. With daft, each branch keeps its own state.

The catch: monorepo caches (`node_modules/`, `pnpm-store/`, `target/`, etc.)
don't fit in `.git/`. They're either per-worktree (more disk, faster swaps) or
shared (less disk, slower invalidations). This recipe walks both.

## Prerequisites

- daft installed; shell integration enabled
- A monorepo using one of: pnpm workspaces, Turborepo, Nx, Bazel, Cargo
  workspaces

## Steps

### 1. Pick a layout

For monorepos, the **contained** layout is usually right — worktrees live as
siblings under a shared parent that holds the `.git/` and any shared tooling.

```bash
daft layout set contained
```
````

### 2. Decide caches: per-worktree vs shared

**Per-worktree (recommended starting point)** — each worktree has its own
`node_modules/`, `target/`, `.venv/`. Fast branch swaps, more disk usage. No
special config needed; daft's default is per-worktree.

**Shared cache** — single cache that all worktrees use. Less disk usage, but
cache invalidations take down all branches simultaneously. Configure via env
vars or symlinks per the cache's documentation.

### 3. Add a `daft.yml` to install workspace deps on worktree create

For a pnpm workspace:

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: install workspace deps
      run: pnpm install --frozen-lockfile
```

For a Turborepo:

```yaml
- name: install + warm
  run: |
    pnpm install --frozen-lockfile
    pnpm turbo run build --filter=...[origin/main] --cache-dir=.turbo-cache
```

(Adjust per your monorepo's tooling.)

Trust:

```bash
git add daft.yml
git commit -m "chore(daft): install workspace deps on worktree create"
git daft-hooks trust
```

### 4. Create a feature branch worktree

```bash
daft start feat/billing
```

The hook installs deps. `cd ~/work/my-project/feat/billing` and start working —
independent of any other branches you have open.

## Verifying it works

```bash
ls node_modules    # exists, populated
pnpm test          # works against this worktree's deps
```

In a sibling worktree (`cd ~/work/my-project/main`), the deps are independent —
installs in one don't affect the other.

## Variations

### Shared `pnpm-store` across worktrees

pnpm uses a content-addressable store; sharing it across worktrees is safe and
saves disk:

```bash
pnpm config set store-dir ~/.pnpm-store
```

Each worktree still has its own `node_modules/`, but the underlying packages are
shared.

### Sparse checkout per worktree

If your monorepo is huge, [#336](https://github.com/avihut/daft/issues/336)
tracks sparse-checkout profile support — define which packages a worktree
includes.

## Troubleshooting

- **Disk fills up fast** — switch to a shared content-addressable store (pnpm)
  or a shared cache (Cargo with `CARGO_TARGET_DIR`).
- **`pnpm install` is slow on every worktree create** — reuse the global pnpm
  store (variation above) and pnpm reuses already-fetched packages.

## Where to next

- **[mise](/cookbook/by-tooling/mise)** — pin Node/pnpm versions in `mise.toml`
- **[Layouts](/worktrees/layouts)** — the contained layout in detail
- **Sparse checkout** — [#336](https://github.com/avihut/daft/issues/336)
  (planned)

````

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/cookbook/by-scenario/monorepo.md
git commit -m "docs(cookbook): write monorepo recipe (anchor)"
````

### Task 6.6: Stub the remaining 10 recipes

**Files:** 10 stub files

Each stub is frontmatter + an outline. They render as valid pages with a "coming
soon" hint, so the sidebar entries don't 404 and the search index includes the
topics.

- [ ] **Step 1: Create the stub for each remaining recipe**

For each path below, write the same skeleton with the title + description +
pillars filled in:

| Path                                          | Title            | Description                                                                | Pillars              |
| --------------------------------------------- | ---------------- | -------------------------------------------------------------------------- | -------------------- |
| `docs/cookbook/by-tooling/nvm.md`             | daft + nvm       | Per-worktree Node versions via `.nvmrc`.                                   | `[worktrees, hooks]` |
| `docs/cookbook/by-tooling/pyenv.md`           | daft + pyenv     | Per-worktree Python versions via `.python-version`.                        | `[worktrees, hooks]` |
| `docs/cookbook/by-tooling/asdf.md`            | daft + asdf      | Multi-language version management via `.tool-versions`.                    | `[worktrees, hooks]` |
| `docs/cookbook/by-language/node.md`           | daft for Node.js | Patterns for `package.json`, `node_modules`, npm/pnpm/yarn under daft.     | `[worktrees, hooks]` |
| `docs/cookbook/by-language/python.md`         | daft for Python  | Patterns for virtualenvs, requirements, `pip` and `uv` under daft.         | `[worktrees, hooks]` |
| `docs/cookbook/by-language/rust.md`           | daft for Rust    | Patterns for `target/`, `cargo` caches, and incremental builds under daft. | `[worktrees, hooks]` |
| `docs/cookbook/by-language/go.md`             | daft for Go      | Patterns for `GOPATH`, modules, and build cache under daft.                | `[worktrees, hooks]` |
| `docs/cookbook/by-scenario/fork-workflow.md`  | Fork workflow    | daft + multi-remote for fork-based workflows (origin + upstream).          | `[worktrees]`        |
| `docs/cookbook/by-scenario/ci-integration.md` | CI integration   | Running daft hooks in CI for parity with local checks.                     | `[hooks]`            |

Stub template (replace placeholders for each file):

```markdown
---
title: <Title>
description: <Description>
pillars: <Pillars>
tooling: []
languages: []
---

# <Title>

> **Status: stub.** This recipe is being written. See
> [#398](https://github.com/avihut/daft/issues/398) for status.

## What this recipe will cover

- (Bullet outline of the recipe — 3-5 bullets)

## Why it matters

(Brief paragraph — when this recipe applies.)

## Where to next

- [Cookbook home](/cookbook/)
- [Anchor recipe: mise](/cookbook/by-tooling/mise)
- [Anchor recipe: direnv](/cookbook/by-tooling/direnv)
- [Anchor recipe: monorepo](/cookbook/by-scenario/monorepo)
```

For each file, fill in the bullet outline using your knowledge of the
tool/language/scenario:

- **nvm**: 4 bullets — install nvm, add `.nvmrc`, post-create hook running
  `nvm install`, verifying activation.
- **pyenv**: 4 bullets — install pyenv, add `.python-version`, post-create hook
  running `pyenv install`, verifying.
- **asdf**: 4 bullets — install asdf, add `.tool-versions`, post-create hook
  running `asdf install`, verifying.
- **node**: 5 bullets — picking a package manager, lockfile per-worktree,
  node_modules per-worktree vs shared store, npm scripts via daft hooks, link to
  mise recipe for version pinning.
- **python**: 5 bullets — virtualenv per worktree, lockfile (requirements.txt vs
  poetry.lock vs uv.lock), post-create hook to create venv, activating in shell,
  link to pyenv recipe.
- **rust**: 4 bullets — `target/` per worktree (default) vs shared via
  `CARGO_TARGET_DIR`, `cargo` cache, incremental builds, link to mise recipe.
- **go**: 4 bullets — `GOPATH` per worktree (default), modules, build cache via
  `GOCACHE`, link to mise.
- **fork-workflow**: 5 bullets — multi-remote layout, origin vs upstream,
  fetching upstream, syncing, the daft `multi-remote` command.
- **ci-integration**: 5 bullets — running daft hooks in GitHub Actions /
  CircleCI / GitLab CI, env parity, caching trust state, when CI hooks differ
  from local hooks.

- [ ] **Step 2: Verify + commit**

```bash
mise run docs:site:build
git add docs/cookbook/by-tooling/ docs/cookbook/by-language/ docs/cookbook/by-scenario/fork-workflow.md docs/cookbook/by-scenario/ci-integration.md
git commit -m "docs(cookbook): stub remaining 10 recipes with outlines

Stub recipes get frontmatter + outline + cross-links to anchor
recipes. They render as valid pages so sidebar entries don't 404
and the search index includes the topics. Full recipe content
follows as additional commits on this branch."
```

---

## Phase 7 — Per-pillar recipes filter

The Worktrees and Hooks pillars each have a "Recipes" sidebar entry that links
to `/cookbook/?pillar=<name>`. The filter is implemented as a small client-side
script on the cookbook index page that reads the `pillar` query param and shows
only matching recipes.

### Task 7.1: Add the recipe-list metadata script

**Files:**

- Create: `docs/.vitepress/theme/recipe-list.ts`
- Modify: `docs/.vitepress/theme/index.ts`

Build-time data: read all cookbook recipe frontmatters into a JSON catalog the
cookbook index can filter against.

- [ ] **Step 1: Create `docs/.vitepress/theme/recipe-list.ts`**

```typescript
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import matter from "gray-matter";

export type Recipe = {
  title: string;
  description: string;
  link: string;
  pillars: string[];
  tooling: string[];
  languages: string[];
};

export function loadRecipes(cookbookDir: string): Recipe[] {
  const recipes: Recipe[] = [];
  for (const category of ["by-tooling", "by-language", "by-scenario"]) {
    const dir = join(cookbookDir, category);
    if (!statSync(dir, { throwIfNoEntry: false })?.isDirectory()) continue;
    for (const file of readdirSync(dir)) {
      if (!file.endsWith(".md")) continue;
      const path = join(dir, file);
      const raw = readFileSync(path, "utf8");
      const { data } = matter(raw);
      recipes.push({
        title: data.title ?? file.replace(/\.md$/, ""),
        description: data.description ?? "",
        link: `/cookbook/${category}/${file.replace(/\.md$/, "")}`,
        pillars: Array.isArray(data.pillars) ? data.pillars : [],
        tooling: Array.isArray(data.tooling) ? data.tooling : [],
        languages: Array.isArray(data.languages) ? data.languages : [],
      });
    }
  }
  return recipes;
}
```

- [ ] **Step 2: Add `gray-matter` as a docs dev dependency**

```bash
cd docs
bun add -D gray-matter
```

- [ ] **Step 3: Verify it parses**

Add a temporary smoke test inline (not committed):

```bash
cd docs
bunx tsx -e 'import {loadRecipes} from "./.vitepress/theme/recipe-list.ts"; console.log(loadRecipes("./cookbook"))'
```

Expected: prints an array of recipe objects with pillars, tooling, languages
populated.

- [ ] **Step 4: Commit**

```bash
git add docs/.vitepress/theme/recipe-list.ts docs/package.json docs/bun.lock
git commit -m "docs(cookbook): build-time recipe metadata loader"
```

### Task 7.2: Add the filter rendering on `cookbook/index.md`

**Files:**

- Modify: `docs/cookbook/index.md`

VitePress supports `<script setup>` in markdown via Vue. Use it to read the
recipe list at build time and filter by URL query param at runtime.

- [ ] **Step 1: Append a Vue component block to `docs/cookbook/index.md`**

Add at the end of the file:

```markdown
## Filtered recipes

<RecipeFilter />

<script setup>
import { ref, computed, onMounted } from 'vue'
import { data as recipes } from '../.vitepress/data/recipes.data.ts'

const pillar = ref(null)

onMounted(() => {
  const params = new URLSearchParams(window.location.search)
  pillar.value = params.get('pillar')
})

const filtered = computed(() => {
  if (!pillar.value) return []
  return recipes.filter(r => r.pillars.includes(pillar.value))
})
</script>

<template v-if="pillar">
  <p>Showing recipes tagged <code>{{ pillar }}</code>:</p>
  <ul>
    <li v-for="r in filtered" :key="r.link">
      <a :href="r.link">{{ r.title }}</a> — {{ r.description }}
    </li>
  </ul>
  <p v-if="filtered.length === 0">No recipes yet for this pillar.</p>
</template>
```

- [ ] **Step 2: Create the data loader at
      `docs/.vitepress/data/recipes.data.ts`**

VitePress's data loader convention — `*.data.ts` files export `data` and are run
at build time.

```typescript
import { defineLoader } from "vitepress";
import { loadRecipes } from "../theme/recipe-list";
import { resolve } from "node:path";

export interface Data {
  title: string;
  description: string;
  link: string;
  pillars: string[];
  tooling: string[];
  languages: string[];
}

declare const data: Data[];
export { data };

export default defineLoader({
  watch: ["../../cookbook/**/*.md"],
  load() {
    const cookbookDir = resolve(__dirname, "../../cookbook");
    return loadRecipes(cookbookDir);
  },
});
```

- [ ] **Step 3: Verify build**

Run: `mise run docs:site:build`. Then `mise run docs:site:preview` and visit:

- `http://localhost:4173/cookbook/?pillar=worktrees`
- `http://localhost:4173/cookbook/?pillar=hooks`

Expected: the "Filtered recipes" section lists matching recipes; absence of
`?pillar=` shows the static taxonomy only.

- [ ] **Step 4: Commit**

```bash
git add docs/cookbook/index.md docs/.vitepress/data/recipes.data.ts
git commit -m "docs(cookbook): add per-pillar recipe filter via URL query param

The Worktrees and Hooks pillar sidebars now have a 'Recipes' entry
that deep-links to /cookbook/?pillar=<name>. The cookbook index
reads the query param and filters its recipe list accordingly. The
canonical home of all recipes remains /cookbook/ — the filter is
just a discoverability shortcut."
```

---

## Phase 8 — Cleanup

Final pass: drain the legacy `guide/` directory, restore `ignoreDeadLinks`,
light landing-page edit, agent-skill check, full link audit, build with strict
link checking.

### Task 8.1: Update Quick Start to narrate the worktree adoption arc

**Files:**

- Modify: `docs/getting-started/quick-start.md`

The Tutorial should explicitly walk the gradient (code → env → automation),
inviting the reader to deepen as they go.

- [ ] **Step 1: Read the current Quick Start**

Read `docs/getting-started/quick-start.md` in full.

- [ ] **Step 2: Restructure into 3 stages matching the gradient**

Replace the page content with:

````markdown
---
title: Quick Start
description:
  Get up and running with daft in minutes — covers the worktree adoption arc.
---

# Quick Start

This guide walks you through the **worktree adoption arc** — three stages of
daft adoption depth. You can stop at any stage and still get value.

## Stage 1: Code isolation

(Migrate the existing "Clone a Repository" + "Create branches" + "Clean up"
sections — keep the working examples.)

That's stage 1: every branch in its own directory, no stashing, no swapping.

## Stage 2: Environment isolation

Worktrees give you code isolation. Real-world branches usually need different
runtime versions, env vars, or running services. Add a tool to handle that:

- **Tool versions**: see the [mise recipe](/cookbook/by-tooling/mise).
- **Env vars / secrets**: see the [direnv recipe](/cookbook/by-tooling/direnv).
- **Both**: combine the two recipes.

Each worktree boots with the right env on `cd`.

## Stage 3: Automation

Setting up the env per worktree gets repetitive — a great fit for
[daft hooks](/hooks/). Hooks fire on worktree create/remove (plus other
code-evolution boundaries; see the [boundaries thesis](/hooks/)). Two examples:

```yaml
# daft.yml
worktree-post-create:
  jobs:
    - name: install deps
      run: pnpm install --frozen-lockfile
    - name: copy envrc
      run:
        "[ ! -f .envrc ] && cp .envrc.example .envrc && direnv allow . || true"
```
````

Trust the new `daft.yml`:

```bash
git daft-hooks trust
```

Now every new worktree boots with deps installed and `.envrc` ready.

## Where to next

- **Pillar overview:** [Worktrees](/worktrees/), [Hooks](/hooks/)
- **Recipes:** [Cookbook](/cookbook/)
- **Why daft:** [About → Why daft](/about/why-daft)

````

When migrating Stage 1 content, copy the existing prose verbatim where reasonable.

- [ ] **Step 3: Verify + commit**

```bash
mise run docs:site:build
git add docs/getting-started/quick-start.md
git commit -m "docs(getting-started): narrate the worktree adoption arc in Quick Start

Restructures Quick Start as a 3-stage tutorial mirroring the
gradient (code → env → automation). Stage 1 keeps the original
worktree walkthrough; Stages 2 and 3 link forward to cookbook
recipes and the Hooks pillar."
````

### Task 8.2: Light landing-page hero update

**Files:**

- Modify: `docs/index.md:1-50` (frontmatter `hero:` and `features:` blocks)

This is **not** the full landing-page revamp (#386 covers that). Just align the
hero copy and feature blurbs with the new pillar IA + thesis.

- [ ] **Step 1: Read current `docs/index.md`**

Note the hero / features structure.

- [ ] **Step 2: Update frontmatter**

Replace the `hero:` and `features:` blocks with:

```yaml
hero:
  name: daft
  text: Parallel dev, by default
  tagline:
    Each branch in its own directory. Hooks at every code-evolution boundary.
    Coordinate across repos. (One of these is still in design.)
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started/quick-start
    - theme: alt
      text: Why daft
      link: /about/why-daft
features:
  - title: Worktrees
    details:
      Every branch gets its own directory. Run feature-A and feature-B in
      different terminals at the same time — no stashing, no context switching.
    link: /worktrees/
    linkText: Worktrees pillar
  - title: Hooks
    details:
      Boundaries at every code-evolution stage — the local-parallel-to-CI
      surface. Today, worktree lifecycle. Soon, the full git-hooks lifecycle.
    link: /hooks/
    linkText: Hooks pillar
  - title: Cookbook
    details:
      Recipes for adopting daft alongside your existing tooling — mise, direnv,
      asdf, monorepos, fork workflows, CI integration.
    link: /cookbook/
    linkText: Cookbook
```

Leave the page body (below the frontmatter) as-is — #386 covers the full revamp.

- [ ] **Step 3: Verify + commit**

```bash
mise run docs:site:build
git add docs/index.md
git commit -m "docs: update landing hero copy + features for pillar IA

Aligns the landing page with the new pillar shape (Worktrees,
Hooks, Cookbook) and the parallel-dev thesis. Full marquee-features
revamp remains scoped to #386."
```

### Task 8.3: Audit `SKILL.md` for stale path references

**Files:**

- Modify (if needed): `SKILL.md`

The agent skill at `SKILL.md` may reference doc paths that have moved.

- [ ] **Step 1: Search for any doc path references**

```bash
grep -nE 'docs/|/guide/|/cli/|/getting-started/|daft\.avihu\.dev' SKILL.md
```

- [ ] **Step 2: Update any matches**

For each match, replace the legacy path with the new path. Most of `SKILL.md`
likely doesn't reference doc URLs; if it does, the rewrite is mechanical.

- [ ] **Step 3: Verify + commit (only if changes were made)**

```bash
git diff SKILL.md  # confirm changes
git add SKILL.md
git commit -m "docs(skill): update doc path references for new IA"
```

If no changes were needed, skip the commit.

### Task 8.4: Drain `docs/guide/` and re-enable strict link checking

**Files:**

- Delete: `docs/guide/` (the directory itself)
- Modify: `docs/.vitepress/config.ts:21` (revert `ignoreDeadLinks`)

By this point, every page in `guide/` has been migrated. Confirm the directory
is empty (or only contains files we forgot), then delete it. Re-enable strict
link checking.

- [ ] **Step 1: Confirm guide/ is empty**

```bash
ls docs/guide/
```

Expected: empty, or "No such file or directory" if it was already cleaned by
`git mv`. If files remain, audit them — any unmigrated page is a bug.

- [ ] **Step 2: Remove the empty directory**

```bash
rmdir docs/guide/ 2>/dev/null || true
```

- [ ] **Step 3: Revert `ignoreDeadLinks`**

In `docs/.vitepress/config.ts`, change `ignoreDeadLinks: true,` back to
`ignoreDeadLinks: false,` and remove the TODO comment from Task 1.2.

- [ ] **Step 4: Verify build with strict links**

Run: `mise run docs:site:build` Expected: build succeeds with no dead-link
warnings. If any dead links surface, fix them.

- [ ] **Step 5: Commit**

```bash
git add docs/.vitepress/config.ts
[ -d docs/guide ] || git add docs/  # captures the dir removal
git commit -m "docs: re-enable strict link checking after IA migration

All pages from docs/guide/ have been migrated to their new pillar
homes. Empty directory removed. ignoreDeadLinks reverted to false
so future PRs catch broken cross-links."
```

### Task 8.5: Final dev-server smoke test

**Files:** none

- [ ] **Step 1: Run the dev server**

```bash
mise run docs:site
```

- [ ] **Step 2: Visit each top-level page in a browser**

Confirm each renders without errors:

- `http://localhost:5173/`
- `http://localhost:5173/getting-started/quick-start`
- `http://localhost:5173/worktrees/`
- `http://localhost:5173/hooks/`
- `http://localhost:5173/cookbook/`
- `http://localhost:5173/cookbook/?pillar=worktrees`
- `http://localhost:5173/cookbook/?pillar=hooks`
- `http://localhost:5173/cookbook/by-tooling/mise`
- `http://localhost:5173/reference/`
- `http://localhost:5173/reference/cli/daft-clone` (verify the rewrite works)
- `http://localhost:5173/about/`
- `http://localhost:5173/about/why-daft`
- `http://localhost:5173/about/changelog`

- [ ] **Step 3: Confirm legacy URL redirects work**

(Cloudflare `_redirects` only fires in production. To test locally, you can use
`bunx wrangler pages dev docs/.vitepress/dist` if wrangler is installed —
optional.)

In production, after this branch ships:

- `https://daft.avihu.dev/guide/hooks` → `/hooks/`
- `https://daft.avihu.dev/guide/layouts` → `/worktrees/layouts`
- `https://daft.avihu.dev/cli/daft-clone` → `/reference/cli/daft-clone`
- `https://daft.avihu.dev/contributing` → `/about/contributing`

These can't be verified locally; verify in CF Pages preview after pushing.

- [ ] **Step 4: Stop the dev server**

`Ctrl-C`.

- [ ] **Step 5: Run a full clean build to be safe**

```bash
rm -rf docs/.vitepress/cache docs/.vitepress/dist
mise run docs:site:build
```

Expected: clean build, no warnings.

- [ ] **Step 6: No commit — this is verification only**

If everything passes, the IA restructure is complete on this branch.

---

## Self-review checklist

After plan completion, before opening the PR:

1. **Spec coverage:** Each item in the spec's "Sidebar structure," "Migration
   plan," "New pages to write," and "Coordination with #330" has a task above. ✓
2. **All legacy paths covered by redirects:** `_redirects` lists every legacy
   URL. ✓
3. **Cookbook recipe stubs render:** every sidebar entry resolves to a page
   (even if a stub). ✓
4. **Per-pillar Recipes filter works:** `?pillar=worktrees` and `?pillar=hooks`
   filter correctly. ✓
5. **Strict link checking passes:** `ignoreDeadLinks: false` and
   `mise run docs:site:build` is clean. ✓
6. **Hooks-as-boundaries thesis is on `hooks/index.md`** verbatim from spec. ✓
7. **`Why daft` page exists** with the parallel-dev thesis. ✓
8. **Glossary, FAQ, Troubleshooting, Comparison, Networking roadmap** all
   written. ✓
9. **Quick Start narrates the gradient.** ✓
10. **Landing hero copy updated.** ✓ (light update; full revamp is #386)
11. **CLI ref still autogenerates to `docs/cli/`** and surfaces at
    `/reference/cli/*` via rewrites. ✓ (no xtask change needed)
12. **`docs/guide/` is empty / removed.** ✓

## Out of plan (deferred — see spec)

- Merge hooks docs (#330's PR)
- Full git-hooks drop-in docs (#468's PR)
- Networking pillar content (#357's PR)
- Visual rebrand (#467)
- Full landing-page pitch revamp (#386)
- Filling in the 10 stub cookbook recipes (follow-up commits on this branch or a
  follow-up plan)
