# Custom log formats

Only worth writing when `logline`'s auto-discovery genuinely is not enough:
you need a stable schema across every file of this kind, an accurate
`log_level`, or a message body separated cleanly from its metadata.

## Minimal shape

A JSON file, one top-level key per format -- that key is both the format's
name and its SQL table name:

```json
{
    "$schema": "https://lnav.org/schemas/format-v1.schema.json",
    "custom_log": {
        "title": "Custom Log",
        "regex": {
            "basic": {
                "pattern": "^(?<timestamp>\\d{4}-\\d{2}-\\d{2} \\d{2}:\\d{2}:\\d{2}\\.\\d{3})>>(?<level>\\w+)>>(?<component>\\w+)>>(?<body>.*)$"
            }
        },
        "level-field": "level",
        "body-field": "body",
        "level": { "error": "ERROR", "info": "INFO" },
        "value": {
            "component": { "kind": "string", "identifier": true }
        },
        "sample": [
            { "line": "2024-03-01 10:00:00.000>>ERROR>>auth>>failed login for bob from 10.0.0.5" }
        ]
    }
}
```

Verified end to end against lnav 0.14.0: without `level-field` and
`body-field`, the named capture groups (`level`, `body`) exist only as their
raw capture names, not the standardized `log_level`/`log_body` columns that
`all_logs`, `:summarize`, and lnav's own UI all expect -- name them
explicitly whenever your regex captures a level or a message body.

## Rules that actually bite

- `regex.<name>.pattern` **must match exactly one kind of message.** lnav
  locks onto whichever regex matched the first lines of a file and stops
  trying the others for the rest of it, so two message shapes need two named
  regexes (or two formats).
- `sample` is not optional. At least one line, and it must match one of your
  regexes, or lnav refuses to load the format at all.
- `value.<name>.kind` (`string`, `integer`, `float`, `json`, `quoted`,
  `timestamp`) decides the SQL column's type -- get it wrong and a `GROUP
  BY` or numeric comparison silently does the wrong thing on text instead of
  erroring.

## Install and use

```
lnav -i myformat.json                 # installs into ~/.config/lnav/formats/installed/
lnav -n -N -c ';SELECT log_time, log_level, component, log_body FROM custom_log' somefile.log
```

`file-pattern` (a regex on the *filename*, not the content) narrows which
files lnav even tries this format against -- worth setting once more than one
custom format is installed, so they stop competing for a lock on files
neither was meant to parse.
