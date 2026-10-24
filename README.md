# tmpmail

Disposable email for scripts, tests and AI agents. Make an address, sign up with it, and
get the verification code back in one line of shell. When one provider is down, tmpmail
moves on to the next.

[![CI](https://github.com/eel-brah/tmpmail/actions/workflows/ci.yml/badge.svg)](https://github.com/eel-brah/tmpmail/actions/workflows/ci.yml)
[![Providers](https://github.com/eel-brah/tmpmail/actions/workflows/providers.yml/badge.svg)](https://github.com/eel-brah/tmpmail/actions/workflows/providers.yml)
[![Release](https://img.shields.io/github/v/release/eel-brah/tmpmail)](https://github.com/eel-brah/tmpmail/releases/latest)
[![crates.io](https://img.shields.io/crates/v/tmpmail)](https://crates.io/crates/tmpmail)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

```sh
addr=$(tmpmail new -q)                   # quickfox605@maxxspace.com
./signup.sh "$addr"                      # anything that sends a mail to it
tmpmail otp --wait --include-existing    # blocks until it arrives, then prints 482913
```

![tmpmail creating an address, waiting for a mail and printing its code and link](assets/demo.gif)

<sub>A real mail, sent to the new address by
[`assets/demo/send-test-mail.py`](assets/demo/send-test-mail.py) and delivered over
mail.tm's push stream. Recorded with [vhs](https://github.com/charmbracelet/vhs) from
[`assets/demo/demo.tape`](assets/demo/demo.tape).</sub>

## Install

```sh
cargo install tmpmail
```

Or take a prebuilt binary from the [releases page](https://github.com/eel-brah/tmpmail/releases/latest):
Linux (x86_64, aarch64, fully static), macOS (Intel, Apple silicon) and Windows. Each
release has `SHA256SUMS`, and every file carries a GitHub build attestation that proves it
was built from this repository by the release workflow:

```sh
gh attestation verify tmpmail-0.1.0-x86_64-unknown-linux-musl.tar.gz --repo eel-brah/tmpmail
```

Nothing else is needed at runtime. Optional extras: `wl-copy`, `xclip`, `xsel` or `pbcopy`
for `copy`, a browser for `read --html`, `notify-send` for `watch --notify`.

## Use it

### In a script or CI

`wait` blocks until a matching mail arrives. `otp` prints just the code and `links` just
the links, verification links first. Every command takes `--json`, and exit codes tell a
timeout (2) apart from a dead provider (3).

```sh
addr=$(tmpmail new -q)
./signup.sh "$addr"
code=$(tmpmail otp --wait --include-existing --from 'acme' --timeout 120)
link=$(tmpmail wait --link --include-existing --subject 'confirm')
tmpmail --json ls | jq -r '.messages[0].subject'
```

`--include-existing` matters when the mail can arrive before `wait` starts. Without it,
`wait` only accepts mail that is new since the call. In GitHub Actions:

```yaml
- name: Install tmpmail
  run: |
    mkdir -p ~/.local/bin
    curl -sSfL https://github.com/eel-brah/tmpmail/releases/download/v0.1.0/tmpmail-0.1.0-x86_64-unknown-linux-musl.tar.gz \
      | tar xz --strip-components=1 -C ~/.local/bin tmpmail-0.1.0-x86_64-unknown-linux-musl/tmpmail
    echo "$HOME/.local/bin" >> "$GITHUB_PATH"

- name: Sign-up test
  run: |
    addr=$(tmpmail new -q)
    curl -sSf https://staging.example.com/signup -d "email=$addr"
    code=$(tmpmail otp --wait --include-existing --timeout 120)
    curl -sSf https://staging.example.com/verify -d "email=$addr" -d "code=$code"
```

The [daily provider check](.github/workflows/providers.yml) creates and lists inboxes from
GitHub's runners, so the free providers are known to work from CI.

### With an AI agent

`tmpmail mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server. An agent
can create an inbox, sign up somewhere and fetch the code without you:

```sh
claude mcp add tmpmail -- tmpmail mcp
```

Other clients take `{ "command": "tmpmail", "args": ["mcp"] }`. The tools are
`create_inbox`, `list_inboxes`, `list_messages`, `read_message`, `wait_for_message`,
`get_verification_code`, `get_links`, `delete_message` and `delete_inbox`. Waits run in
parallel and can be cancelled. Anyone can mail a temp address, so treat what the agent reads
there as untrusted input. [Details](docs/usage.md#mcp-server-for-ai-agents).

### In the terminal

`tmpmail tui` shows your inboxes, their mail and a preview, updated as mail arrives. A
verification code is highlighted, and `y` copies it.

![The tmpmail TUI receiving a mail live and copying its code](assets/tui.gif)

For everything else: `new`, `ls`, `read` (in a pager, or `--html` in your browser), `copy`,
`qr`, `watch --notify`, `rotate`, `drop`. [All commands](#commands) are listed below.

### Coming from the original tmpmail

[sdushantha/tmpmail](https://github.com/sdushantha/tmpmail) was a shell script on a single
backend, 1secmail, which has shut down. This is a rewrite in Rust, and the old interface
still works: `tmpmail -g [ADDR]`, `tmpmail`, `tmpmail -r`, `tmpmail ID`, `-d`, `-c`, `-t`.
It no longer needs `w3m`, `curl` or `jq`.
[Mapping of the old flags](docs/usage.md#original-tmpmail-flags).

## Providers

| Id | Service | Inbox | Lifetime | Notes |
|---|---|---|---|---|
| `mailtm` | [mail.tm](https://mail.tm) | private | until deleted, mail kept 7 days | default; custom username, attachments, delete, login, push |
| `dropmail` | [DropMail.me](https://dropmail.me) | private | renewed while used, restorable | many domains, attachments, push |
| `guerrilla` | [Guerrilla Mail](https://www.guerrillamail.com) | private | 60 min | custom username, attachments |
| `tempmail_lol` | [tempmail.lol](https://tempmail.lol) | private | 1 hour | last resort; no attachments, and a mail with a text attachment arrives with the attachment as its body |
| `mailgw` | [mail.gw](https://mail.gw) | private | like mail.tm | same software as mail.tm |
| `maildrop` | [Maildrop](https://maildrop.cc) | **public** | 24 h idle | opt-in only (`-p maildrop`): anyone can read it; first mail may be delayed by greylisting |

`tmpmail new` tries `mailtm, dropmail, guerrilla, tempmail_lol, mailgw` in that order. It
skips providers that can't do what you asked for (say, `--user`), and with `-d` it goes
straight to one that has the domain. `tmpmail providers --check` shows which are up right
now.

`wait`, `watch` and the TUI hear about new mail at once from providers with push (mail.tm,
DropMail) and poll the others every 5 seconds. Only documented APIs are used. Many sites
block known disposable domains; another provider or domain usually gets through.

Thanks to **mail.tm** for its free API; per its terms this client links back to it.

## Reference

<a id="commands"></a>
<details>
<summary><b>Commands</b></summary>

| Command | What it does |
|---|---|
| `new [ADDR] [-u USER] [-d DOMAIN] [-p PROVIDER] [-l LABEL] [-c]` | Create an inbox (alias `generate`) |
| `ls [-a]` | List messages, newest first, `*` = unread; `-a` = all your inboxes |
| `read [ID\|N] [--recent] [--html [--images]] [--raw] [--no-pager]` | Show a message (long ones in a pager) |
| `wait [-a] [--from RE] [--subject RE] [--body RE] [--timeout S] [--otp\|--link] [--delete]` | Block until a matching message arrives |
| `otp [ID] [--regex RE] [--wait ...]` | Extract a verification code |
| `links [ID] [--match RE] [--first] [--open]` | Extract links, verification links first |
| `watch [-a] [--notify] [--rotate 30m]` | Stream new messages (JSON lines with `--json`); `--rotate` = fresh address on a timer |
| `attachments ID`, `save ID [ATT...] [-o DIR]` | List / download attachments |
| `delete ID`, `delete --all` | Delete messages |
| `whoami`, `copy`, `qr` | Show / copy / QR-code the active address |
| `inboxes [--prune]`, `use ADDR`, `-i ADDR` | Manage several inboxes (by address, local part or label) |
| `login ADDR --password PW` | Attach an existing mail.tm / mail.gw account |
| `rotate [--keep]` | Replace the active address with a fresh one |
| `domains [-p PROVIDER]`, `providers [--check]` | Domains, provider capabilities and live status |
| `tui` | Full-screen browser, updated live |
| `mcp` | MCP server on stdio for AI agents |
| `completions bash\|zsh\|fish`, `man [-o DIR]` | Shell completions, man pages |

Messages can be referenced by full id, by their number in `tmpmail ls`, or by a unique id
prefix. Every option is explained in [docs/usage.md](docs/usage.md), and the `--json`
output in [docs/json.md](docs/json.md).

</details>

<details>
<summary><b>Exit codes</b></summary>

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | other error |
| 2 | `wait` timed out |
| 3 | provider failure (unreachable, rate limited, auth, bad response) |
| 4 | nothing found (no inbox, no such message, no code in the message) |
| 64 | usage error (bad flag, unsupported option for this provider) |

In `--json` mode, errors are printed as `{"error":{"kind":"...","message":"..."}}`.

</details>

<details>
<summary><b>Configuration and environment</b></summary>

`~/.config/tmpmail/config.toml` (or `$TMPMAIL_CONFIG`), all optional:

```toml
providers = ["mailtm", "guerrilla"]   # fallback order for `new`
browser = "firefox"                   # for --html and links --open
clipboard_cmd = "wl-copy"
pager = "less"                        # for long messages in `read` (default $PAGER, else less)
poll_interval = 5                     # seconds, for wait/watch
timeout = 180                         # default wait timeout
push = true                           # live push from mail.tm/DropMail in wait/watch
```

Environment: `TMPMAIL_PROVIDER` (comma list), `TMPMAIL_TIMEOUT`, `TMPMAIL_POLL_INTERVAL`,
`TMPMAIL_PUSH`, `TMPMAIL_INBOX`, `TMPMAIL_PASSWORD`, `TMPMAIL_STATE_DIR`,
`TMPMAIL_DROPMAIL_TOKEN`, `PAGER`, `NO_COLOR`.

</details>

## Privacy and security

- State (addresses and their credentials) lives in `$XDG_STATE_HOME/tmpmail/state.json`
  (`~/.local/state/tmpmail`), directory `0700`, file `0600`, never in `/tmp`.
- Message text is stripped of terminal control sequences before printing.
- `read --html` writes a sanitized copy (no scripts, forms, frames; strict CSP) and
  **blocks remote images** (tracking pixels) unless you pass `--images`. These copies are
  deleted after a day.
- Attachment names are sanitized so they can't escape the target directory, and
  existing files are never overwritten.
- `tmpmail mcp` hands message contents to an AI agent. Anyone can mail a temp address, so
  treat what the agent reads there as untrusted input.
- No telemetry. The only network traffic is to the provider you use.

## Development

```sh
cargo test                  # unit tests + provider tests against a mock HTTP server
cargo clippy --all-targets
./scripts/smoke.sh          # live check against the real providers (creates throwaway inboxes)
vhs assets/demo/demo.tape   # re-record the demo (needs vhs, and outbound SMTP for the test mail)
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
