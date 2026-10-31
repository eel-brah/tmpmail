<p align="center"><img src="src/assets/logo.svg" alt="" width="72" height="72"></p>

# tmpmail desktop app (in development)

A Tauri 2 window on top of tmpmail's core library, with the same inboxes as the CLI (same
`state.json` and config). Inboxes, live mail, the verification code one click (or `y`) away,
a safe HTML preview in light or dark, and the TUI's actions and keys. Not released yet.

## Features

- **Inboxes:** create one (provider or "first that works", a username where the provider
  allows it, a domain from the provider's list, a label), copy the address (click it, the
  Copy button or `c`), make it the active inbox (the CLI's default, marked with a star),
  check now, and delete it (asks first; deleted at the provider where it can be, then
  forgotten here). Each inbox shows its provider, expiry or lifetime, `Live` when a push
  stream is up, and "Public" for Maildrop.
- **Live mail:** the same worker loop as the TUI (`tmpmail::watcher`: polling, push
  streams, state reloads so CLI changes show up within ~5 s). New mail is marked New, and a
  notice names the sender and subject with an Open button.
- **Messages:** unread dot, sender, subject, time and attachment icon; filter by sender or
  subject (`/`); delete (asks first) where the provider supports it.
- **The open message:**
  - The verification code, when `extract` finds one, in a banner at the top with a Copy
    button.
  - Links, the likeliest verification link first, each with Open (system browser, http and
    https only) and Copy.
  - From (copy the sender's address), to and date.
  - Load images (remote images, over **https only**), Original colors in dark mode, and
    Source (the raw message where the provider has it, else the HTML).
  - Attachments with name, type and size, and Save (a native save dialog; never overwrites
    an existing file).
- **Look:** System / Light / Dark switch (remembered), resizable panes (remembered),
  keyboard focus rings, motion only on arrival of mail and dialogs, none with
  `prefers-reduced-motion`.

What each provider allows decides what's shown: Delete (message) is hidden for DropMail and
tempmail.lol; Attachments for tempmail.lol and Maildrop; the domain picker for tempmail.lol
and Maildrop; the username field for DropMail and tempmail.lol; Source shows the raw message
on mail.tm, mail.gw, DropMail and Maildrop and the HTML elsewhere. Deleting an inbox at a
provider that can't (DropMail, tempmail.lol, Maildrop) only forgets it here.

### Keyboard shortcuts

The same keys as `tmpmail tui`, off while you type in a field or a dialog is open. `?`
shows them in the app.

| Key | Action |
|---|---|
| `j` `↓` / `k` `↑` | Next / previous message (opens it) |
| `[` / `]` | Previous / next inbox |
| `n` | New inbox at the first provider that works |
| `N` | New inbox, choose the provider |
| `y` | Copy the verification code |
| `L` / `O` | Copy / open the first link |
| `c` | Copy the address |
| `a` | Make the inbox the active one |
| `r` | Check for mail now |
| `d` / `D` | Delete the message / the inbox (asks first) |
| `/` | Filter messages |
| `?` | Shortcut help |

## Dark mail

The theme reaches the mail frame only through its URL (`?theme=dark`), never through a
script, and Rust picks the colors when it builds the document:

- Plain-text mail, and HTML mail that sets no colors of its own: drawn in the dark theme's
  own background and text colors.
- HTML mail with its own colors (`bgcolor`, or `color` / `background` in a `style`,
  detected in the sanitized HTML by `render::has_own_colors`): drawn as mail clients do,
  with `filter: invert(1) hue-rotate(180deg)` on the document and the same filter again on
  `img`, `picture`, `video`, `svg` and anything with a background image, so photos and logos
  keep their colors. The page behind it is set so it inverts to the app's dark surface.
- "Original colors" (per message) shows it as the sender designed it, on white.

## Setup

- Rust 1.90 or newer (the root CLI crate still builds with 1.85).
- Node 24, and pnpm through corepack: `corepack enable --install-directory ~/.local/bin pnpm`
  (user-level), or prefix every command with `corepack`.
- Linux: WebKitGTK 4.1 and friends. Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`; Fedora:
  `webkit2gtk4.1-devel librsvg2-devel libappindicator-gtk3-devel`. On NVIDIA, a blank window
  is fixed by `WEBKIT_DISABLE_DMABUF_RENDERER=1`. On pure Wayland, copying needs a
  compositor with clipboard data-control support.

```sh
cd gui
pnpm install          # lockfile, exact versions, only releases older than 7 days
pnpm tauri dev        # Vite on :1420 plus the app, reloading on changes
pnpm check            # svelte-check
pnpm lint             # eslint, with svelte/no-at-html-tags as an error
pnpm tauri build --debug --no-bundle   # target/debug/tmpmail-gui with the built frontend
pnpm tauri icon src/assets/logo.svg    # regenerate src-tauri/icons (keep only the desktop ones)
```

`TMPMAIL_STATE_DIR` and `TMPMAIL_CONFIG` work as for the CLI (if either can't be read, the
window says why instead of starting), and `TMPMAIL_GUI_LOG=1` prints every request to the
mail scheme and every navigation.

The interface uses the system UI font for body text and headings (no font is bundled), and
the system monospace font for the verification code. Icons are inline SVG in
`src/lib/Icon.svelte`, drawn after Lucide's (ISC), so no icon package is needed.

## The preview probe

Debug builds include a probe of the mail preview's defenses:

```sh
TMPMAIL_GUI_PROBE=1 target/debug/tmpmail-gui          # Linux CI: xvfb-run -a target/debug/tmpmail-gui
```

It renders a hostile test mail (script, `onerror`, an attempt to reach `__TAURI_INTERNALS__`
and `invoke`, a remote image, a `cid:` image, `target="_blank"` and `javascript:` links,
inline styles) through the real preview, in light and in dark mode (where the test mail's
own colors make it inverted), and through control frames that each drop one layer (CSP
header only, sandbox only, scripts allowed, nothing). Anything that runs requests a URL on
the mail scheme, which Rust records. It also checks the sanitizer and the document builder
directly (remote images dropped with images off, `http:` images dropped and `https:` kept
with them on, a `style` keeping its colors but losing its `url(...)`, the dark document
inverted), since the frames can't tell the sanitizer's work from the CSP's. It prints a JSON
report and exits 0 when every check held (20 checks). `TMPMAIL_GUI_PROBE_WAIT=20` keeps the
window open 20 s more so you can click the test links: popups that reach the app are listed
under `popups_reaching_on_new_window` (and are not opened). The `gui` workflow runs the probe
on Linux, macOS and Windows.

## Security model

1. Mail HTML is sanitized only in Rust (ammonia, `render::sanitize_mail_html`) and never
   enters the app's DOM: lint bans `{@html}`. In the app's frame, a `style` attribute with a
   `url(...)` (or an image function, or a CSS escape) loses just those declarations while
   images are off; the CLI's `read --html` output is unchanged.
2. It is served by the `tmpmail-mail` URI scheme with its own CSP header (no scripts, images
   only inline or from the scheme, `https:` only after "Load images"; the sanitizer drops
   plain `http:` images). Inline `cid:` images are served only as raster image types, never
   SVG. The source view goes through the same scheme as escaped text, under a CSP with no
   images at all.
3. It is shown in `<iframe sandbox="allow-popups">`: no scripts, an opaque origin, no IPC.
   The theme and view options are in the frame's URL; no script ever touches the frame.
4. Links get `target="_blank"`; the app denies every new window and opens only http, https
   and mailto links in the system browser, from Rust. The link list's Open does the same
   for http and https.
5. Privileged work happens in Rust commands: the clipboard (`tauri-plugin-clipboard-manager`),
   the save dialog (`tauri-plugin-dialog`, then `std::fs::write` to a new file under a
   sanitized name) and the opener. The page has **no** plugin permission, only the app's
   own commands, each listed in `capabilities/main.json` and `build.rs`
   (`AppManifest::commands`).
6. Commands check their input: an address must be a stored inbox, a message key one the app
   opened, a link or attachment index one of that message's; usernames, domains, labels and
   ids are checked for shape. Copying takes what to copy (the code, the sender, link N), not
   arbitrary text.
7. The shell has a strict CSP, `freezePrototype`, no global Tauri object, no asset protocol,
   and one capability.
