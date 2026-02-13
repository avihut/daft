---
title: Website Bootstrap Guide
description:
  Guide for setting up the daft documentation website in a separate repository
---

# Website Bootstrap Guide

This guide covers setting up a documentation website that renders the markdown
content from `daft/docs/`.

## Architecture

- **Content lives here**: All markdown documentation lives in `daft/docs/` and
  is maintained alongside the code
- **Website lives separately**: The static site generator and deployment config
  live in a separate repository
- **CLI docs are auto-generated**: The `docs/cli/` files are generated from clap
  definitions and committed to this repo

## Option A: VitePress

[VitePress](https://vitepress.dev/) is a Vue-powered static site generator
designed for documentation.

### Setup

```bash
npm init vitepress@latest daft-docs
cd daft-docs
```

### Configuration

`.vitepress/config.ts`:

```ts
import { defineConfig } from "vitepress";

export default defineConfig({
  title: "daft",
  description: "Git Extensions Toolkit",
  base: "/daft/",
  themeConfig: {
    nav: [
      { text: "Guide", link: "/getting-started/installation" },
      { text: "CLI Reference", link: "/cli/git-worktree-clone" },
      { text: "GitHub", link: "https://github.com/avihut/daft" },
    ],
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
        text: "Guide",
        items: [
          { text: "Worktree Workflow", link: "/guide/worktree-workflow" },
          {
            text: "Adopting Existing Repos",
            link: "/guide/adopting-existing-repos",
          },
          { text: "Hooks", link: "/guide/hooks" },
          { text: "Shortcuts", link: "/guide/shortcuts" },
          { text: "Multi-Remote", link: "/guide/multi-remote" },
          { text: "Configuration", link: "/guide/configuration" },
        ],
      },
      {
        text: "CLI Reference",
        collapsed: false,
        items: [
          { text: "Overview", items: [] },
          {
            text: "Setup",
            items: [
              { text: "worktree-clone", link: "/cli/git-worktree-clone" },
              { text: "worktree-init", link: "/cli/git-worktree-init" },
              { text: "flow-adopt", link: "/cli/git-worktree-flow-adopt" },
            ],
          },
          {
            text: "Branching",
            items: [
              { text: "worktree-checkout", link: "/cli/git-worktree-checkout" },
              {
                text: "worktree-checkout-branch",
                link: "/cli/git-worktree-checkout-branch",
              },
              {
                text: "worktree-checkout-branch-from-default",
                link: "/cli/git-worktree-checkout-branch-from-default",
              },
            ],
          },
          {
            text: "Maintenance",
            items: [
              { text: "worktree-prune", link: "/cli/git-worktree-prune" },
              { text: "worktree-fetch", link: "/cli/git-worktree-fetch" },
              { text: "worktree-carry", link: "/cli/git-worktree-carry" },
              { text: "flow-eject", link: "/cli/git-worktree-flow-eject" },
            ],
          },
          {
            text: "Utilities",
            items: [
              { text: "doctor", link: "/cli/daft-doctor" },
              { text: "release-notes", link: "/cli/daft-release-notes" },
            ],
          },
        ],
      },
      {
        text: "Project",
        items: [
          { text: "Contributing", link: "/contributing" },
          { text: "Changelog", link: "/changelog" },
        ],
      },
    ],
  },
});
```

## Option B: Starlight (Astro)

[Starlight](https://starlight.astro.build/) is an Astro-powered documentation
framework.

### Setup

```bash
npm create astro@latest -- --template starlight daft-docs
cd daft-docs
```

### Configuration

`astro.config.mjs`:

```js
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

export default defineConfig({
  integrations: [
    starlight({
      title: "daft",
      social: { github: "https://github.com/avihut/daft" },
      sidebar: [
        {
          label: "Getting Started",
          items: [
            { label: "Installation", slug: "getting-started/installation" },
            { label: "Quick Start", slug: "getting-started/quick-start" },
            {
              label: "Shell Integration",
              slug: "getting-started/shell-integration",
            },
          ],
        },
        {
          label: "Guide",
          items: [
            { label: "Worktree Workflow", slug: "guide/worktree-workflow" },
            {
              label: "Adopting Existing Repos",
              slug: "guide/adopting-existing-repos",
            },
            { label: "Hooks", slug: "guide/hooks" },
            { label: "Shortcuts", slug: "guide/shortcuts" },
            { label: "Multi-Remote", slug: "guide/multi-remote" },
            { label: "Configuration", slug: "guide/configuration" },
          ],
        },
        {
          label: "CLI Reference",
          autogenerate: { directory: "cli" },
        },
        {
          label: "Project",
          items: [
            { label: "Contributing", slug: "contributing" },
            { label: "Changelog", slug: "changelog" },
          ],
        },
      ],
    }),
  ],
});
```

## Content Sourcing

The recommended approach is a CI-based copy: a GitHub Action in the docs repo
clones the daft repo and copies `docs/` into the site's content directory.

### CI Copy Approach (Recommended)

In the docs repo, create `.github/workflows/deploy.yml`:

```yaml
name: Deploy Docs

on:
  push:
    branches: [main]
  repository_dispatch:
    types: [docs-update]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Fetch docs content from daft
        run: |
          git clone --depth 1 https://github.com/avihut/daft.git /tmp/daft
          # Copy docs content (adjust destination for your site generator)
          cp -r /tmp/daft/docs/* src/content/docs/  # Starlight
          # or: cp -r /tmp/daft/docs/* docs/        # VitePress

      - name: Install dependencies
        run: npm ci

      - name: Build
        run: npm run build

      - name: Deploy
        # Deploy to your hosting provider (GitHub Pages, Netlify, Vercel, etc.)
```

### Alternatives

- **Git submodule**: Add `daft` as a submodule in the docs repo. Requires manual
  submodule updates.
- **npm package**: Publish docs as an npm package. Adds complexity with little
  benefit.

## Triggering Docs Rebuild from daft Releases

Add a `repository_dispatch` step to daft's `release.yml` workflow:

```yaml
- name: Trigger docs rebuild
  uses: peter-evans/repository-dispatch@v3
  with:
    token: ${{ secrets.DOCS_REPO_TOKEN }}
    repository: avihut/daft-docs
    event-type: docs-update
    client-payload: '{"version": "${{ github.ref_name }}"}'
```

This ensures the docs site rebuilds whenever a new daft version is released.

## Sidebar Structure

The recommended sidebar organization groups content by user journey:

1. **Getting Started** - Installation, Quick Start, Shell Integration
2. **Guide** - Worktree Workflow, Adopting Repos, Hooks, Shortcuts,
   Multi-Remote, Configuration
3. **CLI Reference** - 12 commands grouped by category (Setup, Branching,
   Maintenance, Utilities)
4. **Project** - Contributing, Changelog
