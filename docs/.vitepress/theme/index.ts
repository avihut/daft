import type { Theme } from "vitepress";
import DefaultTheme from "vitepress/theme";
import { h } from "vue";
import HomeLanding from "./components/HomeLanding.vue";
import Playground from "./graph/Playground.vue";
import RepoDiagram from "./graph/RepoDiagram.vue";
import RepoTerminal from "./graph/RepoTerminal.vue";
import "./custom.css";
import "./changelog.css";
import "./home.css";

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      "home-hero-info-before": () =>
        h("p", { class: "dl-eyebrow" }, "One binary, every branch"),
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
