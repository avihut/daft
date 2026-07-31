<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { createRepoGraph, type RepoGraph } from "../repo-graph";

interface TermLine {
  kind: "cmd" | "out" | "ok";
  text: string;
}

interface Step {
  cmd: string;
  out: TermLine[];
  play?: (graph: RepoGraph) => void;
}

// The session replayed on loop. Every command is real daft; each one's
// graph effect fires the moment the command finishes typing.
const STEPS: Step[] = [
  {
    cmd: "daft clone git@github.com:acme/shop.git",
    out: [{ kind: "out", text: "cloned shop · worktree main/ ready" }],
    play: (g) => {
      const r = g.addRepo("shop", 0.38, 0.48);
      g.addWorktree(r, "main");
    },
  },
  {
    cmd: "daft start feature/checkout",
    out: [{ kind: "out", text: "branch + worktree created · pushed -u origin" }],
    play: (g) => g.addWorktree(0, "feature/checkout"),
  },
  {
    cmd: "daft start bugfix/tax-rounding",
    out: [{ kind: "out", text: "branch + worktree created · pushed -u origin" }],
    play: (g) => g.addWorktree(0, "bugfix/tax-rounding"),
  },
  {
    cmd: "daft clone git@github.com:acme/shop-api.git",
    out: [{ kind: "out", text: "cloned shop-api · worktree main/ ready" }],
    play: (g) => {
      const r = g.addRepo("shop-api", 0.74, 0.3);
      g.addWorktree(r, "main");
      g.relate(0, 1);
    },
  },
  {
    cmd: "daft merge feature/checkout",
    out: [
      { kind: "ok", text: "✓ fmt   ✓ clippy   ✓ tests" },
      { kind: "out", text: "merged into main · worktree removed" },
    ],
    play: (g) => g.merge(0, "feature/checkout"),
  },
  {
    cmd: "daft prune",
    out: [{ kind: "out", text: "pruned 1 merged branch" }],
  },
];

const FULL_TRANSCRIPT: TermLine[] = STEPS.flatMap((step) => [
  { kind: "cmd" as const, text: step.cmd },
  ...step.out,
]);

// SSR and no-JS render the full transcript; the animation replaces it on mount.
const lines = ref<TermLine[]>(FULL_TRANSCRIPT);
const typing = ref<string | null>(null);
const termEl = ref<HTMLElement | null>(null);
const canvasEl = ref<HTMLCanvasElement | null>(null);

let graph: RepoGraph | null = null;
let timers: number[] = [];
let stopped = false;

function later(fn: () => void, ms: number): void {
  if (stopped) return;
  timers.push(
    window.setTimeout(() => {
      // Stall instead of advancing while the tab is hidden.
      if (document.hidden) later(fn, 500);
      else fn();
    }, ms),
  );
}

function scrollTerm(): void {
  nextTick(() => {
    const el = termEl.value;
    if (el) el.scrollTop = el.scrollHeight;
  });
}

function runStep(index: number): void {
  if (index >= STEPS.length) {
    later(() => {
      lines.value = [];
      graph?.reset();
      later(() => runStep(0), 700);
    }, 4200);
    return;
  }
  const step = STEPS[index];
  typing.value = "";
  scrollTerm();
  let ci = 0;
  const type = () => {
    if (ci < step.cmd.length) {
      typing.value = step.cmd.slice(0, ++ci);
      later(type, 26 + Math.random() * 30);
    } else {
      later(() => {
        typing.value = null;
        lines.value.push({ kind: "cmd", text: step.cmd });
        if (graph) step.play?.(graph);
        let oi = 0;
        const emit = () => {
          if (oi < step.out.length) {
            lines.value.push(step.out[oi++]);
            scrollTerm();
            later(emit, 420);
          } else {
            later(() => runStep(index + 1), 900);
          }
        };
        emit();
      }, 350);
    }
  };
  type();
}

onMounted(() => {
  if (canvasEl.value) graph = createRepoGraph(canvasEl.value);
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduced) {
    // Static: full transcript stays, graph jumps to the final state.
    if (graph) for (const step of STEPS) step.play?.(graph);
    return;
  }
  lines.value = [];
  later(() => runStep(0), 600);
});

onBeforeUnmount(() => {
  stopped = true;
  for (const t of timers) window.clearTimeout(t);
  timers = [];
  graph?.destroy();
  graph = null;
});
</script>

<template>
  <div class="dl-wrap">
    <div class="dl-stage">
      <div ref="termEl" class="dl-term" aria-label="A daft session, replayed">
        <div class="dl-term-dots" aria-hidden="true"><i /><i /><i /></div>
        <div v-for="(line, i) in lines" :key="i" class="dl-ln" :class="`is-${line.kind}`">
          <span v-if="line.kind === 'cmd'" class="dl-prompt">$ </span>{{ line.text }}
        </div>
        <div v-if="typing !== null" class="dl-ln is-cmd">
          <span class="dl-prompt">$ </span>{{ typing }}<span class="dl-caret" />
        </div>
      </div>
      <div class="dl-graph"><canvas ref="canvasEl" /></div>
    </div>
    <p class="dl-cap">
      A <b>real daft session</b> — and your project taking shape as it runs.
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
