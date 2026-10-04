# A case is the working directory, and it is created by saving

REactor gains a **case**: one investigation, held in one directory — its evidence, its
database and every session about it. The case is the working directory the GUI already
opens on. REactor's own files live in a hidden `.reactor/` inside it, and a directory
becomes a case only when the user saves.

```
<case>/                         the workdir; evidence lives here
  reactor_data.sqlite           the agent's data tables (ADR-0046)
  .reactor/
    case.json                   id, title, created, format version
    analysis.db                 sessions, change log, graph, registry, corpus (ADR-0046)
    blobs/sha256/ab/cd/<sha256>
    sessions/<id>/activation.json
    run/                        lock files; safe to delete
```

## Decisions

**Case = workdir.** One case is one directory. Evidence that spans several folders goes
into subfolders of it; a case never refers to local files outside its root. A session
already recorded its `cwd` and `-c` already meant "the newest session for this cwd"
([ADR-0036](0036-reactor-owns-its-session-store-format.md)) — the workdir was a case in
everything but name, with its sessions stored somewhere else.

**A case holds many sessions.** A session is one conversation with the agent inside a
case. In-session branching stays as it is: a branch is "try this turn again
differently", a session is a new conversation about the same case.

**A case is created by saving, not by opening.** Opening REactor in a folder that is not
a case gives an *unsaved state*, kept in `~/.reactor/unsaved/<id>/` with the same layout
as `.reactor/`. It is fully usable and survives a crash. Saving moves it into the folder,
which makes the folder a case. Quitting with an unsaved state asks: save as case, or
delete. Pointing REactor at `~` or a source checkout by accident therefore leaves nothing
behind in it. Files the agent wrote into the workdir are ordinary files and stay either
way.

**Nothing under `.reactor/` stores an absolute path.** Saving moves the directory as a
unit, and a case is copied and handed over as a unit. The session entry's `cwd` field
goes.

**Files are referenced in place.** Anything that exists as a file in the workdir —
samples, dumps, captures, unpacked output, scripts, reports — is referenced by relative
path and SHA-256, never copied or moved. A multi-gigabyte dump exists once, where the
analyst put it. `blobs/` holds only content that has no file: the whole of a truncated
tool output, and fetched web pages with their normalised Markdown. A referenced file can
change behind the database's back; the stored hash detects that and the reference is
flagged stale, never silently trusted.

**The goal and the guidelines belong to the case; the steps stay with the session.** The
goal and guidelines are the strategy of an investigation and are the same in every
session of it. They are stored in the case as versioned rows with author and time, so a
change of strategy is part of the case's history, and branching a session does not
branch the goal. The manifest block is composed from the case's goal and guidelines plus
the session's steps. `/goal` and `/guidelines` write to the case; another open session
sees the change on its next request.

**Export is one encrypted archive.** A case may contain live malware, so an export must
not be runnable by accident. `reactor-gui case export` writes the whole case folder into
a ZIP with AES-256, with a consistent copy of the database. The password is `infected`
unless one is given: the default stops accidental execution and scanner quarantine, a
provided password gives confidentiality. Import unpacks into an empty folder, which is
then a case.

**Automation runs through headless `reactor-gui` commands.** `case create`, `case list`,
`case export`, `case import` and the bulk document ingest
([ADR-0046](0046-the-analysis-database-is-sqlite-inside-the-case.md)) are subcommands of
the GUI executable, dispatched before the window stack starts so they work over SSH and
in scripts. The `reactor` CLI is not touched: it is the agent's tool for looking into the
catalogue through `bash`, and nothing about cases is mapped onto it.

## Consequences

- **Sessions under `~/.reactor/sessions/` are left behind.** REactor is in alpha; they
  are not read, imported or migrated.
- **The manifest splits.** `reactor-context`'s manifest state becomes a case part (goal,
  guidelines) and a session part (steps, switch). Its goldens change on purpose.
- **`activation.json` stays a file**, now at `.reactor/sessions/<id>/`, because
  `reactor-core` reads it and must not depend on the database
  ([ADR-0042](0042-the-gui-hosts-the-agent-in-process.md) is otherwise unchanged).
- **`.reactor/` in a workdir now means two things**: the project-scoped `state.json` of
  [ADR-0003](0003-tools-toml-single-source-of-truth.md) and the case. They do not
  collide, and a folder is a case only if `case.json` exists.
- **The GUI's start flow changes**: a folder is a case, has an unsaved state, or gets a
  new one; the session picker lists the case's sessions. SPEC §3 is rewritten.
- **A headless command links the windowing libraries** even though it opens no window.
  They must be installed on a machine that only automates.
- **No case tier in the settings cascade.** Settings still resolve global then session
  ([ADR-0038](0038-settings-resolve-global-then-session.md)). The goal and guidelines are
  case-level, and they are not settings.

## Considered and rejected

- **A case stored centrally, pointing at evidence folders.** It keeps evidence folders
  clean and lets a case span directories. Rejected: a case is then no longer one folder
  to copy or hand over, and "local references stay inside the case root" stops being a
  rule that can be checked.
- **REactor's files visible in the case root.** Easier to find with other tools.
  Rejected: it clutters the analyst's evidence folder. The one file the analyst is meant
  to open with other tools, `reactor_data.sqlite`, is visible.
- **Create the case on first open.** Rejected: opening a directory would leave a
  database in it, and the user never decided that directory was an investigation.
- **Unsaved state in the system temp directory.** Rejected: it does not survive a crash
  or a reboot, which is when it matters.
- **Copy artifacts into a content-addressed store.** It guarantees the bytes never
  change. Rejected: it doubles disk use for large evidence and hides it in a hashed
  tree, for a guarantee the stored hash gives more cheaply as detection.
- **Goal and guidelines per session, inherited from the previous session.** Rejected:
  two sessions of one case would drift apart on what the case is for.
- **Case commands on the `reactor` CLI.** Rejected: that binary is the agent's catalogue
  tool, and it would gain a database and a C build for something the agent does not use.
- **Import old sessions.** Rejected: an importer and the old reader kept alive for
  alpha-era data.
