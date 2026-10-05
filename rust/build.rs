// SPDX-License-Identifier: GPL-3.0-or-later
//! Embed rust/system: the static NixOS modules copied into every installed
//! /etc/nixos/calamares, so the GUI, helper and installed flake agree.
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy();
        // Build output and editor state are never part of the module tree.
        if name.starts_with('.') || name == "target" || name == "result" {
            continue;
        }
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn main() {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("system");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut out = String::from("pub const SYSTEM_FILES: &[(&str, &[u8])] = &[\n");
    for path in files {
        let relative = path.strip_prefix(&root).unwrap().to_str().unwrap();
        out.push_str(&format!(
            "    ({relative:?}, include_bytes!({:?})),\n",
            path.to_str().unwrap()
        ));
    }
    out.push_str("];\n");
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("system_files.rs"),
        out,
    )
    .unwrap();
}
