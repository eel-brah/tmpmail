<script lang="ts">
  // The settings screen. The provider order, poll interval, push and request
  // timeout are the CLI's config.toml fields (Rust edits the file and keeps
  // its comments); notifications and the tray are the app's own (gui.json).
  // Rust checks everything again when saving.
  import { api, type SettingsView } from "./api";
  import Icon from "./Icon.svelte";
  import { app } from "./store.svelte";
  import { theme, type ThemePref } from "./theme.svelte";

  let dialog: HTMLDialogElement;
  let view = $state<SettingsView | null>(null);
  let loadError = $state<string | null>(null);
  let saveError = $state<string | null>(null);
  let saving = $state(false);

  // The form.
  let providers = $state<{ id: string; name: string; enabled: boolean }[]>([]);
  let pollInterval = $state(5);
  let push = $state(true);
  let httpTimeout = $state(20);
  let notifications = $state(true);
  let tray = $state(false);
  let startMinimized = $state(false);

  const enabled = $derived(providers.filter((p) => p.enabled));
  const linux = $derived(view?.os === "linux");
  const overridden = (name: string) => view?.overrides.includes(name) ?? false;

  const themes: { value: ThemePref; label: string }[] = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];

  $effect(() => {
    if (app.settingsOpen && !dialog.open) {
      view = null;
      loadError = null;
      saveError = null;
      dialog.showModal();
      api.settings().then(
        (v) => {
          view = v;
          providers = v.providers.map((p) => ({ ...p }));
          pollInterval = v.poll_interval;
          push = v.push;
          httpTimeout = v.http_timeout;
          notifications = v.gui.notifications;
          tray = v.gui.tray;
          startMinimized = v.gui.start_minimized;
        },
        (e) => (loadError = String(e)),
      );
    } else if (!app.settingsOpen && dialog.open) {
      dialog.close();
    }
  });

  function move(i: number, delta: number) {
    const j = i + delta;
    if (j < 0 || j >= providers.length) return;
    const list = [...providers];
    [list[i], list[j]] = [list[j]!, list[i]!];
    providers = list;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    saving = true;
    saveError = null;
    try {
      await api.saveSettings({
        providers: enabled.map((p) => p.id),
        poll_interval: pollInterval,
        push,
        http_timeout: httpTimeout,
        notifications,
        tray,
        start_minimized: tray && startMinimized,
      });
      app.settingsOpen = false;
      app.toast("success", "Settings saved");
    } catch (err) {
      saveError = String(err);
    } finally {
      saving = false;
    }
  }

  async function reveal() {
    try {
      await api.revealConfig();
    } catch (err) {
      app.toast("info", String(err));
    }
  }
</script>

{#snippet envNote(name: string)}
  {#if overridden(name)}
    <p class="mt-1 text-2xs text-muted">
      <code class="font-mono">{name}</code> is set in the environment and wins over this.
    </p>
  {/if}
{/snippet}

<dialog
  bind:this={dialog}
  class="w-[min(42rem,calc(100vw-2rem))]"
  aria-labelledby="settings-title"
  onclose={() => (app.settingsOpen = false)}
>
  <form class="flex min-h-0 flex-col" onsubmit={submit}>
    <header class="flex shrink-0 items-start justify-between gap-4 px-6 pt-5">
      <div>
        <h2 id="settings-title" class="text-lg font-semibold tracking-title">Settings</h2>
        <p class="mt-0.5 text-xs text-muted">Providers and checking are shared with the <code class="font-mono">tmpmail</code> command.</p>
      </div>
      <button type="button" class="btn btn-ghost btn-icon -mr-2" aria-label="Close" onclick={() => (app.settingsOpen = false)}>
        <Icon name="x" />
      </button>
    </header>

    <div class="min-h-0 flex-1 overflow-y-auto px-6 py-4">
      {#if loadError}
        <p class="selectable rounded-lg bg-danger-soft px-4 py-3 text-xs break-words text-danger" role="alert">{loadError}</p>
      {:else if !view}
        <p class="py-8 text-center text-xs text-muted">Reading the settings…</p>
      {:else}
        <div class="space-y-6">
          <section aria-labelledby="set-look">
            <h3 id="set-look" class="mb-2 text-xs font-semibold">Theme</h3>
            <div class="inline-flex rounded-lg border border-rule bg-sunken p-0.5" role="radiogroup" aria-labelledby="set-look">
              {#each themes as t (t.value)}
                <button
                  type="button"
                  class={[
                    "h-7 rounded-md px-3 text-xs font-semibold transition-colors",
                    theme.pref === t.value ? "bg-raised text-ink shadow-sm" : "text-muted hover:text-ink",
                  ]}
                  role="radio"
                  aria-checked={theme.pref === t.value}
                  onclick={() => theme.set(t.value)}>{t.label}</button
                >
              {/each}
            </div>
            <p class="mt-1 text-2xs text-muted">Applies at once. The switch in the sidebar does the same.</p>
          </section>

          <section aria-labelledby="set-providers">
            <h3 id="set-providers" class="text-xs font-semibold">Providers</h3>
            <p class="mt-0.5 mb-2 text-2xs text-muted">
              {#if enabled.length}
                New inboxes try {enabled.map((p) => p.name).join(", then ")}.
              {:else}
                Turn on at least one provider.
              {/if}
            </p>
            <ol class="divide-y divide-rule overflow-hidden rounded-lg border border-rule">
              {#each providers as p, i (p.id)}
                {@const rank = enabled.indexOf(p)}
                <li class={["flex items-center gap-3 py-1.5 pr-1.5 pl-3", !p.enabled && "bg-sunken"]}>
                  <input
                    type="checkbox"
                    id="prov-{p.id}"
                    bind:checked={p.enabled}
                    class="accent-[var(--color-ink)]"
                  />
                  <label for="prov-{p.id}" class={["min-w-0 flex-1 text-xs", p.enabled ? "font-semibold" : "text-muted"]}>
                    {p.name}
                    {#if p.id === "maildrop"}<span class="ml-1 font-normal text-danger">public inboxes</span>{/if}
                  </label>
                  <span class="w-12 text-right text-2xs text-muted tabular-nums">{rank >= 0 ? `${rank + 1} of ${enabled.length}` : "off"}</span>
                  <button
                    type="button"
                    class="btn btn-ghost btn-icon !h-7"
                    disabled={i === 0}
                    aria-label="Move {p.name} up"
                    title="Try earlier"
                    onclick={() => move(i, -1)}><Icon name="up" size={14} /></button
                  >
                  <button
                    type="button"
                    class="btn btn-ghost btn-icon !h-7"
                    disabled={i === providers.length - 1}
                    aria-label="Move {p.name} down"
                    title="Try later"
                    onclick={() => move(i, 1)}><Icon name="down" size={14} /></button
                  >
                </li>
              {/each}
            </ol>
            {@render envNote("TMPMAIL_PROVIDER")}
          </section>

          <section aria-labelledby="set-check">
            <h3 id="set-check" class="mb-2 text-xs font-semibold">Checking for mail</h3>
            <div class="grid grid-cols-2 gap-3">
              <label class="block">
                <span class="mb-1.5 block text-xs">Check every</span>
                <span class="flex items-center gap-2">
                  <input class="field !w-24 tabular-nums" type="number" min="2" max="3600" required bind:value={pollInterval} />
                  <span class="text-xs text-muted">seconds</span>
                </span>
                {@render envNote("TMPMAIL_POLL_INTERVAL")}
              </label>
              <label class="block">
                <span class="mb-1.5 block text-xs">Give up on a request after</span>
                <span class="flex items-center gap-2">
                  <input class="field !w-24 tabular-nums" type="number" min="1" max="600" required bind:value={httpTimeout} />
                  <span class="text-xs text-muted">seconds</span>
                </span>
                {#if httpTimeout !== 20}
                  <p class="mt-1 text-2xs text-muted">Saved as <code class="font-mono">http_timeout</code>, new in tmpmail 0.3.0. tmpmail 0.2.0 and earlier refuse unknown config keys; 0.3.0 and later warn.</p>
                {/if}
                {@render envNote("TMPMAIL_HTTP_TIMEOUT")}
              </label>
            </div>
            <label class="mt-3 flex items-start gap-2.5">
              <input type="checkbox" bind:checked={push} class="mt-0.5 accent-[var(--color-ink)]" />
              <span class="text-xs">
                Live push where the provider has it
                <span class="block text-2xs text-muted">mail.tm and DropMail deliver at once instead of at the next check.</span>
              </span>
            </label>
            {@render envNote("TMPMAIL_PUSH")}
          </section>

          <section aria-labelledby="set-desktop">
            <h3 id="set-desktop" class="mb-2 text-xs font-semibold">Desktop</h3>
            <div class="space-y-3">
              <label class="flex items-start gap-2.5">
                <input type="checkbox" bind:checked={notifications} class="mt-0.5 accent-[var(--color-ink)]" />
                <span class="text-xs">
                  Notify me of new mail
                  <span class="block text-2xs text-muted">While tmpmail isn't the focused window. Clicking one opens the message.</span>
                </span>
              </label>
              <label class="flex items-start gap-2.5">
                <input type="checkbox" bind:checked={tray} class="mt-0.5 accent-[var(--color-ink)]" />
                <span class="text-xs">
                  Show a tray icon
                  <span class="block text-2xs text-muted">Closing the window then keeps tmpmail checking mail. Quit from the tray menu.</span>
                </span>
              </label>
              <label class={["ml-6 flex items-start gap-2.5", !tray && "opacity-50"]}>
                <input type="checkbox" bind:checked={startMinimized} disabled={!tray} class="mt-0.5 accent-[var(--color-ink)]" />
                <span class="text-xs">Start hidden in the tray</span>
              </label>
              {#if linux && tray && !view.tray_host}
                <p class="flex gap-2 rounded-lg bg-danger-soft px-3 py-2 text-2xs text-danger" role="status">
                  <Icon name="alert" size={14} class="mt-px" />
                  <span>
                    No tray was found on this desktop, so closing the window still quits. On GNOME, install the
                    AppIndicator extension.
                  </span>
                </p>
              {:else if linux}
                <p class="text-2xs text-muted">
                  On Linux the icon opens its menu (there is no click action), and GNOME shows it only with the AppIndicator
                  extension.
                </p>
              {/if}
            </div>
          </section>

          <section aria-labelledby="set-file">
            <h3 id="set-file" class="mb-2 text-xs font-semibold">Config file</h3>
            <div class="flex items-center gap-2">
              <code class="selectable min-w-0 flex-1 truncate rounded-lg bg-sunken px-3 py-1.5 font-mono text-2xs" title={view.config_path ?? ""}
                >{view.config_path ?? "No config directory on this system"}</code
              >
              <button type="button" class="btn" onclick={reveal} disabled={!view.config_exists} title={view.config_exists ? "Show it in your file manager" : "Created when you save a provider or checking setting"}>
                <Icon name="folder" size={14} />Show in folder
              </button>
            </div>
            <p class="mt-1 text-2xs text-muted">
              {view.config_exists ? "Saving keeps your comments and other keys." : "Not created yet: saving creates it if a shared setting changes."}
              Notification and tray settings are in <code class="font-mono">gui.json</code> next to it.
            </p>
          </section>
        </div>
      {/if}
    </div>

    <footer class="flex shrink-0 items-center justify-end gap-2 border-t border-rule px-6 py-3.5">
      {#if saveError}
        <p class="selectable mr-auto min-w-0 text-2xs break-words text-danger" role="alert">{saveError}</p>
      {/if}
      <button type="button" class="btn" onclick={() => (app.settingsOpen = false)}>Cancel</button>
      <button type="submit" class="btn btn-primary min-w-28" disabled={!view || saving || !enabled.length}>
        {saving ? "Saving…" : "Save settings"}
      </button>
    </footer>
  </form>
</dialog>
