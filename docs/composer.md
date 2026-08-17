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
// and the editor (the dumbshow package) speaks only that contract. Host
// chrome arrives as props — the docs back link, the vitepress dark-mode
// ref, the daft file tag (draft key + `.daft.json` downloads), and the
// dev-only window player handle the UI tests drive. Imported locally (not
// registered in enhanceApp) so the editor stays out of every other page's
// bundle; ClientOnly because the composer is a browser-only surface
// (canvas, localStorage, file pickers).
import "@avihut/dumbshow/style.css";
import { DAFT_PACK } from "./.vitepress/theme/graph/pack";
// The editor needs the writable REF, so bind it as a property access —
// a destructured top-level `isDark` would be template-unwrapped into a
// plain boolean and the toolbar's theme toggle would break.
const vp = useData();
const back = { href: "/", label: "Back to the daft docs", text: "daft" };
const devHandle = import.meta.env.DEV ? "__daftPlayer" : undefined;
const ComposerApp = defineAsyncComponent(() =>
  import("@avihut/dumbshow").then((m) => m.ComposerApp),
);
const InspectorPane = defineAsyncComponent(
  () => import("./.vitepress/theme/graph/composer/InspectorPane.vue"),
);
</script>

<ClientOnly><ComposerApp :lang="DAFT_PACK" :inspector="InspectorPane" :back="back" :is-dark="vp.isDark" :dev-handle="devHandle" file-tag="daft" /></ClientOnly>
