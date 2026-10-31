# tmpmail desktop app (in development)

A Tauri 2 window on top of tmpmail's core library: the same inboxes as the CLI (same
`state.json` and config), a message list, and an HTML preview. It is a spike, not a release:
it lists and creates inboxes, lists their mail and shows one message.

## Setup

- Rust 1.90 or newer (the root CLI crate still builds with 1.85).
- Node 24, and pnpm through corepack: `corepack enable --install-directory ~/.local/bin pnpm`
  (user-level), or prefix every command with `corepack`.
- Linux: WebKitGTK 4.1 and friends. Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`; Fedora:
  `webkit2gtk4.1-devel librsvg2-devel libappindicator-gtk3-devel`. On NVIDIA, a blank window
  is fixed by `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

```sh
cd gui
pnpm install          # lockfile, exact versions, only releases older than 7 days
pnpm tauri dev        # Vite on :1420 plus the app, reloading on changes
pnpm check            # svelte-check
pnpm lint             # eslint, with svelte/no-at-html-tags as an error
pnpm tauri build --debug --no-bundle   # target/debug/tmpmail-gui with the built frontend
```

`TMPMAIL_STATE_DIR` and `TMPMAIL_CONFIG` work as for the CLI (if either can't be read, the
window says why instead of starting), and `TMPMAIL_GUI_LOG=1` prints
every request to the mail scheme and every navigation.

## The preview probe

Debug builds include a probe of the mail preview's defenses:

```sh
TMPMAIL_GUI_PROBE=1 target/debug/tmpmail-gui          # Linux CI: xvfb-run -a target/debug/tmpmail-gui
```

It renders a hostile test mail (script, `onerror`, an attempt to reach `__TAURI_INTERNALS__`
and `invoke`, a remote image, a `cid:` image, `target="_blank"` and `javascript:` links,
inline styles) through the real preview, and through control frames that each drop one layer
(CSP header only, sandbox only, scripts allowed, nothing). Anything that runs requests a URL
on the mail scheme, which Rust records. It also runs the sanitizer on the test mail directly
(remote images dropped with images off, `http:` images dropped and `https:` kept with them
on), since the frames can't tell the sanitizer's work from the CSP's. It prints a JSON report
and exits 0 when every check held. `TMPMAIL_GUI_PROBE_WAIT=20` keeps the window open 20 s more so you can click the test
links: popups that reach the app are listed under `popups_reaching_on_new_window` (and are
not opened). The `gui` workflow runs the probe on Linux, macOS and Windows.

## Security model

1. Mail HTML is sanitized only in Rust (ammonia, `render::sanitize_mail_html`) and never
   enters the app's DOM: lint bans `{@html}`.
2. It is served by the `tmpmail-mail` URI scheme with its own CSP header (no scripts, images
   only inline or from the scheme, `https:` only after "Load images"; the sanitizer drops
   plain `http:` images). Inline `cid:` images are served only as raster image types, never
   SVG.
3. It is shown in `<iframe sandbox="allow-popups">`: no scripts, an opaque origin, no IPC.
4. Links get `target="_blank"`; the app denies every new window and opens only http, https
   and mailto links in the system browser, from Rust.
5. The shell has a strict CSP, `freezePrototype`, no global Tauri object, no asset protocol,
   and one capability that allows only the app's own commands.
