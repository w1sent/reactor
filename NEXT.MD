# NEXT — filesystem panels (work in progress)

Temporary hand-off file. Delete it when the list below is done; the lasting
documentation belongs in `crates/reactor-gui/SPEC.md` and an ADR.

## The request

Several panels around the filesystem, all openable from the Files panel, and
from filesystem references clicked in the transcript. Each viewer supports
multiple instances (like the console). Delivery is phased, one commit per panel.

## Done (on branch `migrate-to-rig`'s successor work, already committed)

- `a03cc62` gpui-kit 0.7.0 (Root draws notification/dialog layers itself).
- `7f1894d` **`reactor-fs`** crate (no gpui): `Vfs` trait (`read_dir`, `stat`,
  `open` -> `ReadAt`, optional `local_path`, mutations default to unsupported),
  `VPath`, `FileRef`, `LocalFs`, `hash_file` (MD5/SHA-1/SHA-256/CRC-32),
  `identify`/`identify_file` (magic via `infer`, `file(1)` when installed),
  `search_names`, `search_content`, `find_bytes`, `parse_hex`.
- `0df4878` Foundation + **Hex viewer**: `files::Viewer` enum, `files::suggest`,
  `ReactorApp::open_file(viewer, FileRef, ..)`, `ReactorApp::resolve_file(text)`
  (relative/in-workdir -> `workdir_fs`, else `host_fs`), `/open [hex] <path>`.
- `12a1670` **Files panel** (`files/explorer.rs`, `files/props.rs`): lazy tree
  relative to the workdir, name (glob/substring) and content (regex) search,
  context menu (open, open in viewer, open externally, show in file manager,
  copy path, properties with file/magic + metadata, hashes, new file/folder,
  rename, delete, refresh). `PanelKind::Files`, in the Default and Analysis presets.

## Still to do

1. **Text editor** (`files/editor.rs`) — was just starting. Findings:
   - `gpui_kit::component::input::EditorState::new(window, cx).language("rust")
     .line_number(true).folding(true)`; `Editor::new(&state)`;
     `state.set_highlighter(name, cx)` switches language; `searchable(true)`.
   - Bundle every grammar: enable gpui-kit feature `tree-sitter-languages` in
     `crates/reactor-gui/Cargo.toml` (native C grammars; check it builds).
   - Language detection: pure fn `detect_language(file_name, first_line)` by
     extension, special names (Makefile, CMakeLists.txt), shebang; unit-tested.
     Manual override: dropdown in a status bar listing
     `gpui_kit::component::highlighter::Language::all()` (see the
     `dropdown_menu` use in `app.rs` ~l.2158).
   - Add `Vfs::write` (default unsupported; `LocalFs` implements). Save with
     Ctrl/Cmd+S (action + key context), dirty marker in the title, confirm on
     close when dirty, read-only when the Vfs is not writable.
   - Non-UTF-8 files: offer the hex viewer rather than lossy-decoding.
   - Add a `line: Option<u32>` to `open_file` so content-search hits (and
     transcript `path:line`) jump to the line; Files results already know it.
   - Fonts via `ui.text(Slot::Mono, ..)`.
2. **Table viewer** (`files/table.rs`) on gpui-kit's data-table: CSV/TSV/etc.
   (delimiter detection + manual choice), sorting, search, user-set number of
   header rows. Keep a source-agnostic table model so Excel etc. can be added
   later (deferred). Multiple instances.
3. **Data viewer** (`files/data.rs`): generic, panel *and* popup (dialog); images
   first via gpui-kit 0.7.0's image component (https://gpui-kit.com/component/image/),
   fed from in-memory bytes so it works for virtual filesystems. Multiple instances.
4. **`Viewer::suggest`** should use the extension and `identify` (text -> editor,
   csv/tsv -> table, image -> data viewer, else hex); extend `Viewer::ALL`.
5. **Transcript references**: make file paths (and `path:line`) in the transcript
   clickable, opening through `resolve_file` + `open_file`, with a way to pick
   another viewer (e.g. right-click menu).
6. **Hex viewer follow-ups** (deliberately simple in v1): go-to offset, selection,
   copy, data inspector, bookmarks.
7. **Explorer follow-ups**: scope content search to the selected folder, open
   with Enter, results context menu, async directory loading for slow Vfs
   backends, mounting a forensic image as another `Vfs`.
8. **Docs/cleanup**: SPEC.md section for the new panels (§6), an ADR for
   `reactor-fs`/`Vfs` as the seam, README crate list (`reactor-fs`), `/panel`
   and `/open` in docs, a `howto` if useful. Run `cargo fmt`, clippy, tests.

## Notes

- No GUI screenshots (see memory); the user checks visually. None of the new
  panels has been looked at in a running window yet — expect layout polish.
- Icons: the full Lucide catalogue is available (`Trash`, not `Trash2`).
- Context-menu submenus need a `Context<PopupMenu>`, which the tree's menu
  callback does not give, so the explorer uses flat items.
