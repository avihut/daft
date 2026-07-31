<script setup lang="ts">
import { nextTick, ref } from "vue";
import { compile } from "../graph/engine";
import { HERO_SCRIPT } from "../graph/hero-script";
import RepoDiagram from "../graph/RepoDiagram.vue";

// Pure and window-free, so it runs during SSR too: the server renders the
// full transcript (readable without JS) and the first client tick trims it
// back to wherever the timeline actually is.
const COMPILED = compile(HERO_SCRIPT);
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
const diagram = ref<InstanceType<typeof RepoDiagram> | null>(null);
let lastSig = "";

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
  diagram.value?.seekCheckpoint(step);
  scrollTerm(true);
}

// The daft verb renders gold — the shell mirrors the diagram's color law.
function daftPart(text: string): string {
  return text.startsWith("daft") ? "daft" : "";
}
function restPart(text: string): string {
  return text.startsWith("daft") ? text.slice(4) : text;
}
</script>

<template>
  <div class="dl-wrap">
    <div class="dl-stage">
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
            :aria-label="`Jump to step ${line.step + 1}: ${HERO_SCRIPT[line.step].title}`"
            :title="HERO_SCRIPT[line.step].title"
            @click.stop="jumpTo(line.step)"
          />
          <template v-if="line.kind === 'cmd'"><span class="dl-prompt">$ </span><span class="dl-daft">{{ daftPart(line.text) }}</span>{{ restPart(line.text) }}</template>
          <template v-else>{{ line.text }}</template>
        </div>
        <div v-if="typing !== null" class="dl-ln is-cmd">
          <span class="dl-prompt">$ </span><span class="dl-daft">{{ daftPart(typing) }}</span>{{ restPart(typing) }}<span class="dl-caret" />
        </div>
      </div>
      <RepoDiagram
        ref="diagram"
        class="dl-graph"
        :script="HERO_SCRIPT"
        @tick="onTick"
        @step="(i) => (activeStep = i)"
      />
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
