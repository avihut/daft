/**
 * Docs embed — the compiled script wrapped in daft's globally registered
 * viewers, ready to paste into any docs page. This is the pack side of
 * the export menu (DiagramLanguage.embedSnippet): only the language knows
 * which components display it. The still variant rides along as a comment
 * because a still is just the same script never played.
 */

import type { StepDef as StepDefOf } from "@avihut/dumbshow";
import type { Act } from "../../render";

type StepDef = StepDefOf<Act>;

export function embedSnippet(steps: StepDef[]): string {
  return [
    "<script setup>",
    `const SCRIPT = ${JSON.stringify(steps)};`,
    "</script>",
    "",
    '<RepoDiagram :script="SCRIPT" />',
    '<RepoTerminal :script="SCRIPT" />',
    "",
    '<!-- Static frame instead: <RepoDiagram :script="SCRIPT" still /> -->',
    "",
  ].join("\n");
}
