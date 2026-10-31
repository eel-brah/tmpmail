<script lang="ts">
  // The only place mail HTML is shown. The body comes from the Rust scheme
  // handler (sanitized by ammonia, with its own CSP header) into a frame
  // that may open popups and nothing else: no scripts, no same-origin, so
  // it can't reach the app or Tauri's IPC. Never add allow-scripts or
  // allow-same-origin here. The theme and view options travel in the URL.
  //
  // Keyboard focus never stays in the frame: a click there moves it in,
  // and the app's keys (j/k, d, c, /, ?, …) would stop, since nothing in the
  // frame can pass them on. It comes back to the wrapper around the frame as
  // soon as it lands there. A click still opens links, wheel and scrollbar
  // scrolling and text selection still work; Ctrl+C then reaches the app, not
  // the mail, so copying from the mail is through its context menu (a hint
  // says so), and the mail no longer scrolls with Space or Page Down. The
  // frame is out of the Tab order: its web links are all in the Links list
  // above it, and Tab from the wrapper goes on past the mail.
  import { onMount } from "svelte";
  import { app } from "./store.svelte";

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

  let frame: HTMLIFrameElement;
  let wrapper: HTMLDivElement;
  /** The last Tab press, to skip over the frame in its direction if one still lands in it. */
  let tab = { at: -Infinity, back: false };
  /** Focus came back from the frame, and nothing else took it since: the user may be selecting mail text. */
  let fromMail = false;

  const FOCUSABLE = "a[href], button, input, select, textarea, [tabindex]";

  /** Whether Tab can reach `el`. */
  function tabbable(el: HTMLElement): boolean {
    return (
      el.tabIndex >= 0 &&
      !el.matches(":disabled") &&
      !el.closest("dialog:not([open]), [inert]") &&
      el.getClientRects().length > 0 &&
      getComputedStyle(el).visibility === "visible"
    );
  }

  /** The first element Tab reaches after (or before) the wrapper, outside it. */
  function beyond(back: boolean): HTMLElement | undefined {
    const all = [...document.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !wrapper.contains(el) && tabbable(el));
    const after = (el: HTMLElement) => (wrapper.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
    return back ? all.filter((el) => !after(el)).at(-1) : all.find(after);
  }

  function takeBack() {
    if (document.activeElement !== frame) return;
    // Tab and Shift+Tab skip the frame (an engine may still tab into it).
    // Only right after the key, so a click into the mail isn't taken for it.
    if (performance.now() - tab.at < 100) {
      const next = beyond(tab.back);
      if (next) {
        next.focus();
        return;
      }
    }
    wrapper.focus({ preventScroll: true });
    // A selection left in the app would be what Ctrl+C copies, instead of
    // the hint about the mail's.
    document.getSelection()?.removeAllRanges();
    fromMail = true;
  }

  // The window loses the focus to the frame: it is in the frame once the
  // event is over (some engines set activeElement after the event).
  function onwindowblur() {
    setTimeout(takeBack, 0);
  }

  // A click into the mail of a window that wasn't focused (from another
  // app, or after a native dialog) gives no blur: a light check catches it.
  onMount(() => {
    const timer = setInterval(() => {
      if (document.activeElement === frame && document.hasFocus()) takeBack();
    }, 250);
    return () => clearInterval(timer);
  });

  function onwindowkeydown(e: KeyboardEvent) {
    if (e.key === "Tab") tab = { at: performance.now(), back: e.shiftKey };
    // Esc leaves the mail altogether.
    if (e.key === "Escape" && document.activeElement === wrapper && !e.ctrlKey && !e.metaKey && !e.altKey) wrapper.blur();
    if (
      fromMail &&
      document.activeElement === wrapper &&
      (e.ctrlKey || e.metaKey) &&
      !e.altKey &&
      e.key.toLowerCase() === "c" &&
      !document.getSelection()?.toString()
    ) {
      fromMail = false;
      app.toast("info", "To copy text from the mail, right-click it and choose Copy");
    }
  }
</script>

<svelte:window onblur={onwindowblur} onkeydown={onwindowkeydown} />

<!-- Focus moving elsewhere in the app ends the hint; the window losing it doesn't. -->
<div
  bind:this={wrapper}
  class="h-full w-full outline-none"
  tabindex="-1"
  role="region"
  aria-label={title}
  onfocusout={(e) => {
    if (e.relatedTarget && e.relatedTarget !== frame) fromMail = false;
  }}
>
  <iframe
    bind:this={frame}
    class={["block h-full w-full border-0", theme === "dark" && !original ? "bg-[#222222]" : "bg-white"]}
    {src}
    {title}
    tabindex="-1"
    sandbox="allow-popups"
    referrerpolicy="no-referrer"
    {onload}
  ></iframe>
</div>
