---
title: Diagram composer
description:
  Build daft diagrams and animated stories visually — drag elements, type
  commands, scrub the timeline, export stills and animations.
layout: false
---

<script setup>
import { defineAsyncComponent } from "vue";
import { useData } from "vitepress";
// The page is where daft meets the editor: it hands the language pack in,
// and the editor (@dumbshow/vue over @dumbshow/core) speaks only that contract. Host
// chrome arrives as props — the docs back link, the vitepress dark-mode
// ref as a v-model, the daft file tag (draft key + `.daft.json` downloads), and the
// dev-only window player handle the UI tests drive. Imported locally (not
// registered in enhanceApp) so the editor stays out of every other page's
// bundle; ClientOnly because the composer is a browser-only surface
// (canvas, localStorage, file pickers).
import "@dumbshow/core/style.css";
import { DAFT_PACK } from "./.vitepress/theme/graph/pack";
// The theme is a v-model on the vitepress ref's value: the toolbar's
// toggle asks through `update:isDark`, and vitepress flips the class.
const vp = useData();
const back = { href: "/", label: "Back to the daft docs", text: "daft" };
const devHandle = import.meta.env.DEV ? "__daftPlayer" : undefined;
const ComposerApp = defineAsyncComponent(() =>
  import("@dumbshow/vue").then((m) => m.ComposerApp),
);
const InspectorPane = defineAsyncComponent(
  () => import("./.vitepress/theme/graph/composer/InspectorPane.vue"),
);
</script>

<ClientOnly><ComposerApp :lang="DAFT_PACK" :inspector="InspectorPane" :back="back" v-model:is-dark="vp.isDark.value" :dev-handle="devHandle" file-tag="daft" /></ClientOnly>
