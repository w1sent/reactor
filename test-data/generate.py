#!/usr/bin/env python3
"""Regenerate the sample files under test-data/. Deterministic: no clock, fixed seed.

    python3 test-data/generate.py            # everything except large/
    python3 test-data/generate.py --large    # also large/: a 500k-row table and a 64 MiB binary (gitignored)

Needs Pillow for the raster images.
"""
import csv
import io
import math
import random
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).parent
rng = random.Random(1337)


def write(rel, data):
    path = ROOT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        data = data.encode("utf-8")
    path.write_bytes(data)


def csv_text(rows, delimiter=","):
    buf = io.StringIO()
    csv.writer(buf, delimiter=delimiter, lineterminator="\n").writerows(rows)
    return buf.getvalue()


# --- table viewer -----------------------------------------------------------

def tables():
    people = [
        ["id", "name", "department", "salary", "start date", "notes"],
        [1, "Ada Lovelace", "Engineering", 125000, "2019-03-04", "Wrote the first program"],
        [2, "Grace Hopper", "Engineering", 118500.5, "2018-11-19", 'Said "it\'s easier to ask forgiveness"'],
        [3, "Linus, Jr.", "Support", 64000, "2021-07-01", "Comma in the name"],
        [4, "Zoë Müller", "Sales", 71000, "2020-01-13", "Non-ASCII: ü ö ñ 日本語 🙂"],
        [5, "alan turing", "research", 99999, "2017-05-23", "Lowercase, to test case-insensitive sort"],
        [6, "Margaret Hamilton", "Engineering", 130000, "2016-09-09", "Line one\nline two inside one cell"],
        [7, "Empty Salary", "Support", "", "2022-02-02", ""],
        [8, "Dennis Ritchie", "Engineering", 9, "2015-06-15", "Salary 9 sorts after 100000 as text, before as a number"],
        [9, "Ken Thompson", "Engineering", 100000, "2015-06-15", ""],
        [10, "  Padded  ", "Sales", 52000, "2023-10-30", "Leading and trailing spaces"],
    ]
    write("table/employees.csv", csv_text(people))
    write("table/employees.tsv", csv_text(people, "\t"))
    write("table/employees.psv", csv_text(people, "|"))
    # No telling extension: the delimiter has to be detected.
    write("table/detect-semicolon.txt", csv_text(people, ";"))
    write("table/detect-pipe.dat", csv_text(people, "|"))

    write("table/numbers.csv", csv_text([
        ["value", "label"],
        [10, "ten"], [9, "nine"], [100, "hundred"], [-5, "negative"], [3.14, "pi"],
        ["1e3", "scientific"], ["", "empty"], ["abc", "text"], [0, "zero"], ["2", "two"],
    ]))

    write("table/ragged.csv", "a,b,c\n1,2\n1,2,3,4,5\n\nx\n,,,\n")
    write("table/header-only.csv", "id,name,value\n")
    write("table/empty.csv", "")
    write("table/single-column.csv", "name\nalpha\nbeta\ngamma\n")
    write("table/no-header.csv", "1,2,3\n4,5,6\n7,8,9\n")
    write("table/quotes.csv",
          'plain,"with, comma","with ""quotes""","multi\nline"\n'
          '"unterminated-free",x,y,z\n')
    # Header spread over two rows, to try the header-rows setting at 2.
    write("table/two-header-rows.csv", csv_text([
        ["", "2024", "2024", "2025", "2025"],
        ["region", "H1", "H2", "H1", "H2"],
        ["north", 120, 135, 150, 162],
        ["south", 98, 101, 110, 117],
        ["east", 143, 150, 149, 171],
        ["west", 77, 80, 95, 99],
    ]))
    write("table/crlf.csv", "id,name\r\n1,windows\r\n2,line endings\r\n")
    write("table/bom.csv", b"\xef\xbb\xbfid,name\n1,utf-8 with a byte order mark\n")

    # Many columns: horizontal scrolling.
    cols = 60
    rows = [[f"col{c}" for c in range(cols)]]
    for r in range(40):
        rows.append([r * cols + c for c in range(cols)])
    write("table/wide.csv", csv_text(rows))

    # A few thousand rows: scrolling, sorting and filtering stay responsive.
    depts = ["Engineering", "Sales", "Support", "Research", "Legal"]
    rows = [["id", "uuid", "department", "score", "ratio", "active"]]
    for i in range(1, 5001):
        rows.append([i, f"{rng.getrandbits(128):032x}", rng.choice(depts),
                     rng.randint(0, 1000), round(rng.random(), 6),
                     rng.choice(["true", "false"])])
    write("table/medium.csv", csv_text(rows))


def large_table():
    rows = ["id,a,b,c,d,e,f,g"]
    for i in range(1, 500_001):
        rows.append(",".join([str(i)] + [str(rng.randint(0, 10**6)) for _ in range(7)]))
    write("large/500k-rows.csv", "\n".join(rows) + "\n")
    # 64 MiB: the hex viewer pages lazily, so size must not matter.
    write("large/64MiB.bin", bytes((i * 31 + (i >> 8)) & 0xFF for i in range(64 * 1024 * 1024)))


# --- data viewer: images ----------------------------------------------------

def image(width, height):
    from PIL import Image
    img = Image.new("RGBA", (width, height))
    px = img.load()
    for y in range(height):
        for x in range(width):
            checker = ((x // 16) + (y // 16)) % 2
            r = int(255 * x / max(width - 1, 1))
            g = int(255 * y / max(height - 1, 1))
            b = 200 if checker else 60
            a = 255
            # Transparent corner, to see how transparency is drawn.
            if x < width // 6 and y < height // 6:
                a = 0
            px[x, y] = (r, g, b, a)
    return img


def images():
    from PIL import Image
    img = image(320, 200)
    buf = io.BytesIO()
    img.save(buf, "PNG"); write("images/gradient.png", buf.getvalue())
    for name, fmt, mode in [("gradient.jpg", "JPEG", "RGB"), ("gradient.bmp", "BMP", "RGB"),
                            ("gradient.tiff", "TIFF", "RGBA"), ("gradient.webp", "WEBP", "RGBA")]:
        buf = io.BytesIO()
        img.convert(mode).save(buf, fmt)
        write(f"images/{name}", buf.getvalue())
    # Animated GIF: three frames.
    frames = [Image.new("P", (120, 120), c) for c in (0, 1, 2)]
    pal = [255, 0, 0, 0, 255, 0, 0, 0, 255] + [0] * (256 * 3 - 9)
    for f in frames:
        f.putpalette(pal)
    buf = io.BytesIO()
    frames[0].save(buf, "GIF", save_all=True, append_images=frames[1:], duration=300, loop=0)
    write("images/three-frames.gif", buf.getvalue())
    buf = io.BytesIO()
    image(64, 64).save(buf, "ICO", sizes=[(16, 16), (32, 32), (64, 64)])
    write("images/icon.ico", buf.getvalue())
    # Extremes: a 1x1 pixel and a large image, to try fit and zoom.
    buf = io.BytesIO(); Image.new("RGBA", (1, 1), (255, 128, 0, 255)).save(buf, "PNG")
    write("images/one-pixel.png", buf.getvalue())
    buf = io.BytesIO(); image(4000, 2500).save(buf, "PNG", compress_level=9)
    write("images/large-4000x2500.png", buf.getvalue())

    star = " ".join(
        f"{100 + 90 * math.cos(math.pi * (k / 5 - 0.5)) * (1 if k % 2 == 0 else 0.4):.1f},"
        f"{100 + 90 * math.sin(math.pi * (k / 5 - 0.5)) * (1 if k % 2 == 0 else 0.4):.1f}"
        for k in range(10))
    write("images/star.svg",
          '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200">\n'
          '  <rect width="200" height="200" fill="#0d1017"/>\n'
          f'  <polygon points="{star}" fill="#ffb454" stroke="#e6b450" stroke-width="3"/>\n'
          '  <circle cx="100" cy="100" r="18" fill="#59c2ff"/>\n'
          '</svg>\n')
    # No extension: the viewer sniffs the bytes.
    write("images/no-extension", (ROOT / "images/gradient.png").read_bytes())
    # Not an image at all, named like one: the error path.
    write("images/not-really.png", "this is plain text, not a PNG\n")
    write("images/truncated.png", (ROOT / "images/gradient.png").read_bytes()[:200])


# --- hex viewer and editor --------------------------------------------------

def binaries():
    write("binary/all-bytes.bin", bytes(range(256)) * 4)
    write("binary/random-64k.bin", bytes(rng.getrandbits(8) for _ in range(65536)))
    # A minimal ELF-ish header and a few strings, so identify/hex have something to chew.
    write("binary/fake-header.bin",
          b"\x7fELF\x02\x01\x01" + b"\x00" * 9 + struct.pack("<HHI", 2, 0x3e, 1)
          + b"\x00" * 40 + b"hello from the hex viewer\x00http://example.invalid/\x00")
    write("binary/zero-length.bin", b"")


def texts():
    write("text/sample.py", '#!/usr/bin/env python3\n"""Highlighting sample."""\n\n\ndef fib(n: int) -> int:\n    return n if n < 2 else fib(n - 1) + fib(n - 2)\n\n\nif __name__ == "__main__":\n    print([fib(i) for i in range(10)])\n')
    write("text/sample.rs", '//! Highlighting sample.\nfn main() {\n    let xs: Vec<u32> = (1..=5).collect();\n    println!("{:?}", xs.iter().map(|x| x * x).sum::<u32>());\n}\n')
    write("text/sample.json", '{\n  "name": "reactor",\n  "tags": ["forensics", "reverse"],\n  "nested": {"n": 1, "ok": true, "none": null}\n}\n')
    write("text/sample.toml", '[package]\nname = "sample"\nversion = "0.1.0"\n\n[dependencies]\nserde = { version = "1", features = ["derive"] }\n')
    write("text/utf8-mixed.txt", "ASCII, ünïcödé, 日本語, عربى, emoji 🙂, tab\tinside\n")
    write("text/crlf.txt", "line one\r\nline two\r\nline three\r\n")
    write("text/long-line.txt", "x" * 20000 + "\n")
    write("text/not-utf8.txt", b"valid start \xff\xfe invalid bytes \xc3\x28 end\n")
    write("text/many-lines.txt", "".join(f"line {i}\n" for i in range(1, 50001)))


def main():
    tables()
    images()
    binaries()
    texts()
    if "--large" in sys.argv:
        large_table()


if __name__ == "__main__":
    main()
