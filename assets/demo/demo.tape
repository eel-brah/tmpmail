# Records assets/demo.gif: new address -> wait for the code -> links.
# The mail is real: `signup` sends it to the new address with
# send-test-mail.py. Re-record from the repo root after `cargo build --release`:
#   DNS=1.1.1.1 vhs assets/demo/demo.tape

Output assets/demo.gif

Set Shell bash
Set Theme "Kanagawa"
Set FontSize 20
Set Width 1200
Set Height 450
Set Padding 28
Set TypingSpeed 45ms
Set WindowBar Colorful
Set WindowBarSize 36

Hide
Type "export PATH=$PWD/target/release:$PATH TMPMAIL_STATE_DIR=$(mktemp -d) PS1='\[\e[2m\]$\[\e[0m\] '"
Enter
Type 'signup() { (python3 assets/demo/send-test-mail.py "$1" --delay 2 >/dev/null 2>&1 &); }'
Enter
Type "clear"
Enter
Show

Type "addr=$(tmpmail new -q); echo $addr"
Sleep 400ms
Enter
Sleep 1.5s
Type "signup $addr        # any site that mails a code"
Sleep 400ms
Enter
Sleep 500ms
Type "tmpmail otp --wait --include-existing"
Sleep 400ms
Enter
Wait+Screen@40s /482913/
Sleep 1.5s
Type "tmpmail links --first"
Sleep 400ms
Enter
Sleep 2s
Type "tmpmail ls"
Sleep 400ms
Enter
Sleep 4s
Hide
Type "tmpmail -q drop"
Enter
