<script setup lang="ts">
import { computed, ref } from "vue";
import {
  AttributesForm,
  type ComposerDoc,
  type EditorSelection,
  freezePlacements,
  type ItemSelection,
  type VerbArgs,
  type VocabLang,
} from "@avihut/dumbshow";
import type { Compiled } from "../pack";
import {
  removeSeedRepo,
  removeSeedWt,
  renameEntity,
  scenePlacements,
  updateSeedWt,
} from "../seed";
import type { World } from "../verbs";
import type { EntitySelection } from "./elements";
import EntityAttributes from "./EntityAttributes.vue";
import { fileRows, registryRows, worldRows } from "./views";

/**
 * The right pane: state tabs (World | daft | Files) over the
 * always-visible Attributes form. This is a pack-side component — the tab
 * views and entity semantics are daft's own — presenting the generic
 * AttributesForm for timeline items and the daft EntityAttributes for
 * scene entities. Entity edits (renames, seed changes) apply here and
 * flow up as one generic `edit` event; the editor shell never learns the
 * daft-shaped verbs this pane speaks.
 */

const props = defineProps<{
  lang: VocabLang;
  selection: EditorSelection | null;
  doc: ComposerDoc;
  /** The final world — what the Attributes form resolves against. */
  world: World;
  /** The world at the playhead — what the views read. */
  viewWorld: World;
  /** The compiled scene — frozen geometry for renames. */
  compiled: Compiled;
}>();

const emit = defineEmits<{
  setArgs: [index: number, args: VerbArgs];
  setSilent: [index: number, silent: boolean];
  setChapter: [index: number, title: string];
  setBeat: [index: number, secs: number];
  remove: [index: number];
  /** A pack-applied document edit: the next document plus where to land. */
  edit: [next: ComposerDoc, land: number | null];
  /** Replace the selection (or clear it) after a pack-applied edit. */
  select: [sel: EditorSelection | null];
  notice: [message: string];
  selectEntity: [sel: EntitySelection];
}>();

const TABS = ["World", "daft", "Files"] as const;
const tab = ref<(typeof TABS)[number]>("World");

const rows = computed(() => worldRows(props.viewWorld));
const registry = computed(() => registryRows(props.viewWorld));
const files = computed(() => fileRows(props.viewWorld));

const entitySelection = computed<EntitySelection | null>(() =>
  props.selection?.type === "entity"
    ? (props.selection.sel as EntitySelection)
    : null,
);
const itemSelection = computed<ItemSelection | null>(() =>
  props.selection?.type === "item" ? props.selection : null,
);

/** The Attributes rename: freeze geometry, rewrite the name everywhere. */
function doRenameEntity(
  kind: "repo" | "branch",
  from: string,
  to: string,
): void {
  let next = freezePlacements(props.doc, scenePlacements(props.compiled));
  next = renameEntity(next, kind, from, to);
  emit("edit", next, null);
  const cur = entitySelection.value;
  if (cur) {
    if (kind === "repo" && cur.repo === from)
      emit("select", { type: "entity", sel: { ...cur, repo: to } });
    else if (kind === "branch" && cur.wt === from)
      emit("select", { type: "entity", sel: { ...cur, wt: to } });
  }
  emit("notice", `Renamed ${from} → ${to} everywhere`);
}

function doUpdateSeedWt(
  repo: string,
  branch: string,
  patch: { port?: string; agent?: boolean; merged?: boolean },
): void {
  emit("edit", updateSeedWt(props.doc, repo, branch, patch), null);
}

function doRemoveSeedRepo(name: string): void {
  emit("edit", removeSeedRepo(props.doc, name), null);
  emit("select", null);
}

function doRemoveSeedWt(repo: string, branch: string): void {
  emit("edit", removeSeedWt(props.doc, repo, branch), null);
  emit("select", null);
}

function isSelected(kind: "repo" | "wt", repo: string, wt?: string): boolean {
  const s = entitySelection.value;
  return (
    s !== null && s.kind === kind && s.repo === repo &&
    (kind === "repo" || s.wt === wt)
  );
}

function selectPath(path: string): void {
  const parts = path.split("/").filter((p) => p && p !== "~");
  if (parts.length === 1) emit("selectEntity", { kind: "repo", repo: parts[0] });
  else if (parts.length > 1)
    emit("selectEntity", {
      kind: "wt",
      repo: parts[0],
      wt: parts.slice(1).join("/"),
    });
}

function pathSelected(path: string): boolean {
  const parts = path.split("/").filter((p) => p && p !== "~");
  if (parts.length === 1) return isSelected("repo", parts[0]);
  if (parts.length > 1)
    return isSelected("wt", parts[0], parts.slice(1).join("/"));
  return false;
}
</script>

<template>
  <div class="dx-insp-body">
    <div class="dx-rtabs" role="tablist" aria-label="State views">
      <button
        v-for="name in TABS"
        :key="name"
        role="tab"
        type="button"
        :class="{ on: tab === name }"
        :aria-selected="tab === name"
        @click="tab = name"
      >
        {{ name }}
      </button>
    </div>

    <div v-if="tab === 'World'" class="dx-view">
      <p v-if="!rows.length" class="dx-empty" style="padding: 10px 2px">
        Nothing exists at the playhead yet.
      </p>
      <div v-else class="dx-tree">
        <button
          v-for="(row, i) in rows"
          :key="i"
          class="dx-trow"
          :class="{
            repo: row.kind === 'repo',
            wt: row.kind === 'wt',
            sel:
              row.kind === 'repo'
                ? isSelected('repo', row.repo)
                : isSelected('wt', row.repo, row.wt),
          }"
          type="button"
          @click="
            row.kind === 'repo'
              ? emit('selectEntity', { kind: 'repo', repo: row.repo })
              : emit('selectEntity', { kind: 'wt', repo: row.repo, wt: row.wt })
          "
        >
          <span class="dx-dot" :class="{ gold: row.main, hollow: row.merged }" />
          {{ row.kind === "repo" ? row.repo : row.wt }}
          <span
            v-if="row.agent"
            class="dx-agent-chip"
            title="An agent works here"
          />
          <span class="dx-grow" />
          <span v-if="row.port" class="dx-port">{{ row.port }}</span>
        </button>
      </div>
      <div
        v-for="([a, b], i) in viewWorld.rels"
        :key="`rel-${i}`"
        class="dx-rel-line"
      >
        <span class="dx-rel-dash" />{{ a }} ↔ {{ b }}
      </div>
    </div>

    <div v-else-if="tab === 'daft'" class="dx-view">
      <p v-if="!registry.length" class="dx-empty" style="padding: 10px 2px">
        The catalog is empty at the playhead.
      </p>
      <button
        v-for="r in registry"
        :key="r.name"
        class="dx-crow"
        :class="{ sel: isSelected('repo', r.name) }"
        type="button"
        @click="emit('selectEntity', { kind: 'repo', repo: r.name })"
      >
        <span class="dx-dot" />{{ r.name }}
        <span class="dx-path">{{ r.path }}</span>
      </button>
      <template v-if="viewWorld.rels.length">
        <h3 class="dx-view-h">Relations</h3>
        <div
          v-for="([a, b], i) in viewWorld.rels"
          :key="`drel-${i}`"
          class="dx-rel-line"
        >
          <span class="dx-rel-dash" />{{ a }} ↔ {{ b }}
        </div>
      </template>
      <p class="dx-attrs-note">
        What daft knows at the playhead — its repo registry and relations.
        Cloning registers, repo add adopts, relations come from repo link.
      </p>
    </div>

    <div v-else class="dx-view">
      <div class="dx-ftree">
        <button
          v-for="row in files"
          :key="row.path"
          class="dx-frow"
          :class="{
            dir: row.dir,
            sel: pathSelected(row.path),
            cwd: viewWorld.cwd === row.path,
          }"
          type="button"
          @click="selectPath(row.path)"
        >
          {{ row.text }}
        </button>
      </div>
      <p class="dx-attrs-note">
        The directories daft would actually make — clones land in ~, one
        folder per branch. A hollow worktree's folder stays until prune.
      </p>
    </div>

    <hr class="dx-split" />
    <EntityAttributes
      v-if="entitySelection"
      :selection="entitySelection"
      :doc="doc"
      :world="world"
      @rename-entity="doRenameEntity"
      @update-seed-wt="doUpdateSeedWt"
      @remove-seed-repo="doRemoveSeedRepo"
      @remove-seed-wt="doRemoveSeedWt"
    />
    <AttributesForm
      v-else
      :lang="lang"
      :selection="itemSelection"
      @set-args="(i, a) => emit('setArgs', i, a)"
      @set-silent="(i, s) => emit('setSilent', i, s)"
      @set-chapter="(i, t) => emit('setChapter', i, t)"
      @set-beat="(i, s) => emit('setBeat', i, s)"
      @remove="emit('remove', $event)"
    />
  </div>
</template>
