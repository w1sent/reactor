# Install recipes are keyed by package manager, not by distro

An install recipe's key names the **package manager** that runs it — `pacman`,
`apt`, `dnf`, `brew`, `uv`, `pipx`, `cargo`, `npm` — rather than the distribution
it is expected on (`arch`, `debian`, `macos`).

```toml
[tool.jadx.install]
pacman = "pacman -S jadx"
apt    = "apt install jadx"
brew   = "brew install jadx"
manual = "https://github.com/skylot/jadx/releases"
```

## Why the manager and not the distro

**The key becomes a testable predicate.** A manager key is selectable exactly
when its binary is on `PATH`, which is the same `shutil.which` check REactor
already performs to detect tools. A distro key is not: `arch = "pacman -S rr"`
requires parsing `/etc/os-release`, and having decided the machine is Arch you
still have to hope `pacman` is actually reachable — which it is not inside a
container, a chroot, or a Nix-managed shell.

**Language-ecosystem managers have no distro to belong to.** `pip`, `uv`,
`pipx`, `cargo`, `npm` and `go` cut across every platform. Under distro keys they
either get invented pseudo-distros (`pip`, `cargo` as if they were operating
systems) or get duplicated into every distro key that could host them. The
catalogue already had both problems: `arch`/`debian`/`macos` sat beside `pip`/
`uv`/`pipx`/`cargo`/`npm` in the same table with no rule distinguishing them.

**It matches how the recipe is actually written.** The recipe string starts with
the manager's name. Keying it by anything else stores the same fact twice and
lets the two disagree.

## Selection

`[platform]` in the catalogue declares the managers REactor knows and the order
it prefers them in:

```toml
[platform]
prefer = ["pacman", "apt", "dnf", "brew", "uv", "pipx", "cargo", "npm", "pip"]

[platform.manager]
pacman = { binary = "pacman", os = "linux", sudo = true }
brew   = { binary = "brew",   os = "darwin" }
uv     = { binary = "uv" }
```

A recipe is a **candidate** when its key names a declared manager whose binary is
on `PATH` and whose `os` constraint matches the running platform. Candidates are
ranked by `prefer`; a declared manager absent from `prefer` ranks last, in
catalogue declaration order. The top candidate is the recommended recipe.

An install key that names **no declared manager** — `manual`, `linux`, a bare URL
— is a **note**: always shown, never selected, never executed. That one rule
covers every free-text case without special-casing `manual`, and it means
`reactor install` can only ever run a command whose manager it has verified.

Because `[platform]` lives in the catalogue and the catalogue is the user's file
([ADR-0003](0003-tools-toml-single-source-of-truth.md)), a user on a manager
REactor has never heard of adds four lines rather than filing a bug.

## Consequences

- No `/etc/os-release` parsing, no distro table, no distro-family aliasing
  (Ubuntu-is-Debian, Manjaro-is-Arch), and no failure mode where REactor
  correctly identifies the distro and then recommends a command that is not
  installed.
- Preference is expressed once, globally, in the file the user owns. "I use `uv`,
  never `pip`" is one line, not an edit to every entry.
- `sudo` is recorded per manager rather than baked into recipe strings, so the
  recipes stay copy-pasteable and `reactor install` can prefix consistently.
- Recipes shown to the user list *all* candidates plus the notes, not just the
  winner. The ranking picks a default; it does not hide the alternatives.
- The catalogue's existing `arch`/`debian`/`macos` keys had to be renamed. Done
  as part of the same change, before either file was seeded anywhere.

## Considered and rejected

- **Distro keys with `/etc/os-release` detection** — rejected; see above. The
  detection is more code, less reliable, and still does not answer the question
  that matters ("can this command run here?").
- **Both, with a precedence rule** (distro key wins, fall back to manager key) —
  rejected. Two namespaces in one table, a precedence rule to remember, and the
  distro half still carries every problem it has on its own.
- **A fixed manager table in the CLI's source** — rejected; it makes adding a
  manager a code change in a project whose stated property is that adding a tool
  is a TOML block, and it puts the preference order somewhere the user cannot
  reach.
- **Per-tool declaration order as the only preference signal** — rejected as the
  *primary* rule; it expresses the catalogue author's preference, not the
  machine owner's, and would have to be re-edited per entry. Kept as the
  tiebreak.
- **Probing the manager for whether it can supply the package** (`pacman -Si`,
  `brew info`) — rejected for v1; it is a network or database round trip per
  candidate per tool, to refine a recommendation the user is about to read
  anyway.
