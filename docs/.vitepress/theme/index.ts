import type { Theme } from "vitepress";
import DefaultTheme from "vitepress/theme";
import { h } from "vue";
import HomeLanding from "./components/HomeLanding.vue";
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
} satisfies Theme;
