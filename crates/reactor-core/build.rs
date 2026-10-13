//! Lists `prompts/scenarios/` so the library can carry the scenarios the way it carries
//! `tools.toml`: `reactor setup` seeds them from the binary, which has no checkout to read
//! them from. The table is generated, so a scenario added to the directory ships without a
//! second list to keep in step.

use std::fs;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let path = e.path();
        if path.is_dir() {
            collect(&path, &rel, out);
        } else if path.is_file() {
            out.push((rel, path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest.join("../../prompts/scenarios");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut files = Vec::new();
    collect(&root, "", &mut files);

    let mut src = String::from("&[\n");
    for (rel, path) in &files {
        let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
        src.push_str(&format!(
            "    ({rel:?}, include_bytes!({:?}) as &[u8]),\n",
            abs.to_string_lossy()
        ));
    }
    src.push_str("]\n");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    fs::write(out.join("scenarios.rs"), src).expect("write the scenario table");
}
