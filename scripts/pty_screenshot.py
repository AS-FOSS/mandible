#!/usr/bin/env python3
"""Render a TUI program's real screen output in a headless environment.

CI and agent sandboxes have no controlling terminal, so running mandible directly
fails with "enable raw mode: No such device or address". This forks a real
pseudo-terminal, gives it an explicit window size, drives it with keystrokes,
and replays the output through a terminal emulator to produce the actual screen
as text.

The explicit `TIOCSWINSZ` call is the part naive attempts miss: without it the
pty inherits a 0x0 size and ratatui renders an empty frame, which looks exactly
like a broken program.

This catches *content* regressions that `TestBackend` unit tests cannot, because
those use synthetic fixtures rather than the real 739-tool catalog. Both the
markdown-leak and ragged-rewrap defects were found this way.

Requires `pyte` (terminal emulator):

    python3 -m venv .venv && .venv/bin/pip install pyte

Usage:

    pty_screenshot.py <cols> <rows> <program> [args...]
    pty_screenshot.py --keys 'j,j,<right>,/push' 100 28 ./target/release/mandible git

Keys are comma-separated. Literal text is sent as-is; `<right>`, `<left>`,
`<up>`, `<down>`, `<enter>`, `<esc>`, `<tab>`, `<bs>` send the escape sequence.
A screen is captured after startup and after each key group.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import select
import struct
import sys
import termios
import time

KEY_SEQUENCES = {
    "<up>": b"\x1b[A",
    "<down>": b"\x1b[B",
    "<right>": b"\x1b[C",
    "<left>": b"\x1b[D",
    "<enter>": b"\r",
    "<esc>": b"\x1b",
    "<tab>": b"\t",
    "<bs>": b"\x7f",
    "<space>": b" ",
}


def encode_keys(spec: str) -> bytes:
    """Translate one key-group spec into the bytes to write to the pty."""
    out = b""
    i = 0
    while i < len(spec):
        if spec[i] == "<":
            end = spec.find(">", i)
            token = spec[i : end + 1].lower() if end != -1 else ""
            if token in KEY_SEQUENCES:
                out += KEY_SEQUENCES[token]
                i = end + 1
                continue
        out += spec[i].encode()
        i += 1
    return out


def capture(argv, cols, rows, key_groups, settle=1.5, step=0.6):
    """Run `argv` in a pty, returning [(label, [screen lines]), ...]."""
    try:
        import pyte
    except ImportError:
        sys.exit(
            "pyte is required: python3 -m venv .venv && "
            ".venv/bin/pip install pyte"
        )

    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLUMNS"] = str(cols)
        os.environ["LINES"] = str(rows)
        os.execvp(argv[0], argv)

    # Without this the pty is 0x0 and the program renders nothing.
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    alive = True

    def pump(duration):
        nonlocal alive
        end = time.time() + duration
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                alive = False
                return
            if not chunk:
                alive = False
                return
            stream.feed(chunk)

    pump(settle)
    shots = [("startup", list(screen.display))]

    for group in key_groups:
        if not alive:
            break
        try:
            os.write(fd, encode_keys(group))
        except OSError:
            break
        pump(step)
        shots.append((group, list(screen.display)))

    try:
        os.write(fd, b"q")
    except OSError:
        pass
    pump(0.4)

    try:
        os.kill(pid, 9)
    except OSError:
        pass
    try:
        os.waitpid(pid, os.WNOHANG)
    except OSError:
        pass
    try:
        os.close(fd)
    except OSError:
        pass

    return shots


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--keys", default="", help="comma-separated key groups")
    parser.add_argument("cols", type=int)
    parser.add_argument("rows", type=int)
    parser.add_argument("program")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    ns = parser.parse_args()

    groups = [g for g in ns.keys.split(",") if g]
    shots = capture(
        [ns.program, *ns.args], ns.cols, ns.rows, groups
    )

    for label, lines in shots:
        print("=" * ns.cols)
        print(f"### {label}")
        print("=" * ns.cols)
        for line in lines:
            print(line.rstrip())
        print()


if __name__ == "__main__":
    main()
