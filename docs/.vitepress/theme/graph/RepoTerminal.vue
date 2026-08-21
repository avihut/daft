<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  compile,
  createPlayer,
  observeVisibility,
  type TermLine,
  transcriptAt,
  visibleLines,
} from "@dumbshow/core";
import type { Player, StepDef } from "./pack";

const props = withDefaults(
  defineProps<{
    script: StepDef[];
    /**
     * Player from a composition host — same contract as RepoDiagram: a ref
     * that starts `null`, attached on arrival. Omit the prop entirely and
     * the viewer creates and owns its own (headless) player.
     */
    player?: Player | null;
    /** Own player only: start the timeline on mount. */
    autoplay?: boolean;
    /** Own player only: wrap back to the start when the timeline ends. */
    loop?: boolean;
  }>(),
  { player: undefined, autoplay: true, loop: true },
);

// Compile + transcript are pure and window-free, so they run during SSR
// too: the server renders the full transcript (readable without JS) and the
// first client sync trims it back to wherever the timeline actually is.
const COMPILED = compile(props.script);
const ALL_LINES = COMPILED.term;

const lines = ref<TermLine[]>(visibleLines(ALL_LINES, ALL_LINES.length));
const typing = ref<string | null>(null);
const activeStep = ref(0);
const termEl = ref<HTMLElement | null>(null);
let lastSig = "";

const owns = props.player === undefined;
let player: Player | null = null;
const cleanups: (() => void)[] = [];

function scrollTerm(force: boolean): void {
  nextTick(() => {
    const el = termEl.value;
    if (!el) return;
    // Follow the tail only when the reader is already there — a user who
    // scrolled up to reread stays where they are.
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 56;
    if (force || nearBottom) el.scrollTop = el.scrollHeight;
  });
}

function onTick(t: number): void {
  const shown = transcriptAt(ALL_LINES, t);
  const sig = `${shown.count}:${shown.typing === null ? -1 : shown.typing.length}`;
  if (sig === lastSig) return;
  lastSig = sig;
  const grew =
    shown.count > lines.value.length ||
    (shown.typing !== null && shown.typing.length > 0);
  lines.value = visibleLines(ALL_LINES, shown.count);
  typing.value = shown.typing;
  if (grew) scrollTerm(false);
}

function jumpTo(step: number): void {
  player?.seekCheckpoint(step);
  scrollTerm(true);
}

function attach(p: Player): void {
  if (player) return;
  player = p;
  activeStep.value = p.current();
  cleanups.push(
    p.onFrame(onTick),
    p.onStep((i) => {
      activeStep.value = i;
    }),
  );
  onTick(p.clock());
}

onMounted(() => {
  if (owns) {
    const p = createPlayer({
      script: props.script,
      autoplay: props.autoplay,
      loop: props.loop,
      devHandle: import.meta.env.DEV ? "__daftPlayer" : undefined,
    });
    attach(p);
    if (termEl.value) cleanups.push(observeVisibility(termEl.value, p));
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
  if (owns) player?.destroy();
  player = null;
});

// The daft verb renders gold — the shell mirrors the diagram's color law.
function daftPart(text: string): string {
  return text.startsWith("daft") ? "daft" : "";
}
function restPart(text: string): string {
  return text.startsWith("daft") ? text.slice(4) : text;
}
</script>

<template>
  <div ref="termEl" class="dl-term" aria-label="A daft session, replayed">
    <div class="dl-term-dots" aria-hidden="true"><i /><i /><i /></div>
    <div
      v-for="(line, i) in lines"
      :key="i"
      class="dl-ln"
      :class="[`is-${line.kind}`, { 'is-active-step': line.step === activeStep }]"
      :role="line.kind === 'cmd' ? 'button' : undefined"
      :tabindex="line.kind === 'cmd' ? 0 : undefined"
      :title="line.kind === 'cmd' ? script[line.step].title : undefined"
      @click="line.kind === 'cmd' && jumpTo(line.step)"
      @keydown.enter.prevent="line.kind === 'cmd' && jumpTo(line.step)"
      @keydown.space.prevent="line.kind === 'cmd' && jumpTo(line.step)"
    >
      <template v-if="line.kind === 'cmd'"><span class="dl-prompt">$ </span><span class="dl-daft">{{ daftPart(line.text) }}</span>{{ restPart(line.text) }}</template>
      <template v-else>{{ line.text }}</template>
    </div>
    <div v-if="typing !== null" class="dl-ln is-cmd">
      <span class="dl-prompt">$ </span><span class="dl-daft">{{ daftPart(typing) }}</span>{{ restPart(typing) }}<span class="dl-caret" />
    </div>
  </div>
</template>
