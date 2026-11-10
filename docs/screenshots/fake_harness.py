#!/usr/bin/env python3
import os
import select
import signal
import sys
import termios
import time
import tty

if len(sys.argv) > 2 and sys.argv[1] == "agents" and sys.argv[2] == "--json":
    print("[]")
    sys.exit(0)

NAME = "demo-agent"
typed = ""
log = []
resized = True


def on_winch(*_):
    global resized
    resized = True


def draw():
    columns, rows = os.get_terminal_size()
    width = max(20, columns - 2)
    out = ["\x1b[2J\x1b[H"]
    out.append(f"\x1b[1m {NAME}\x1b[0m \x1b[2mdemo harness, not a real agent\x1b[0m\r\n\r\n")
    for kind, line in log[-(rows - 8):]:
        if kind == "user":
            out.append(f"\x1b[36m>\x1b[0m {line}\r\n\r\n")
        else:
            out.append(f"\x1b[32m●\x1b[0m {line}\r\n\r\n")
    prompt_row = rows - 3
    out.append(f"\x1b[{prompt_row};1H\x1b[2m╭{'─' * (width - 2)}╮\x1b[0m")
    out.append(f"\x1b[{prompt_row + 1};1H\x1b[2m│\x1b[0m \x1b[36m>\x1b[0m {typed}\x1b[{prompt_row + 1};{width}H\x1b[2m│\x1b[0m")
    out.append(f"\x1b[{prompt_row + 2};1H\x1b[2m╰{'─' * (width - 2)}╯\x1b[0m")
    out.append(f"\x1b[{prompt_row + 3};2H\x1b[2m? for shortcuts\x1b[0m")
    out.append(f"\x1b[{prompt_row + 1};{5 + len(typed)}H")
    sys.stdout.write("".join(out))
    sys.stdout.flush()


def respond(prompt):
    log.append(("user", prompt))
    log.append(("agent", "Looking at the retry backoff. It doubles the delay each attempt and caps it at 8 seconds."))
    log.append(("agent", "I added a test that checks the delay sequence 0.5, 1, 2, 4, 8, 8."))


signal.signal(signal.SIGWINCH, on_winch)
if os.environ.get("DEMO_HARNESS_SEED"):
    respond("Add a test for the retry backoff")
fd = sys.stdin.fileno()
saved = termios.tcgetattr(fd)
tty.setraw(fd)
try:
    while True:
        if resized:
            resized = False
            draw()
        ready, _, _ = select.select([fd], [], [], 0.2)
        if not ready:
            continue
        data = os.read(fd, 64).decode(errors="ignore")
        for char in data:
            if char in "\r\n":
                if typed:
                    respond(typed)
                    typed = ""
            elif char in "\x7f\b":
                typed = typed[:-1]
            elif char == "\x04":
                sys.exit(0)
            elif char.isprintable():
                typed += char
        draw()
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, saved)
