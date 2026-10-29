#!/usr/bin/env sh
# Live smoke test against the real providers. Creates throwaway inboxes in an
# isolated state dir, then deletes them. Sends no mail.
# Exits non-zero only when every provider fails: tmpmail falls back to the next
# provider, so one being down still leaves it working.
set -eu

cargo build --release -q
bin="$(pwd)/target/release/tmpmail"
state="$(mktemp -d)"
trap 'rm -rf "$state"' EXIT
dir="$state"
# Git Bash on Windows: the binary has .exe, and tmpmail wants a Windows path.
if [ -f "$bin.exe" ]; then
    bin="$bin.exe"
    dir="$(cygpath -w "$state")"
fi
export TMPMAIL_STATE_DIR="$dir" TMPMAIL_CONFIG="$dir/no-config.toml"

"$bin" providers --check
ok=0
for p in mailtm dropmail guerrilla tempmail_lol; do
    if addr=$("$bin" new -q -p "$p" 2>&1); then
        if "$bin" -q -i "$addr" ls >/dev/null; then
            echo "ok    $p  $addr"; ok=$((ok + 1))
        else
            echo "FAIL  $p  list"
        fi
        "$bin" -q drop "$addr" 2>/dev/null || true
    else
        echo "FAIL  $p  $addr"
    fi
done
echo "$ok provider(s) working"
[ "$ok" -gt 0 ]
