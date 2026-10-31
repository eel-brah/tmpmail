The first preview of tmpmail's desktop app, for Linux, macOS and Windows. It is built on the
same core as the `tmpmail` command-line tool and shares its inboxes and `config.toml`.

**This is a preview.** It works on Linux; the macOS and Windows builds come from CI and have
only been checked by the automated security probe, not clicked through on a real machine.
Please report what breaks: <https://github.com/qorvyx/tmpmail/issues>.

## What it does

- Inboxes at mail.tm, mail.gw, DropMail, Guerrilla Mail, tempmail.lol and Maildrop: create
  one (the first provider that works, or one you pick, with a username and domain where the
  provider allows it, and a label), copy the address, make it the CLI's active inbox, delete
  it.
- Live mail on the same worker as `tmpmail tui` (polling, plus push on mail.tm and
  DropMail). New mail is marked New.
- The verification code in a banner with a Copy button, and the likeliest verification link
  first, each with Open and Copy.
- A safe HTML preview, in light or dark (mail with its own colors is inverted, with an
  "Original colors" switch). Remote images load only on request, over https only.
- Attachments: Save, or drag them out on Linux and macOS.
- Notifications for new mail while the window is in the background, with the code when
  there is one. A click opens the message.
- An opt-in tray icon (New address, Copy code, Quit) that keeps checking with the window
  closed.
- A settings screen for the provider order, poll interval, push, request timeout and the
  desktop options.
- The TUI's keyboard shortcuts (`?` lists them).

## Download

| System | File |
|---|---|
| Windows 10/11 (x64) | `tmpmail-desktop-0.1.0-x64-setup.exe` |
| macOS 10.15+ (Apple silicon and Intel) | `tmpmail-desktop-0.1.0-universal.dmg` |
| Debian, Ubuntu (x86_64) | `tmpmail-desktop-0.1.0-amd64.deb` |
| Fedora and other RPM-based (x86_64) | `tmpmail-desktop-0.1.0-x86_64.rpm` |
| Other Linux (x86_64) | `tmpmail-desktop-0.1.0-x86_64.AppImage` |

Check a download against `SHA256SUMS`, or its build attestation:

```sh
gh attestation verify tmpmail-desktop-0.1.0-x86_64.AppImage --repo qorvyx/tmpmail
```

## Installing an unsigned app

The app isn't signed with a paid certificate, so each system warns once.

- **macOS:** open the DMG and drag tmpmail to Applications. Open it once: macOS refuses.
  Then go to System Settings → Privacy & Security, click **Open Anyway** next to the
  message about tmpmail, and enter your password. On macOS 15 (Sequoia) and later,
  Control-click → Open no longer skips this. If you trust the download, `xattr -dr
  com.apple.quarantine /Applications/tmpmail.app` does the same from a terminal.
- **Windows:** SmartScreen says "Windows protected your PC": click **More info**, then
  **Run anyway**. The installer installs for your user only, with no admin prompt, and
  fetches WebView2 if it's missing. If **Smart App Control** is on, it blocks the installer
  outright with no way past; it can only be turned off as a whole (Windows Security → App &
  browser control).
- **Linux:** `sudo apt install ./tmpmail-desktop-0.1.0-amd64.deb`, or `sudo dnf install
  ./tmpmail-desktop-0.1.0-x86_64.rpm`; both pull in WebKitGTK 4.1. The AppImage runs
  as is after `chmod +x` (it may need `libfuse2`; without FUSE, run it with
  `--appimage-extract-and-run`). On NVIDIA, if the window stays blank, start it with
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

**Use tmpmail CLI 0.3.0 or later alongside the app.** The settings screen can write the
`http_timeout` key to `config.toml`, which tmpmail 0.2.0 refuses (0.3.0 and later warn about
keys they don't know).

## Known limits

- No auto-update yet: watch the releases page.
- Windows: no dragging attachments out (a drag can hang the app there); Save works.
- Linux: the tray needs a StatusNotifier host (stock GNOME needs the AppIndicator
  extension), and clicks on the tray icon itself aren't reported, so everything is in its
  menu. Copying on pure Wayland needs a compositor with clipboard data-control support.
- macOS: the notification permission prompt and the tray have not been tried on a real
  Mac. An ad-hoc signed app may be asked for permissions again after each update.
- Windows: notification clicks work only while the toast is on screen.
- x86_64 only on Linux and Windows for now.
