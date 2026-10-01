//! Shell completion (ADR-0015).
//!
//! The scripts shell back into `reactor __complete <tools|toolsets>` for the
//! dynamic part (ids). That is deliberately not `--format json`: it prints bare,
//! newline-separated ids straight from the catalogue with no probing, so a
//! <TAB> press costs a TOML parse and nothing else.

pub const SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

pub fn script(shell: &str) -> Option<&'static str> {
    match shell {
        "bash" => Some(include_str!("../completions/reactor.bash")),
        "zsh" => Some(include_str!("../completions/reactor.zsh")),
        "fish" => Some(include_str!("../completions/reactor.fish")),
        _ => None,
    }
}
