#!/usr/bin/env python3
"""Run a command on a PTY that behaves like a real terminal.

Bare script(1) hands daft's TUI a pty with nobody on the master side, so
crossterm's cursor-position query (ESC[6n) never gets an answer and the
inline viewport fails to initialize ("cursor position could not be read").
This wrapper plays the terminal's role: it sets a sane window size, drains
the master into a log file, answers every DSR query with a synthetic
"row 1, col 1" report, and exits with the command's status.

Usage: pty_run.py <log-file> <command> [args...]
"""

import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios


def main():
    log_path, *cmd = sys.argv[1:]
    master, slave = pty.openpty()
    # ratatui needs a non-zero viewport; 24x80 matches a classic terminal.
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
    os.close(slave)

    tail = b""
    with open(log_path, "wb") as log:
        while True:
            try:
                readable, _, _ = select.select([master], [], [], 0.2)
            except InterruptedError:
                continue
            if master in readable:
                try:
                    chunk = os.read(master, 4096)
                except OSError:
                    chunk = b""
                if not chunk:
                    break
                log.write(chunk)
                log.flush()
                # Answer cursor-position queries; keep a short tail so a
                # query split across reads is still recognized.
                tail = (tail + chunk)[-16:]
                if b"\x1b[6n" in tail:
                    try:
                        os.write(master, b"\x1b[1;1R")
                    except OSError:
                        pass
                    tail = b""
            elif proc.poll() is not None:
                # Command exited and the pty went quiet: drain and stop.
                while True:
                    readable, _, _ = select.select([master], [], [], 0.1)
                    if master not in readable:
                        break
                    try:
                        chunk = os.read(master, 4096)
                    except OSError:
                        break
                    if not chunk:
                        break
                    log.write(chunk)
                break

    os.close(master)
    sys.exit(proc.wait())


if __name__ == "__main__":
    main()
