# Records assets/tui.gif: the TUI receiving a mail live and copying its code.
# The mail is real (send-test-mail.py, sent 5 s after the TUI starts).
# Re-record from the repo root after `cargo build --release`:
#   DNS=1.1.1.1 vhs assets/demo/tui.tape

Output assets/tui.gif

Set Shell bash
Set Theme "Kanagawa"
Set FontSize 16
Set Width 1200
Set Height 600
Set Padding 16
Set WindowBar Colorful
Set WindowBarSize 36

Hide
Type "export PATH=$PWD/target/release:$PATH TMPMAIL_STATE_DIR=$(mktemp -d)"
Enter
Wait
Type "tmpmail -q new -p mailtm -l github >/dev/null; tmpmail -q new -p mailtm -l newsletter >/dev/null; addr=$(tmpmail new -q -p mailtm -l signup)"
Enter
Wait@60s
Type '(python3 assets/demo/send-test-mail.py "$addr" --delay 5 >/dev/null 2>&1 &); clear; tmpmail tui'
Enter
Sleep 1s
Show

Sleep 2s
Wait+Screen@40s /Confirm your email/
Sleep 1.5s
Type "l"
Sleep 2.5s
Type "y"
Sleep 2.5s
Type "?"
Sleep 3s
Type "?"
Sleep 500ms
Hide
Type "q"
Type "for l in github newsletter signup; do tmpmail -q drop $l; done"
Enter
Sleep 3s
