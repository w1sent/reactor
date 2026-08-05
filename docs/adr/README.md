# Architecture decision records

One file per significant design decision, numbered in the order taken. Each
records what was decided, why, what it costs, and — in a closing "Considered and
rejected" section — the alternatives that were walked and why they lost.

Write the ADR before implementing, not after.

| # | Decision |
|---|---|
| [0001](0001-pi-is-the-only-target-harness.md) | pi is the only target harness |
| [0002](0002-package-ships-assets-tools-are-sibling-repos.md) | The pi package ships assets only; standalone tools are sibling repos |
| [0003](0003-tools-toml-single-source-of-truth.md) | `tools.toml` is the single source of truth, and it lives in `~/.pi/reactor/` |
| [0004](0004-config-updates-via-plain-diff.md) | Config updates are a plain `diff`, merged by hand |
| [0005](0005-reactor-cli-stdlib-python.md) | One `reactor` CLI, stdlib-only Python, and extensions shell out to it |
| [0006](0006-registry-injected-into-system-prompt.md) | The tool registry is injected into the system prompt, from a cached probe |
| [0007](0007-deactivation-is-soft.md) | Deactivation is soft: context management, not enforcement |
| [0008](0008-aggregate-upstream-skills.md) | REactor aggregates upstream skills; `--help` is the documentation |
| [0009](0009-scenarios-advance-by-tool-result.md) | Scenarios advance by tool result, not by marker or heuristic |
| [0010](0010-install-recipes-keyed-by-package-manager.md) | Install recipes are keyed by package manager, not by distro |
| [0011](0011-selector-edits-overrides-not-outcomes.md) | The selector edits overrides, not activation outcomes |
| [0012](0012-extensions-tested-through-pi-s-own-loader.md) | Extensions are tested through pi's own loader |
