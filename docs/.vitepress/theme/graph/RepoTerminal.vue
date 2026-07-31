<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import {
  compile,
  createPlayer,
  observeVisibility,
  type Player,
  type StepDef,
} from "./engine";

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

// Pure and window-free, so it runs during SSR too: the server renders the
// full transcript (readable without JS) and the first client sync trims it
// back to wherever the timeline actually is.
const COMPILED = compile(props.script);
const ALL_LINES = COMPILED.term;

interface ShownLine {
  kind: string;
  text: string;
  step: number;
  checkpoint: boolean;
}

function shown(count: number): ShownLine[] {
  return ALL_LINES.slice(0, count).map((l) => ({
    kind: l.kind,
    text: l.text,
    step: l.step,
    checkpoint: l.checkpoint,
  }));
}

const lines = ref<ShownLine[]>(shown(ALL_LINES.length));
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
  let count = 0;
  let typingText: string | null = null;
  for (const line of ALL_LINES) {
    if (line.at > t) break;
    if (line.kind === "cmd" && t < line.typed) {
      const progress = (t - line.at) / (line.typed - line.at);
      typingText = line.text.slice(
        0,
        Math.floor(line.text.length * Math.max(0, progress)),
      );
      break;
    }
    count++;
  }
  const sig = `${count}:${typingText === null ? -1 : typingText.length}`;
  if (sig === lastSig) return;
  lastSig = sig;
  const grew =
    count > lines.value.length || (typingText !== null && typingText.length > 0);
  lines.value = shown(count);
  typing.value = typingText;
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
      @click="line.kind === 'cmd' && jumpTo(line.step)"
    >
      <button
        v-if="line.checkpoint"
        class="dl-chk"
        :class="{ on: line.step === activeStep }"
        type="button"
        :aria-label="`Jump to step ${line.step + 1}: ${script[line.step].title}`"
        :title="script[line.step].title"
        @click.stop="jumpTo(line.step)"
      />
      <template v-if="line.kind === 'cmd'"><span class="dl-prompt">$ </span><span class="dl-daft">{{ daftPart(line.text) }}</span>{{ restPart(line.text) }}</template>
      <template v-else>{{ line.text }}</template>
    </div>
    <div v-if="typing !== null" class="dl-ln is-cmd">
      <span class="dl-prompt">$ </span><span class="dl-daft">{{ daftPart(typing) }}</span>{{ restPart(typing) }}<span class="dl-caret" />
    </div>
  </div>
</template>
