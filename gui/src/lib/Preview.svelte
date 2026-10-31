<script lang="ts">
  // The only place mail HTML is shown. The body comes from the Rust scheme
  // handler (sanitized by ammonia, with its own CSP header) into a frame
  // that may open popups and nothing else: no scripts, no same-origin, so
  // it can't reach the app or Tauri's IPC. Never add allow-scripts or
  // allow-same-origin here. The theme and view options travel in the URL.
  interface Props {
    origin: string;
    mailKey: number;
    images?: boolean;
    theme?: "light" | "dark";
    original?: boolean;
    source?: boolean;
    title: string;
    onload?: () => void;
  }
  let { origin, mailKey, images = false, theme = "light", original = false, source = false, title, onload }: Props =
    $props();

  let src = $derived.by(() => {
    const q = [`theme=${theme}`];
    if (images && !source) q.push("images=1");
    if (original && !source) q.push("original=1");
    return `${origin}/mail/${mailKey}${source ? "/source" : ""}?${q.join("&")}`;
  });
</script>

<iframe
  class={["block h-full w-full border-0", theme === "dark" && !original ? "bg-[#222222]" : "bg-white"]}
  {src}
  {title}
  sandbox="allow-popups"
  referrerpolicy="no-referrer"
  {onload}
></iframe>
