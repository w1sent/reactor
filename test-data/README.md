# test-data

Sample files for trying the GUI viewers by hand (`/open table test-data/table/employees.csv`,
`/look test-data/images/star.svg`, ...). Nothing here is real data; every file is produced by
`generate.py`, which is deterministic, so regenerating changes nothing.

```bash
python3 test-data/generate.py           # everything below, except large/
python3 test-data/generate.py --large   # also large/ (gitignored): 500k-row CSV, 64 MiB binary
```

Pillow is needed for the raster images.

| Folder | For | What is in it |
|---|---|---|
| `table/` | table viewer | One table in comma, tab and pipe form (`employees.*`), and the same with semicolon/pipe under extensions that force delimiter *detection* (`detect-*`). Quoted commas, quotes and newlines, non-ASCII, padded and empty cells, numbers that sort differently as text (`numbers.csv`), ragged rows, a two-row header (`two-header-rows.csv`, set header rows to 2), 60 columns (`wide.csv`), 5,000 rows (`medium.csv`), CRLF, a BOM, and the degenerate cases: empty file, header only, single column, no header. |
| `images/` | data viewer | PNG, JPEG, BMP, TIFF, WebP, animated GIF, multi-size ICO and SVG; a 1×1 and a 4000×2500 PNG for fit and zoom; a PNG with no extension (sniffed); a text file named `.png` and a truncated PNG for the error path. |
| `binary/` | hex viewer | All 256 byte values, 64 KiB of seeded noise, an ELF-like header with strings, and an empty file. |
| `text/` | editor | Python, Rust, JSON and TOML for highlighting; mixed Unicode, CRLF, a 20,000-character line, 50,000 lines, and a file that is not valid UTF-8 (the editor should offer the hex viewer). |
| `large/` | all | Generated on demand, not committed. |
