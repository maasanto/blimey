//! Embeds the built web UI (`ui/dist`) in the binary. Without it the binary still builds and
//! serves a page saying how to build the UI, so Rust-only work never needs Node.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const BUILD_COMMAND: &str = "pnpm --dir ui install && pnpm --dir ui build";

fn main() {
    let ui = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../ui");
    let dist = ui.join("dist");
    let sources = [ui.join("src"), ui.join("index.html")];
    for watched in sources.iter().chain([&dist]) {
        println!("cargo::rerun-if-changed={}", watched.display());
    }

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mut files = Vec::new();
    collect(&dist, "", &mut files);
    if files.is_empty() {
        println!(
            "cargo::warning=ui/dist is missing: the binary serves a placeholder page. Run `{BUILD_COMMAND}`, then build again."
        );
        let placeholder = out.join("placeholder.html");
        fs::write(
            &placeholder,
            format!(
                "<!doctype html><title>Blimey</title><p>This binary was built without its web UI. Run <code>{BUILD_COMMAND}</code>, then <code>cargo build</code>.</p>"
            ),
        )
        .unwrap();
        files.push(("index.html".into(), placeholder));
    } else if sources.iter().map(|source| newest(source)).max() > Some(newest(&dist)) {
        println!(
            "cargo::warning=ui/dist is older than ui/src: the binary embeds a stale UI. Run `{BUILD_COMMAND}`, then build again."
        );
    }

    let entries: String = files
        .iter()
        .map(|(name, path)| format!("    ({name:?}, include_bytes!({path:?})),\n"))
        .collect();
    fs::write(
        out.join("ui_files.rs"),
        format!("static UI_FILES: &[(&str, &[u8])] = &[\n{entries}];\n"),
    )
    .unwrap();
}

/// Every file under `dir`, as (path relative to `ui/dist` with `/` separators, full path).
fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        let name = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
        if path.is_dir() {
            collect(&path, &format!("{name}/"), files);
        } else {
            files.push((name, path));
        }
    }
}

/// The latest modification time of `path` or anything under it.
fn newest(path: &Path) -> SystemTime {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .map(|entry| newest(&entry.unwrap().path()))
        .fold(modified, SystemTime::max)
}
