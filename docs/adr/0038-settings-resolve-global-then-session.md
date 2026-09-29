# Settings resolve global default, then session override

One resolution rule, for every setting REactor holds:

```
~/.reactor/settings.json   ->   session override in the store   ->   effective
     (the CLI writes this)      (the GUI writes this by default)
```

A session override wins where present; absent means inherit. This covers tool
and toolset activation, context mode and its knobs
([ADR-0037](0037-context-reduction-is-one-budget-manager.md)), identity, the
agent model and the summarizer model.

## Why

**Every setting wanted this, separately, until they were counted together.**
Activation should default per machine and be adjustable per investigation.
Context mode should default per machine and be forceable for one session full
of dumps. Identity is per session by nature but wants a default. Building the
cascade once is smaller than building three ad-hoc versions of it and
explaining which settings behave which way.

**The CLI has no session, and that decides who writes what.** An outside
harness consuming the portable surface
([ADR-0035](0035-portable-surface-is-machine-facts.md)) has no session
identity to scope anything to, so `reactor tools enable` writes the global
default — which is also the behaviour it has today. The GUI, which does have a
session, writes session scope by default and needs an explicit action to
promote a choice to the default.

**Per-extension config files were right for pi and are wrong here.**
[ADR-0016](0016-extension-toggles-live-in-their-own-pi-side-file.md) chose one
small file per extension because pi loads extensions in isolation, they share
no state, and that is pi's own convention. With no extension loader
([ADR-0033](0033-reactor-is-a-rust-project-on-rig.md)) the isolation that
justified the split is gone, and what remains is four files that have to be
read in a fixed order to answer one question.

## Consequences

- **`reactor.json`, `pi-rolling-context.json` and `pi-auto-continue.json` do
  not carry over.** Their contents become keys in `settings.json`. The pi
  flavor keeps its own files unchanged — those extensions are frozen, not
  ported ([ADR-0035](0035-portable-surface-is-machine-facts.md)).
- **`hiddenServices` survives as a key**, and stops being a file three
  separate extensions each parse for themselves.
- **The GUI needs a visible scope.** A setting changed in a session must show
  whether it applied to this session or to the default, or the cascade becomes
  the confusing kind of magic. "Make this the default" is an explicit action.
- **Session overrides live in the session store**
  ([ADR-0036](0036-reactor-owns-its-session-store-format.md)), not in a
  parallel directory. Forking a session forks its overrides with it, which is
  the behaviour a fork should have.
- **Resolution is a pure function of two inputs**, and is tested as one. No
  setting is allowed a bespoke lookup path.

## Considered and rejected

- **One file per feature, as [ADR-0016](0016-extension-toggles-live-in-their-own-pi-side-file.md)
  chose.** Rejected: its premise was extension isolation, which no longer
  exists.
- **Session-scoped only.** Rejected: every new session would start from
  nothing, and the CLI would have nowhere to write.
- **Global only.** Rejected: it is today's behaviour, and the concrete
  complaint against it is two windows on two targets wanting different
  toolsets.
- **Three levels — global, project, session.** Rejected as speculative: no
  setting has yet wanted to key off the working directory, and the level can
  be inserted later precisely because resolution is one function.
