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
  // Under reduced motion the player itself lands on the story's last step,
  // paused (engine contract) — nothing to do here.
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
    <div ref="stageEl" class="dl-stage dl-hero-stage">
      <RepoTerminal :script="HERO_SCRIPT" :player="player" />
      <RepoDiagram class="dl-graph" :script="HERO_SCRIPT" :player="player" />
    </div>
    <p class="dl-cap">
      A <b>daft session replayed live</b> — not a video. Click any command to
      jump to it; the graph is what those commands did to the project.
    </p>
  </div>
</template>
