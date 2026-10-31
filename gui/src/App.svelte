<script lang="ts">
  import { onMount } from "svelte";
  import Preview from "./lib/Preview.svelte";
  import ProbeView from "./lib/ProbeView.svelte";
  import {
    ago,
    api,
    on,
    senderName,
    type InboxView,
    type Init,
    type Notice,
    type OpenedEvent,
    type Summary,
  } from "./lib/api";

  interface MailState {
    messages: Summary[];
    loaded: boolean;
    live: boolean;
    error: string | null;
    expired: boolean;
  }

  let init = $state<Init | null>(null);
  let startError = $state<string | null>(null);
  let inboxes = $state<InboxView[]>([]);
  let mail = $state<Record<string, MailState>>({});
  let selected = $state<string | null>(null);
  let openId = $state<string | null>(null);
  let opened = $state<OpenedEvent | null>(null);
  let images = $state(false);
  let creating = $state(false);
  let pickProvider = $state("");
  let notice = $state<Notice | null>(null);
  let now = $state(Date.now());

  let current = $derived(inboxes.find((i) => i.address === selected) ?? null);
  let currentMail = $derived(selected ? mail[selected] : undefined);
  // The worker's latest word wins over the inbox list's (it may be older).
  function isExpired(inbox: InboxView): boolean {
    return mail[inbox.address]?.expired ?? inbox.expired;
  }
  function providerName(id: string): string {
    return init?.providers.find((p) => p.id === id)?.name ?? id;
  }

  function select(address: string) {
    if (selected === address) return;
    selected = address;
    openId = null;
    opened = null;
    void api.selectInbox(address);
  }

  function open(s: Summary) {
    if (!selected) return;
    openId = s.id;
    opened = null;
    images = false;
    void api.openMessage(selected, s.id);
  }

  async function create() {
    creating = true;
    notice = null;
    try {
      await api.createInbox(pickProvider || null);
    } catch (e) {
      creating = false;
      notice = { kind: "error", text: String(e) };
    }
  }

  onMount(() => {
    const unlisten = [
      on("inboxes", (p) => {
        inboxes = p.inboxes;
        if (!selected || !inboxes.some((i) => i.address === selected)) {
          const first = inboxes.find((i) => i.active) ?? inboxes[0];
          selected = null;
          openId = null;
          opened = null;
          if (first) select(first.address);
        }
      }),
      on("messages", (p) => {
        const messages = mail[p.address]?.messages ?? [];
        if ("messages" in p) {
          mail[p.address] = { messages: p.messages, loaded: true, live: p.live, error: null, expired: false };
        } else if ("error" in p) {
          mail[p.address] = { messages, loaded: true, live: p.live, error: p.error, expired: false };
        } else {
          mail[p.address] = { messages, loaded: true, live: false, error: null, expired: true };
        }
      }),
      on("opened", (p) => {
        if (p.address === selected && p.id === openId) opened = p;
      }),
      on("created", (p) => {
        creating = false;
        selected = null;
        select(p.address);
      }),
      on("notice", (p) => {
        if (p.kind === "error") creating = false;
        notice = p;
      }),
    ];
    // Listen first, then ask for the data, so the first events aren't lost.
    void Promise.all(unlisten)
      .then(() => api.init())
      .then((i) => (init = i))
      .catch((e) => (startError = String(e)));
    const tick = window.setInterval(() => (now = Date.now()), 30000);
    return () => {
      window.clearInterval(tick);
      void Promise.all(unlisten).then((fns) => fns.forEach((f) => f()));
    };
  });
</script>

{#if startError}
  <main class="grid h-full place-items-center p-8">
    <p class="max-w-md text-center text-alert">tmpmail could not start: {startError}</p>
  </main>
{:else if init?.probe}
  <ProbeView origin={init.origin} plan={init.probe} />
{:else}
  <div class="grid h-full grid-cols-[15rem_21rem_minmax(0,1fr)]">
    <!-- Inboxes -->
    <nav class="flex min-h-0 flex-col border-r border-rule bg-panel" aria-label="Inboxes">
      <div class="px-4 pt-4 pb-3">
        <h1 class="text-md font-semibold tracking-tight">tmpmail</h1>
      </div>
      <form
        class="flex flex-col gap-2 border-b border-rule px-4 pb-4"
        onsubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <label class="text-2xs text-muted" for="provider">Provider</label>
        <select
          id="provider"
          class="rounded border border-rule bg-raised px-2 py-1.5 text-xs"
          bind:value={pickProvider}
          disabled={creating}
        >
          <option value="">First that works</option>
          {#each init?.providers ?? [] as p (p.id)}
            <option value={p.id}>{p.name}{p.private ? "" : " (public inbox)"}</option>
          {/each}
        </select>
        <button
          type="submit"
          class="rounded bg-accent px-3 py-1.5 text-xs font-medium text-on-accent disabled:opacity-60"
          disabled={creating}
        >
          {creating ? "Creating…" : "New inbox"}
        </button>
      </form>
      <ul class="min-h-0 flex-1 overflow-y-auto py-1">
        {#each inboxes as inbox (inbox.address)}
          {@const count = mail[inbox.address]?.messages.length}
          <li>
            <button
              class={[
                "flex w-full flex-col items-start px-4 py-2 text-left",
                inbox.address === selected ? "bg-accent-soft" : "hover:bg-raised",
                isExpired(inbox) && "opacity-55",
              ]}
              aria-current={inbox.address === selected}
              onclick={() => select(inbox.address)}
            >
              <span class="w-full truncate text-xs font-medium">{inbox.label ?? inbox.address}</span>
              <span class="text-2xs text-muted">
                {providerName(inbox.provider)}{isExpired(inbox) ? ", expired" : ""}{count
                  ? `, ${count} mail${count === 1 ? "" : "s"}`
                  : ""}
              </span>
            </button>
          </li>
        {:else}
          <li class="px-4 py-3 text-xs text-muted">No inboxes yet. Create one above.</li>
        {/each}
      </ul>
    </nav>

    <!-- Messages -->
    <section class="flex min-h-0 flex-col border-r border-rule bg-raised" aria-label="Messages">
      {#if current}
        <header class="border-b border-rule px-4 pt-4 pb-3">
          <p class="text-lg leading-tight font-semibold tracking-tight break-all select-text">
            {current.address}
          </p>
          <p class="mt-1 text-2xs text-muted">
            {providerName(current.provider)}
            {#if isExpired(current)}<span class="text-alert">, expired</span>
            {:else if currentMail?.live}<span class="text-accent">, live</span>{/if}
            {#if !current.private}<span class="text-alert">, public: anyone can read it</span>{/if}
          </p>
        </header>
        {#if currentMail?.error}
          <p class="mx-4 mt-3 rounded bg-alert-soft px-3 py-2 text-xs text-alert">{currentMail.error}</p>
        {/if}
        <ul class="min-h-0 flex-1 overflow-y-auto">
          {#each currentMail?.messages ?? [] as s (s.id)}
            <li class="border-b border-rule">
              <button
                class={[
                  "flex w-full flex-col gap-0.5 px-4 py-2.5 text-left",
                  s.id === openId ? "bg-accent-soft" : "hover:bg-panel",
                ]}
                aria-current={s.id === openId}
                onclick={() => open(s)}
              >
                <span class="flex w-full items-baseline gap-2">
                  <span class={["min-w-0 flex-1 truncate text-xs", !s.seen && "font-semibold"]}>
                    {senderName(s.from)}
                  </span>
                  <span class="shrink-0 text-2xs text-faint">{ago(s.date, now)}</span>
                </span>
                <span class={["w-full truncate text-xs", s.seen ? "text-muted" : "text-ink"]}>
                  {s.subject || "(no subject)"}
                </span>
              </button>
            </li>
          {:else}
            <li class="px-4 py-6 text-xs text-muted">
              {#if isExpired(current)}
                This inbox has expired. No more mail will arrive.
              {:else if currentMail?.loaded}
                No mail yet. New mail shows up here by itself.
              {:else}
                Checking for mail…
              {/if}
            </li>
          {/each}
        </ul>
      {:else}
        <p class="p-6 text-xs text-muted">Pick an inbox, or create one.</p>
      {/if}
    </section>

    <!-- Preview -->
    <section class="flex min-h-0 min-w-0 flex-col" aria-label="Message">
      {#if opened && "key" in opened && init}
        <header class="flex items-start gap-4 border-b border-rule bg-raised px-6 pt-4 pb-3">
          <div class="min-w-0 flex-1">
            <h2 class="text-md leading-snug font-semibold select-text">{opened.subject || "(no subject)"}</h2>
            <p class="mt-1 truncate text-xs text-muted select-text">
              {opened.from}{opened.date ? `, ${new Date(opened.date).toLocaleString()}` : ""}
            </p>
            {#if opened.attachments.length}
              <p class="mt-1 truncate text-2xs text-faint">
                Attached: {opened.attachments.map((a) => a.filename).join(", ")}
              </p>
            {/if}
          </div>
          <button
            class="shrink-0 rounded border border-rule px-3 py-1.5 text-xs hover:bg-panel"
            aria-pressed={images}
            onclick={() => (images = !images)}
            title="Remote images can tell the sender you opened the mail"
          >
            {images ? "Hide images" : "Load images"}
          </button>
        </header>
        <div class="min-h-0 flex-1">
          <Preview origin={init.origin} mailKey={opened.key} {images} title="Message body" />
        </div>
      {:else if opened && "error" in opened}
        <p class="m-6 rounded bg-alert-soft px-3 py-2 text-xs text-alert">{opened.error}</p>
      {:else if openId}
        <p class="p-6 text-xs text-muted">Opening…</p>
      {:else}
        <p class="p-6 text-xs text-muted">Select a message to read it.</p>
      {/if}
    </section>
  </div>
{/if}

{#if notice}
  <div
    class={[
      "fixed right-4 bottom-4 flex max-w-sm items-start gap-3 rounded-md border px-4 py-3 text-xs shadow-lg",
      notice.kind === "error" ? "border-alert/40 bg-alert-soft text-alert" : "border-rule bg-raised text-ink",
    ]}
    role="status"
  >
    <span class="min-w-0 flex-1">{notice.text}</span>
    <button class="shrink-0 text-muted hover:text-ink" onclick={() => (notice = null)} aria-label="Dismiss">✕</button>
  </div>
{/if}
