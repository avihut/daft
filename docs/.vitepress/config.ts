import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vitepress";

const cargoToml = readFileSync(
  resolve(import.meta.dirname, "../../Cargo.toml"),
  "utf-8",
);
const version = cargoToml.match(/^version\s*=\s*"(.+?)"/m)?.[1] ?? "unknown";

export default defineConfig({
  vite: {
    resolve: {
      preserveSymlinks: true,
    },
  },
  title: "daft",
  description: "Git Extensions Toolkit",
  srcExclude: ["WEBSITE-BOOTSTRAP.md", "HISTORY.md"],
  ignoreDeadLinks: true,
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

      // Reformat changelog version headings:
      //   ## [1.0.22] - 2026-02-07  →  version without brackets, date on next line muted
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
          /^\[(.+?)\]\s*-\s*(\d{4}-\d{2}-\d{2})$/,
        );
        if (!match) {
          return defaultHeadingOpen(tokens, idx, options, env, self);
        }
        const [, ver, date] = match;
        inline.content = ver;
        inline.children = inline.children || [];
        inline.children = [{ ...inline.children[0], content: ver }];
        // Append date as a span after the heading via raw HTML
        const closeToken = tokens[idx + 2];
        if (closeToken && closeToken.type === "heading_close") {
          closeToken.attrSet = closeToken.attrSet || (() => {});
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
      { text: "CLI Reference", link: "/cli/git-worktree-clone" },
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
        text: "CLI Reference",
        collapsed: false,
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
          {
            text: "Configuration",
            items: [{ text: "daft-hooks", link: "/cli/git-daft-hooks" }],
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
