<script lang="ts">
  // A drag handle on a pane's right edge; arrow keys move it too.
  let { width = $bindable(), min, max, label }: { width: number; min: number; max: number; label: string } = $props();

  let start: { x: number; w: number } | null = null;
  const clamp = (w: number) => Math.round(Math.min(max, Math.max(min, w)));

  function down(e: PointerEvent) {
    start = { x: e.clientX, w: width };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function move(e: PointerEvent) {
    if (start) width = clamp(start.w + e.clientX - start.x);
  }
  function key(e: KeyboardEvent) {
    if (e.key === "ArrowLeft") width = clamp(width - 16);
    else if (e.key === "ArrowRight") width = clamp(width + 16);
    else return;
    e.preventDefault();
  }
</script>

<!-- A focusable separator is a widget (WAI-ARIA window splitter). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="group absolute top-0 -right-[3px] bottom-0 z-10 w-[6px] cursor-col-resize touch-none"
  role="separator"
  aria-orientation="vertical"
  aria-label={label}
  aria-valuenow={width}
  aria-valuemin={min}
  aria-valuemax={max}
  tabindex="0"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={() => (start = null)}
  onkeydown={key}
>
  <div class="mx-auto h-full w-px bg-rule transition-colors group-hover:w-[2px] group-hover:bg-hi-edge group-focus-visible:bg-hi-edge"></div>
</div>
