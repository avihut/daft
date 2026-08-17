<script setup lang="ts">
import DemoStage from "./DemoStage.vue";
</script>

<template>
  <DemoStage />

  <section class="dl-wrap dl-sec">
    <h2>One command per superpower</h2>
    <p class="dl-lead">
      Each pillar shows up as the command you'd actually type — no feature
      prose, just what happens.
    </p>
    <div class="dl-cmdgrid">
      <a class="dl-cmdcard" href="/worktrees/">
        <div class="dl-mini">
          <div class="dl-ln is-cmd"><span class="dl-prompt">$ </span>daft start feature/auth</div>
          <div class="dl-ln is-out">branch + worktree created · pushed -u origin</div>
          <div class="dl-ln is-out">→ my-project/feature/auth/</div>
        </div>
        <p class="dl-cardcap"><b>New branch, new directory</b> — remote tracking set, one step.</p>
      </a>
      <a class="dl-cmdcard" href="/worktrees/pull-requests">
        <div class="dl-mini">
          <div class="dl-ln is-cmd"><span class="dl-prompt">$ </span>daft go review/pr-214</div>
          <div class="dl-ln is-out">worktree created from origin/review/pr-214</div>
          <div class="dl-ln is-out">→ my-project/review/pr-214/</div>
        </div>
        <p class="dl-cardcap"><b>Review beside your work</b> — checkouts stop displacing you.</p>
      </a>
      <a class="dl-cmdcard" href="/worktrees/merging">
        <div class="dl-mini">
          <div class="dl-ln is-cmd"><span class="dl-prompt">$ </span>daft merge feature/auth</div>
          <div class="dl-ln is-ok">✓ fmt&ensp;&ensp;✓ clippy&ensp;&ensp;✓ tests</div>
          <div class="dl-ln is-out">merged into main · worktree removed</div>
        </div>
        <p class="dl-cardcap"><b>Merge from anywhere</b> — hooks gate it like CI would.</p>
      </a>
      <a class="dl-cmdcard" href="/reference/cli/daft-sync">
        <div class="dl-mini">
          <div class="dl-ln is-cmd"><span class="dl-prompt">$ </span>daft sync</div>
          <div class="dl-ln is-out">pruned 3 merged branches</div>
          <div class="dl-ln is-out">updated 4 worktrees</div>
        </div>
        <p class="dl-cardcap"><b>The whole repo brought current</b> in one pass.</p>
      </a>
    </div>
  </section>

  <section class="dl-wrap dl-sec">
    <h2>The loop you stop running</h2>
    <div class="dl-ba">
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
  </section>

  <section class="dl-band">
    <div class="dl-wrap">
      <img class="dl-mark light-mark" src="/brand/daft-donut.svg" alt="" width="30" height="36" />
      <img class="dl-mark dark-mark" src="/brand/daft-donut-white.svg" alt="" width="30" height="36" />
      <h2>Give every branch a home.</h2>
      <div class="dl-ctas">
        <a class="dl-btn is-gold" href="/getting-started/quick-start">Get started</a>
        <a class="dl-btn is-ghost" href="/about/why-daft">Why daft</a>
      </div>
    </div>
  </section>
</template>
