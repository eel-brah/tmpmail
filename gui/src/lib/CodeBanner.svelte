<script lang="ts">
  // The app's main job: the verification code, big, on a highlighter-yellow
  // ticket with a tear-off stub that copies it.
  import Icon from "./Icon.svelte";

  let { code, onCopy, sample = false }: { code: string; onCopy?: () => Promise<boolean>; sample?: boolean } = $props();

  let copied = $state(false);
  let timer: number | undefined;

  async function copy() {
    if (onCopy && (await onCopy())) {
      copied = true;
      window.clearTimeout(timer);
      timer = window.setTimeout(() => (copied = false), 1800);
    }
  }

  // Long digit runs read better in groups of three: 482 913.
  const groups = $derived(
    /^\d{6,8}$/.test(code) ? (code.match(/\d{1,3}(?=(\d{3})*$)/g) ?? [code]) : code.split(/[ -]/),
  );

  // Shrinks a long code to fit its box (a monospace character is about
  // 0.62em wide, a group gap 0.35em); past the smallest size it wraps
  // instead of being cut.
  const fit = $derived((code.length * 0.62 + Math.max(0, groups.length - 1) * 0.35).toFixed(2));
</script>

<div class="ticket-wrap">
<div class="ticket relative flex items-stretch overflow-hidden rounded-xl bg-hi text-hi-ink">
  <div class="code-box min-w-0 flex-1 px-5 py-3.5">
    <p class="text-2xs font-semibold opacity-75" aria-hidden="true">Verification code</p>
    <!-- Read out digit by digit; outside the selectable text, so copying the
         code by hand copies only the code. -->
    <p class="sr-only">Verification code: {code.split("").join(" ")}</p>
    <p
      class="selectable mt-0.5 font-mono leading-none font-bold tracking-[0.02em] [overflow-wrap:anywhere] slashed-zero"
      style:font-size="clamp(1.25rem, calc(100cqi / {fit}), var(--text-code))"
      aria-hidden="true"
    >
      <span class="inline-flex flex-wrap gap-x-[0.35em] gap-y-1">
        {#each groups as g, i (i)}<span>{g}</span>{/each}
      </span>
    </p>
  </div>
  <div class="stub relative flex shrink-0 items-center px-4">
    {#if sample}
      <span class="flex h-11 items-center gap-2 px-2 text-sm font-semibold">
        <span class="rounded border border-current/40 px-1.5 text-[11px] leading-4">y</span>copies it
      </span>
    {:else}
    <button
      class="flex h-11 items-center gap-2 rounded-lg bg-hi-ink px-4 text-sm font-semibold text-hi transition-transform active:translate-y-px"
      onclick={copy}
      title="Copy the code (y)"
    >
      <Icon name={copied ? "check" : "copy"} size={16} />
      {copied ? "Copied" : "Copy"}
      <span class="ml-1 rounded border border-current/40 px-1 text-[11px] leading-4 opacity-70">y</span>
    </button>
    {/if}
  </div>
</div>

</div>

<style>
  .ticket-wrap {
    container-type: inline-size;
  }
  .code-box {
    container-type: inline-size;
  }
  /* A narrow pane: the stub goes under the code, torn off along the top.
     (Nested selectors: the build moves this block before the rules above.) */
  @container (max-width: 22rem) {
    .ticket-wrap .ticket {
      flex-direction: column;
    }
    .ticket-wrap .stub {
      border-left: 0;
      border-top: 2px dashed color-mix(in srgb, var(--color-hi-ink) 30%, transparent);
      padding-block: 10px;
    }
    .ticket-wrap .stub::before {
      top: -9px;
      left: -8px;
    }
    .ticket-wrap .stub::after {
      top: -9px;
      bottom: auto;
      left: auto;
      right: -8px;
    }
  }
  /* The perforation between the code and its stub, with a notch at each
     end cut in the page's color. */
  .stub {
    border-left: 2px dashed color-mix(in srgb, var(--color-hi-ink) 30%, transparent);
  }
  .stub::before,
  .stub::after {
    content: "";
    position: absolute;
    left: -9px;
    width: 16px;
    height: 16px;
    border-radius: 999px;
    background: var(--color-raised);
  }
  .stub::before {
    top: -8px;
  }
  .stub::after {
    bottom: -8px;
  }
</style>
