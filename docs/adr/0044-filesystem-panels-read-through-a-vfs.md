# The filesystem panels read through a `Vfs`

> **Amended by [ADR-0052](0052-the-window-is-a-rail-a-drawer-and-a-workspace.md):** the
> explorer is no longer a fixed panel of a side dock. It is one of the rail's panels, shown in
> the drawer, because the side dock had become six panels sharing one tab strip. The viewers
> are still created on demand as centre tabs.

The GUI gains a file explorer and five viewers (text editor, table, data viewer, hex
viewer, and the explorer's own properties popup). They are built on one seam so that the
day a forensic image's filesystem is mounted, none of them changes.

## Decisions

**`reactor-fs` is its own crate, with no gpui.** It holds `Vfs` (browse, stat, open for
random access, optional mutations), `VPath`, `FileRef`, `LocalFs`, hashing, identification
and search. It is tested without a window, and a future image reader implements `Vfs`
without depending on the GUI.

**Every viewer opens a `FileRef`, never a `PathBuf`.** A `FileRef` is a filesystem and a
path in it. Mutating operations default to *unsupported* on the trait; the explorer greys
out what the filesystem cannot do instead of hiding it. `local_path` is optional and is
the only way to hand a file to another program, so "open externally" is disabled for
anything not on disk.

**The explorer's tree is relative to the working directory.** `LocalFs` is rooted there,
so `/` is the workdir. A path outside it (a transcript reference to `/etc/hosts`) is a
`FileRef` on a second `LocalFs` rooted at `/`.

**Identification pipes the file's head to `file -b -`** when `file(1)` exists, so a file
that is only inside an image is described the same way a local one is; the built-in magic
numbers (`infer`) are the fallback, and also decide text versus binary for choosing a
viewer.

**Viewers are created on demand, any number of them, as centre tabs.** Only the explorer
is a fixed panel (`PanelKind::Files`). Which viewer opens a file is `files::suggest`
(text → editor, `csv`/`tsv` → table, image → data viewer, otherwise hex); the user can
always choose another.

**The editor bundles every tree-sitter grammar** (gpui-kit's `tree-sitter-languages`),
detects the language from the name and `#!` line, and lets the user override it. Saving is
`Vfs::write`.

**The table viewer's grid is a `Sheet` list** behind a parser, so spreadsheet formats can
be added as another parser feeding the same panel.

**The data viewer is one view with two hosts**, a panel and a popup. gpui-kit 0.7.0 has no
image component, so images are decoded from bytes (`image` crate) into gpui's `RenderImage`
— which is what makes them work for files that exist only in a virtual filesystem.

**Filesystem references in the assistant's text are links.** A word is linked only when it
names an existing file; inline code that is a path becomes a link around the code span.
Click opens the suited viewer, Shift the hex viewer, Ctrl/Cmd the editor, Alt a popup.
References in tool output and commands are not linked.

## Consequences

- Reads go through `ReadAt`, on the UI thread for viewers (page-cached in the hex viewer).
  That is fine for local disks; a slow backend will need the viewers' loads moved onto the
  background executor.
- `LocalFs` follows symlinks that leave the root; it is a convenience view, not a sandbox.
- The tree-sitter grammars add native C code to the build, and pin `cc` to 1.2.x through
  `tree-sitter-sequel`.
