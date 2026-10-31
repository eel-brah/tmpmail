<script lang="ts" module>
  // One icon set, drawn inline: 24 px grid, 2 px round strokes. Shapes follow
  // Lucide's (ISC license) so they read as one family; no icon package is
  // needed for this handful.
  type Shape =
    | { d: string }
    | { circle: [number, number, number] }
    | { rect: [number, number, number, number, number] };

  const ICONS = {
    plus: [{ d: "M5 12h14" }, { d: "M12 5v14" }],
    copy: [{ rect: [8, 8, 14, 14, 2] }, { d: "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" }],
    check: [{ d: "M20 6 9 17l-5-5" }],
    trash: [
      { d: "M3 6h18" },
      { d: "M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" },
      { d: "M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" },
      { d: "M10 11v6" },
      { d: "M14 11v6" },
    ],
    refresh: [
      { d: "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" },
      { d: "M21 3v5h-5" },
      { d: "M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" },
      { d: "M8 16H3v5" },
    ],
    star: [{ d: "M12 2.5l2.94 5.96 6.56.95-4.75 4.63 1.12 6.54L12 17.5l-5.87 3.08 1.12-6.54L2.5 9.41l6.56-.95z" }],
    search: [{ circle: [11, 11, 7] }, { d: "m21 21-4.3-4.3" }],
    x: [{ d: "M18 6 6 18" }, { d: "m6 6 12 12" }],
    external: [{ d: "M15 3h6v6" }, { d: "M10 14 21 3" }, { d: "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" }],
    download: [{ d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }, { d: "m7 10 5 5 5-5" }, { d: "M12 15V3" }],
    paperclip: [
      { d: "m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48" },
    ],
    sun: [
      { circle: [12, 12, 4] },
      { d: "M12 2v2" },
      { d: "M12 20v2" },
      { d: "m4.93 4.93 1.41 1.41" },
      { d: "m17.66 17.66 1.41 1.41" },
      { d: "M2 12h2" },
      { d: "M20 12h2" },
      { d: "m6.34 17.66-1.41 1.41" },
      { d: "m19.07 4.93-1.41 1.41" },
    ],
    moon: [{ d: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" }],
    monitor: [{ rect: [2, 3, 20, 14, 2] }, { d: "M8 21h8" }, { d: "M12 17v4" }],
    keyboard: [
      { rect: [2, 4, 20, 16, 2] },
      { d: "M6 8h.01" },
      { d: "M10 8h.01" },
      { d: "M14 8h.01" },
      { d: "M18 8h.01" },
      { d: "M8 12h.01" },
      { d: "M12 12h.01" },
      { d: "M16 12h.01" },
      { d: "M7 16h10" },
    ],
    image: [{ rect: [3, 3, 18, 18, 2] }, { circle: [9, 9, 2] }, { d: "m21 15-3.09-3.09a2 2 0 0 0-2.82 0L6 21" }],
    contrast: [{ circle: [12, 12, 10] }, { d: "M12 18a6 6 0 0 0 0-12v12z" }],
    code: [{ d: "m16 18 6-6-6-6" }, { d: "m8 6-6 6 6 6" }],
    alert: [
      { d: "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" },
      { d: "M12 9v4" },
      { d: "M12 17h.01" },
    ],
    mail: [{ rect: [2, 4, 20, 16, 2] }, { d: "m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" }],
    clock: [{ circle: [12, 12, 10] }, { d: "M12 6v6l4 2" }],
    link: [
      { d: "M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" },
      { d: "M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" },
    ],
    chevron: [{ d: "m6 9 6 6 6-6" }],
    globe: [{ circle: [12, 12, 10] }, { d: "M2 12h20" }, { d: "M12 2a15.3 15.3 0 0 1 0 20 15.3 15.3 0 0 1 0-20" }],
  } satisfies Record<string, Shape[]>;

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, size = 16, class: cls = "" }: { name: IconName; size?: number; class?: string } = $props();
  const shapes = $derived<Shape[]>(ICONS[name]);
</script>

<svg
  class={["shrink-0", cls]}
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#each shapes as s, i (i)}
    {#if "d" in s}
      <path d={s.d} />
    {:else if "circle" in s}
      <circle cx={s.circle[0]} cy={s.circle[1]} r={s.circle[2]} />
    {:else}
      <rect x={s.rect[0]} y={s.rect[1]} width={s.rect[2]} height={s.rect[3]} rx={s.rect[4]} />
    {/if}
  {/each}
</svg>
