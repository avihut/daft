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
  srcExclude: ["WEBSITE-BOOTSTRAP.md", "HISTORY.md"],
  ignoreDeadLinks: false,
  cleanUrls: true,
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
      //   ## [1.0.24](compare-url) - 2026-02-15  →  same treatment (new release-plz format)
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
      { text: "Guide", link: "/getting-started/installation" },
      { text: "CLI Reference", link: "/cli/daft-clone" },
      { text: `v${version}`, link: "/changelog" },
      { text: "GitHub", link: "https://github.com/avihut/daft" },
    ],
    footer: {
      message:
        'Released under the <a href="https://github.com/avihut/daft/blob/master/LICENSE">MIT License</a>.',
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
          { text: "Agent Skill", link: "/guide/claude-skill" },
        ],
      },
      {
        text: "daft Commands",
        collapsed: false,
        items: [
          {
            text: "Setup",
            items: [
              { text: "clone", link: "/cli/daft-clone" },
              { text: "init", link: "/cli/daft-init" },
              { text: "adopt", link: "/cli/daft-adopt" },
            ],
          },
          {
            text: "Branching",
            items: [
              { text: "go", link: "/cli/daft-go" },
              { text: "start", link: "/cli/daft-start" },
              { text: "remove", link: "/cli/daft-remove" },
            ],
          },
          {
            text: "Maintenance",
            items: [
              { text: "prune", link: "/cli/daft-prune" },
              { text: "update", link: "/cli/daft-update" },
              { text: "carry", link: "/cli/daft-carry" },
              { text: "eject", link: "/cli/daft-eject" },
            ],
          },
          {
            text: "Utilities",
            items: [
              { text: "doctor", link: "/cli/daft-doctor" },
              { text: "release-notes", link: "/cli/daft-release-notes" },
              { text: "shell-init", link: "/cli/daft-shell-init" },
              { text: "completions", link: "/cli/daft-completions" },
              { text: "setup", link: "/cli/daft-setup" },
            ],
          },
          {
            text: "Configuration",
            items: [
              { text: "hooks", link: "/cli/git-daft-hooks" },
              { text: "multi-remote", link: "/cli/daft-multi-remote" },
            ],
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
              { text: "worktree-clone", link: "/cli/git-worktree-clone" },
              { text: "worktree-init", link: "/cli/git-worktree-init" },
              { text: "flow-adopt", link: "/cli/git-worktree-flow-adopt" },
            ],
          },
          {
            text: "Branching",
            items: [
              { text: "worktree-checkout", link: "/cli/git-worktree-checkout" },
              { text: "worktree-branch", link: "/cli/git-worktree-branch" },
              {
                text: "worktree-branch-delete (deprecated)",
                link: "/cli/git-worktree-branch-delete",
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
