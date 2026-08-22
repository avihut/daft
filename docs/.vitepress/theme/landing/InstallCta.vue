<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import {
  detectPlatform,
  INSTALL,
  INSTALL_DOCS,
  type Platform,
  QUICK_START,
} from "./install";

// The fold's call to action is the install line itself: one command per
// platform, a copy button, and nothing else to decide. The default tab is a
// guess from the user agent after mount (SSR renders macOS); the tabs stay
// clickable either way.
const props = withDefaults(defineProps<{ compact?: boolean }>(), {
  compact: false,
});

const active = ref<Platform>("macos");
const copied = ref(false);
let timer: ReturnType<typeof setTimeout> | null = null;

const line = computed(
  () => INSTALL.find((l) => l.id === active.value) ?? INSTALL[0],
);

onMounted(() => {
  active.value = detectPlatform(navigator.userAgent, navigator.platform);
});
onBeforeUnmount(() => {
  if (timer) clearTimeout(timer);
});

async function copy(): Promise<void> {
  try {
    await navigator.clipboard.writeText(line.value.command);
  } catch {
    // No clipboard (insecure context, permissions): the command is plain
    // selectable text right there — nothing to recover from.
    return;
  }
  copied.value = true;
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    copied.value = false;
  }, 1600);
}
</script>

<template>
  <div class="dl-install" :class="{ 'is-compact': props.compact }">
    <div class="dl-install-tabs" role="tablist" aria-label="Platform">
      <button
        v-for="l in INSTALL"
        :key="l.id"
        type="button"
        role="tab"
        class="dl-install-tab"
        :class="{ on: l.id === active }"
        :aria-selected="l.id === active"
        @click="active = l.id"
      >
        {{ l.label }}
      </button>
    </div>
    <div class="dl-install-row">
      <code class="dl-install-cmd" :data-platform="line.id"><span class="dl-prompt">{{ line.shell === "pwsh" ? "> " : "$ " }}</span>{{ line.command }}</code>
      <button
        type="button"
        class="dl-install-copy"
        :class="{ done: copied }"
        :aria-label="copied ? 'Copied' : 'Copy install command'"
        @click="copy"
      >
        {{ copied ? "Copied" : "Copy" }}
      </button>
    </div>
  </div>
  <p v-if="!props.compact" class="dl-install-meta">
    MIT or Apache-2.0 · <a :href="INSTALL_DOCS">All install methods</a> ·
    <a :href="QUICK_START">Quick start</a>
  </p>
</template>
