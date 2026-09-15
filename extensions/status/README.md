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

## Reading it

Every service is one block — state glyph, service id, state words — and the
blocks are separated by a dim middot. The glyph and the words carry the
*state* colour, so a problem is spottable without reading anything: green
`●` up, red `✗` down, dim `?` unknown (probe did not answer) and dim `○`
not installed. An absence is not a fault, so neither of the dim states
sounds an alarm. Down services sort first; they are the ones that mean "do
something".

A service's *id* is coloured from a fixed rotation of theme colours that
never carry state meaning (`accent`, `mdLink`, `thinkingHigh`,
`thinkingXhigh`, `syntaxType`), handed out by sorted id. `bn` is the same
colour in the footer and the panel, `adb` is always a different one, and no
service's name is ever green, red or yellow — those colours mean exactly one
thing here.

```
🛠 3/4 tools · ✗ refusing down · ● answering 2 devices   footer: everything fits
· ✗ refusing · ● answering                             too narrow for the details
· 1 down · 3 up                                        too narrow for the names
```

While the tool count — the line's anchor, whose key sorts first — is on the
line, the whole line leads with the dim `·`, so the anchor and the services
read as blocks of one line rather than two strings pi joined with a space.
When the toolbox is off, nothing leads and no separator dangles.

When the blocks do not fit the terminal width (less a fixed allowance for
the other extensions' entries that share the line), the footer sheds detail
in three steps — full blocks, names only, counts — never truncating a
number into a different number. The width is re-read every refresh, so a
resize lands with the next turn.

The panel wraps instead of shedding: a row too wide for its window drops
its label column first, then stacks label and state words onto continuation
lines indented under the service id.

```
  services
  ● answering    2 devices
  ✗ bn           BN session  down
  ○ uninstalled              not installed
```

`mute` writes `hiddenServices` into `<agent dir>/reactor.json` and takes
effect on the very next refresh — no reload needed. Refreshes happen on
`session_start` and once per turn, never on a timer. Colours come from
`ctx.ui.theme`, re-read per refresh, so a `/theme` switch shows up the same
way.

Why per turn rather than a timer, and why the panel needs a terminal:
[ADR-0014](../../docs/adr/0014-extensions-share-the-cache-not-each-other.md)
and the [extensions README](../README.md).