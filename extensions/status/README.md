# status

What is actually running, right now, in the footer and — when you want the
detail — a panel above the editor. Reads `reactor services`, which runs the
catalogue's service probes (the tools that talk to something live: `bn`,
`adb`).

```
/reactor-status              show the panel (refreshing it)
/reactor-status refresh      re-probe now
/reactor-status hide         hide the panel (footer stays)
/reactor-status mute <id>    leave a service out of footer and panel
/reactor-status unmute <id>  bring it back
```

`mute` writes `hiddenServices` into `<agent dir>/reactor.json` and takes
effect on the very next refresh — no reload needed. Refreshes happen on
`session_start` and once per turn, never on a timer.

Why per turn rather than a timer, and why the panel needs a terminal:
[ADR-0014](../../docs/adr/0014-extensions-share-the-cache-not-each-other.md)
and the [extensions README](../README.md).
