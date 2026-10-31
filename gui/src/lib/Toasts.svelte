<script lang="ts">
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";
</script>

<div class="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[min(24rem,calc(100vw-2rem))] flex-col gap-2" aria-live="polite">
  {#each app.toasts as t (t.id)}
    <div
      class={[
        "pointer-events-auto flex items-start gap-2.5 rounded-xl border px-3.5 py-3 text-xs shadow-[var(--shadow-pop)] [animation:slide-in_180ms_var(--ease-out)]",
        t.kind === "error" ? "border-danger/40 bg-danger-soft text-danger" : "border-rule bg-raised text-ink",
      ]}
      role={t.kind === "error" ? "alert" : "status"}
    >
      <span class={["mt-px", t.kind === "success" ? "text-live" : t.kind === "mail" ? "text-hi-edge" : ""]}>
        <Icon name={t.kind === "error" ? "alert" : t.kind === "success" ? "check" : t.kind === "mail" ? "mail" : "refresh"} size={15} />
      </span>
      <span class="selectable min-w-0 flex-1 break-words">
        {#if t.kind === "mail"}<span class="font-semibold">New mail.</span>{/if}
        {t.text}
      </span>
      {#if t.action}
        {@const action = t.action}
        <button
          class="-my-1 shrink-0 rounded-md px-2 py-1 font-semibold underline-offset-2 hover:underline"
          onclick={() => {
            action.run();
            app.dismiss(t.id);
          }}>{action.label}</button
        >
      {/if}
      <button class="shrink-0 opacity-60 hover:opacity-100" onclick={() => app.dismiss(t.id)} aria-label="Dismiss">
        <Icon name="x" size={14} />
      </button>
    </div>
  {/each}
</div>
