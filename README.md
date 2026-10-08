# tmpmail

Temporary email from the terminal. A Rust rewrite of the idea behind
[sdushantha/tmpmail](https://github.com/sdushantha/tmpmail), whose only backend
(1secmail) shut down. This version:

- **talks to several providers** and falls back to the next one when one is down,
- **fails loudly**: a dead API gives `mailtm: unreachable (HTTP 502)`, not `integer expected`,
- **is built for automation**: `wait`, `otp`, `links`, `--json`, stable exit codes,
- **keeps the old interface**: `tmpmail -g`, `tmpmail`, `tmpmail -r`, `tmpmail ID` still work.

## Install

```sh
cargo install --path .        # binary lands in ~/.cargo/bin/tmpmail
```

Needs nothing else at runtime. Optional: `wl-copy`/`xclip`/`xsel`/`pbcopy` for `copy`,
a browser for `read --html`, `notify-send` for `watch --notify`.

## Quick start

```sh
tmpmail new                 # create an address (random, readable) and make it active
tmpmail                     # list messages (same as `tmpmail ls`)
tmpmail read 1              # read message #1 from the list (or by id, or --recent)
tmpmail read 1 --html       # open the sanitized HTML in your browser
tmpmail copy                # copy the address to the clipboard
tmpmail drop                # delete the inbox at the provider and forget it
```

## Automation / CI

```sh
addr=$(tmpmail new -q)
./signup.sh "$addr"
code=$(tmpmail wait --otp --from 'github' --timeout 120)   # blocks until a code arrives
link=$(tmpmail wait --link --subject 'confirm')            # or a verification link
tmpmail --json ls | jq '.messages[0].subject'
```

| Exit code | Meaning |
|---|---|
| 0 | success |
| 1 | other error |
| 2 | `wait` timed out |
| 3 | provider failure (unreachable, rate limited, auth, bad response) |
| 4 | nothing found (no inbox, no such message, no code in the message) |
| 64 | usage error (bad flag, unsupported option for this provider) |

`--json` output is documented in [docs/json.md](docs/json.md). Errors are printed as
`{"error":{"kind":"...","message":"..."}}` in JSON mode.

## Commands

| Command | What it does |
|---|---|
| `new [ADDR] [-u USER] [-d DOMAIN] [-p PROVIDER] [-l LABEL] [-c]` | Create an inbox (alias `generate`) |
| `ls [-a]` | List messages, newest first, `*` = unread; `-a` = all your inboxes |
| `read [ID\|N] [--recent] [--html [--images]] [--raw] [--no-pager]` | Show a message (long ones in a pager) |
| `wait [-a] [--from RE] [--subject RE] [--body RE] [--timeout S] [--otp\|--link] [--delete]` | Block until a matching message arrives |
| `otp [ID] [--regex RE] [--wait ...]` | Extract a verification code |
| `links [ID] [--match RE] [--first] [--open]` | Extract links, verification links first |
| `watch [-a] [--notify] [--rotate 30m]` | Stream new messages (JSON lines with `--json`); `-a` = all inboxes; `--rotate` = fresh address on a timer |
| `attachments ID`, `save ID [ATT...] [-o DIR]` | List / download attachments |
| `delete ID`, `delete --all` | Delete messages |
| `whoami`, `copy`, `qr` | Show / copy / QR-code the active address |
| `inboxes [--prune]`, `use ADDR`, `-i ADDR` | Manage several inboxes (by address, local part or label) |
| `login ADDR --password PW` | Attach an existing mail.tm / mail.gw account |
| `rotate [--keep]` | Replace the active address with a fresh one |
| `domains [-p PROVIDER]`, `providers [--check]` | Domains, provider capabilities and live status |
| `completions bash\|zsh\|fish`, `man [-o DIR]` | Shell completions, man pages |

Messages can be referenced by full id, by their number in `tmpmail ls`, or by a unique id prefix.

**Every option and short flag (`-l`, `-g`, `-u`, `-t`...) is explained in [docs/usage.md](docs/usage.md).**

## Providers

| Id | Service | Inbox | Lifetime | Notes |
|---|---|---|---|---|
| `mailtm` | [mail.tm](https://mail.tm) | private | until deleted, mail kept 7 days | default; custom username, attachments, delete, login, push |
| `dropmail` | [DropMail.me](https://dropmail.me) | private | renewed while used, restorable | many domains, attachments, push |
| `guerrilla` | [Guerrilla Mail](https://www.guerrillamail.com) | private | 60 min | custom username, attachments |
| `tempmail_lol` | [tempmail.lol](https://tempmail.lol) | private | 1 hour | simplest, last resort |
| `mailgw` | [mail.gw](https://mail.gw) | private | like mail.tm | same software as mail.tm |
| `maildrop` | [Maildrop](https://maildrop.cc) | **public** | 24 h idle | opt-in only (`-p maildrop`): anyone can read it; first mail may be delayed by greylisting |

`tmpmail new` tries them in order `mailtm, dropmail, guerrilla, tempmail_lol, mailgw`,
skipping providers that can't satisfy the options you passed (e.g. `--user`). With `-d`,
it goes straight to a provider that has the domain.

`wait` and `watch` hear about new mail at once from providers with push (mail.tm, DropMail)
and poll the others every 5 seconds.
Only documented APIs are used. Many sites block known disposable domains; trying another
provider or domain usually helps.

Thanks to **mail.tm** for its free API; per its terms this client links back to it.

## Configuration

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

## Privacy and security

- State (addresses and their credentials) lives in `$XDG_STATE_HOME/tmpmail/state.json`
  (`~/.local/state/tmpmail`), directory `0700`, file `0600`, never in `/tmp`.
- Message text is stripped of terminal control sequences before printing.
- `read --html` writes a sanitized copy (no scripts, forms, frames; strict CSP) and
  **blocks remote images** (tracking pixels) unless you pass `--images`. These copies are
  deleted after a day.
- Attachment names are sanitized so they can't escape the target directory, and
  existing files are never overwritten.
- No telemetry. The only network traffic is to the provider you use.

## Development

```sh
cargo test                  # unit tests + provider tests against a mock HTTP server
cargo clippy --all-targets
./scripts/smoke.sh          # live check against the real providers (creates throwaway inboxes)
```


## License

MIT
