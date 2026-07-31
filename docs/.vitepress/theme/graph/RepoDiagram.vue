<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { createPlayer, type Player, type StepDef } from "./engine";

const props = withDefaults(
  defineProps<{
    script: StepDef[];
    /** Show the play/pause control chip. */
    controls?: boolean;
    /** Start the timeline on mount (ignored under reduced motion). */
    autoplay?: boolean;
    /** Render the settled end state instead of playing — static diagrams. */
    still?: boolean;
  }>(),
  { controls: true, autoplay: true, still: false },
);

const emit = defineEmits<{ tick: [t: number]; step: [index: number] }>();

const canvasEl = ref<HTMLCanvasElement | null>(null);
const stepIdx = ref(0);
const playing = ref(false);
const titles = props.script.map((s) => s.title);
let player: Player | null = null;

onMounted(() => {
  if (!canvasEl.value) return;
  player = createPlayer({
    canvas: canvasEl.value,
    script: props.script,
    autoplay: props.autoplay && !props.still,
    onTick: (t) => emit("tick", t),
    onStep: (i) => {
      stepIdx.value = i;
      emit("step", i);
    },
    onPlayState: (p) => {
      playing.value = p;
    },
  });
  if (props.still) player?.settle(props.script.length - 1);
  // Dev-only handle so the timeline can be driven from the console/tests.
  if (import.meta.env.DEV && player) {
    Object.assign(window, { __daftPlayer: player });
  }
});

onBeforeUnmount(() => {
  player?.destroy();
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
  <div class="dg-panel">
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
