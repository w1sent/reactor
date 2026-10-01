# reactor-gui

The native REactor frontend (gpui-kit). It hosts the Rust agent (`reactor-agent`)
in its own process — no child process, no wire protocol
([ADR-0042](../../docs/adr/0042-the-gui-hosts-the-agent-in-process.md)).
[`SPEC.md`](SPEC.md) is the specification.

```bash
cargo build -p reactor-gui        # not a default workspace member (native deps)
cargo run -p reactor-gui -- <workdir>       # new session
cargo run -p reactor-gui -- -c <workdir>    # continue the newest one
cargo run -p reactor-gui -- -r <workdir>    # pick a session
```

It needs a native windowing stack at build time: fontconfig, xkbcommon and
Vulkan/Wayland/X11 development packages. `REACTOR_GUI_CLIENT=cli` makes the
catalogue panels use the `reactor` binary instead of the library (a debug
fallback).

The catalogue panels talk to `reactor-core` through `reactor-client`
(`LibClient` in-process, `CliClient` as the fallback).

## Icon

`assets/icon.ico` / `icon.png` at the repo root are the application icon. The window
gets it from `chrome::window_options` (X11), the Windows `.exe` embeds it (`build.rs`), a
macOS `.app` takes `assets/icon.icns` (`cargo bundle`, metadata in `Cargo.toml`). On Linux,
Wayland and launchers read it from a desktop entry, which `cargo install` does not copy:
`reactor setup` installs it (and the icon) under `~/.local/share` once `reactor-gui` is
installed; `--no-launcher` skips that.
