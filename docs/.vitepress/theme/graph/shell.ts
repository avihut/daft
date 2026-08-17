/**
 * The shell parser — typed lines become timeline operations.
 *
 * This is the reverse of OpSpec.command(): `parseCommand` maps an argv
 * back to an op id plus args, resolved against the world (and cwd) at the
 * insertion point so targets are validated before anything is stored. An
 * invalid line returns an error to print ephemerally — it never becomes a
 * timeline item. This module is deliberately editor-free: it is the seed
 * of the daft sandbox's future web terminal.
 */

import {
  OPS,
  resolveCd,
  type VerbArgs,
  type World,
  type WorldRepo,
} from "./verbs";

/** Whitespace split with double-quote grouping. */
export function tokenize(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quoted = false;
  for (const ch of line.trim()) {
    if (ch === '"') {
      quoted = !quoted;
      continue;
    }
    if (!quoted && /\s/.test(ch)) {
      if (cur) out.push(cur);
      cur = "";
      continue;
    }
    cur += ch;
  }
  if (cur) out.push(cur);
  return out;
}

export type ParseOutcome =
  | { ok: true; op: string; args: VerbArgs }
  | { ok: false; error: string };

const err = (error: string): ParseOutcome => ({ ok: false, error });
const ok = (op: string, args: VerbArgs): ParseOutcome => ({
  ok: true,
  op,
  args,
});

function cwdRepo(world: World, cwd: string): WorldRepo | undefined {
  const name = cwd.split("/").filter((p) => p && p !== "~")[0];
  return world.repos.find((r) => r.name === name) ?? world.repos[0];
}

function cwdKey(cwd: string): string | null {
  const parts = cwd.split("/").filter((p) => p && p !== "~");
  if (parts.length < 2) return null;
  return `${parts[0]}:${parts.slice(1).join("/")}`;
}

function activeBranches(repo: WorldRepo | undefined): string[] {
  return repo ? repo.wts.filter((w) => !w.removed).map((w) => w.branch) : [];
}

/** Where a branch lives — the cwd repo first, then anywhere. */
function findCarrier(
  world: World,
  cwd: string,
  branch: string,
  filter: (repo: WorldRepo) => string[],
): string | null {
  const home = cwdRepo(world, cwd);
  if (home && filter(home).includes(branch)) return `${home.name}:${branch}`;
  for (const r of world.repos)
    if (filter(r).includes(branch)) return `${r.name}:${branch}`;
  return null;
}

function gate(world: World, op: string, args: VerbArgs): ParseOutcome {
  const spec = OPS.find((o) => o.id === op);
  if (!spec) return err(`unknown daft command: ${op}`);
  if (!spec.available(world))
    return err(`daft ${spec.label}: nothing to act on here yet`);
  return ok(op, args);
}

export function parseCommand(
  line: string,
  world: World,
  cwd: string,
): ParseOutcome {
  const argv = tokenize(line);
  if (!argv.length) return err("");

  if (argv[0] === "cd") {
    const target = argv[1] ?? "~";
    const walked = resolveCd(world, cwd, target);
    if ("error" in walked) return err(`cd: ${walked.error}`);
    return ok("cd", { path: target });
  }

  if (argv[0] !== "daft")
    return err(`this shell tells daft stories — try a daft command, or cd`);

  const verb = argv[1];
  const rest = argv.slice(2);
  const home = cwdRepo(world, cwd);

  switch (verb) {
    case "clone": {
      const url = rest[0];
      if (!url) return err("daft clone: which repository? give it a URL");
      const name =
        url.match(/\/([^/]+?)(?:\.git)?$/)?.[1] ??
        url
          .replace(/\.git$/, "")
          .split(":")
          .pop() ??
        url;
      if (world.repos.some((r) => r.name === name))
        return err(`daft clone: ${name} is already in the story`);
      return ok("clone", { name });
    }
    case "start": {
      if (!rest.length) return err("daft start: name the branch to start");
      if (rest.length >= 2) {
        // Local-first, like the real grammar: two names read as
        // repo + branch only when the first is not a local branch.
        const [a, b] = rest;
        if (
          home &&
          activeBranches(home).includes(b) &&
          !world.repos.some((r) => r.name === a)
        )
          return err(`daft start: ${b} already exists in ${home.name}`);
        return ok("start", { repo: a, branch: b });
      }
      const branch = rest[0];
      if (home && activeBranches(home).includes(branch))
        return err(`daft start: ${branch} already exists in ${home.name}`);
      return gate(world, "start", {
        branch,
        ...(home ? { repo: home.name } : {}),
      });
    }
    case "go": {
      if (!rest.length) return err("daft go: where to? name a branch or repo");
      if (rest.length >= 2) {
        const repo = world.repos.find((r) => r.name === rest[0]);
        if (!repo)
          return err(
            `error: unknown repo "${rest[0]}" — nothing cloned or added by that name`,
          );
        if (!activeBranches(repo).includes(rest[1]))
          return err(`daft go: no worktree ${rest[1]} in ${rest[0]}`);
        return ok("go", { target: `${rest[0]}:${rest[1]}` });
      }
      const arg = rest[0];
      if (home && activeBranches(home).includes(arg))
        return ok("go", { target: `${home.name}:${arg}` });
      const repo = world.repos.find((r) => r.name === arg);
      if (repo) return ok("go", { target: `${arg}:main` });
      return err(
        `error: unknown repo "${arg}" — nothing cloned or added by that name`,
      );
    }
    case "list":
      return gate(world, "list", {});
    case "exec": {
      const dash = rest.indexOf("--");
      const command = rest.slice(dash + 1).join(" ");
      if (dash < 0 || !command)
        return err("daft exec: give it a command after --");
      return gate(world, "exec", { command });
    }
    case "push": {
      const branch = rest[0];
      if (!branch) {
        // Bare push means "this worktree" — which must itself be pushable;
        // silently pushing something else would be a wrong mutation.
        const key = cwdKey(cwd);
        if (!key) return err("daft push: name a branch, or cd into a worktree");
        const cut = key.indexOf(":");
        const repo = world.repos.find((x) => x.name === key.slice(0, cut));
        const wt = repo?.wts.find(
          (w) => !w.removed && w.branch === key.slice(cut + 1),
        );
        if (!wt || wt.branch === "main" || wt.merged || wt.pushed)
          return err(
            `daft push: nothing to push from ${key.replace(":", "/")}`,
          );
        return ok("push", { target: key });
      }
      const target = findCarrier(world, cwd, branch, (r) =>
        r.wts
          .filter((w) => !w.removed && !w.merged && !w.pushed)
          .map((w) => w.branch),
      );
      if (!target) return err(`daft push: no unpushed worktree for ${branch}`);
      return ok("push", { target });
    }
    case "merge": {
      const branch = rest[0];
      if (!branch) return err("daft merge: name the branch to land");
      const target = findCarrier(world, cwd, branch, (r) =>
        r.wts
          .filter((w) => !w.removed && !w.merged && w.branch !== "main")
          .map((w) => w.branch),
      );
      if (!target) return err(`daft merge: no unmerged worktree for ${branch}`);
      return ok("merge", { target });
    }
    case "carry": {
      const copy = rest.includes("--copy");
      const branch = rest.filter((a) => a !== "--copy")[0];
      if (!branch) return err("daft carry: name the branch to carry into");
      const from = cwdKey(cwd);
      if (!from)
        return err(
          "daft carry: cd into the worktree holding the changes first",
        );
      if (!home || !activeBranches(home).includes(branch))
        return err(`daft carry: no worktree ${branch} here`);
      const to = `${home.name}:${branch}`;
      if (to === from) return err("daft carry: already standing there");
      return ok("carry", { from, to, mode: copy ? "copy" : "move" });
    }
    case "rename": {
      const [source, to] = rest;
      if (!source || !to)
        return err("daft rename: needs a source branch and its new name");
      const target = findCarrier(world, cwd, source, (r) =>
        r.wts
          .filter((w) => !w.removed && w.branch !== "main")
          .map((w) => w.branch),
      );
      if (!target) return err(`daft rename: no worktree named ${source}`);
      return ok("rename", { target, to });
    }
    case "update":
      return gate(world, "update", {});
    case "prune":
      return gate(world, "prune", {});
    case "sync":
      return gate(
        world,
        "sync",
        rest.includes("--push") ? { push: "yes" } : {},
      );
    case "remove": {
      const flagIdx = rest.indexOf("--repo");
      if (flagIdx >= 0) {
        const repoName = rest[flagIdx + 1];
        const branch = rest.filter(
          (_, i) => i !== flagIdx && i !== flagIdx + 1,
        )[0];
        if (!repoName || !branch)
          return err("daft remove: --repo needs a repo and a branch");
        const repo = world.repos.find((r) => r.name === repoName);
        if (!repo || !activeBranches(repo).includes(branch))
          return err(`daft remove: no worktree ${branch} in ${repoName}`);
        return ok("remove", { target: `${repoName}:${branch}` });
      }
      const branch = rest[0];
      if (!branch) return err("daft remove: name the branch to tear down");
      if (branch === "main") return err("daft remove: main stays");
      const target = findCarrier(world, cwd, branch, (r) =>
        r.wts
          .filter((w) => !w.removed && w.branch !== "main")
          .map((w) => w.branch),
      );
      if (!target) return err(`daft remove: no worktree named ${branch}`);
      return ok("remove", { target });
    }
    case "repo": {
      const sub = rest[0];
      const arg = rest[1];
      if (sub === "add")
        return gate(
          world,
          "repo-add",
          arg ? { name: arg.split("/").pop() } : {},
        );
      if (sub === "remove") {
        if (!arg) return err("daft repo remove: name the repo");
        if (!world.repos.some((r) => r.name === arg))
          return err(
            `error: unknown repo "${arg}" — nothing cloned or added by that name`,
          );
        return ok("repo-remove", { name: arg });
      }
      if (sub === "link") {
        if (!arg) return err("daft repo link: name the repo to link to");
        if (!world.repos.some((r) => r.name === arg))
          return err(
            `error: unknown repo "${arg}" — nothing cloned or added by that name`,
          );
        const a = home?.name;
        if (!a || a === arg)
          return err("daft repo link: cd into the repo to link from");
        if (
          world.rels.some(
            ([x, y]) => (x === a && y === arg) || (x === arg && y === a),
          )
        )
          return err(`daft repo link: ${a} and ${arg} are already related`);
        return ok("repo-link", { a, b: arg });
      }
      if (sub === "unlink") {
        if (!arg) return err("daft repo unlink: name the repo to unlink");
        const a = home?.name;
        const rel =
          world.rels.find(
            ([x, y]) => (x === a && y === arg) || (x === arg && y === a),
          ) ?? world.rels.find(([x, y]) => x === arg || y === arg);
        if (!rel) return err(`daft repo unlink: no relation touches ${arg}`);
        return ok("repo-unlink", { target: `${rel[0]} ↔ ${rel[1]}` });
      }
      return err("daft repo: add, remove, link, or unlink");
    }
    default:
      return err(
        verb
          ? `unknown daft command: ${verb}`
          : "daft: which verb? try clone, start, go, list…",
      );
  }
}
