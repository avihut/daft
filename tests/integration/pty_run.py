#!/usr/bin/env python3
"""Run a command on a PTY that behaves like a real terminal.

Bare script(1) hands daft's TUI a pty with nobody on the master side, so
crossterm's cursor-position query (ESC[6n) never gets an answer and the
inline viewport fails to initialize ("cursor position could not be read").
This wrapper plays the terminal's role: it sets a sane window size, drains
the master into a log file, answers every DSR query with a synthetic
"row 1, col 1" report, and exits with the command's status.

It can also type: --send-after PATTERN:BYTES writes BYTES to the pty the
first time PATTERN appears in the output. Repeat the flag to script a
sequence (two Ctrl-Cs for a two-stage cancel, say); each trigger fires at
most once, in the order its pattern appears. BYTES accepts Python escapes,
so \\x03 is Ctrl-C.

Usage: pty_run.py [--send-after PATTERN:BYTES]... <log-file> <command> [args...]
"""

import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios


def parse_args(argv):
    """Split leading flags from the log path and command."""
    triggers = []
    ctty = False
    while argv and argv[0] in ("--send-after", "--ctty"):
        if argv[0] == "--ctty":
            ctty = True
            argv = argv[1:]
            continue
        spec = argv[1]
        pattern, _, payload = spec.partition(":")
        triggers.append(
            [
                pattern.encode(),
                payload.encode().decode("unicode_escape").encode("latin-1"),
            ]
        )
        argv = argv[2:]
    return triggers, ctty, argv[0], argv[1:]


def main():
    triggers, ctty, log_path, cmd = parse_args(sys.argv[1:])
    master, slave = pty.openpty()
    # ratatui needs a non-zero viewport; 24x80 matches a classic terminal.
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

    # --ctty: make the child a session leader owning this pty, so writing
    # \x03 to the master actually raises SIGINT in it. Without it the pty has
    # no foreground process group and the interrupt goes nowhere. Opt-in,
    # because a new session also detaches the child from the caller's job
    # control — only interrupt tests want that.
    def become_session_leader():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

    proc = subprocess.Popen(
        cmd,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        close_fds=True,
        preexec_fn=become_session_leader if ctty else None,
    )
    os.close(slave)

    tail = b""
    seen = b""
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
                # Fire any scripted input whose cue has now appeared. The
                # first pending trigger only, so a sequence stays ordered
                # even when one chunk satisfies several cues.
                if triggers:
                    seen += chunk
                    if triggers[0][0] in seen:
                        try:
                            os.write(master, triggers[0][1])
                        except OSError:
                            pass
                        triggers.pop(0)
                        seen = b""
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
