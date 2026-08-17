<script setup lang="ts">
import { computed } from "vue";
import type { ComposerDoc } from "@avihut/dumbshow";
import type { World } from "../verbs";
import type { EntitySelection } from "./elements";

/**
 * The Attributes form for daft entities — the pack side of the attributes
 * slot. A selected repo or worktree shows identity plus its seed fields
 * when the seed owns it; renames are entity renames, the document-wide
 * rewrite that makes every past command tell the new name. Timeline items
 * render through the generic AttributesForm instead — the inspector pane
 * decides which shows.
 */

const props = defineProps<{
  selection: EntitySelection;
  doc: ComposerDoc;
  /** The world after the whole timeline ran (derived.world). */
  world: World;
}>();

const emit = defineEmits<{
  renameEntity: [kind: "repo" | "branch", from: string, to: string];
  updateSeedWt: [
    repo: string,
    branch: string,
    patch: { port?: string; agent?: boolean; merged?: boolean },
  ];
  removeSeedRepo: [name: string];
  removeSeedWt: [repo: string, branch: string];
}>();

/** The selected entity, resolved against seed and final world. */
const entity = computed(() => {
  const s = props.selection;
  const seedRepo = props.doc.seed.repos.find((r) => r.name === s.repo);
  if (s.kind === "repo") {
    return {
      kind: "repo" as const,
      name: s.repo,
      seed: seedRepo ?? null,
      live: props.world.repos.find((r) => r.name === s.repo) ?? null,
    };
  }
  const wt = s.wt ?? "";
  return {
    kind: "wt" as const,
    repo: s.repo,
    name: wt,
    seed: seedRepo?.wts.find((w) => w.branch === wt) ?? null,
    live:
      props.world.repos
        .find((r) => r.name === s.repo)
        ?.wts.find((w) => w.branch === wt) ?? null,
  };
});

function commitEntityName(event: Event): void {
  const e = entity.value;
  const to = (event.target as HTMLInputElement).value.trim();
  if (!to || to === e.name) return;
  emit("renameEntity", e.kind === "repo" ? "repo" : "branch", e.name, to);
}

function commitSeedPort(event: Event): void {
  const e = entity.value;
  if (e.kind !== "wt") return;
  emit("updateSeedWt", e.repo, e.name, {
    port: (event.target as HTMLInputElement).value.trim(),
  });
}

function toggleSeed(flagName: "agent" | "merged"): void {
  const e = entity.value;
  if (e.kind !== "wt" || !e.seed) return;
  emit("updateSeedWt", e.repo, e.name, {
    [flagName]: !(e.seed[flagName] === true),
  });
}
</script>

<template>
  <section class="dx-attrs" aria-label="Attributes">
    <h3>Attributes</h3>

    <div class="dx-insp-head">
      <span class="dx-tag">{{
        entity.kind === "repo" ? "repo" : "worktree"
      }}</span>
      <b>{{
        entity.kind === "wt" ? `${entity.repo} · ${entity.name}` : entity.name
      }}</b>
      <span v-if="entity.seed" class="dx-mini">
        <button
          type="button"
          aria-label="Delete from the scene"
          @click="
            entity.kind === 'repo'
              ? emit('removeSeedRepo', entity.name)
              : emit('removeSeedWt', entity.repo, entity.name)
          "
        >
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true">
            <path d="M3.5 5h9M6.5 5V3.5h3V5m-5 0 .6 7.5h5.8L11.5 5" />
          </svg>
        </button>
      </span>
    </div>

    <div class="dx-field">
      <label for="dx-f-ename">{{
        entity.kind === "repo" ? "Name" : "Branch"
      }}</label>
      <input
        id="dx-f-ename"
        type="text"
        spellcheck="false"
        :value="entity.name"
        @change="commitEntityName"
        @keydown.enter="($event.target as HTMLInputElement).blur()"
      />
    </div>
    <p class="dx-attrs-note" style="margin: -3px 0 9px">
      Renaming rewrites every reference — seed, arguments, past commands.
    </p>

    <template v-if="entity.kind === 'wt' && entity.seed">
      <div class="dx-field">
        <label for="dx-f-eport">Port</label>
        <input
          id="dx-f-eport"
          type="text"
          spellcheck="false"
          placeholder="none"
          :value="entity.seed.port ?? ''"
          @change="commitSeedPort"
        />
      </div>
      <div class="dx-swrow">
        <span>Agent working here</span>
        <button
          class="dx-sw"
          :class="{ on: entity.seed.agent === true }"
          type="button"
          role="switch"
          :aria-checked="entity.seed.agent === true"
          aria-label="Agent working here"
          @click="toggleSeed('agent')"
        >
          <i />
        </button>
      </div>
      <div class="dx-swrow">
        <span>Merged (hollow)</span>
        <button
          class="dx-sw"
          :class="{ on: entity.seed.merged === true }"
          type="button"
          role="switch"
          :aria-checked="entity.seed.merged === true"
          aria-label="Merged"
          @click="toggleSeed('merged')"
        >
          <i />
        </button>
      </div>
    </template>

    <template v-else-if="entity.kind === 'wt'">
      <div class="dx-kv"><span>Port</span><span>{{ entity.live?.port ?? "none" }}</span></div>
      <div class="dx-kv"><span>Agent</span><span>{{ entity.live?.agent ? "yes" : "no" }}</span></div>
      <div class="dx-kv"><span>Merged</span><span>{{ entity.live?.merged ? "yes" : "no" }}</span></div>
      <p class="dx-attrs-note">
        Born from the timeline — select its step to edit arguments; state
        toggles happen through events and verbs.
      </p>
    </template>

    <p v-else-if="!entity.seed" class="dx-attrs-note">
      Born from the timeline — select its creating step to edit arguments.
    </p>
  </section>
</template>
