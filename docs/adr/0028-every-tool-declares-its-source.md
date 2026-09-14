# Every tool declares its source

Every catalogue entry gains one optional-but-shipped field: the tool's true
upstream, as a URL.

```toml
[tool.usql]
name   = "usql"
desc   = "universal SQL client: ..."
source = "https://github.com/xo/usql"
```

The source is provenance, not documentation: it answers "if I wanted to verify
or obtain this tool myself, where does it actually come from?" — the
canonical repository where one exists, the project's own site otherwise
(`https://www.sqlite.org/`, `https://exiftool.org/`). Prefer the repo over the
marketing page when both exist; the repo is what the install one-liner clones
and what a suspicious agent can diff a downloaded binary's claims against.

## Why the catalogue and not the install table

The manual install paths — the `manual` note and, since
[ADR-0027](0027-manual-install-oneliner-is-executed-only-under-explicit-flags.md),
the `manual-install-oneliner` — are the one place REactor runs or recommends
commands that no package manager has vetted. A one-liner is only trustworthy
relative to where the tool comes from: `git clone https://github.com/redis/redis`
is trustworthy because `https://github.com/redis/redis` is Redis's own repo, not
because the command looks reasonable. Stating the source as a field, next to
the install table, makes that trust relationship explicit and checkable by eye
in `reactor tools show` before anything is run. Registries lie by omission
(PyPI's `cqlsh` is a community packaging, not Apache's own); the source field
records the truth even when a redistributable package has a different name.

## Rules

- `source` is a single `https://` URL. No `http`, no DOI, no "see README" —
  the shipped-config test enforces both presence and scheme, so a new entry
  cannot ship without deciding where its tool comes from.
- The canonical repository wins over the product site (`github.com/redis/redis`
  over redis.io); the project site wins when there is no repository
  (`exiftool.org`, `sqlite.org`). For a tool whose catalogue entry is a
  technique rather than an artifact (`decompile-python`), the source is the
  documentation that defines the technique.
- The loader rejects a non-https `source` rather than silently accepting it;
  authenticity is not a lint, it is the field's reason to exist.

## Consequences

- `reactor tools show` (human and JSON) carries `source`, so an agent about to
  run a `--force-install-manual` oneliner can see, in the same output, whether
  the command's URLs match the tool's declared provenance.
- Every entry needs its source decided at authoring time — the same moment the
  install recipes are checked against manager indexes, which is when a
  wrong-source mistake is cheapest to catch.
- Community repackagings get their distinction written into the install
  comments (`cqlsh`: PyPI is `jeffwidman/cqlsh`, the tool's source is
  `github.com/apache/cassandra`) rather than left to be discovered the hard
  way.