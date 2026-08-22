<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, shallowRef } from "vue";
import { createPlayer, observeVisibility } from "@dumbshow/core";
import type { Player, StepDef } from "../graph/pack";
import RepoDiagram from "../graph/RepoDiagram.vue";
import RepoTerminal from "../graph/RepoTerminal.vue";

// A landing point's demonstration: the same two viewers as the hero on one
// shared player, looping while on screen. Nothing here is point-specific —
// the script comes from the verb registry (landing/points.ts) and every
// capability is a player/viewer feature.
const props = defineProps<{ id: string; script: StepDef[] }>();

const player = shallowRef<Player | null>(null);
const stageEl = ref<HTMLElement | null>(null);
let stopVisibility: (() => void) | null = null;

onMounted(() => {
  const p = createPlayer({
    script: props.script,
    autoplay: true,
    loop: true,
    // One handle per point, distinct from the hero's `__daftPlayer`, so a
    // harness can drive each scene and the hero anchor keeps its player.
    devHandle: import.meta.env.DEV ? `__daftPoint_${props.id}` : undefined,
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
  <div ref="stageEl" class="dl-stage dl-point-stage">
    <RepoTerminal :script="script" :player="player" />
    <RepoDiagram class="dl-graph" :script="script" :player="player" />
  </div>
</template>
