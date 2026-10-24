#!/usr/bin/env python3
"""Send one test mail with a verification code to a temp address.

Used by the demo recordings (assets/demo/*.tape) so they show real mail
arriving. Delivers straight to the address's MX on port 25, so it needs a
network that allows outbound SMTP, plus `dig`.

    send-test-mail.py ADDRESS [--code 482913] [--delay SECONDS]

Some resolvers block temp-mail hosts; set DNS=1.1.1.1 to look the MX up
through another server.
"""

import argparse
import os
import smtplib
import subprocess
import time
from email.message import EmailMessage


def dig(*args):
    server = os.environ.get("DNS")
    cmd = ["dig", "+short", *([f"@{server}"] if server else []), *args]
    return subprocess.run(cmd, capture_output=True, text=True).stdout.split("\n")


def mail_hosts(domain):
    rows = sorted(
        (int(line.split()[0]), line.split()[1].rstrip("."))
        for line in dig("MX", domain)
        if line.strip()
    )
    hosts = [h for _, h in rows] or [domain]
    # Connect by IP so the system resolver isn't asked again.
    return [ip for h in hosts for ip in [a for a in dig("A", h) if a][:1]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("address")
    ap.add_argument("--code", default="482913")
    ap.add_argument("--delay", type=float, default=0)
    a = ap.parse_args()
    time.sleep(a.delay)

    domain = a.address.split("@")[1]
    m = EmailMessage()
    m["From"] = f"Demo app <hello@{domain}>"
    m["To"] = a.address
    m["Subject"] = "Confirm your email"
    m.set_content(
        f"Welcome!\n\nYour verification code is {a.code}.\n"
        f"Or confirm here: https://example.com/verify?token=t{a.code}\n"
    )
    m.add_alternative(
        f"<p>Your verification code is <b>{a.code}</b>.</p>"
        f'<p><a href="https://example.com/verify?token=t{a.code}">Confirm email</a></p>',
        subtype="html",
    )
    last = None
    for host in mail_hosts(domain):
        try:
            with smtplib.SMTP(host, 25, timeout=20) as s:
                s.ehlo("demo.example.com")
                if s.has_extn("starttls"):
                    s.starttls()
                    s.ehlo("demo.example.com")
                s.send_message(m)
                return
        except (OSError, smtplib.SMTPException) as e:
            last = e
    raise SystemExit(f"could not deliver to {a.address}: {last}")


if __name__ == "__main__":
    main()
