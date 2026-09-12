# identity

One command selects the agent's working persona. The selected identity is a
block in the system prompt — it shapes how the agent works for the whole
session: what it prioritizes, what evidence standards it holds, what it hands
to whom. Switching mid-session is one command; the block changes on the next
turn. Nothing is injected until an identity is selected, so a session that
never uses this extension has a byte-identical system prompt.

Reasoning and design constraints live in
[ADR-0026](../../docs/adr/0026-identity-is-a-persona-block-in-the-system-prompt.md);
the inventory of all extensions is in [the extensions README](../README.md).

## The built-in identities

| Identity | Focus |
|---|---|
| `reverse-engineer` | Code-level artifact analysis: static triage, unpacking, disassembly and decompilation, dynamic verification, recovered algorithms, extracted configuration, detection logic. |
| `cyber-forensics` | Malware incident reconstruction end to end: compromise, execution chain, persistence, lateral movement, C2 — evidence correlated into an attack narrative, ATT&CK-mapped, with an operational IOC set. |
| `forensics` | General system and user-activity forensics: what happened on a system and what a user or actor did — defensible acquisition, timelines, chain of custody, NIST SP 800-86 / ISO/IEC 27037 discipline. Independent of malware. |
| `software-engineer` | Tooling for the analysis team: parsers, extractors, deobfuscators, decoders — clean, tested, validated against ground truth, never touching original evidence. |
| `infrastructure` | Safe, reproducible analysis environments: isolation verified before detonation, snapshots and baselines, integrity-hashed evidence storage, CI for the team's tooling. Never analyzes, never authors findings. |
| `publisher` | Turning findings into defensible deliverables: executive summary, technical body, IOC appendix — every claim traced to evidence, ICD 203 estimative standards, no invented severity. |

The identities form a handoff graph: the reverse engineer hands incident
context to Cyber-Forensics, the analysts hand tooling to the Software
Engineer, everyone hands narrative to the Publisher, and the Infrastructure
identity supports all of them without ever analyzing.

## Selecting

```
/identity                    what is active, and what exists (autocomplete
                             offers all of these with descriptions)
/identity forensics          select a built-in by name
/identity off                explicit off -- overrides a configured default
/identity show [name]        read the full text before or after selecting
```

## Adding new ones during a session

Write one adhoc, use it, and save it when it proves useful:

```
/identity write <text>       one-liner; active immediately as "custom"
/identity editor             same, in $VISUAL/$EDITOR/nano -- the terminal is
                             handed to the editor and returned afterwards
/identity save <name>        persist the current custom text as a named,
                             reusable identity (warns before overwriting)
/identity delete <name>      remove a saved identity
```

A saved identity goes into `~/.pi/agent/pi-identity.json` — the same file
holds the global `default` and is hand-editable. Built-in names are refused
for saves, so a saved identity can never shadow one.

### Writing a good one

The built-ins all follow the same shape; copy it:

- **Mission** — one sentence, what this persona is for.
- **Scope** — the artifact types, platforms and situations it covers.
- **Responsibilities** — what it does, in priority order.
- **Outputs** — the concrete artifacts it produces.
- **Quality standards** — what "done" means, what is never done.
- **Boundaries** — what it hands to which other identity, and what it
  refuses to do.

Keep it to what changes the agent's behavior; anything the model would do
anyway is prompt weight with no effect.
