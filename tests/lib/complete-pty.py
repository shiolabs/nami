#!/usr/bin/env python3
"""Types into a shell in a pseudo-terminal, as a person does, and prints what
the shell shows after Tab: tab completion as it really works, for
tests/completions-inner.sh.

usage: complete-pty.py <bash|zsh|fish> <line> [<line>...]
Each line is typed after the prompt, then Tab twice (thrice in bash: they list on
the last); what the screen shows from there, its escapes taken out, is
printed under `=== <line>`. The line is then cleared for the next one.
"""
import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time

shell, lines = sys.argv[1], sys.argv[2:]
ESCAPES = re.compile(rb"\x1b\[[0-9;?<=>]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[=>78DEHMc]|\x1bP[^\x1b]*\x1b\\")


def read(fd, wait):
    """What the terminal shows within `wait` seconds of quiet."""
    out = b""
    end = time.time() + wait
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            out += data
            end = max(end, time.time() + 0.4)
    return out


def plain(data):
    text = ESCAPES.sub(b"", data).replace(b"\r", b"")
    return text.decode("utf-8", "replace")


pid, fd = pty.fork()
if pid == 0:
    env = dict(os.environ, TERM="xterm-256color", COLUMNS="140", LINES="50")
    if shell == "fish":
        os.execvpe("fish", ["fish", "-i"], env)
    os.execvpe(shell, [shell, "-i"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 140, 0, 0))
read(fd, 3.0)  # its start: rc files, the first prompt
for line in lines:
    os.write(fd, line.encode())
    time.sleep(0.3)
    # Tab: what's common to all that fit goes in; Tab again lists them (bash,
    # zsh; fish lists them the first time).
    os.write(fd, b"\t")
    shown = read(fd, 1.5)
    os.write(fd, b"\t")
    shown += read(fd, 1.5)
    if shell == "bash":
        # (bash lists on a second Tab in a row that adds nothing.)
        os.write(fd, b"\t")
        shown += read(fd, 1.5)
    print(f"=== {line}")
    print(plain(shown))
    # Clear the line: Ctrl-C, a new prompt.
    os.write(fd, b"\x03")
    read(fd, 1.0)
os.write(fd, b"exit\r")
read(fd, 1.0)
