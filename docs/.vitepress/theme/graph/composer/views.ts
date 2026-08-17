/**
 * State views — three readings of one world, all at the playhead.
 *
 * World is the graph as rows; daft is what the tool itself knows (its
 * repo registry and relations); Files is the directories daft would
 * actually make — clones land in ~, one folder per branch, and a hollow
 * worktree's folder stays until prune removes it. (`resolveCd`, the walk
 * the shell's cd uses, lives with the world model in ../verbs.ts.)
 */

import type { World } from "../verbs";

export interface WorldRow {
  kind: "repo" | "wt";
  repo: string;
  wt?: string;
  port?: string;
  agent?: boolean;
  merged?: boolean;
  main?: boolean;
}

export function worldRows(world: World): WorldRow[] {
  const rows: WorldRow[] = [];
  for (const repo of world.repos) {
    rows.push({ kind: "repo", repo: repo.name });
    for (const wt of repo.wts) {
      if (wt.removed) continue;
      rows.push({
        kind: "wt",
        repo: repo.name,
        wt: wt.branch,
        port: wt.port,
        agent: wt.agent,
        merged: wt.merged,
        main: wt.branch === "main",
      });
    }
  }
  return rows;
}

export interface RegistryRow {
  name: string;
  path: string;
}

export function registryRows(world: World): RegistryRow[] {
  return world.repos.map((r) => ({ name: r.name, path: `~/${r.name}` }));
}

export interface FileRow {
  /** Mono-rendered row: box-drawing prefix plus the name. */
  text: string;
  path: string;
  dir: boolean;
}

export function fileRows(world: World): FileRow[] {
  const rows: FileRow[] = [{ text: "~", path: "~", dir: true }];
  world.repos.forEach((repo, ri) => {
    const lastRepo = ri === world.repos.length - 1;
    rows.push({
      text: `${lastRepo ? "└─ " : "├─ "}${repo.name}`,
      path: `~/${repo.name}`,
      dir: true,
    });
    const wts = repo.wts.filter((w) => !w.removed);
    wts.forEach((wt, wi) => {
      const lastWt = wi === wts.length - 1;
      rows.push({
        text: `${lastRepo ? "   " : "│  "}${lastWt ? "└─ " : "├─ "}${wt.branch}`,
        path: `~/${repo.name}/${wt.branch}`,
        dir: false,
      });
    });
  });
  return rows;
}
