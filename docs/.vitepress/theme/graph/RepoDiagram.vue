<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  attachDiagramView,
  createPlayer,
  type DiagramView,
  observeVisibility,
} from "@dumbshow/core";
import { DAFT_PACK, type Player, type StepDef } from "./pack";

const props = withDefaults(
  defineProps<{
    script: StepDef[];
    /**
     * Player from a composition host pairing this viewer with others. Pass a
     * ref that starts `null`; the viewer attaches when it arrives, and the
     * host owns play state and visibility gating. Omit the prop entirely and
     * the viewer creates and owns its own player.
     */
    player?: Player | null;
    /** Show the play/pause control chip. */
    controls?: boolean;
    /** Own player only: start the timeline on mount (off = reduced motion). */
    autoplay?: boolean;
    /** Render the settled end state instead of playing — static diagrams. */
    still?: boolean;
    /** Own player only: wrap back to the start when the timeline ends. */
    loop?: boolean;
  }>(),
  {
    player: undefined,
    controls: true,
    autoplay: true,
    still: false,
    loop: true,
  },
);

const emit = defineEmits<{ tick: [t: number]; step: [index: number] }>();

const panelEl = ref<HTMLElement | null>(null);
const canvasEl = ref<HTMLCanvasElement | null>(null);
const stepIdx = ref(0);
const playing = ref(false);
const titles = props.script.map((s) => s.title);

const owns = props.player === undefined;
let player: Player | null = null;
let view: DiagramView<unknown> | null = null;
const cleanups: (() => void)[] = [];

function attach(p: Player): void {
  if (view || !canvasEl.value) return;
  player = p;
  view = attachDiagramView(canvasEl.value, p, DAFT_PACK.scene);
  stepIdx.value = p.current();
  playing.value = p.playing();
  cleanups.push(
    p.onFrame((t) => emit("tick", t)),
    p.onStep((i) => {
      stepIdx.value = i;
      emit("step", i);
    }),
    p.onPlayState((on) => {
      playing.value = on;
    }),
  );
}

onMounted(() => {
  if (owns) {
    const p = createPlayer({
      script: props.script,
      autoplay: props.autoplay && !props.still,
      loop: props.loop,
      devHandle: import.meta.env.DEV ? "__daftPlayer" : undefined,
    });
    attach(p);
    if (props.still) p.settle(props.script.length - 1);
    else if (panelEl.value) cleanups.push(observeVisibility(panelEl.value, p));
  } else {
    watch(
      () => props.player,
      (p) => {
        if (p) attach(p);
      },
      { immediate: true },
    );
  }
});

onBeforeUnmount(() => {
  for (const off of cleanups) off();
  view?.destroy();
  if (owns) player?.destroy();
  view = null;
  player = null;
});

defineExpose({
  seekCheckpoint(index: number) {
    player?.seekCheckpoint(index);
  },
  toggle() {
    player?.toggle();
  },
});
</script>

<template>
  <div ref="panelEl" class="dg-panel">
    <canvas ref="canvasEl" />
    <div v-if="controls" class="dg-controls">
      <button
        class="dg-btn"
        type="button"
        :aria-label="playing ? 'Pause' : 'Play'"
        @click="player?.toggle()"
      >
        <svg v-if="playing" viewBox="0 0 12 12" aria-hidden="true">
          <path d="M2.6 1.5h2.4v9H2.6zM7 1.5h2.4v9H7z" />
        </svg>
        <svg v-else viewBox="0 0 12 12" aria-hidden="true">
          <path d="M3.2 1.6v8.8L10.2 6z" />
        </svg>
      </button>
      <span class="dg-step-title">{{ titles[stepIdx] }}</span>
    </div>
  </div>
</template>
