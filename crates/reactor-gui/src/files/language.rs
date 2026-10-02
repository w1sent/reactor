//! Which grammar highlights a file: by name, by extension, by a `#!` line. The names are the
//! ones gpui-kit's highlighter registry knows.

/// The grammar for a file called `file_name` whose first line is `first_line`; `"text"` when
/// nothing recognises it.
pub fn detect(file_name: &str, first_line: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    match lower.as_str() {
        "makefile" | "gnumakefile" | "bsdmakefile" => return "make",
        "cmakelists.txt" => return "cmake",
        "dockerfile" | ".bashrc" | ".bash_profile" | ".profile" | ".zshrc" | "pkgbuild" => {
            return "bash";
        }
        "cargo.lock" | "pipfile" | "poetry.lock" => return "toml",
        ".babelrc" | ".eslintrc" | "tsconfig.json" => return "json",
        _ => {}
    }
    if let Some((_, ext)) = lower.rsplit_once('.')
        && let Some(language) = by_extension(ext)
    {
        return language;
    }
    by_shebang(first_line).unwrap_or("text")
}

fn by_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "py" | "pyw" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" | "json5" | "geojson" => "json",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "inl" => "cpp",
        "cs" => "csharp",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "swift" => "swift",
        "rb" | "rake" | "gemspec" => "ruby",
        "php" | "phtml" => "php",
        "lua" => "lua",
        "zig" => "zig",
        "ex" | "exs" => "elixir",
        "sh" | "bash" | "zsh" | "ksh" | "fish" => "bash",
        "html" | "htm" | "xhtml" => "html",
        "css" | "scss" => "css",
        "md" | "markdown" | "mdx" => "markdown",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "sql" => "sql",
        "diff" | "patch" => "diff",
        "proto" => "proto",
        "graphql" | "gql" => "graphql",
        "svelte" => "svelte",
        "astro" => "astro",
        "ejs" => "ejs",
        "erb" => "erb",
        "cmake" => "cmake",
        "mk" | "mak" => "make",
        _ => return None,
    })
}

fn by_shebang(first_line: &str) -> Option<&'static str> {
    let line = first_line.strip_prefix("#!")?;
    let mut words = line.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        // `env -S python3 -u`: the program is the first word that is not a flag.
        program = words.find(|w| !w.starts_with('-'))?;
    }
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    Some(match program {
        "sh" | "bash" | "zsh" | "ksh" | "dash" | "ash" => "bash",
        "python" | "pypy" => "python",
        "node" | "nodejs" | "deno" | "bun" => "javascript",
        "ruby" => "ruby",
        "php" => "php",
        "lua" | "luajit" => "lua",
        _ => return None,
    })
}

/// Every grammar the user can choose by hand.
pub fn all() -> Vec<&'static str> {
    use gpui_kit::component::highlighter::Language;
    let mut names: Vec<&'static str> = Language::all()
        .map(|l| l.name())
        .filter(|name| *name != "markdown_inline")
        .collect();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_extension_and_name() {
        assert_eq!(detect("main.rs", ""), "rust");
        assert_eq!(detect("App.TSX", ""), "tsx");
        assert_eq!(detect("Makefile", ""), "make");
        assert_eq!(detect("CMakeLists.txt", ""), "cmake");
        assert_eq!(detect("notes.txt", ""), "text");
        assert_eq!(detect("archive.tar.gz", ""), "text");
    }

    #[test]
    fn shebang_decides_when_the_name_does_not() {
        assert_eq!(detect("run", "#!/usr/bin/env python3"), "python");
        assert_eq!(detect("run", "#!/bin/bash -e"), "bash");
        assert_eq!(detect("run", "#!/usr/bin/env -S node --flag"), "javascript");
        assert_eq!(detect("run", "#!/usr/bin/perl"), "text");
        assert_eq!(detect("run", "just text"), "text");
    }

    #[test]
    fn the_extension_beats_the_shebang() {
        assert_eq!(detect("tool.py", "#!/bin/sh"), "python");
    }

    #[test]
    fn every_detected_grammar_is_selectable() {
        let all = all();
        for name in [
            "rust", "python", "bash", "json", "text", "markdown", "cpp", "toml",
        ] {
            assert!(all.contains(&name), "{name} missing");
        }
    }
}
