<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { StepDef } from "./engine";
import { VERBS, type VerbInvocation, type World } from "./verbs";

const props = defineProps<{
  invocations: VerbInvocation[];
  world: World;
  mapping: number[];
  steps: StepDef[];
}>();

const emit = defineEmits<{
  add: [inv: VerbInvocation];
  remove: [index: number];
  focus: [index: number];
  clear: [];
}>();

const selVerb = ref(VERBS[0].id);
const spec = computed(() => VERBS.find((v) => v.id === selVerb.value) ?? VERBS[0]);
const specFields = computed(() => spec.value.fields(props.world));
const formArgs = ref<Record<string, string>>({});
const copied = ref(false);

// Re-seed the form whenever the verb or the world changes: choices refresh
// to only-valid targets, and defaults advance (next branch, next repo).
watch(
  [selVerb, () => props.world],
  () => {
    const next: Record<string, string> = {};
    for (const f of specFields.value) next[f.key] = f.value;
    formArgs.value = next;
  },
  { immediate: true },
);

const canAdd = computed(() => spec.value.available(props.world));

function add(): void {
  if (!canAdd.value) return;
  emit("add", { verb: spec.value.id, args: { ...formArgs.value } });
}

function cmdOf(index: number): string {
  const stepIdx = props.mapping[index];
  if (stepIdx < 0)
    return `daft ${props.invocations[index].verb} — waiting on an earlier step`;
  const step = props.steps[stepIdx];
  for (const beat of step.beats) if ("cmd" in beat) return beat.cmd;
  return step.title;
}

async function copy(): Promise<void> {
  try {
    await navigator.clipboard.writeText(JSON.stringify(props.steps, null, 2));
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 1600);
  } catch {
    // Clipboard unavailable (permissions, insecure context) — nothing to do.
  }
}
</script>

<template>
  <div class="dgc">
    <div class="dgc-palette" role="group" aria-label="Verbs">
      <button
        v-for="v in VERBS"
        :key="v.id"
        type="button"
        class="dgc-verb"
        :class="{ on: v.id === selVerb }"
        :disabled="!v.available(world)"
        @click="selVerb = v.id"
      >
        {{ v.label }}
      </button>
    </div>
    <div class="dgc-form">
      <code class="dgc-syntax">{{ spec.syntax }}</code>
      <label v-for="f in specFields" :key="f.key" class="dgc-field">
        <span>{{ f.label }}</span>
        <select v-if="f.kind === 'choice'" v-model="formArgs[f.key]">
          <option v-for="c in f.choices" :key="c" :value="c">{{ c }}</option>
        </select>
        <input v-else v-model="formArgs[f.key]" type="text" @keyup.enter="add" />
      </label>
      <button type="button" class="dgc-add" :disabled="!canAdd" @click="add">
        Add
      </button>
    </div>
    <p v-if="!invocations.length" class="dgc-hint">
      Build a scenario verb by verb — it replays live below as you add, and
      every command becomes a checkpoint you can jump back to.
    </p>
    <ol v-else class="dgc-rows">
      <li
        v-for="(inv, i) in invocations"
        :key="i"
        class="dgc-row"
        :class="{ dead: mapping[i] < 0 }"
      >
        <button
          type="button"
          class="dgc-rowbtn"
          :disabled="mapping[i] < 0"
          @click="emit('focus', i)"
        >
          <i>{{ i + 1 }}</i>
          <code>{{ cmdOf(i) }}</code>
        </button>
        <button
          type="button"
          class="dgc-x"
          :aria-label="`Remove step ${i + 1}`"
          @click="emit('remove', i)"
        >
          ✕
        </button>
      </li>
    </ol>
    <div v-if="invocations.length" class="dgc-actions">
      <button type="button" class="dgc-ghost" @click="copy">
        {{ copied ? "Copied" : "Copy script" }}
      </button>
      <button type="button" class="dgc-ghost" @click="emit('clear')">
        Clear
      </button>
    </div>
  </div>
</template>

<style>
.dgc {
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
  padding: 12px 14px;
  margin-bottom: 14px;
}
.dgc-palette {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.dgc-verb {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  border-radius: 999px;
  padding: 3px 12px;
  cursor: pointer;
}
.dgc-verb.on {
  border-color: color-mix(in srgb, var(--daft-gold) 55%, var(--vp-c-divider));
  color: var(--vp-c-text-1);
}
.dgc-verb:disabled {
  opacity: 0.38;
  cursor: default;
}
.dgc-form {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
  margin-top: 10px;
}
.dgc-syntax {
  font-family: var(--vp-font-family-mono);
  font-size: 11.5px;
  color: var(--vp-c-text-3);
}
.dgc-field {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12.5px;
  color: var(--vp-c-text-2);
}
.dgc-field input,
.dgc-field select {
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  border-radius: 8px;
  padding: 3px 8px;
  font-size: 12.5px;
  font-family: var(--vp-font-family-mono);
}
.dgc-field input {
  width: 150px;
}
.dgc-add {
  border: 1px solid transparent;
  background: var(--vp-button-brand-bg);
  color: var(--vp-button-brand-text);
  border-radius: 8px;
  padding: 3px 14px;
  font-size: 12.5px;
  font-weight: 650;
  cursor: pointer;
}
.dgc-add:hover {
  background: var(--vp-button-brand-hover-bg);
}
.dgc-add:disabled {
  opacity: 0.38;
  cursor: default;
}
.dgc-hint {
  font-size: 12.5px;
  color: var(--vp-c-text-3);
  margin: 10px 0 0;
}
.dgc-rows {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin: 10px 0 0;
  padding: 0;
}
.dgc-row {
  display: flex;
  align-items: center;
  gap: 6px;
}
.dgc-rowbtn {
  display: flex;
  align-items: center;
  gap: 8px;
  border: 0;
  background: none;
  color: var(--vp-c-text-2);
  padding: 2px 4px;
  cursor: pointer;
  min-width: 0;
}
.dgc-rowbtn:hover {
  color: var(--vp-c-text-1);
}
.dgc-rowbtn i {
  font-style: normal;
  font-family: var(--vp-font-family-mono);
  font-size: 10px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--vp-c-bg);
  border: 1px solid var(--vp-c-divider);
  color: var(--vp-c-text-3);
  flex: none;
}
.dgc-rowbtn code {
  font-family: var(--vp-font-family-mono);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.dgc-row.dead .dgc-rowbtn {
  opacity: 0.45;
  cursor: default;
}
.dgc-x {
  border: 0;
  background: none;
  color: var(--vp-c-text-3);
  font-size: 11px;
  cursor: pointer;
  padding: 2px 6px;
}
.dgc-x:hover {
  color: var(--daft-rust-text);
}
.dgc-actions {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
.dgc-ghost {
  border: 1px solid var(--vp-c-divider);
  background: none;
  color: var(--vp-c-text-2);
  border-radius: 8px;
  padding: 3px 12px;
  font-size: 12px;
  cursor: pointer;
}
.dgc-ghost:hover {
  border-color: var(--vp-c-text-3);
  color: var(--vp-c-text-1);
}
.dgc-verb:focus-visible,
.dgc-add:focus-visible,
.dgc-rowbtn:focus-visible,
.dgc-x:focus-visible,
.dgc-ghost:focus-visible {
  outline: 2px solid var(--daft-gold);
  outline-offset: 2px;
}
</style>
