#!/usr/bin/env python3
"""End-to-end key check: run the real `cuz` binary in a pseudo-terminal, send the
raw bytes terminals actually send, and read the focused pane off the screen.

The Rust key tests feed ideal key events (Alt+Left) into the keymap; a macOS
terminal sends ESC b. This checks the bytes. Exits non-zero on a mismatch.

    pip install pyte
    cargo build --release
    python3 scripts/pty_keys.py
"""
import fcntl, os, pathlib, pty, select, struct, sys, termios, time

import pyte

REPO = pathlib.Path(__file__).resolve().parent.parent
W, H = 150, 40

ARROW = {"→": b"\x1b[C", "←": b"\x1b[D", "↑": b"\x1b[A"}
MAC_OPT = {"⌥→": b"\x1bf", "⌥←": b"\x1bb"}          # Terminal.app / iTerm2 defaults
XTERM_ALT = {"⌥→": b"\x1b[1;3C", "⌥←": b"\x1b[1;3D"}

# (label, steps, focus after each step including the start)
CASES = [
    ("plain arrows, nothing typed", [("→", ARROW["→"]), ("←", ARROW["←"]), ("←", ARROW["←"])],
     ["composer", "Changes", "composer", "Sessions"]),
    ("Option+arrows as macOS sends them", [("⌥→", MAC_OPT["⌥→"]), ("⌥←", MAC_OPT["⌥←"]), ("⌥←", MAC_OPT["⌥←"])],
     ["composer", "Changes", "composer", "Sessions"]),
    ("into the code pane and back", [("↑", ARROW["↑"]), ("⌥→", MAC_OPT["⌥→"]), ("⌥→", MAC_OPT["⌥→"]), ("⌥←", MAC_OPT["⌥←"])],
     ["composer", "composer", "Changes", "Code", "Changes"]),
    ("Alt+arrows, xterm encoding", [("⌥→", XTERM_ALT["⌥→"]), ("⌥←", XTERM_ALT["⌥←"])],
     ["composer", "Changes", "composer"]),
    ("arrows edit once there is text", [("type", b"hello world"), ("→", ARROW["→"]), ("⌥←", MAC_OPT["⌥←"]), ("←", ARROW["←"])],
     ["composer", "composer", "composer", "composer", "composer"]),
    ("Tab leaves and returns while typing", [("type", b"hello"), ("Tab", b"\t"), ("Shift+Tab", b"\x1b[Z")],
     ["composer", "composer", "Changes", "composer"]),
    ("Tab cycles past a missing code stop", [("Tab", b"\t")] * 3,
     ["composer", "Changes", "Sessions", "composer"]),
    ("Shift+Tab cycles past a missing code stop", [("Shift+Tab", b"\x1b[Z")] * 2,
     ["composer", "Sessions", "Changes"]),
    ("Tab enters code when there is code", [("↑", ARROW["↑"]), ("Tab", b"\t"), ("Tab", b"\t"), ("Tab", b"\t")],
     ["composer", "composer", "Changes", "Code", "Sessions"]),
    ("Enter reads an input in full, Esc returns", [("Enter", b"\r"), ("Space", b" "), ("Esc", b"\x1b")],
     ["composer", "Reader", "Reader", "composer"]),
    # Known limitation, kept executable: crossterm reads ESC ESC as a lone Esc,
    # so the ESC-prefixed arrow encoding leaves the composer. Use Tab instead.
    ("ESC-prefixed arrow (known limitation)", [("ESC ESC [C", b"\x1b\x1b[C")],
     ["composer", "Segments list"]),
]


class Screen(pyte.Screen):
    """Answers the device-attributes query crossterm sends at startup, as a
    real terminal would, so the keyboard-enhancement probe returns promptly."""

    def __init__(self, fd, *a):
        super().__init__(*a)
        self.fd = fd

    def write_process_input(self, data):
        os.write(self.fd, data.encode())


def pump(fd, stream, secs):
    end = time.time() + secs
    while time.time() < end:
        if select.select([fd], [], [], 0.05)[0]:
            try:
                stream.feed(os.read(fd, 65536).decode("utf-8", "replace"))
            except OSError:
                return


def focus(screen):
    """Which pane owns the keyboard, read off the screen."""
    if any("as of input" in row for row in screen.display):
        return "Code"
    if "· in full" in screen.display[0]:
        return "Reader"
    top = screen.display[0]
    for title, name in (("1 Sessions", "Sessions"), ("2 Segments", "Segments list"), ("3 Changes", "Changes")):
        x = top.find(title)
        if x >= 0 and screen.buffer[0][x].bold:
            return name
    status = screen.display[H - 1]
    if "type to write" in status or "typing ·" in status:
        return "composer"
    return "unknown: " + status.strip()[:50]


def run(steps):
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(REPO)
        os.execv(str(REPO / "target/release/cuz"), ["cuz", "demo"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", H, W, 0, 0))
    screen = Screen(fd, W, H)
    stream = pyte.Stream(screen)
    pump(fd, stream, 1.5)
    seen = [focus(screen)]
    for _, data in steps:
        os.write(fd, data)
        pump(fd, stream, 0.35)
        seen.append(focus(screen))
    os.kill(pid, 9)
    os.waitpid(pid, 0)
    return seen


def main():
    failed = 0
    for label, steps, want in CASES:
        got = run(steps)
        ok = got == want
        failed += not ok
        keys = " ".join(name for name, _ in steps)
        print(f"{'ok  ' if ok else 'FAIL'} {label:40s} {keys:28s} → {' › '.join(got)}")
        if not ok:
            print(f"     expected {' › '.join(want)}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
