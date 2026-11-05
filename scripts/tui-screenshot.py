#!/usr/bin/env python3
"""Screenshot of the terminal UI as SVG (docs/images/tui.svg).

Runs `ferry tui` in a pseudo-terminal with an example configuration, inside a
network namespace without any network (`unshare -rn`, so no real device of the
local network appears), waits until it has drawn its screen, then replays the
output through a small terminal emulator and writes the screen as SVG.

    cargo build -p ferry-cli
    python3 -I scripts/tui-screenshot.py target/debug/ferry docs/images/tui.svg
"""

import html
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import fcntl
import time

COLS, ROWS = 124, 32
PALETTE = {  # the 16 ANSI colours (a dark theme)
    0: "#1e1e1e", 1: "#e06c75", 2: "#98c379", 3: "#e5c07b", 4: "#61afef", 5: "#c678dd", 6: "#56b6c2", 7: "#d7dae0",
    8: "#5c6370", 9: "#ff7b86", 10: "#b5e890", 11: "#ffd68a", 12: "#7cc4ff", 13: "#de9bff", 14: "#7fd8e3", 15: "#ffffff",
}
FG, BG = "#d7dae0", "#16181d"

CONFIG = """device_name = "Desktop"
default_download_dir = "/home/alex/Downloads"

[peers.3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f3c7f]
alias = "Work laptop"
auto_accept = true
verified = true
added_at = "2026-10-01T10:00:00Z"

[peers.9a419a419a419a419a419a419a419a419a419a419a419a419a419a419a419a41]
alias = "Alex's phone"
auto_accept = false
verified = true
added_at = "2026-10-02T10:00:00Z"
"""


def color256(n):
    if n < 16:
        return PALETTE[n]
    if n < 232:
        n -= 16
        steps = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (steps[n // 36], steps[(n // 6) % 6], steps[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


class Screen:
    """Just enough of a terminal for ratatui/crossterm output."""

    def __init__(self):
        self.cells = [[(" ", None, None, False) for _ in range(COLS)] for _ in range(ROWS)]
        self.x = self.y = 0
        self.fg = self.bg = None
        self.bold = False
        self.reverse = False

    def sgr(self, params):
        codes = [int(p) if p else 0 for p in params.split(";")] if params else [0]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                self.fg = self.bg = None
                self.bold = self.reverse = False
            elif c == 7:
                self.reverse = True
            elif c == 27:
                self.reverse = False
            elif c == 1:
                self.bold = True
            elif c == 22:
                self.bold = False
            elif 30 <= c <= 37:
                self.fg = PALETTE[c - 30]
            elif 90 <= c <= 97:
                self.fg = PALETTE[c - 90 + 8]
            elif 40 <= c <= 47:
                self.bg = PALETTE[c - 40]
            elif 100 <= c <= 107:
                self.bg = PALETTE[c - 100 + 8]
            elif c == 39:
                self.fg = None
            elif c == 49:
                self.bg = None
            elif c in (38, 48) and i + 1 < len(codes):
                if codes[i + 1] == 5 and i + 2 < len(codes):
                    value = color256(codes[i + 2])
                    i += 2
                elif codes[i + 1] == 2 and i + 4 < len(codes):
                    value = "#%02x%02x%02x" % tuple(codes[i + 2:i + 5])
                    i += 4
                else:
                    value = None
                if c == 38:
                    self.fg = value
                else:
                    self.bg = value
            i += 1

    def feed(self, text):
        pos = 0
        csi = re.compile(r"\x1b\[([?0-9;]*)([A-Za-z@])")
        while pos < len(text):
            ch = text[pos]
            if ch == "\x1b":
                m = csi.match(text, pos)
                if m:
                    self.control(m.group(1), m.group(2))
                    pos = m.end()
                    continue
                pos += 2  # other escapes (e.g. ESC 7/8): ignored
                continue
            if ch == "\r":
                self.x = 0
            elif ch == "\n":
                self.y = min(self.y + 1, ROWS - 1)
            elif ch >= " ":
                if self.x < COLS and self.y < ROWS:
                    fg, bg = self.fg, self.bg
                    if self.reverse:
                        fg, bg = bg or BG, fg or FG
                    self.cells[self.y][self.x] = (ch, fg, bg, self.bold)
                self.x += 1
            pos += 1

    def control(self, params, cmd):
        if params.startswith("?"):
            return
        nums = [int(p) if p else 0 for p in params.split(";")] if params else []
        if cmd == "H" or cmd == "f":
            row = (nums[0] if nums else 1) or 1
            col = (nums[1] if len(nums) > 1 else 1) or 1
            self.y, self.x = min(row - 1, ROWS - 1), min(col - 1, COLS - 1)
        elif cmd == "m":
            self.sgr(params)
        elif cmd == "J" and (nums[:1] or [0])[0] == 2:
            self.cells = [[(" ", None, None, False) for _ in range(COLS)] for _ in range(ROWS)]
        elif cmd == "K":
            for x in range(self.x, COLS):
                self.cells[self.y][x] = (" ", None, self.bg, False)
        elif cmd == "C":
            self.x = min(self.x + (nums[0] if nums else 1), COLS - 1)
        elif cmd == "G":
            self.x = min((nums[0] if nums else 1) - 1, COLS - 1)

    def svg(self):
        cw, ch = 8.4, 18
        width, height = COLS * cw + 24, ROWS * ch + 24
        out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}" '
               f'viewBox="0 0 {width:.0f} {height:.0f}" font-family="\'JetBrains Mono\',\'DejaVu Sans Mono\',Menlo,Consolas,monospace" font-size="14">',
               f'<rect width="100%" height="100%" rx="8" fill="{BG}"/>']
        for y, row in enumerate(self.cells):
            # Runs of cells with the same background, as one rectangle each.
            x = 0
            while x < COLS:
                bg = row[x][2]
                start = x
                while x < COLS and row[x][2] == bg:
                    x += 1
                if bg:
                    out.append(f'<rect x="{12 + start * cw:.1f}" y="{12 + y * ch}" width="{(x - start) * cw:.1f}" height="{ch}" fill="{bg}"/>')
            # Runs of characters with the same style.
            x = 0
            while x < COLS:
                c, fg, _, bold = row[x]
                if c == " ":
                    x += 1
                    continue
                start, text = x, ""
                while x < COLS and row[x][1] == fg and row[x][3] == bold and (row[x][0] != " " or (x + 1 < COLS and row[x + 1][0] != " ")):
                    text += row[x][0]
                    x += 1
                weight = ' font-weight="bold"' if bold else ""
                out.append(f'<text x="{12 + start * cw:.1f}" y="{12 + y * ch + 13}" fill="{fg or FG}"{weight} '
                           f'textLength="{len(text) * cw:.1f}" lengthAdjust="spacingAndGlyphs" xml:space="preserve">{html.escape(text)}</text>')
        out.append("</svg>")
        return "\n".join(out)


def main():
    ferry, target = os.path.abspath(sys.argv[1]), sys.argv[2]
    with tempfile.TemporaryDirectory() as config_dir:
        with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as f:
            f.write(CONFIG)
        env = dict(os.environ, FERRY_CONFIG_DIR=config_dir, FERRY_LANG="en", TERM="xterm-256color",
                   COLUMNS=str(COLS), LINES=str(ROWS))
        command = [ferry, "tui"]
        if shutil.which("unshare"):
            # A made-up local network, so the screen looks like at home.
            setup = ("ip link set lo up; ip link add lan0 type dummy && ip addr add 192.168.50.20/24 dev lan0 "
                     "&& ip link set lan0 up; exec \"$@\"")
            command = ["unshare", "-rn", "sh", "-c", setup, "sh", *command]
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(command[0], command, env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        output = b""
        end = time.time() + 6
        while time.time() < end:
            ready, _, _ = select.select([fd], [], [], 0.1)
            if ready:
                try:
                    output += os.read(fd, 1 << 16)
                except OSError:
                    break
        os.write(fd, b"q")
        time.sleep(0.5)
        try:
            os.kill(pid, 9)
        except ProcessLookupError:
            pass
    screen = Screen()
    # Only the last full redraw: from the last "clear screen" on.
    text = output.decode("utf-8", errors="replace")
    screen.feed(text)
    with open(target, "w", encoding="utf-8") as f:
        f.write(screen.svg())
    print(target)


if __name__ == "__main__":
    main()
