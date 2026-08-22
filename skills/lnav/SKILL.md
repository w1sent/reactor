---
name: lnav
description: Query log files with lnav in headless mode -- SQL over recognized formats, or over fields auto-discovered from unstructured text; reusable scripts; JSON/CSV output. Use when the user wants to analyze or query log files, or needs structured data extracted from messy, inconsistent log text.
requires: [lnav]
---

# lnav

Headless, always: `-n` skips the curses UI, and `-N` stops lnav from opening
this machine's own syslog when no file is named. There is no interactive
session to fall back on here, so both flags belong on every invocation.

## 1. Load and see what lnav already knows

```
lnav -n -N somefile.log
```

prints the file as lnav parsed it. If it matches one of lnav's built-in
formats (`access_log`, `syslog_log`, `journald_json_log`, and around a
hundred more) you get real fields for free -- timestamps normalized,
`log_level` assigned, and a same-named SQL table ready to query. List every
format lnav knows, and see which one actually got used for this file, with:

```
lnav -n -N -c ";SELECT name FROM pragma_table_list ORDER BY name" somefile.log
```

Two tables always exist, whatever did or didn't match: `all_logs` (every
loaded line, any format, any file -- for cross-format correlation or pure
regex work) and `logline` (per-line auto-discovered fields -- see step 2).

Done when: you can name the table to query next -- the format-specific one,
or `all_logs`/`logline` if nothing matched.

## 2. Query it

Most of the actual work is `;SELECT` against whichever table step 1 pointed
at. Two cases:

- **A recognized format** -- query its real columns directly, with normal
  `WHERE`/`GROUP BY`/`ORDER BY`:
  `;SELECT sc_status, count(*) FROM access_log GROUP BY sc_status`. This is
  reliable; treat it as ordinary SQL, and prefer it over `:filter-in`/
  `:filter-out` for anything past "does this line contain a substring" --
  filtering on a real column is more precise than a line-level regex.
- **Unstructured or ad-hoc text** (nothing matched) -- lnav still looks for
  `key=value` pairs and known patterns (IPs, paths, UUIDs, timestamps) in
  every line and exposes them as columns on `logline`, named after the key,
  or `col_0`/`col_1`/... where there is no key. **Use this for inspection
  only** -- always as `SELECT * FROM logline`, eyeballed against the raw
  line, never `WHERE`/`GROUP BY`/naming a specific discovered column in the
  select list. Verified on lnav 0.14.0: naming a discovered column directly
  (`SELECT status FROM logline`) silently returns the wrong value even
  though `SELECT *` shows it correctly, `WHERE <discovered col> = 'x'`
  silently matches nothing, and the *first* `key=value` field discovered on
  each line binds to a stray value (observed: the line's leading date) no
  matter what it's named or where the rest bind correctly. None of this
  raises an error -- it just looks like a normal, wrong answer.

If the field you need is exactly the one discovery gets wrong, or you need
to filter or aggregate on a discovered field at all, that is the sign to
stop and write a custom format instead of fighting `logline` -- see
`references/formats.md`.

Done when: a recognized-format query returns the exact fields you need with
real values; a `logline` inspection has been eyeballed against the raw line,
not merely run.

## 3. Emit it as data, not as a terminal view

lnav's default output is a padded, human-scale table -- fine to glance at,
useless to parse programmatically. End every headless invocation whose
result you need to *use* with one of:

```
-c ':write-json-to /dev/stdout'        # one JSON array
-c ':write-jsonlines-to /dev/stdout'   # one JSON object per line -- large results
-c ':write-csv-to /dev/stdout'         # CSV
```

chained after the `;SELECT` that produced the result:

```
lnav -n -N -c ';SELECT c_ip, count(*) AS hits FROM access_log GROUP BY c_ip ORDER BY hits DESC' -c ':write-json-to /dev/stdout' access.log
```

Done when: you have JSON or CSV in hand, not an aligned-column table you
would otherwise have to re-parse.

## Going further

- **The same query, run again later, or against several files** -- put it in
  a `.lnav` script and run with `-f script.lnav`. A script mixes `;` SQL
  statements, `:` lnav commands, `|` sub-script includes, and `#` comments,
  one per line, in the order they run -- everything from steps 2 and 3 works
  unchanged inside one.
- **Auto-discovery genuinely isn't enough** -- the level/component/message
  split matters, timestamps aren't standard, or this exact shape of file will
  recur and deserves a stable schema -- define a custom format. See
  `references/formats.md`.
- **This shape of task has a known recipe** -- ranking a field, tagging
  matching lines, correlating across files -- check `references/cookbook.md`
  before writing one from scratch.
