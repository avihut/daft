import type { Theme } from "vitepress";
import DefaultTheme from "vitepress/theme";
import { h } from "vue";
import Playground from "./graph/Playground.vue";
import RepoDiagram from "./graph/RepoDiagram.vue";
import RepoTerminal from "./graph/RepoTerminal.vue";
import HomeLanding from "./landing/HomeLanding.vue";
import InstallCta from "./landing/InstallCta.vue";
import "./custom.css";
import "./changelog.css";
import "./graph/viewers.css";
import "./landing/home.css";

export default {
  extends: DefaultTheme,
  Layout() {
    // The landing (see theme/landing/CLAUDE.md): eyebrow and the install
    // line inside the VitePress hero, everything else below it.
    return h(DefaultTheme.Layout, null, {
      "home-hero-info-before": () =>
        h("p", { class: "dl-eyebrow" }, "One binary, every branch"),
      "home-hero-info-after": () => h(InstallCta),
      "home-hero-after": () => h(HomeLanding),
    });
  },
  enhanceApp({ app }) {
    // Global registration lets docs pages embed diagrams (and the
    // playground) straight from markdown — see theme/graph/CLAUDE.md.
    app.component("RepoDiagram", RepoDiagram);
    app.component("RepoTerminal", RepoTerminal);
    app.component("GraphPlayground", Playground);
  },
} satisfies Theme;
