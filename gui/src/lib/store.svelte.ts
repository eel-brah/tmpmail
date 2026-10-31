// The window's state and every action on it. Components read from `app` and
// call its methods; the Rust side answers with events, handled in `listen`.
import { SvelteSet } from "svelte/reactivity";
import {
  api,
  on,
  senderName,
  type CreateArgs,
  type InboxView,
  type Init,
  type OpenedEvent,
  type ProviderInfo,
  type Summary,
} from "./api";

export interface MailState {
  messages: Summary[];
  loaded: boolean;
  live: boolean;
  error: string | null;
  expired: boolean;
}

export interface Toast {
  id: number;
  kind: "error" | "info" | "success" | "mail";
  text: string;
  action?: { label: string; run: () => void };
}

export type Confirm =
  | { kind: "message"; address: string; id: string; subject: string }
  | { kind: "inbox"; address: string };

/// Ids of messages that arrived while the window was open: "address\nid".
const freshKey = (address: string, id: string) => `${address}\n${id}`;

function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

class AppState {
  init = $state<Init | null>(null);
  startError = $state<string | null>(null);
  inboxes = $state<InboxView[]>([]);
  mail = $state<Record<string, MailState>>({});
  selected = $state<string | null>(null);
  openId = $state<string | null>(null);
  opened = $state<OpenedEvent | null>(null);
  creating = $state(false);
  query = $state("");
  fresh = new SvelteSet<string>();
  toasts = $state<Toast[]>([]);
  now = $state(Date.now());

  // Dialogs.
  newInboxOpen = $state(false);
  helpOpen = $state(false);
  settingsOpen = $state(false);
  confirm = $state<Confirm | null>(null);

  // The open message's view.
  images = $state(false);
  original = $state(false);
  source = $state(false);

  /** Attachments ready to drag out, by "key/index"; false while downloading. */
  dragReady = $state<Record<string, boolean>>({});

  /** Message ids each inbox had at its first listing, to spot new mail. */
  #known = new Map<string, Set<string>>();
  /** What a notification click that started the app opens, once the inboxes are in. */
  #pendingReveal: { address: string; id: string | null } | null = null;
  #toastId = 0;

  current = $derived(this.inboxes.find((i) => i.address === this.selected) ?? null);
  currentMail = $derived(this.selected ? this.mail[this.selected] : undefined);
  visible = $derived.by(() => {
    const list = this.currentMail?.messages ?? [];
    const q = this.query.trim().toLowerCase();
    if (!q) return list;
    return list.filter((s) => s.from.toLowerCase().includes(q) || s.subject.toLowerCase().includes(q));
  });
  openedMessage = $derived(this.opened && "key" in this.opened ? this.opened : null);

  provider(id: string): ProviderInfo | undefined {
    return this.init?.providers.find((p) => p.id === id);
  }

  providerName(id: string): string {
    return this.provider(id)?.name ?? id;
  }

  /** The worker's latest word wins over the inbox list's (it may be older). */
  isExpired(inbox: InboxView): boolean {
    return this.mail[inbox.address]?.expired ?? inbox.expired;
  }

  isFresh(address: string, id: string): boolean {
    return this.fresh.has(freshKey(address, id));
  }

  // ---------- toasts ----------

  toast(kind: Toast["kind"], text: string, action?: Toast["action"]) {
    const id = ++this.#toastId;
    this.toasts = [...this.toasts.slice(-3), { id, kind, text, action }];
    const ms = kind === "error" ? 9000 : kind === "mail" ? 7000 : 3500;
    window.setTimeout(() => this.dismiss(id), ms);
  }

  dismiss(id: number) {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  /** Runs a command, turning a failure into an error toast. */
  async #run<T>(p: Promise<T>): Promise<T | undefined> {
    try {
      return await p;
    } catch (e) {
      this.toast("error", errorText(e));
      return undefined;
    }
  }

  // ---------- inboxes ----------

  select(address: string) {
    if (this.selected === address) return;
    this.selected = address;
    this.query = "";
    this.closeMessage();
    void this.#run(api.selectInbox(address));
  }

  step(inboxDelta: number) {
    const n = this.inboxes.findIndex((i) => i.address === this.selected);
    const next = this.inboxes[Math.min(this.inboxes.length - 1, Math.max(0, n + inboxDelta))];
    if (next) this.select(next.address);
  }

  refresh() {
    if (!this.selected) return;
    void this.#run(api.selectInbox(this.selected).then(() => true)).then((ok) => {
      if (ok) this.toast("info", "Checking for new mail");
    });
  }

  async create(args: CreateArgs): Promise<boolean> {
    this.creating = true;
    try {
      await api.createInbox(args);
      return true;
    } catch (e) {
      this.creating = false;
      this.toast("error", errorText(e));
      return false;
    }
  }

  async copyAddress(address = this.selected) {
    if (!address) return;
    const ok = await this.#run(api.copyAddress(address).then(() => true));
    if (ok) this.toast("success", "Address copied");
  }

  makeActive(address = this.selected) {
    if (!address) return;
    void this.#run(api.useInbox(address));
  }

  deleteInbox(address: string) {
    void this.#run(api.deleteInbox(address));
  }

  // ---------- messages ----------

  open(s: Summary) {
    if (!this.selected) return;
    if (this.openId === s.id && this.opened) return;
    this.openId = s.id;
    this.opened = null;
    this.images = false;
    this.original = false;
    this.source = false;
    this.fresh.delete(freshKey(this.selected, s.id));
    void this.#run(api.openMessage(this.selected, s.id));
  }

  closeMessage() {
    this.openId = null;
    this.opened = null;
  }

  /** Moves through the visible messages and opens the one it lands on. */
  move(delta: number) {
    const list = this.visible;
    if (!list.length) return;
    const n = list.findIndex((s) => s.id === this.openId);
    const next = n < 0 ? (delta > 0 ? 0 : list.length - 1) : Math.min(list.length - 1, Math.max(0, n + delta));
    const s = list[next];
    if (s) this.open(s);
  }

  deleteMessage(address: string, id: string) {
    void this.#run(api.deleteMessage(address, id));
  }

  async copyFromMessage(part: "code" | "sender" | "link", index?: number): Promise<boolean> {
    const m = this.openedMessage;
    if (!m) return false;
    const ok = await this.#run(api.copyFromMessage(m.key, part, index).then(() => true));
    if (ok) this.toast("success", part === "code" ? "Code copied" : part === "sender" ? "Sender copied" : "Link copied");
    return !!ok;
  }

  openLink(index: number) {
    const m = this.openedMessage;
    if (m) void this.#run(api.openLink(m.key, index));
  }

  async saveAttachment(index: number) {
    const m = this.openedMessage;
    if (!m) return;
    const name = await this.#run(api.saveAttachment(m.key, index));
    if (name) this.toast("success", `Saved ${name}`);
  }

  /** Downloads an attachment for dragging, when the pointer reaches it. */
  prepareDrag(index: number) {
    const m = this.openedMessage;
    if (!m || !this.init?.drag) return;
    const k = `${m.key}/${index}`;
    if (k in this.dragReady) return;
    this.dragReady[k] = false;
    api.prepareAttachment(m.key, index).then(
      () => (this.dragReady[k] = true),
      (e) => {
        delete this.dragReady[k];
        this.toast("error", errorText(e));
      },
    );
  }

  dragOut(index: number) {
    const m = this.openedMessage;
    if (!m) return;
    api.dragAttachment(m.key, index).catch((e) => {
      // Its file may have been cleaned up: prepare it again next time.
      delete this.dragReady[`${m.key}/${index}`];
      this.toast("error", errorText(e));
    });
  }

  /** Opens an inbox and, if given, one of its messages (a notification was clicked). */
  reveal(address: string, id: string | null) {
    if (!this.inboxes.some((i) => i.address === address)) return;
    this.select(address);
    if (!id) return;
    const s = this.mail[address]?.messages.find((m) => m.id === id);
    if (s) this.open(s);
    else {
      // Not listed here yet: open it by id.
      this.openId = id;
      this.opened = null;
      void this.#run(api.openMessage(address, id));
    }
  }

  /** Opens what a notification click asked for before the inboxes were in. Returns whether it did. */
  #revealPending(): boolean {
    const r = this.#pendingReveal;
    if (!r || !this.inboxes.length || document.querySelector("dialog[open]")) return false;
    this.#pendingReveal = null;
    if (!this.inboxes.some((i) => i.address === r.address)) return false;
    this.reveal(r.address, r.id);
    return true;
  }

  // ---------- events ----------

  #onMessages(address: string, list: Summary[]) {
    const known = this.#known.get(address);
    if (known) {
      const arrived = list.filter((s) => !known.has(s.id));
      for (const s of arrived) this.fresh.add(freshKey(address, s.id));
      const newest = arrived[0];
      if (newest) {
        const more = arrived.length > 1 ? ` and ${arrived.length - 1} more` : "";
        this.toast("mail", `${senderName(newest.from)}: ${newest.subject || "(no subject)"}${more}`, {
          label: "Open",
          run: () => {
            this.select(address);
            this.open(newest);
          },
        });
      }
    }
    this.#known.set(address, new Set(list.map((s) => s.id)));
  }

  listen(): () => void {
    const unlisten = [
      on("inboxes", (p) => {
        this.inboxes = p.inboxes;
        // A notification click's inbox first, so only that one is listed now.
        if (this.#revealPending()) return;
        if (!this.selected || !this.inboxes.some((i) => i.address === this.selected)) {
          const first = this.inboxes.find((i) => i.active) ?? this.inboxes[0];
          this.selected = null;
          this.closeMessage();
          if (first) this.select(first.address);
        }
      }),
      on("messages", (p) => {
        const old = this.mail[p.address];
        const messages = old?.messages ?? [];
        if ("messages" in p) {
          this.#onMessages(p.address, p.messages);
          this.mail[p.address] = { messages: p.messages, loaded: true, live: p.live, error: null, expired: false };
        } else if ("error" in p) {
          this.mail[p.address] = { messages, loaded: true, live: p.live, error: p.error, expired: false };
        } else {
          this.mail[p.address] = { messages, loaded: true, live: false, error: null, expired: true };
        }
      }),
      on("opened", (p) => {
        // Opened means read: the Rust side saved it for the CLI and TUI too.
        if ("key" in p) {
          const s = this.mail[p.address]?.messages.find((m) => m.id === p.id);
          if (s) s.seen = true;
        }
        if (p.address === this.selected && p.id === this.openId) this.opened = p;
      }),
      on("created", (p) => {
        if (p.from_tray) {
          // The window's own create (if any) is still running: leave it be.
          if (!document.querySelector("dialog[open]")) this.select(p.address);
          return;
        }
        this.creating = false;
        this.newInboxOpen = false;
        this.selected = null;
        this.select(p.address);
        this.toast("success", `Inbox ready: ${p.address}`, {
          label: "Copy",
          run: () => void this.copyAddress(p.address),
        });
      }),
      on("deleted", (p) => {
        const m = this.mail[p.address];
        if (m) m.messages = m.messages.filter((s) => s.id !== p.id);
        this.#known.get(p.address)?.delete(p.id);
        if (p.address === this.selected && p.id === this.openId) this.closeMessage();
      }),
      on("create_failed", (p) => {
        this.creating = false;
        this.toast("error", p.text);
      }),
      on("notice", (p) => this.toast(p.kind, p.text)),
      on("reveal", (p) => {
        if (document.querySelector("dialog[open]")) return;
        // Before the inboxes are in: once they are.
        if (!this.inboxes.length) this.#pendingReveal = p;
        else this.reveal(p.address, p.id);
      }),
    ];
    // Listen first, then ask for the data, so the first events aren't lost.
    void Promise.all(unlisten)
      .then(() => api.init())
      .then((i) => {
        this.init = i;
        if (i.reveal) this.#pendingReveal = i.reveal;
        this.#revealPending();
      })
      .catch((e) => (this.startError = errorText(e)));
    const tick = window.setInterval(() => (this.now = Date.now()), 30000);
    return () => {
      window.clearInterval(tick);
      // The page may not unlisten in Rust (see capabilities/main.json):
      // this only drops the handlers here, and the refused call is ignored.
      void Promise.all(unlisten).then((fns) =>
        fns.forEach((f) => void Promise.resolve(f() as unknown).catch(() => {})),
      );
    };
  }
}

export const app = new AppState();
