<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { createPlayer } from "@dumbshow/core";
import { CATALOG } from "./catalog";
import { GALLERY } from "./gallery";
import type { Player, StepDef } from "./pack";
import RepoDiagram from "./RepoDiagram.vue";
import RepoTerminal from "./RepoTerminal.vue";
import { buildScenario } from "./verbs";

const MODES = ["gallery", "verbs"] as const;
const LAYOUTS = ["both", "diagram", "terminal"] as const;
const RATES = [0.25, 0.5, 1, 2];

const mode = ref<(typeof MODES)[number]>("gallery");
const scriptId = ref(GALLERY[0].id);
const catalogId = ref(CATALOG[0].id);
const layout = ref<(typeof LAYOUTS)[number]>("both");
const rate = ref(1);
const loop = ref(true);

const galleryEntry = computed(
  () => GALLERY.find((g) => g.id === scriptId.value) ?? GALLERY[0],
);
const catalogEntry = computed(
  () => CATALOG.find((c) => c.id === catalogId.value) ?? CATALOG[0],
);
const demoBuild = computed(() => buildScenario(catalogEntry.value.demo));

const activeSteps = computed<StepDef[]>(() =>
  mode.value === "gallery" ? galleryEntry.value.script : demoBuild.value.steps,
);
// Viewers compile their script once at setup, so a script change must
// remount them — the key tracks the underlying script's identity.
const stageKey = computed(() =>
  mode.value === "gallery" ? `g:${scriptId.value}` : `v:${catalogId.value}`,
);

const player = shallowRef<Player | null>(null);
const playing = ref(false);
const stepIdx = ref(0);
const time = ref(0);
const duration = ref(0);
let cleanups: (() => void)[] = [];

function teardown(): void {
  for (const off of cleanups) off();
  cleanups = [];
  player.value?.destroy();
  player.value = null;
}

function boot(steps: StepDef[], after?: (p: Player) => void): void {
  teardown();
  duration.value = 0;
  time.value = 0;
  stepIdx.value = 0;
  playing.value = false;
  if (!steps.length) return;
  const p = createPlayer({
    script: steps,
    autoplay: true,
    loop: loop.value,
    devHandle: import.meta.env.DEV ? "__daftPlayer" : undefined,
  });
  p.setRate(rate.value);
  cleanups = [
    p.onFrame((t) => {
      time.value = t;
    }),
    p.onStep((i) => {
      stepIdx.value = i;
    }),
    p.onPlayState((on) => {
      playing.value = on;
    }),
  ];
  player.value = p;
  duration.value = p.compiled.duration;
  after?.(p);
  playing.value = p.playing();
  stepIdx.value = p.current();
  time.value = p.clock();
}

function reboot(): void {
  if (mode.value === "gallery") {
    boot(galleryEntry.value.script);
    return;
  }
  // Land on the verb's checkpoint — its context built instantly by event
  // replay — and play: the verb executes the moment the catalog opens it.
  const b = demoBuild.value;
  const focus = b.mapping[catalogEntry.value.focus] ?? b.steps.length - 1;
  boot(b.steps, (p) => {
    p.seekCheckpoint(Math.max(focus, 0));
    p.play();
  });
}

onMounted(reboot);
watch([mode, scriptId, catalogId], reboot);
watch(rate, (r) => player.value?.setRate(r));
watch(loop, (on) => player.value?.setLoop(on));
onBeforeUnmount(teardown);

function scrub(event: Event): void {
  const input = event.target as HTMLInputElement;
  player.value?.seek(Number(input.value));
}
</script>

<template>
  <div class="dgp">
    <header class="dgp-head">
      <h1>Diagram playground</h1>
      <p>
        Two rooms, one stage: replay the gallery, or open any verb's canonical
        animation — both through the same player, diagram, and terminal the
        landing page uses. Click a command in the terminal to land on its
        checkpoint, paused.
      </p>
    </header>

    <div class="dgp-bar">
      <div class="dgp-seg" role="group" aria-label="Mode">
        <button
          v-for="m in MODES"
          :key="m"
          type="button"
          class="dgp-segbtn"
          :class="{ on: mode === m }"
          @click="mode = m"
        >
          {{ m }}
        </button>
      </div>
      <label v-if="mode === 'gallery'" class="dgp-field">
        <span>Script</span>
        <select v-model="scriptId">
          <option v-for="g in GALLERY" :key="g.id" :value="g.id">
            {{ g.label }}
          </option>
        </select>
      </label>
      <div class="dgp-seg" role="group" aria-label="Viewers">
        <button
          v-for="l in LAYOUTS"
          :key="l"
          type="button"
          class="dgp-segbtn"
          :class="{ on: layout === l }"
          @click="layout = l"
        >
          {{ l }}
        </button>
      </div>
      <label class="dgp-field">
        <span>Rate</span>
        <select v-model.number="rate">
          <option v-for="r in RATES" :key="r" :value="r">{{ r }}×</option>
        </select>
      </label>
      <label class="dgp-check">
        <input v-model="loop" type="checkbox" />
        <span>Loop</span>
      </label>
    </div>

    <div v-if="mode === 'verbs'" class="dgp-verbs">
      <button
        v-for="c in CATALOG"
        :key="c.id"
        type="button"
        class="dgp-verb"
        :class="{ on: c.id === catalogId }"
        @click="catalogId = c.id"
      >
        {{ c.title }}
      </button>
    </div>
    <p v-if="mode === 'verbs'" class="dgp-verbinfo">
      <code>{{ catalogEntry.syntax }}</code>
      <span>{{ catalogEntry.blurb }}</span>
    </p>

    <div :key="stageKey" class="dl-stage dgp-stage" :class="`is-${layout}`">
      <RepoTerminal
        v-if="layout !== 'diagram'"
        :script="activeSteps"
        :player="player"
      />
      <RepoDiagram
        v-if="layout !== 'terminal'"
        class="dl-graph"
        :script="activeSteps"
        :player="player"
        :controls="false"
      />
    </div>

    <div class="dgp-transport">
      <button
        class="dgp-btn"
        type="button"
        :aria-label="playing ? 'Pause' : 'Play'"
        @click="player?.toggle()"
      >
        {{ playing ? "Pause" : "Play" }}
      </button>
      <button
        class="dgp-btn"
        type="button"
        aria-label="Previous step"
        @click="player?.prev()"
      >
        ←
      </button>
      <button
        class="dgp-btn"
        type="button"
        aria-label="Next step"
        @click="player?.next()"
      >
        →
      </button>
      <input
        class="dgp-scrub"
        type="range"
        min="0"
        :max="duration.toFixed(2)"
        step="0.01"
        :value="time.toFixed(2)"
        aria-label="Timeline"
        @input="scrub"
      />
      <span class="dgp-time">
        {{ time.toFixed(1) }}s / {{ duration.toFixed(1) }}s
      </span>
    </div>

    <div class="dgp-steps">
      <button
        v-for="(s, i) in activeSteps"
        :key="i"
        type="button"
        class="dgp-step"
        :class="{ on: i === stepIdx }"
        @click="player?.seekStep(i)"
      >
        <i>{{ i + 1 }}</i>{{ s.title }}
      </button>
    </div>
  </div>
</template>

<style>
.dgp {
  margin: 24px auto 72px;
  max-width: 1152px;
  padding: 0 24px;
}
@media (min-width: 640px) {
  .dgp {
    padding: 0 48px;
  }
}
.dgp-head h1 {
  font-size: 24px;
  font-weight: 800;
  letter-spacing: -0.02em;
  margin: 8px 0 6px;
}
.dgp-head p {
  font-size: 14px;
  color: var(--vp-c-text-2);
  max-width: 72ch;
  margin: 0 0 20px;
}
.dgp-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 14px;
  margin-bottom: 14px;
}
.dgp-field {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12.5px;
  color: var(--vp-c-text-2);
}
.dgp-field select {
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  border-radius: 8px;
  padding: 4px 8px;
  font-size: 12.5px;
}
.dgp-seg {
  display: flex;
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  overflow: hidden;
}
.dgp-segbtn {
  border: 0;
  background: none;
  color: var(--vp-c-text-3);
  font-size: 12px;
  padding: 5px 12px;
  cursor: pointer;
  text-transform: capitalize;
}
.dgp-segbtn.on {
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
}
.dgp-check {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12.5px;
  color: var(--vp-c-text-2);
}
.dgp-check input {
  accent-color: var(--daft-gold);
}
.dgp-verbs {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 10px;
}
.dgp-verb {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  border: 1px solid var(--vp-c-divider);
  background: none;
  color: var(--vp-c-text-2);
  border-radius: 999px;
  padding: 3px 12px;
  cursor: pointer;
}
.dgp-verb.on {
  border-color: color-mix(in srgb, var(--daft-gold) 55%, var(--vp-c-divider));
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg-soft);
}
.dgp-verbinfo {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 10px;
  font-size: 13px;
  color: var(--vp-c-text-2);
  margin: 0 0 12px;
}
.dgp-verbinfo code {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  color: var(--daft-gold-text);
}
.dgp-stage {
  margin-top: 0;
}
.dgp-stage.is-diagram,
.dgp-stage.is-terminal {
  grid-template-columns: 1fr;
}
.dgp-transport {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 14px;
}
.dgp-btn {
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-1);
  border-radius: 8px;
  padding: 4px 12px;
  font-size: 12.5px;
  cursor: pointer;
  white-space: nowrap;
}
.dgp-btn:hover {
  border-color: var(--vp-c-text-3);
}
.dgp-scrub {
  flex: 1;
  min-width: 120px;
  accent-color: var(--daft-gold);
}
.dgp-time {
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  color: var(--vp-c-text-3);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.dgp-steps {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 12px;
}
.dgp-step {
  display: flex;
  align-items: center;
  gap: 7px;
  border: 1px solid var(--vp-c-divider);
  background: none;
  color: var(--vp-c-text-2);
  border-radius: 999px;
  padding: 4px 12px 4px 5px;
  font-size: 12.5px;
  cursor: pointer;
}
.dgp-step i {
  font-style: normal;
  font-family: var(--vp-font-family-mono);
  font-size: 10.5px;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--vp-c-bg-soft);
  color: var(--vp-c-text-3);
}
.dgp-step.on {
  border-color: color-mix(in srgb, var(--daft-gold) 55%, var(--vp-c-divider));
  color: var(--vp-c-text-1);
}
.dgp-step.on i {
  background: var(--daft-gold);
  color: #1c1710;
}
.dgp-btn:focus-visible,
.dgp-segbtn:focus-visible,
.dgp-step:focus-visible,
.dgp-verb:focus-visible {
  outline: 2px solid var(--daft-gold);
  outline-offset: 2px;
}
</style>
