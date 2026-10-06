# JSON output

Every command accepts `--json`. Output goes to stdout. Timestamps are RFC 3339 in UTC,
and absent values are `null`. Fields may be added later but won't be renamed or removed
within a major version.

## Errors (any command)

```json
{"error": {"kind": "unreachable", "message": "mailtm: unreachable (HTTP 502)"}}
```

`kind` is one of `unreachable`, `rate_limited`, `auth`, `bad_response`, `not_found`,
`unsupported`, `address_taken`, `expired`, `no_inbox`, `timeout`, `usage`, `error`.
The exit code matches (see the README).

## Inbox: `new`, `whoami`, `use`, `login`, `rotate`; `inboxes` returns a list of these

```json
{
  "address": "quietotter482@maxxspace.com",
  "provider": "mailtm",
  "label": "github",
  "created_at": "2026-10-05T10:00:00Z",
  "expires_at": null,
  "expired": false,
  "active": true,
  "private": true
}
```

Credentials are never printed.

## Message summary: `ls` and `watch` lines

`ls` returns `{"address": "...", "messages": [Summary, ...]}`, newest first.

`ls --all-inboxes` returns `{"inboxes": [{"address", "messages": [...], "error"?}, ...]}`.
`error` (`{"kind", "message"}`) is present only for an inbox that couldn't be read.

`watch --json` prints one Summary per line as messages arrive, plus an `"inbox"` field
with the receiving address.

```json
{
  "id": "6aa6c0...",
  "from": "GitHub <noreply@github.com>",
  "subject": "Your verification code",
  "intro": "Use 482913 to ...",
  "date": "2026-10-05T10:01:12Z",
  "seen": false,
  "has_attachments": false
}
```

## Message: `read`, and `wait` without `--otp` or `--link`

`wait` adds an `"inbox"` field with the receiving address (useful with `--all-inboxes`).

```json
{
  "id": "6aa6c0...",
  "from": "GitHub <noreply@github.com>",
  "to": ["quietotter482@maxxspace.com"],
  "subject": "Your verification code",
  "date": "2026-10-05T10:01:12Z",
  "text": "Use 482913 to sign in.",
  "html": "<p>Use <b>482913</b> ...</p>",
  "attachments": [
    {"id": "ATTACH000001", "filename": "invoice.pdf", "content_type": "application/pdf", "size": 48213}
  ]
}
```

## Other commands

| Command | Output |
|---|---|
| `otp` | `{"code", "candidates": [{"code", "score"}], "inbox", "message_id", "from", "subject"}` |
| `wait --otp` | `{"code", "candidates": [{"code", "score"}], "inbox", "message_id", "from", "subject"}` |
| `wait --link` | `{"link", "inbox", "message_id"}` |
| `links` | `{"links": [...], "message_id"}` |
| `attachments` | `{"message_id", "attachments": [...]}` |
| `save` | `{"saved": ["path", ...]}` |
| `delete` | `{"deleted": N}` |
| `drop` | `{"dropped": "address", "active": "address or null"}` |
| `copy` | `{"copied": "address"}` |
| `domains` | `{"provider", "domains": [...]}` |
| `providers` | `[{"id", "name", "website", "in_fallback_order", "capabilities": {...}, "status": {"ok", "ms"} \| {"ok": false, "error"} \| null}]` |
