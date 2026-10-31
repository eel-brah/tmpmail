<script lang="ts">
  // Debug builds only (TMPMAIL_GUI_PROBE=1): shows the hostile test mail
  // through the real preview component and through control frames, then
  // tells Rust to report what got through.
  import { onMount } from "svelte";
  import Preview from "./Preview.svelte";
  import { api, type ProbePlan } from "./api";

  let { origin, plan }: { origin: string; plan: ProbePlan } = $props();

  const total = $derived(plan.real.length + plan.controls.length);
  let loaded = $state(0);
  let status = $state("Loading the test frames");

  function frameLoaded() {
    loaded += 1;
  }

  onMount(() => {
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      status = "Reporting";
      void api.probeFinish();
    };
    // Give the frames time to load and anything inside them time to run.
    const settle = window.setInterval(() => {
      if (loaded >= total) {
        window.clearInterval(settle);
        status = plan.wait_secs > 0 ? `Waiting ${plan.wait_secs}s: click the links` : "Settling";
        window.setTimeout(finish, 2500 + plan.wait_secs * 1000);
      }
    }, 200);
    // Rust has its own watchdog; this one just reports whatever loaded.
    const fallback = window.setTimeout(finish, 20000 + plan.wait_secs * 1000);
    return () => {
      window.clearInterval(settle);
      window.clearTimeout(fallback);
    };
  });
</script>

<main class="flex h-full flex-col gap-3 p-4">
  <header class="flex items-baseline justify-between">
    <h1 class="text-lg font-semibold">Preview probe</h1>
    <p class="text-muted">{status} ({loaded}/{total} frames)</p>
  </header>
  <div class="grid min-h-0 flex-1 grid-cols-3 gap-3">
    {#each plan.real as r (r.name)}
      <section class="flex min-h-0 flex-col rounded-md border border-rule bg-raised">
        <h2 class="px-3 py-1.5 text-xs font-medium">{r.name}: the real preview</h2>
        <div class="min-h-0 flex-1">
          <Preview {origin} mailKey={r.key} images={r.images} title={r.name} onload={frameLoaded} />
        </div>
      </section>
    {/each}
    {#each plan.controls as c (c.name)}
      <section class="flex min-h-0 flex-col rounded-md border border-dashed border-rule bg-raised">
        <h2 class="px-3 py-1.5 text-xs font-medium">
          {c.name}: control, sandbox {c.sandbox === null ? "off" : `"${c.sandbox}"`}
        </h2>
        <div class="min-h-0 flex-1">
          <iframe
            class="h-full w-full border-0 bg-white"
            src={c.url}
            title={c.name}
            sandbox={c.sandbox ?? undefined}
            referrerpolicy="no-referrer"
            onload={frameLoaded}
          ></iframe>
        </div>
      </section>
    {/each}
  </div>
</main>
