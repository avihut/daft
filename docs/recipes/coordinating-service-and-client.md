---
title: Coordinating a service and its client
description:
  Declare the relationship between a service repo and its client repo once, then
  branch, hop, and test across both as a single change.
pillars: [graph, worktrees]
---

# Coordinating a service and its client

## Starting state

Two repos, cloned side by side:

```
~/code/
├── api/            # the service — owns openapi.yaml
│   ├── main/
│   └── feat-*/
└── api-client/     # the generated TS client, published to npm
    ├── main/
    └── feat-*/
```

Every API change is really two changes. The ritual: edit the endpoint in `api`,
then `cd ../api-client`, create a branch — typing the same branch name by hand —
regenerate from the spec, fix the fallout, run both test suites, open two PRs,
and mention in each description that they belong together.

The pain has already happened: a renamed response field shipped in the service
while the client half sat on a branch spelled slightly differently
(`feat/rename-user-id` vs `feat-rename-user-id`), got missed in review, and
staging broke until someone diffed the two repos by hand. Nothing in either repo
records that they move together — the relationship lives in your head.

The reach for daft: declare the relationship once, in the service's `daft.yml`,
and let the same branch open, run, and test across both repos as one coordinated
set.

## What changes

The service's `daft.yml` gains a top-level `relations:` list — one entry per
related repo, keyed by remote URL. The URL is what makes the manifest portable:
it's committed and shared, and each machine's repo catalog resolves it to
wherever that repo happens to be cloned locally.

With the edge declared, three commands become cross-repo:

- `daft start <branch> --with-related` creates the branch and worktree in the
  service **and** in every related repo, each based on its own default branch.
- `daft go <repo> [<branch>]` hops between the repos (creating the branch's
  worktree over there when it doesn't exist yet).
- `daft exec --related -- <cmd>` runs a command across this branch's worktrees
  in every repo that has one.

## Recipe

From the service repo, declare the client with `daft repo link` (it resolves the
target — a catalog name, a repo path, or a remote URL — to the portable remote
URL and writes a well-formed entry):

```bash
cd ~/code/api/main
daft repo link git@github.com:acme/api-client.git --name client --kind consumer
git add daft.yml && git commit -m "Relate api-client"   # share it with the team
```

That records the edge in the service repo's `daft.yml`, next to hooks:

```yaml
# api/daft.yml
relations:
  - url: git@github.com:acme/api-client.git
    name: client # optional friendly label
    kind: consumer # optional, free-form
```

`daft repo unlink client` removes it again. Both repos must be cloned locally
(daft's catalog registers every clone automatically). Then the coordinated flow,
end to end:

```bash
cd ~/code/api/main

# One branch, both repos. Each side is based on its own default branch.
daft start feat/rename-user-id --with-related

# You land in api/feat-rename-user-id. Make the service change, then hop:
daft go api-client feat/rename-user-id

# Regenerate, fix, hop back:
daft go api feat/rename-user-id

# Run the check across every repo that carries this branch:
daft exec --related -- npm test
```

Piece by piece:

1. **`relations:`** is a committed, team-shared edge. Teammates who clone both
   repos get the same coordination with zero setup — resolution happens through
   each machine's own catalog, so clone locations don't need to match.
2. **`daft start … --with-related`** refuses to run if a related repo isn't
   cloned yet (it fails before creating anything, and tells you the `daft clone`
   command to fix it). Uncommitted changes (`--carry`) and `-x` commands stay in
   the current repo; lifecycle hooks run in a related repo only when that repo
   is explicitly trusted.
3. **`daft go api-client feat/rename-user-id`** opens that branch's worktree
   over there, creating it on demand. Bare `daft go api-client` lands on the
   client's default-branch worktree.
4. **`daft exec --related`** targets the current branch's worktree in the
   current repo plus every related repo. A related repo without that worktree is
   skipped with a notice, so the command runs against exactly the repos the
   change actually touches.

Finish the way you already do: push and open a PR per repo. The branches share a
name, so the pairing is visible without a note in the description.

## Variants

By **relationship shape** — who declares an edge to whom. Edges are directed:
declaring the client in the service's manifest does not create the reverse edge.

### Service and client (one direction)

The Recipe above. Changes originate in the service, so the service declares the
client. Running `daft exec --related` from the client does nothing special —
which is fine when coordination only ever starts on the service side.

### Library and its consumers

The library declares every consumer, so one `daft start`/`daft exec --related`
from the library fans out across all of them:

```yaml
# lib/daft.yml
relations:
  - url: git@github.com:acme/web.git
  - url: git@github.com:acme/worker.git
  - url: git@github.com:acme/cli.git
```

Consumers a machine hasn't cloned are reported as not cloned (`exec` skips them
with a warning; `start --with-related` requires them).

### Bidirectional

When changes start on either side, declare the edge both ways — one entry in
each repo's `daft.yml` pointing at the other. Each side's manifest stands alone,
so the two declarations don't conflict; they just make `--related` work from
wherever you happen to be.

## Idempotency & safety

`relations:` is inert data — adding, editing, or removing entries never touches
the other repo. Resolution happens at command time against the local catalog.

`daft start <branch> --with-related` is a creation, not a navigation: re-running
it for a branch that already exists fails the same way plain `daft start` does.
To re-enter an in-flight coordinated change, use `daft go <repo> <branch>` and
`daft exec --related` — both are safely re-runnable.

::: warning Relations are URLs, not names

The manifest resolves by normalized remote URL, never by catalog name — names
are machine-local and may differ per clone. If `daft exec --related` reports a
repo as not cloned that you're sure exists, its remote URL doesn't match the
manifest entry (a fork origin is the usual culprit).

:::

## Where to next

- **[Polyrepo, monorepo feel](/recipes/polyrepo-monorepo)** — the fleet-wide
  half of the Graph pillar: navigating and maintaining every cataloged repo.
- **[Trust & security](/hooks/trust-and-security)** — why hooks only fire in
  related repos you've explicitly trusted.
- **[Running commands across worktrees](/worktrees/running-commands)** — the
  single-repo `daft exec` this pattern extends.
