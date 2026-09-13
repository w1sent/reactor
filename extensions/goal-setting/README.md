# goal-setting

The session manifest: the user's goal, session guidelines, and steps the agent
maintains itself — kept in the system prompt so it survives pi's compaction,
rolling-context's fade, and everything else that touches old messages.

```
/goal <text>            set the goal (this is what activates the steps tool)
/goal clear             remove the goal again
/guidelines <text>      session-specific rules, into the system prompt
/guidelines clear       remove the guidelines
/frame                  view the manifest: goal, guidelines, steps, switch
/manifest [on|off]      pause or resume the whole extension -- the block and
                       update_steps go quiet, the data is preserved
/manifest clear         reset: goal, guidelines and steps all gone
/derive [all|goal|
         guidelines|
         steps]         derive the manifest from this session with a direct
                       model call -- no chat message, no agent loop, no tools.
                       The result is written straight into the manifest;
                       /frame reviews it. Scoped subcommands apply only their
                       own part.
```

`update_steps` is the agent's own tool for overwriting the step list: each
step a conceptual summary with a 3-word status, clamped to a length limit,
with a warning once the count exceeds the soft limit. It is inactive until a
goal is set *and* the switch is on — in a fresh session the tool simply is not
usable.

The manifest block is injected on content only: no goal, no block, and an
untouched session's system prompt stays byte-identical. Config (soft limit,
clamp lengths, the derive call's session-tail character budget) lives in
`~/.pi/agent/pi-goal-setting.json`. Why the manifest is its own extension
rather than part of the fade:
[ADR-0024](../../docs/adr/0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md).

`/derive` hands the model a serialized tail of the session and a JSON shape to
fill; it never enters the session, never starts an agent turn, and cannot call
tools. The call goes through pi's own model registry, so auth — including
custom providers from `models.json` — is resolved by pi itself. A response
that is not the requested JSON applies nothing.
