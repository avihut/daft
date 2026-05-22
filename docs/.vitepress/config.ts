import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vitepress";

const cargoToml = readFileSync(
  resolve(import.meta.dirname, "../../Cargo.toml"),
  "utf-8",
);
const version = cargoToml.match(/^version\s*=\s*"(.+?)"/m)?.[1] ?? "unknown";
const GITHUB_REPO = "https://github.com/avihut/daft";

export default defineConfig({
  vite: {
    resolve: {
      preserveSymlinks: true,
    },
  },
  title: "daft",
  description: "Git Extensions Toolkit",
  srcExclude: ["WEBSITE-BOOTSTRAP.md", "HISTORY.md", "superpowers/**"],
  ignoreDeadLinks: false,
  cleanUrls: true,
  rewrites: {
    "cli/:command.md": "reference/cli/:command.md",
  },
  lastUpdated: true,
  sitemap: {
    hostname: "https://daft.avihu.dev",
  },
  head: [
    ["link", { rel: "icon", type: "image/png", href: "/favicon.png" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "daft" }],
    ["meta", { property: "og:locale", content: "en_US" }],
    [
      "meta",
      {
        property: "og:image",
        content: "https://daft.avihu.dev/og-image.png",
      },
    ],
    ["meta", { property: "og:image:width", content: "1200" }],
    ["meta", { property: "og:image:height", content: "630" }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    [
      "meta",
      {
        name: "twitter:image",
        content: "https://daft.avihu.dev/og-image.png",
      },
    ],
  ],
  transformPageData(pageData) {
    const canonicalPath = pageData.relativePath
      .replace(/index\.md$/, "")
      .replace(/\.md$/, "");
    const canonicalUrl = `https://daft.avihu.dev/${canonicalPath}`;
    const title = pageData.frontmatter.title || pageData.title;
    const description =
      pageData.frontmatter.description || "Git Extensions Toolkit";

    pageData.frontmatter.head ??= [];
    pageData.frontmatter.head.push(
      ["link", { rel: "canonical", href: canonicalUrl }],
      ["meta", { property: "og:title", content: title }],
      ["meta", { property: "og:description", content: description }],
      ["meta", { property: "og:url", content: canonicalUrl }],
      ["meta", { name: "twitter:title", content: title }],
      ["meta", { name: "twitter:description", content: description }],
    );
  },
  markdown: {
    config: (md) => {
      // Escape angle-bracket placeholders like <branch>, <name>, etc.
      // that appear in CLI docs, preventing Vue from parsing them as HTML elements.
      const defaultRender =
        md.renderer.rules.html_inline || ((tokens, idx) => tokens[idx].content);
      md.renderer.rules.html_inline = (tokens, idx, options, env, self) => {
        const content = tokens[idx].content;
        // If it looks like a placeholder (e.g. <branch-name>, <BRANCH>),
        // escape it so Vue doesn't try to parse it as a component
        if (/^<[a-zA-Z][-a-zA-Z_]*>$/.test(content)) {
          return content.replace(/</g, "&lt;").replace(/>/g, "&gt;");
        }
        return defaultRender(tokens, idx, options, env, self);
      };

      // Strip the [Unreleased] section from the changelog so it never
      // appears on the deployed docs site (it's always empty on master).
      md.core.ruler.push("remove-unreleased", (state) => {
        const tokens = state.tokens;
        for (let i = 0; i < tokens.length; i++) {
          if (tokens[i].type !== "heading_open" || tokens[i].tag !== "h2")
            continue;
          const inline = tokens[i + 1];
          if (
            !inline ||
            inline.type !== "inline" ||
            !/^\[Unreleased\]/i.test(inline.content)
          )
            continue;
          // Find the next h2 (or end of tokens) and remove everything between
          let end = tokens.length;
          for (let j = i + 3; j < tokens.length; j++) {
            if (tokens[j].type === "heading_open" && tokens[j].tag === "h2") {
              end = j;
              break;
            }
          }
          tokens.splice(i, end - i);
          break;
        }
      });

      // Auto-link bare #NNN issue/PR references to GitHub.
      // Skips references already inside a markdown link.
      md.core.ruler.push("autolink-pr-references", (state) => {
        for (const blockToken of state.tokens) {
          if (blockToken.type !== "inline" || !blockToken.children) continue;

          const newChildren = [];
          let insideLink = false;

          for (const token of blockToken.children) {
            if (token.type === "link_open") insideLink = true;
            if (token.type === "link_close") insideLink = false;

            if (
              token.type !== "text" ||
              insideLink ||
              !/#\d+/.test(token.content)
            ) {
              newChildren.push(token);
              continue;
            }

            // Split text around #NNN patterns and wrap each in a link
            for (const part of token.content.split(/(#\d+)/)) {
              if (!part) continue;
              const prMatch = part.match(/^#(\d+)$/);
              if (prMatch) {
                const linkOpen = new state.Token("link_open", "a", 1);
                linkOpen.attrs = [
                  ["href", `${GITHUB_REPO}/pull/${prMatch[1]}`],
                ];
                newChildren.push(linkOpen);

                const text = new state.Token("text", "", 0);
                text.content = part;
                newChildren.push(text);

                const linkClose = new state.Token("link_close", "a", -1);
                newChildren.push(linkClose);
              } else {
                const text = new state.Token("text", "", 0);
                text.content = part;
                newChildren.push(text);
              }
            }
          }

          blockToken.children = newChildren;
        }
      });

      // Reformat changelog version headings:
      //   ## [1.0.22] - 2026-02-07            →  version + date on separate line
      //   ## [1.0.24](compare-url) - 2026-02-15  →  same treatment (with GitHub compare URL)
      const defaultHeadingOpen =
        md.renderer.rules.heading_open ||
        ((tokens, idx, options, _env, self) =>
          self.renderToken(tokens, idx, options));
      md.renderer.rules.heading_open = (tokens, idx, options, env, self) => {
        if (tokens[idx].tag !== "h2") {
          return defaultHeadingOpen(tokens, idx, options, env, self);
        }
        const inline = tokens[idx + 1];
        if (!inline || inline.type !== "inline") {
          return defaultHeadingOpen(tokens, idx, options, env, self);
        }
        const match = inline.content.match(
          /^\[(.+?)\](?:\(.*?\))?\s*-\s*(\d{4}-\d{2}-\d{2})$/,
        );
        if (!match) {
          return defaultHeadingOpen(tokens, idx, options, env, self);
        }
        const [, ver, date] = match;
        inline.content = ver;
        // Replace children with a single text token (handles both plain text
        // and link token sequences from the compare-URL format)
        const textToken = new inline.children[0].constructor("text", "", 0);
        textToken.content = ver;
        inline.children = [textToken];
        // Append date as a paragraph after the closing </h2>
        const closeToken = tokens[idx + 2];
        if (closeToken && closeToken.type === "heading_close") {
          const origClose =
            md.renderer.rules.heading_close ||
            ((t, i, o, _e, s) => s.renderToken(t, i, o));
          const origCloseOnce = origClose;
          md.renderer.rules.heading_close = (t, i, o, e, s) => {
            if (t[i] === closeToken) {
              md.renderer.rules.heading_close = origCloseOnce;
              return `</h2>\n<p class="changelog-date">${date}</p>\n`;
            }
            return origCloseOnce(t, i, o, e, s);
          };
        }
        return defaultHeadingOpen(tokens, idx, options, env, self);
      };
    },
  },
  themeConfig: {
    search: {
      provider: "local",
    },
    nav: [
      { text: "Worktrees", link: "/worktrees/" },
      { text: "Hooks", link: "/hooks/" },
      { text: "Recipes", link: "/recipes/" },
      { text: `v${version}`, link: "/about/changelog" },
      { text: "GitHub", link: "https://github.com/avihut/daft" },
    ],
    footer: {
      message:
        'Released under <a href="https://github.com/avihut/daft/blob/master/LICENSE-MIT">MIT</a> or <a href="https://github.com/avihut/daft/blob/master/LICENSE-APACHE">Apache-2.0</a>.',
      copyright: "Copyright © 2025-present Avihu Turzion",
    },
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
          {
            text: "Running daft from anywhere (-C)",
            link: "/worktrees/from-anywhere",
          },
          { text: "Merging across worktrees", link: "/worktrees/merging" },
          { text: "Shortcuts", link: "/worktrees/shortcuts" },
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
        ],
      },
      {
        text: "Recipes",
        items: [
          { text: "Overview", link: "/recipes/" },
          {
            text: "Adoption",
            collapsed: false,
            items: [
              {
                text: "Adopting from direnv",
                link: "/recipes/adopting-from-direnv",
              },
              {
                text: "Adopting from mise",
                link: "/recipes/adopting-from-mise",
              },
              {
                text: "Migrating from a bin/setup.sh ritual",
                link: "/recipes/walkthroughs/migrating-from-setup-sh",
              },
              {
                text: "Layering direnv on daft",
                link: "/recipes/layering-direnv",
              },
              {
                text: "Layering mise on daft",
                link: "/recipes/layering-mise",
              },
            ],
          },
          {
            text: "Walkthroughs",
            collapsed: false,
            items: [
              {
                text: "Rust binary with debug warmup",
                link: "/recipes/walkthroughs/rust-binary",
              },
              {
                text: "Node monorepo with services",
                link: "/recipes/walkthroughs/node-monorepo-services",
              },
              {
                text: "Python/uv with mise + sops",
                link: "/recipes/walkthroughs/python-uv-secrets",
              },
              {
                text: "GitHub Actions with daft hooks",
                link: "/recipes/walkthroughs/github-actions",
              },
            ],
          },
          {
            text: "Patterns: Setup",
            collapsed: false,
            items: [
              {
                text: "Toolchain bootstrap",
                link: "/recipes/toolchain-bootstrap",
              },
              {
                text: "Background warmup",
                link: "/recipes/background-warmup",
              },
              {
                text: "Env vars & secrets",
                link: "/recipes/env-vars-and-secrets",
              },
              {
                text: "Services with ports",
                link: "/recipes/services-with-ports",
              },
              {
                text: "Editor integration",
                link: "/recipes/editor-integration",
              },
            ],
          },
          {
            text: "Patterns: Steady state",
            collapsed: false,
            items: [
              { text: "Declarative envs", link: "/recipes/declarative-envs" },
              { text: "CI parity", link: "/recipes/ci-parity" },
            ],
          },
          {
            text: "Patterns: Teardown",
            collapsed: false,
            items: [
              {
                text: "Cleanup on remove",
                link: "/recipes/cleanup-on-remove",
              },
            ],
          },
          {
            text: "References",
            collapsed: true,
            items: [
              { text: "Sharing caches", link: "/recipes/sharing-caches" },
              { text: "Troubleshooting", link: "/recipes/troubleshooting" },
              {
                text: "Anti-pattern: shared mutable state",
                link: "/recipes/anti-patterns/shared-mutable-state",
              },
              {
                text: "Anti-pattern: secrets in hooks",
                link: "/recipes/anti-patterns/secrets-in-hooks",
              },
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
              { text: "daft (top-level)", link: "/reference/cli/daft" },
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
                  { text: "merge", link: "/reference/cli/daft-merge" },
                  { text: "prune", link: "/reference/cli/daft-prune" },
                  { text: "update", link: "/reference/cli/daft-update" },
                  { text: "carry", link: "/reference/cli/daft-carry" },
                  { text: "exec", link: "/reference/cli/daft-exec" },
                  { text: "eject", link: "/reference/cli/daft-eject" },
                  {
                    text: "repo remove",
                    link: "/reference/cli/daft-repo-remove",
                  },
                ],
              },
              {
                text: "Utilities",
                items: [
                  { text: "list", link: "/reference/cli/daft-list" },
                  { text: "doctor", link: "/reference/cli/daft-doctor" },
                  {
                    text: "release-notes",
                    link: "/reference/cli/daft-release-notes",
                  },
                  {
                    text: "shell-init",
                    link: "/reference/cli/daft-shell-init",
                  },
                  {
                    text: "completions",
                    link: "/reference/cli/daft-completions",
                  },
                  { text: "setup", link: "/reference/cli/daft-setup" },
                ],
              },
              {
                text: "Configuration",
                items: [
                  { text: "config", link: "/reference/cli/daft-config" },
                  { text: "hooks", link: "/reference/cli/git-daft-hooks" },
                  { text: "layout", link: "/reference/cli/daft-layout" },
                  {
                    text: "multi-remote",
                    link: "/reference/cli/daft-multi-remote",
                  },
                ],
              },
              {
                text: "Git Commands",
                collapsed: true,
                items: [
                  {
                    text: "Setup",
                    items: [
                      {
                        text: "worktree-clone",
                        link: "/reference/cli/git-worktree-clone",
                      },
                      {
                        text: "worktree-init",
                        link: "/reference/cli/git-worktree-init",
                      },
                      {
                        text: "flow-adopt",
                        link: "/reference/cli/git-worktree-flow-adopt",
                      },
                    ],
                  },
                  {
                    text: "Branching",
                    items: [
                      {
                        text: "worktree-checkout",
                        link: "/reference/cli/git-worktree-checkout",
                      },
                      {
                        text: "worktree-branch",
                        link: "/reference/cli/git-worktree-branch",
                      },
                      {
                        text: "worktree-branch-delete (deprecated)",
                        link: "/reference/cli/git-worktree-branch-delete",
                      },
                    ],
                  },
                  {
                    text: "Maintenance",
                    items: [
                      {
                        text: "worktree-sync",
                        link: "/reference/cli/git-worktree-sync",
                      },
                      {
                        text: "worktree-merge",
                        link: "/reference/cli/git-worktree-merge",
                      },
                      {
                        text: "worktree-list",
                        link: "/reference/cli/git-worktree-list",
                      },
                      {
                        text: "worktree-prune",
                        link: "/reference/cli/git-worktree-prune",
                      },
                      {
                        text: "worktree-fetch",
                        link: "/reference/cli/git-worktree-fetch",
                      },
                      {
                        text: "worktree-carry",
                        link: "/reference/cli/git-worktree-carry",
                      },
                      {
                        text: "worktree-exec",
                        link: "/reference/cli/git-worktree-exec",
                      },
                      {
                        text: "flow-eject",
                        link: "/reference/cli/git-worktree-flow-eject",
                      },
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
          { text: "Overview", link: "/about/" },
          { text: "Why daft", link: "/about/why-daft" },
          { text: "Glossary", link: "/about/glossary" },
          { text: "FAQ", link: "/about/faq" },
          { text: "Troubleshooting", link: "/about/troubleshooting" },
          { text: "Comparison", link: "/about/comparison" },
          { text: "Contributing", link: "/about/contributing" },
          { text: "Changelog", link: "/about/changelog" },
        ],
      },
    ],
  },
});
