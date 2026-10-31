// Typed access to the Rust side. Every command and event the window uses is
// here; the `main` capability allows exactly these commands.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface ProviderInfo {
  id: string;
  name: string;
  private: boolean;
  attachments: boolean;
}

export interface ProbePlan {
  real: { name: string; key: number; images: boolean }[];
  controls: { name: string; url: string; sandbox: string | null }[];
  wait_secs: number;
}

export interface Init {
  origin: string;
  providers: ProviderInfo[];
  probe: ProbePlan | null;
}

export interface InboxView {
  address: string;
  provider: string;
  label: string | null;
  created_at: string;
  expires_at: string | null;
  expired: boolean;
  active: boolean;
  private: boolean;
}

export interface Summary {
  id: string;
  from: string;
  subject: string;
  intro: string;
  date: string | null;
  seen: boolean;
  has_attachments: boolean;
}

export interface AttachmentInfo {
  id: string;
  filename: string;
  content_type: string;
  size: number | null;
}

export type MessagesEvent =
  | { address: string; live: boolean; messages: Summary[] }
  | { address: string; error: string };

export type OpenedEvent =
  | {
      address: string;
      id: string;
      key: number;
      from: string;
      to: string[];
      subject: string;
      date: string | null;
      attachments: AttachmentInfo[];
    }
  | { address: string; id: string; error: string };

export interface Notice {
  kind: "error" | "info";
  text: string;
}

export interface Events {
  inboxes: { inboxes: InboxView[] };
  messages: MessagesEvent;
  opened: OpenedEvent;
  created: { address: string };
  notice: Notice;
}

export function on<K extends keyof Events>(
  name: K,
  handler: (payload: Events[K]) => void,
): Promise<UnlistenFn> {
  return listen<Events[K]>(name, (e) => handler(e.payload));
}

export const api = {
  init: () => invoke<Init>("init"),
  createInbox: (provider: string | null) => invoke<void>("create_inbox", { provider }),
  selectInbox: (address: string) => invoke<void>("select_inbox", { address }),
  openMessage: (address: string, id: string) => invoke<void>("open_message", { address, id }),
  probeFinish: () => invoke<void>("probe_finish"),
};

/// "5m ago", "3h ago", "2d ago".
export function ago(iso: string | null, now = Date.now()): string {
  if (!iso) return "";
  const secs = Math.round((now - Date.parse(iso)) / 1000);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

/// The display name of "Name <addr>", else the address.
export function senderName(from: string): string {
  const m = /^\s*"?([^"<]*?)"?\s*<[^>]+>\s*$/.exec(from);
  return m?.[1] ? m[1] : from;
}
