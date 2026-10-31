<script lang="ts">
  // The only place mail HTML is shown. The body comes from the Rust scheme
  // handler (sanitized by ammonia, with its own CSP header) into a frame
  // that may open popups and nothing else: no scripts, no same-origin, so
  // it can't reach the app or Tauri's IPC. Never add allow-scripts or
  // allow-same-origin here.
  interface Props {
    origin: string;
    mailKey: number;
    images: boolean;
    title: string;
    onload?: () => void;
  }
  let { origin, mailKey, images, title, onload }: Props = $props();

  let src = $derived(`${origin}/mail/${mailKey}${images ? "?images=1" : ""}`);
</script>

<iframe
  class="h-full w-full border-0 bg-white"
  {src}
  {title}
  sandbox="allow-popups"
  referrerpolicy="no-referrer"
  {onload}
></iframe>
