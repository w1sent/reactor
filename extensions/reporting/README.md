# reporting

"Document as you go," enforced gently. Off by default; `/report on` opts the
session in, and a block enters the system prompt asking the agent to write up
each step of its work into the reporting folder as it goes.

```
/report on|off           opt this session in or out
/report level 0|1|2      0 prompt only; 1 nag when nothing lands; 2 revert
/report status           level, folder, and where things stand
/report folder <path>    where the write-up goes (default: ./report)
/report reset            back to defaults
```

Level 1 tracks whether anything in the reporting folder changed on disk since
the last check — a write, a `bash` redirect, an edit in another window, all
count the same way — and nags once per turn past a threshold. Level 2
escalates: the turn that ignored the requirement is reverted and the prompt
re-demanded, up to `maxReverts` times, then it falls back to level-1 nagging.

Config in `~/.pi/agent/pi-reactor-reporting.json`; the footer block reads
`¶ reporting` / `· low` (muted) / `· strict` (warning). Why enforcement is a filesystem probe rather than tool-call inspection:
[ADR-0023](../../docs/adr/0023-reporting-enforcement-is-a-filesystem-probe-not-a-heuristic.md);
the write-up structure lives in the `reactor-reporting` skill.
