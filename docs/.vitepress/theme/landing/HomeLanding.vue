<script setup lang="ts">
import DemoStage from "./DemoStage.vue";
import InstallCta from "./InstallCta.vue";
import { LANDING_POINTS, pointScript, segments } from "./points";
import PointScene from "./PointScene.vue";

// The page below the hero: the session, then the five points in workflow
// order, then the close. Copy and demonstrations come from landing/points.ts;
// the shape and the rules for changing it are in landing/CLAUDE.md.
const SCENES = LANDING_POINTS.map((point) => ({
  point,
  script: pointScript(point),
}));
</script>

<template>
  <DemoStage />

  <section class="dl-wrap dl-points" aria-label="What daft does">
    <article
      v-for="({ point, script }, i) in SCENES"
      :id="`point-${point.id}`"
      :key="point.id"
      class="dl-point"
    >
      <div class="dl-point-text">
        <span class="dl-point-n" aria-hidden="true">{{ String(i + 1).padStart(2, "0") }}</span>
        <h2>{{ point.title }}</h2>
        <p v-for="(para, j) in point.body" :key="j">
          <template v-for="(seg, k) in segments(para)" :key="k"><code v-if="seg.code">{{ seg.text }}</code><template v-else>{{ seg.text }}</template></template>
        </p>
        <a class="dl-point-link" :href="point.link.href">{{ point.link.text }} →</a>
      </div>
      <div class="dl-point-demo">
        <PointScene v-if="script" :id="point.id" :script="script" />
        <div v-else class="dl-ba" aria-label="Before and after daft">
          <div class="dl-pane is-bad">
            <h3>Before</h3>
            <pre><span class="x">git stash</span>
<span class="x">git checkout feature-b</span>
<span class="x">npm install</span>   <span class="w"># wait…</span>
<span class="x">npm run build</span> <span class="w"># wait…</span>
<span class="w"># IDE state gone. Where was I?</span></pre>
          </div>
          <div class="dl-pane is-good">
            <h3>After</h3>
            <pre><span class="k">daft go feature-b</span>
<span class="ok">already installed · already built</span>
<span class="k">daft go -</span>
<span class="ok">right where you left it</span></pre>
          </div>
        </div>
      </div>
    </article>
  </section>

  <section class="dl-band">
    <div class="dl-wrap">
      <img class="dl-mark light-mark" src="/brand/daft-donut.svg" alt="" width="30" height="36" />
      <img class="dl-mark dark-mark" src="/brand/daft-donut-white.svg" alt="" width="30" height="36" />
      <h2>Give every branch a home.</h2>
      <p class="dl-band-text">
        Plain git underneath: every worktree daft makes is a git worktree, every
        verb doubles as a git subcommand, and removing daft leaves your repo
        exactly as it is. One binary, and nothing else about your setup
        changes.
      </p>
      <InstallCta compact />
      <div class="dl-ctas">
        <a class="dl-btn is-gold" href="/getting-started/quick-start">Get started</a>
        <a class="dl-btn is-ghost" href="/about/why-daft">Why daft</a>
      </div>
    </div>
  </section>
</template>
