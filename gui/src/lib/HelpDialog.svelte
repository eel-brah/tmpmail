<script lang="ts">
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";

  let dialog: HTMLDialogElement;

  $effect(() => {
    if (app.helpOpen && !dialog.open) dialog.showModal();
    else if (!app.helpOpen && dialog.open) dialog.close();
  });

  // The same keys as the TUI, where the action exists here.
  const groups: { title: string; keys: [string[], string][] }[] = [
    {
      title: "Messages",
      keys: [
        [["j", "↓"], "Next message"],
        [["k", "↑"], "Previous message"],
        [["y"], "Copy the verification code"],
        [["L"], "Copy the first link"],
        [["O"], "Open the first link"],
        [["d"], "Delete the message"],
        [["/"], "Filter messages"],
      ],
    },
    {
      title: "Inboxes",
      keys: [
        [["n"], "New inbox, first provider that works"],
        [["N"], "New inbox, choose the provider"],
        [["[", "]"], "Previous or next inbox"],
        [["c"], "Copy the address"],
        [["a"], "Make it the active inbox"],
        [["r"], "Check for mail now"],
        [["D"], "Delete the inbox"],
      ],
    },
    {
      title: "App",
      keys: [
        [["?"], "Show these shortcuts"],
        [[/Mac/.test(navigator.platform) ? "⌘ ," : "Ctrl ,"], "Settings"],
        [["Esc"], "Close a dialog"],
      ],
    },
  ];
</script>

<dialog
  bind:this={dialog}
  class="w-[min(40rem,calc(100vw-2rem))]"
  aria-labelledby="help-title"
  onclose={() => (app.helpOpen = false)}
>
  <div class="flex min-h-0 flex-col">
    <header class="flex shrink-0 items-center justify-between px-6 pt-5">
      <h2 id="help-title" class="text-lg font-semibold tracking-title">Keyboard shortcuts</h2>
      <button class="btn btn-ghost btn-icon -mr-2" aria-label="Close" onclick={() => (app.helpOpen = false)}>
        <Icon name="x" />
      </button>
    </header>
    <div class="min-h-0 flex-1 overflow-y-auto">
      <p class="px-6 pt-1 text-xs text-muted">The same keys as <code class="font-mono">tmpmail tui</code>. They're off while you type in a field.</p>
      <div class="grid grid-cols-1 gap-x-8 gap-y-5 px-6 pt-4 pb-6 sm:grid-cols-2">
        {#each groups as g (g.title)}
          <section class={g.title === "App" ? "sm:col-start-2" : ""}>
            <h3 class="mb-2 text-xs font-semibold text-muted">{g.title}</h3>
            <dl class="space-y-1.5">
              {#each g.keys as [keys, what] (what)}
                <div class="flex items-center justify-between gap-3 text-xs">
                  <dt class="order-2 flex shrink-0 gap-1">
                    {#each keys as k (k)}<kbd>{k}</kbd>{/each}
                  </dt>
                  <dd class="order-1">{what}</dd>
                </div>
              {/each}
            </dl>
          </section>
        {/each}
      </div>
    </div>
  </div>
</dialog>
