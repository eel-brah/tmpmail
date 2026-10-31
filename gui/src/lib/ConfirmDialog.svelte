<script lang="ts">
  // Asks before deleting a message or an inbox, like the TUI's d and D.
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";

  let dialog: HTMLDialogElement;
  const c = $derived(app.confirm);
  const inbox = $derived(c?.kind === "inbox" ? app.inboxes.find((i) => i.address === c.address) : undefined);
  const caps = $derived(inbox ? app.provider(inbox.provider)?.caps : undefined);

  $effect(() => {
    if (c && !dialog.open) dialog.showModal();
    else if (!c && dialog.open) dialog.close();
  });

  function confirm() {
    if (c?.kind === "message") app.deleteMessage(c.address, c.id);
    else if (c?.kind === "inbox") app.deleteInbox(c.address);
    app.confirm = null;
  }
</script>

<dialog
  bind:this={dialog}
  class="w-[min(26rem,calc(100vw-2rem))]"
  aria-labelledby="confirm-title"
  onclose={() => (app.confirm = null)}
>
  {#if c}
    <div class="flex min-h-0 flex-col">
      <div class="flex min-h-0 flex-1 gap-4 overflow-y-auto px-6 pt-6 pb-5">
        <span class="grid h-10 w-10 shrink-0 place-items-center rounded-full bg-danger-soft text-danger">
          <Icon name="trash" size={18} />
        </span>
        <div class="min-w-0">
          {#if c.kind === "message"}
            <h2 id="confirm-title" class="text-md font-semibold">Delete this message?</h2>
            <p class="mt-1 truncate text-xs text-muted" title={c.subject}>{c.subject || "(no subject)"}</p>
            <p class="mt-2 text-xs text-muted">It's deleted at the provider too. This can't be undone.</p>
          {:else}
            <h2 id="confirm-title" class="text-md font-semibold">Delete this inbox?</h2>
            <p class="mt-1 truncate text-xs font-semibold" title={c.address}>{c.address}</p>
            <p class="mt-2 text-xs text-muted">
              {#if caps?.delete_inbox}
                It's deleted at {app.providerName(inbox?.provider ?? "")} and forgotten here. Its mail goes with it.
              {:else}
                tmpmail forgets it. {app.providerName(inbox?.provider ?? "")} can't delete addresses, so it simply expires there.
              {/if}
            </p>
          {/if}
        </div>
      </div>
      <footer class="flex shrink-0 justify-end gap-2 border-t border-rule px-6 py-3.5">
        <button class="btn" onclick={() => (app.confirm = null)}>Keep it</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button class="btn btn-danger" onclick={confirm} autofocus>
          {c.kind === "message" ? "Delete message" : "Delete inbox"}
        </button>
      </footer>
    </div>
  {/if}
</dialog>
