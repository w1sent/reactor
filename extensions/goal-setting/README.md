# goal-setting

The session manifest: the user's goal, session guidelines, and steps the agent
maintains itself — kept in the system prompt so it survives pi's compaction,
rolling-context's fade, and everything else that touches old messages.

```
/goal <text>            set the goal (this is what activates the steps tool)
/guidelines <text>      session-specific rules, into the system prompt
/frame                  view the manifest: goal, guidelines, steps, switch
/manifest [on|off]      the switch for update_steps
```

`update_steps` is the agent's own tool for overwriting the step list: each
step a conceptual summary with a 3-word status, clamped to a length limit,
with a warning once the count exceeds the soft limit. It is inactive until a
goal is set *and* the switch is on — in a fresh session the tool simply is not
usable.

The manifest block is injected on content only: no goal, no block, and an
untouched session's system prompt stays byte-identical. Config (soft limit,
clamp lengths) lives in `~/.pi/agent/pi-goal-setting.json`. Why the manifest
is its own extension rather than part of the fade:
[ADR-0024](../../docs/adr/0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md).
