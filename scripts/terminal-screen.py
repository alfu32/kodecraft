"""Small incremental screen reader for the CSI commands emitted by Crossterm.

This records cells/cursor positions, not styles. Source rendering already escapes
wide/control characters, so the exercised application's output uses single cells.
"""
import codecs
import re

CSI = re.compile(r"\x1b\[([0-?]*)([ -/]*)([@-~])")


class Screen:
    def __init__(self, width, height):
        self.width, self.height = width, height
        self.cells = [[" "] * width for _ in range(height)]
        self.x = self.y = 0
        self.saved = (0, 0)
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        text = self.pending
        index = 0
        while index < len(text):
            ch = text[index]
            if ch == "\x1b":
                if index + 1 >= len(text):
                    break
                if text[index + 1] == "[":
                    match = CSI.match(text, index)
                    if match is None:
                        break
                    self.command(match[1], match[3])
                    index = match.end()
                    continue
                if text[index + 1] == "]":
                    ends = [at for at in [text.find("\x07", index + 2), text.find("\x1b\\", index + 2)] if at >= 0]
                    if not ends:
                        break
                    end = min(ends)
                    index = end + (2 if text[end] == "\x1b" else 1)
                    continue
                if text[index + 1] == "7":
                    self.saved = (self.x, self.y)
                elif text[index + 1] == "8":
                    self.x, self.y = self.saved
                index += 2
                continue
            if ch == "\r":
                self.x = 0
            elif ch == "\n":
                self.y = min(self.height - 1, self.y + 1)
            elif ch == "\b":
                self.x = max(0, self.x - 1)
            elif ch >= " " and ch != "\x7f":
                if self.x >= self.width:
                    self.x = 0
                    self.y = min(self.height - 1, self.y + 1)
                self.cells[self.y][self.x] = ch
                self.x += 1
            index += 1
        self.pending = text[index:]

    def command(self, params, final):
        # Private mode changes, reports, SGR and scroll regions do not paint cells.
        if params.startswith(("?", ">", "<", "=")) or final in "mhlrctq":
            return
        values = [int(part) if part.isdigit() else 0 for part in params.split(";")]
        amount = (values[0] if values else 0) or 1
        if final in "Hf":
            self.y = min(self.height - 1, max(0, amount - 1))
            self.x = min(self.width - 1, max(0, (values[1] if len(values) > 1 else 1) - 1))
        elif final == "G":
            self.x = min(self.width - 1, amount - 1)
        elif final == "d":
            self.y = min(self.height - 1, amount - 1)
        elif final == "A":
            self.y = max(0, self.y - amount)
        elif final == "B":
            self.y = min(self.height - 1, self.y + amount)
        elif final == "C":
            self.x = min(self.width - 1, self.x + amount)
        elif final == "D":
            self.x = max(0, self.x - amount)
        elif final == "s":
            self.saved = (self.x, self.y)
        elif final == "u":
            self.x, self.y = self.saved
        elif final == "J":
            mode = values[0]
            for y, row in enumerate(self.cells):
                for x in range(self.width):
                    if mode == 2 or mode == 0 and (y, x) >= (self.y, self.x) or mode == 1 and (y, x) <= (self.y, self.x):
                        row[x] = " "
        elif final == "K":
            mode = values[0]
            for x in range(self.width):
                if mode == 2 or mode == 0 and x >= self.x or mode == 1 and x <= self.x:
                    self.cells[self.y][x] = " "

    def text(self):
        return "\n".join("".join(row) for row in self.cells).encode("utf-8")
