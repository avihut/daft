# Benchmark Dashboard Design

## Goal

Move benchmark result storage out of the public daft repo into a private
`daft-benchmarks` repo. Add a simple Astro-based dashboard deployed to GitHub
Pages that shows performance trends over time as line charts.

## Key Decisions

- **Three-way comparison**: daft (default) vs daft (gitoxide) vs git scripting
- **Single platform**: CI only (Linux, GitHub Actions)
- **Visualization**: Line charts over time (Chart.js)
- **Tech stack**: Astro SSG (static site generator)
- **Hosting**: GitHub Pages from the private repo (public site, private source)
- **Data format**: Raw hyperfine JSON wrapped with metadata envelope
- **Architecture**: Monorepo — data + dashboard in one private repo

## Data Flow

1. daft repo CI (`bench.yml`) runs benchmarks on push to master
2. Each scenario runs 3 commands: daft (default), daft (gitoxide), git scripting
3. CI collects all `benches/results/*.json` files
4. Wraps them into a single metadata envelope:
   ```json
   {
     "version": "1.0.30",
     "commit": "abc1234",
     "date": "2026-02-20T12:00:00Z",
     "runner_os": "Linux",
     "benchmarks": {
       "init": { "results": [ ... ] },
       "clone-small": { "results": [ ... ] }
     }
   }
   ```
5. Pushes as `data/YYYY-MM-DD-<commit-short>.json` to the private
   `daft-benchmarks` repo using a PAT or deploy key
6. The push triggers a deploy workflow in `daft-benchmarks` that rebuilds the
   Astro site and publishes to GitHub Pages

## Repository Structure (daft-benchmarks)

```
daft-benchmarks/
├── data/                          # Raw benchmark results (committed by CI)
│   ├── 2026-02-20-abc1234.json
│   └── ...
├── site/                          # Astro dashboard source
│   ├── astro.config.mjs
│   ├── package.json
│   ├── src/
│   │   ├── pages/
│   │   │   └── index.astro        # Main dashboard page
│   │   ├── components/
│   │   │   └── BenchChart.astro   # Reusable chart component
│   │   └── lib/
│   │       └── data.ts            # Load & transform JSON at build time
│   └── public/
├── .github/workflows/
│   └── deploy.yml                 # Build Astro + deploy to GitHub Pages
└── README.md
```

## Dashboard UI

- Single page with header showing latest version and date
- One line chart per benchmark group (clone, checkout, init, prune, fetch,
  branch-delete, workflow)
- 3 lines per chart: daft (default), daft (gitoxide), git scripting
- Size selector for scenarios with small/medium/large variants
- Y-axis: mean execution time in ms; X-axis: version/date
- Hover tooltip: exact values, stddev, ratio (e.g. "git is 2.7x faster")
- Chart.js for rendering

## CI Changes in daft Repo

- `bench_compare` gains a third command for daft+gitoxide
- Remove commit-results-to-daft-repo steps (no more `docs/benchmarks/index.md`
  or `benches/history/` commits)
- Add push-to-private-repo step using a repository secret
- Keep artifact upload as backup (90-day retention)

## Portability

If the dashboard is ever merged back into the daft docs site:

1. Embed as an iframe in a VitePress page
2. Port Chart.js logic to a Vue component
3. Or mount as a subpath alongside VitePress on Cloudflare Pages

The data format is independent of the frontend — any approach works.
