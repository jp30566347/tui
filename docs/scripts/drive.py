#!/usr/bin/env python3
"""Drive a TUI in a pty on a fixed grid and write an asciicast v2 file.

The app renders to stderr, so both streams go to the same pty and the
recording is what a real terminal would have shown. Keystrokes are sent on a
wall-clock schedule from a script of (delay, keys) pairs, which keeps the
resulting GIF paced like someone actually using the app rather than like a
key-repeat.
"""
import argparse, fcntl, json, os, pty, select, signal, struct, termios, time

# The docs site palette, so a GIF dropped on the page is the same terminal as
# the one in the CSS around it. agg reads this straight out of the header.
THEME = {
    "fg": "#e8ebf4",
    "bg": "#0b0c11",
    "palette": ":".join([
        "#1a1d27", "#f2647a", "#5fd39a", "#e8c46a",
        "#7aa2ff", "#b78cf2", "#4dc4d6", "#c7cde0",
        "#646b84", "#ff8095", "#7ee5b3", "#ffd98a",
        "#9bb8ff", "#ccaaff", "#74dbe9", "#ffffff",
    ]),
}

# crossterm probes the terminal at startup and blocks on the answer: a cursor
# position report, a device attributes report, and a kitty keyboard protocol
# query. A bare pty answers none of them, so the app gives up with "the cursor
# position could not be read". Answer the three the way a plain xterm would.
REPLIES = [
    (b"\x1b[?u", b"\x1b[?0u"),      # kitty keyboard flags: none supported
    (b"\x1b[6n", b"\x1b[1;1R"),     # cursor position
    (b"\x1b[>0c", b"\x1b[>0;276;0c"),
    (b"\x1b[>c", b"\x1b[>0;276;0c"),
    (b"\x1b[0c", b"\x1b[?1;2c"),
    (b"\x1b[c", b"\x1b[?1;2c"),
]


def answer_queries(fd, data):
    """Answer every query in one read of the app's output. Both apps send
    theirs in a single burst at startup, so a query is never split across two
    reads and a plain substring scan is enough."""
    for query, reply in REPLIES:
        for _ in range(data.count(query)):
            os.write(fd, reply)
        data = data.replace(query, b"")


def set_size(fd, cols, rows):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--cols", type=int, default=110)
    ap.add_argument("--rows", type=int, default=32)
    ap.add_argument("--script", required=True,
                    help="JSON list of [delay, keys] or [delay, keys, note]")
    ap.add_argument("--title", default="")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()

    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    steps = json.load(open(a.script))

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLORTERM"] = "truecolor"
        os.environ["LINES"] = str(a.rows)
        os.environ["COLUMNS"] = str(a.cols)
        os.execvp(cmd[0], cmd)

    set_size(fd, a.cols, a.rows)

    out = open(a.out, "w")
    out.write(json.dumps({
        "version": 2, "width": a.cols, "height": a.rows,
        "timestamp": int(time.time()), "title": a.title,
        "env": {"TERM": "xterm-256color", "SHELL": "/bin/sh"},
        "theme": THEME,
    }) + "\n")

    t0 = time.time()
    # Absolute send times, so a slow fetch never shifts the rest of the script.
    schedule, t = [], 0.0
    for step in steps:
        # A third element is a note for whoever reads the script file; JSON
        # has nowhere else to put one.
        delay, keys = step[0], step[1]
        t += delay
        schedule.append((t, keys))
    end = schedule[-1][0] + 0.6 if schedule else 5.0
    i = 0

    try:
        while True:
            now = time.time() - t0
            if i < len(schedule) and now >= schedule[i][0]:
                os.write(fd, schedule[i][1].encode())
                i += 1
                continue
            if now >= end:
                break
            r, _, _ = select.select([fd], [], [], 0.05)
            if fd in r:
                try:
                    data = os.read(fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                answer_queries(fd, data)
                out.write(json.dumps([round(time.time() - t0, 6), "o",
                                      data.decode("utf-8", "replace")]) + "\n")
                out.flush()
    finally:
        out.close()
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
    print(f"wrote {a.out} ({os.path.getsize(a.out)} bytes)")

main()
