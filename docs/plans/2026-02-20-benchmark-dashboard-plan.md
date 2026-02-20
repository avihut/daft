# Benchmark Dashboard Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Move benchmark data to a private repo with an Astro dashboard on
GitHub Pages, and add three-way comparison (daft vs daft+gitoxide vs git).

**Architecture:** The daft repo CI runs benchmarks and pushes result JSON to a
private `daft-benchmarks` repo. That repo contains both the raw data and an
Astro static site that reads data at build time and renders Chart.js line
charts. A GitHub Pages deploy workflow rebuilds the site on every data push.

**Tech Stack:** Bash, hyperfine, Astro, Chart.js, GitHub Actions, GitHub Pages

---

## Part 1: Three-Way Benchmarks (daft repo)

### Task 1: Update bench_framework.sh for three-way comparison

**Files:**

- Modify: `benches/bench_framework.sh`

**Step 1: Update bench_compare to run three commands**

The key insight: the daft+gitoxide variant uses the exact same daft command —
the only difference is that `daft.experimental.gitoxide=true` is set in the
isolated `GIT_CONFIG_GLOBAL` file. Hyperfine supports per-command `--prepare`
flags (positional pairing), so we add three prepare steps.

Replace the `bench_compare` function with:

```bash
bench_compare() {
    local name="$1"
    local prepare_cmd="$2"
    local daft_cmd="$3"
    local git_cmd="$4"
    shift 4

    local json_out="$RESULTS_DIR/${name}.json"
    local md_out="$RESULTS_DIR/${name}.md"

    log "Running: $name"

    # Gitoxide toggle: set/unset in the isolated GIT_CONFIG_GLOBAL
    local unset_gix="git config --file \"$GIT_CONFIG_GLOBAL\" --unset-all daft.experimental.gitoxide 2>/dev/null; true"
    local set_gix="git config --file \"$GIT_CONFIG_GLOBAL\" daft.experimental.gitoxide true"

    # Build per-command prepare: base cleanup + gitoxide toggle
    local prep_daft=""
    local prep_gix=""
    local prep_git=""
    if [[ -n "$prepare_cmd" ]]; then
        prep_daft="$prepare_cmd && $unset_gix"
        prep_gix="$prepare_cmd && $set_gix"
        prep_git="$prepare_cmd && $unset_gix"
    else
        prep_daft="$unset_gix"
        prep_gix="$set_gix"
        prep_git="$unset_gix"
    fi

    hyperfine \
        --warmup 3 \
        --min-runs 10 \
        --prepare "$prep_daft" \
        --prepare "$prep_gix" \
        --prepare "$prep_git" \
        --export-json "$json_out" \
        --export-markdown "$md_out" \
        "$@" \
        --command-name "daft" "$daft_cmd" \
        --command-name "daft-gitoxide" "$daft_cmd" \
        --command-name "git" "$git_cmd"

    log_success "Saved: $json_out"
}
```

No changes needed to any scenario scripts — they still call `bench_compare` with
the same 4 arguments. The framework automatically adds the gitoxide variant.

**Step 2: Verify it works**

Run: `mise run bench:init`

Expected: hyperfine output shows three benchmarks (daft, daft-gitoxide, git)
instead of two. The JSON file should have 3 entries in the `results` array.

**Step 3: Commit**

```bash
git add benches/bench_framework.sh
git commit -m "chore(bench): add three-way comparison (daft vs gitoxide vs git)"
```

### Task 2: Add result packaging script

**Files:**

- Create: `benches/package_results.sh`

**Step 1: Create the packaging script**

This script collects all JSON result files and wraps them in a metadata
envelope. CI calls this after benchmarks complete.

```bash
#!/usr/bin/env bash
# Package benchmark results into a single JSON envelope with metadata.
# Usage: package_results.sh <output-file>
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="$BENCH_DIR/results"

OUTPUT="${1:?Usage: package_results.sh <output-file>}"

# Gather metadata
VERSION=$(daft --version 2>/dev/null | awk '{print $2}' || echo "unknown")
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
DATE=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
RUNNER_OS="${RUNNER_OS:-$(uname -s)}"

# Build the envelope using jq
BENCHMARKS="{}"
for json_file in "$RESULTS_DIR"/*.json; do
    [[ -f "$json_file" ]] || continue
    name="$(basename "$json_file" .json)"
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$name" --slurpfile data "$json_file" '. + {($name): $data[0]}')
done

jq -n \
    --arg version "$VERSION" \
    --arg commit "$COMMIT" \
    --arg date "$DATE" \
    --arg runner_os "$RUNNER_OS" \
    --argjson benchmarks "$BENCHMARKS" \
    '{
        version: $version,
        commit: $commit,
        date: $date,
        runner_os: $runner_os,
        benchmarks: $benchmarks
    }' > "$OUTPUT"

echo "Packaged results to $OUTPUT"
```

**Step 2: Make it executable and test locally**

Run:

```bash
chmod +x benches/package_results.sh
mise run bench:init
benches/package_results.sh /tmp/test-envelope.json
cat /tmp/test-envelope.json | jq keys
```

Expected: `["benchmarks", "commit", "date", "runner_os", "version"]`

**Step 3: Commit**

```bash
git add benches/package_results.sh
git commit -m "chore(bench): add result packaging script"
```

### Task 3: Clean up daft repo (remove in-repo benchmark publishing)

**Files:**

- Delete: `docs/benchmarks/index.md`
- Delete: `benches/history/.gitkeep`
- Modify: `docs/.vitepress/config.ts` (remove Benchmarks nav/sidebar entries)
- Modify: `.github/workflows/bench.yml` (rewrite for external push)

**Step 1: Remove docs/benchmarks/index.md and benches/history/.gitkeep**

```bash
rm -f docs/benchmarks/index.md
rmdir docs/benchmarks 2>/dev/null || true
rm -f benches/history/.gitkeep
rmdir benches/history 2>/dev/null || true
```

**Step 2: Remove Benchmarks entries from VitePress config**

In `docs/.vitepress/config.ts`:

- Remove `{ text: "Benchmarks", link: "/benchmarks/" }` from the `nav` array
  (line 217)
- Remove `{ text: "Benchmarks", link: "/benchmarks/" }` from the Project sidebar
  items (line 311)

**Step 3: Rewrite bench.yml to push results to private repo**

Replace `.github/workflows/bench.yml` with:

```yaml
name: Benchmarks

on:
  push:
    branches: [master]
    paths-ignore:
      - "*.md"
      - "docs/**"
  workflow_dispatch:

jobs:
  bench:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust dependencies
        uses: Swatinem/rust-cache@v2

      - name: Install mise
        uses: jdx/mise-action@v3

      - name: Install hyperfine
        run: cargo install hyperfine

      - name: Build daft and set up symlinks
        run: mise run dev

      - name: Run benchmarks
        run: mise run bench

      - name: Package results
        run: |
          COMMIT=$(git rev-parse --short HEAD)
          DATE=$(date -u '+%Y-%m-%d')
          benches/package_results.sh "/tmp/bench-${DATE}-${COMMIT}.json"

      - name: Push to daft-benchmarks
        env:
          BENCH_REPO_TOKEN: ${{ secrets.BENCH_REPO_TOKEN }}
        run: |
          COMMIT=$(git rev-parse --short HEAD)
          DATE=$(date -u '+%Y-%m-%d')
          FILENAME="${DATE}-${COMMIT}.json"

          git clone https://x-access-token:${BENCH_REPO_TOKEN}@github.com/avihut/daft-benchmarks.git /tmp/daft-benchmarks
          cp "/tmp/bench-${FILENAME}" "/tmp/daft-benchmarks/data/${FILENAME}"
          cd /tmp/daft-benchmarks
          git config --local user.name "github-actions[bot]"
          git config --local user.email "github-actions[bot]@users.noreply.github.com"
          git add "data/${FILENAME}"
          git commit -m "bench: daft ${DATE} (${COMMIT})"
          git push

      - name: Upload raw results
        uses: actions/upload-artifact@v6
        if: always()
        with:
          name: benchmark-results
          path: benches/results/
          retention-days: 90
```

Note: `BENCH_REPO_TOKEN` is a GitHub PAT with `repo` scope that can push to the
private `daft-benchmarks` repo. Create it in GitHub Settings > Developer
Settings

> Personal Access Tokens, then add as a repository secret in avihut/daft.

**Step 4: Commit**

```bash
git add -A docs/benchmarks docs/.vitepress/config.ts .github/workflows/bench.yml benches/history
git commit -m "chore(bench): remove in-repo publishing, push to private repo"
```

---

## Part 2: Private Repo + Dashboard

### Task 4: Initialize daft-benchmarks repo

**Files:**

- Create: `/Users/avihu/Projects/daft-benchmarks/`

**Step 1: Create the repo directory and initialize git**

```bash
mkdir -p /Users/avihu/Projects/daft-benchmarks/data
cd /Users/avihu/Projects/daft-benchmarks
git init
```

**Step 2: Create README.md**

```markdown
# daft-benchmarks

Private benchmark data and dashboard for [daft](https://github.com/avihut/daft).

## Structure

- `data/` — Raw benchmark results (one JSON envelope per CI run)
- `site/` — Astro dashboard (deployed to GitHub Pages)

## Dashboard

Deployed to GitHub Pages automatically on every data push.
```

**Step 3: Create .gitignore**

```
node_modules/
dist/
.astro/
```

**Step 4: Create a seed data file**

Copy any existing benchmark result as seed data so the dashboard has something
to render during development. Run `mise run bench:init` in the daft repo first
if no results exist, then:

```bash
cd /Users/avihu/Projects/daft/chore/benchmark
benches/package_results.sh /Users/avihu/Projects/daft-benchmarks/data/seed.json
```

**Step 5: Commit**

```bash
cd /Users/avihu/Projects/daft-benchmarks
git add -A
git commit -m "chore: initialize repo with seed data"
```

### Task 5: Set up Astro project

**Files:**

- Create: `site/package.json`
- Create: `site/astro.config.mjs`
- Create: `site/tsconfig.json`
- Create: `site/src/pages/index.astro` (placeholder)

**Step 1: Scaffold Astro project**

```bash
cd /Users/avihu/Projects/daft-benchmarks
pnpm create astro@latest site -- --template minimal --no-install --no-git --typescript strict
```

If the interactive prompts are problematic, create files manually:

`site/package.json`:

```json
{
  "name": "daft-benchmarks-dashboard",
  "type": "module",
  "scripts": {
    "dev": "astro dev",
    "build": "astro build",
    "preview": "astro preview"
  },
  "dependencies": {
    "astro": "^5.5.0",
    "chart.js": "^4.4.0"
  }
}
```

`site/astro.config.mjs`:

```javascript
import { defineConfig } from "astro/config";

export default defineConfig({
  outDir: "../dist",
});
```

`site/tsconfig.json`:

```json
{
  "extends": "astro/tsconfigs/strict"
}
```

`site/src/pages/index.astro` (placeholder):

```astro
---
---

<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width" />
    <title>daft benchmarks</title>
  </head>
  <body>
    <h1>daft benchmarks</h1>
    <p>Dashboard coming soon.</p>
  </body>
</html>
```

**Step 2: Install dependencies**

```bash
cd /Users/avihu/Projects/daft-benchmarks/site
pnpm install
```

**Step 3: Verify it builds**

```bash
pnpm run build
```

Expected: Build succeeds, output in `../dist/`.

**Step 4: Commit**

```bash
cd /Users/avihu/Projects/daft-benchmarks
git add -A
git commit -m "chore: scaffold Astro project"
```

### Task 6: Create data loading library

**Files:**

- Create: `site/src/lib/data.ts`

**Step 1: Create the data loader**

This module reads all JSON envelopes from `data/` at build time and transforms
them into chart-friendly time series.

`site/src/lib/data.ts`:

```typescript
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

interface HyperfineResult {
  command: string;
  mean: number;
  stddev: number;
  median: number;
  min: number;
  max: number;
  times: number[];
  user: number;
  system: number;
}

interface BenchmarkEnvelope {
  version: string;
  commit: string;
  date: string;
  runner_os: string;
  benchmarks: Record<string, { results: HyperfineResult[] }>;
}

export interface DataPoint {
  date: string;
  version: string;
  commit: string;
  daft: number;
  daftStddev: number;
  gitoxide: number;
  gitoxideStddev: number;
  git: number;
  gitStddev: number;
}

export interface BenchmarkSeries {
  name: string;
  points: DataPoint[];
}

/** Scenario groupings for the dashboard. */
export const SCENARIO_GROUPS: Record<string, string[]> = {
  Init: ["init"],
  Clone: ["clone-small", "clone-medium", "clone-large"],
  "Clone (with hooks)": [
    "clone-hooks-small",
    "clone-hooks-medium",
    "clone-hooks-large",
  ],
  Checkout: [
    "checkout-existing-small",
    "checkout-existing-medium",
    "checkout-existing-large",
    "checkout-new-branch-small",
    "checkout-new-branch-medium",
    "checkout-new-branch-large",
  ],
  "Checkout (with hooks)": [
    "checkout-hooks-existing-small",
    "checkout-hooks-existing-medium",
    "checkout-hooks-existing-large",
    "checkout-hooks-new-branch-small",
    "checkout-hooks-new-branch-medium",
    "checkout-hooks-new-branch-large",
  ],
  Prune: ["prune"],
  Fetch: ["fetch"],
  "Branch Delete": ["branch-delete"],
  "Full Workflow": [
    "workflow-full-small",
    "workflow-full-medium",
    "workflow-full-large",
  ],
};

function extractDataPoint(
  envelope: BenchmarkEnvelope,
  benchName: string,
): DataPoint | null {
  const bench = envelope.benchmarks[benchName];
  if (!bench?.results) return null;

  const daft = bench.results.find((r) => r.command === "daft");
  const gitoxide = bench.results.find((r) => r.command === "daft-gitoxide");
  const git = bench.results.find((r) => r.command === "git");

  if (!daft || !git) return null;

  return {
    date: envelope.date,
    version: envelope.version,
    commit: envelope.commit,
    daft: daft.mean * 1000,
    daftStddev: daft.stddev * 1000,
    gitoxide: gitoxide ? gitoxide.mean * 1000 : 0,
    gitoxideStddev: gitoxide ? gitoxide.stddev * 1000 : 0,
    git: git.mean * 1000,
    gitStddev: git.stddev * 1000,
  };
}

export function loadAllData(): Map<string, BenchmarkSeries> {
  const dataDir = resolve(import.meta.dirname, "../../../data");
  const files = readdirSync(dataDir).filter((f) => f.endsWith(".json"));

  const envelopes: BenchmarkEnvelope[] = files
    .map((f) => JSON.parse(readFileSync(resolve(dataDir, f), "utf-8")))
    .sort((a, b) => a.date.localeCompare(b.date));

  const allBenchNames = new Set<string>();
  for (const env of envelopes) {
    for (const key of Object.keys(env.benchmarks)) {
      allBenchNames.add(key);
    }
  }

  const series = new Map<string, BenchmarkSeries>();

  for (const name of allBenchNames) {
    const points: DataPoint[] = [];
    for (const env of envelopes) {
      const point = extractDataPoint(env, name);
      if (point) points.push(point);
    }
    if (points.length > 0) {
      series.set(name, { name, points });
    }
  }

  return series;
}
```

**Step 2: Verify it compiles**

```bash
cd /Users/avihu/Projects/daft-benchmarks/site
pnpm run build
```

Expected: Build succeeds (the module is imported at build time).

**Step 3: Commit**

```bash
cd /Users/avihu/Projects/daft-benchmarks
git add site/src/lib/data.ts
git commit -m "feat: add benchmark data loading library"
```

### Task 7: Create chart component

**Files:**

- Create: `site/src/components/BenchChart.astro`

**Step 1: Create the Chart.js line chart component**

Chart.js runs client-side, so we render a `<canvas>` and use a `<script>` tag.

`site/src/components/BenchChart.astro`:

```astro
---
import type { DataPoint } from "../lib/data";

interface Props {
  title: string;
  points: DataPoint[];
}

const { title, points } = Astro.props;
const chartId = `chart-${title.replace(/\W+/g, "-").toLowerCase()}`;
---

<div class="chart-container">
  <h3>{title}</h3>
  <canvas id={chartId}></canvas>
</div>

<script define:vars={{ chartId, points, title }}>
  import("https://cdn.jsdelivr.net/npm/chart.js@4/+esm").then((module) => {
    const Chart = module.Chart;
    const registerables = module.registerables || [];
    if (registerables.length) Chart.register(...registerables);

    const canvas = document.getElementById(chartId);
    if (!canvas) return;

    const labels = points.map((p) => {
      const d = new Date(p.date);
      return `${p.version}\n${d.toLocaleDateString()}`;
    });

    new Chart(canvas, {
      type: "line",
      data: {
        labels,
        datasets: [
          {
            label: "daft",
            data: points.map((p) => p.daft),
            borderColor: "#3b82f6",
            backgroundColor: "#3b82f680",
            tension: 0.2,
          },
          {
            label: "daft (gitoxide)",
            data: points.map((p) => p.gitoxide),
            borderColor: "#8b5cf6",
            backgroundColor: "#8b5cf680",
            tension: 0.2,
          },
          {
            label: "git",
            data: points.map((p) => p.git),
            borderColor: "#ef4444",
            backgroundColor: "#ef444480",
            tension: 0.2,
          },
        ],
      },
      options: {
        responsive: true,
        plugins: {
          tooltip: {
            callbacks: {
              afterBody(items) {
                const idx = items[0].dataIndex;
                const p = points[idx];
                const ratio = (p.git / p.daft).toFixed(2);
                const gixRatio = p.gitoxide
                  ? (p.git / p.gitoxide).toFixed(2)
                  : "N/A";
                return [
                  `stddev: daft ±${p.daftStddev.toFixed(1)}ms, git ±${p.gitStddev.toFixed(1)}ms`,
                  `ratio: git/daft = ${ratio}x, git/gitoxide = ${gixRatio}x`,
                  `commit: ${p.commit}`,
                ];
              },
            },
          },
        },
        scales: {
          y: {
            title: { display: true, text: "Time (ms)" },
            beginAtZero: true,
          },
        },
      },
    });
  });
</script>

<style>
  .chart-container {
    margin: 1.5rem 0;
    padding: 1rem;
    border: 1px solid #e5e7eb;
    border-radius: 8px;
  }
  h3 {
    margin: 0 0 0.5rem;
    font-size: 1.1rem;
    color: #1f2937;
  }
</style>
```

**Step 2: Commit**

```bash
cd /Users/avihu/Projects/daft-benchmarks
git add site/src/components/BenchChart.astro
git commit -m "feat: add Chart.js line chart component"
```

### Task 8: Create main dashboard page

**Files:**

- Modify: `site/src/pages/index.astro`

**Step 1: Build the dashboard page**

Replace the placeholder with the full dashboard that groups charts by scenario.

`site/src/pages/index.astro`:

```astro
---
import BenchChart from "../components/BenchChart.astro";
import { loadAllData, SCENARIO_GROUPS } from "../lib/data";

const allSeries = loadAllData();

// Get latest run info
const latestPoints = [...allSeries.values()].flatMap((s) => s.points);
const latest = latestPoints.sort((a, b) =>
  b.date.localeCompare(a.date),
)[0];
---

<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width" />
    <title>daft benchmarks</title>
    <style>
      * { box-sizing: border-box; margin: 0; padding: 0; }
      body {
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        max-width: 1200px;
        margin: 0 auto;
        padding: 2rem 1rem;
        color: #1f2937;
        background: #fafafa;
      }
      header {
        margin-bottom: 2rem;
        padding-bottom: 1rem;
        border-bottom: 2px solid #e5e7eb;
      }
      header h1 { font-size: 1.8rem; }
      header p { color: #6b7280; margin-top: 0.25rem; }
      .group { margin-bottom: 3rem; }
      .group h2 {
        font-size: 1.4rem;
        margin-bottom: 1rem;
        padding-bottom: 0.5rem;
        border-bottom: 1px solid #e5e7eb;
      }
      .no-data { color: #9ca3af; font-style: italic; }
    </style>
  </head>
  <body>
    <header>
      <h1>daft benchmarks</h1>
      {latest && (
        <p>
          Latest: v{latest.version} ({latest.commit}) —{" "}
          {new Date(latest.date).toLocaleDateString()}
        </p>
      )}
    </header>

    {Object.entries(SCENARIO_GROUPS).map(([groupName, benchNames]) => {
      const chartsInGroup = benchNames
        .filter((name) => allSeries.has(name))
        .map((name) => ({ name, series: allSeries.get(name)! }));

      return (
        <section class="group">
          <h2>{groupName}</h2>
          {chartsInGroup.length === 0 ? (
            <p class="no-data">No data yet.</p>
          ) : (
            chartsInGroup.map(({ name, series }) => (
              <BenchChart title={name} points={series.points} />
            ))
          )}
        </section>
      );
    })}
  </body>
</html>
```

**Step 2: Build and preview**

```bash
cd /Users/avihu/Projects/daft-benchmarks/site
pnpm run build && pnpm run preview
```

Open `http://localhost:4321` in a browser. Should see grouped charts with seed
data.

**Step 3: Commit**

```bash
cd /Users/avihu/Projects/daft-benchmarks
git add site/src/pages/index.astro
git commit -m "feat: add main dashboard page with grouped charts"
```

### Task 9: Create GitHub Pages deploy workflow

**Files:**

- Create: `.github/workflows/deploy.yml`

**Step 1: Create the workflow**

`.github/workflows/deploy.yml`:

```yaml
name: Deploy Dashboard

on:
  push:
    branches: [main]

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Setup pnpm
        uses: pnpm/action-setup@v4
        with:
          version: latest

      - name: Setup Node
        uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: pnpm
          cache-dependency-path: site/pnpm-lock.yaml

      - name: Install dependencies
        run: pnpm install --frozen-lockfile
        working-directory: site

      - name: Build site
        run: pnpm run build
        working-directory: site

      - name: Upload artifact
        uses: actions/upload-pages-artifact@v3
        with:
          path: dist

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@v4
```

**Step 2: Commit**

```bash
cd /Users/avihu/Projects/daft-benchmarks
git add .github/workflows/deploy.yml
git commit -m "ci: add GitHub Pages deploy workflow"
```

### Task 10: Create GitHub repo and push

**Step 1: Create the private repo on GitHub**

```bash
cd /Users/avihu/Projects/daft-benchmarks
gh repo create avihut/daft-benchmarks --private --source=. --push
```

**Step 2: Enable GitHub Pages**

```bash
gh api repos/avihut/daft-benchmarks/pages \
  --method POST \
  --field build_type=workflow
```

Note: If this API call fails, enable Pages manually in the repo settings
(Settings > Pages > Source: GitHub Actions).

**Step 3: Create the BENCH_REPO_TOKEN secret in the daft repo**

1. Go to https://github.com/settings/tokens and create a fine-grained PAT with:
   - Repository access: `avihut/daft-benchmarks` only
   - Permissions: Contents (read and write)
2. In the daft repo, go to Settings > Secrets and variables > Actions
3. Add repository secret `BENCH_REPO_TOKEN` with the PAT value

This step is manual — it cannot be automated via CLI.

### Task 11: End-to-end verification

**Step 1: Run full benchmark suite in daft repo**

```bash
cd /Users/avihu/Projects/daft/chore/benchmark
mise run bench:init
```

Verify three-way output (daft, daft-gitoxide, git).

**Step 2: Package and push results manually**

```bash
benches/package_results.sh /Users/avihu/Projects/daft-benchmarks/data/test-run.json
cd /Users/avihu/Projects/daft-benchmarks
git add data/test-run.json
git commit -m "bench: test run"
git push
```

**Step 3: Verify dashboard builds and deploys**

Check GitHub Actions in `daft-benchmarks` — the deploy workflow should trigger.
Once deployed, visit the GitHub Pages URL to see the dashboard with the test
data.
