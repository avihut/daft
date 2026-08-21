<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, shallowRef } from "vue";
import { createPlayer, observeVisibility } from "@dumbshow/core";
import { HERO_SCRIPT } from "../graph/hero-script";
import type { Player } from "../graph/pack";
import RepoDiagram from "../graph/RepoDiagram.vue";
import RepoTerminal from "../graph/RepoTerminal.vue";

// The hero is nothing bespoke: the landing script replayed on loop through
// the two standard viewers, which share one player so they cannot drift.
// Anything the hero "can do" is a player/viewer capability, not hero code.
const player = shallowRef<Player | null>(null);
const stageEl = ref<HTMLElement | null>(null);
let stopVisibility: (() => void) | null = null;

onMounted(() => {
  const p = createPlayer({
    script: HERO_SCRIPT,
    autoplay: true,
    loop: true,
    devHandle: import.meta.env.DEV ? "__daftPlayer" : undefined,
  });
  player.value = p;
  if (stageEl.value) stopVisibility = observeVisibility(stageEl.value, p);
});

onBeforeUnmount(() => {
  stopVisibility?.();
  player.value?.destroy();
  player.value = null;
});
</script>

<template>
  <div class="dl-wrap">
    <div ref="stageEl" class="dl-stage">
      <RepoTerminal :script="HERO_SCRIPT" :player="player" />
      <RepoDiagram class="dl-graph" :script="HERO_SCRIPT" :player="player" />
    </div>
    <p class="dl-cap">
      A <b>real daft session</b> — and your project taking shape as it runs.
      Click a command to step through it.
    </p>
    <div class="dl-trust">
      <a class="dl-tile is-git" href="/about/why-daft">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            class="stroke"
            d="M6 8v8M6.4 13.8c6.4-.6 10.2-1.6 11.2-4.6"
            fill="none"
            stroke-width="1.9"
            stroke-linecap="round"
          />
          <circle class="fill" cx="6" cy="5.4" r="2.5" />
          <circle class="fill" cx="6" cy="18.6" r="2.5" />
          <circle class="accent" cx="18" cy="6.6" r="2.5" />
        </svg>
        <h3>Integrates naturally with Git</h3>
        <p>
          Plain worktrees underneath, and every verb doubles as a git
          subcommand. Remove daft any time — your repo keeps working.
        </p>
      </a>
      <a class="dl-tile is-agents" href="/reference/agent-skill">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path
            class="fill"
            d="M10.4 4.2 12 10l5.8 1.6L12 13.2l-1.6 5.8-1.6-5.8L3 11.6 8.8 10Z"
          />
          <path class="accent" d="M18.4 15.4l.9 2.7 2.7.9-2.7.9-.9 2.7-.9-2.7-2.7-.9 2.7-.9Z" />
        </svg>
        <h3>Native to AI coding agents</h3>
        <p>
          The daft agent skill teaches them every verb — give each agent its
          own worktree and let them work in parallel.
        </p>
      </a>
    </div>
  </div>
</template>
