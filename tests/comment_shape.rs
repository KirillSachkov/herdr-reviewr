//! Comments stay short: a run of comment lines is two at most, in code, scripts, and configs.

use std::fs;
use std::path::Path;

#[test]
fn no_comment_runs_past_two_lines() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut long = Vec::new();
    for dir in ["src", "tests", "examples", "scripts", "herdr", ".github"] {
        walk(&root.join(dir), &mut long);
    }
    for file in ["Cargo.toml", "deny.toml", "herdr-plugin.toml", "justfile"] {
        check(&root.join(file), "#", &mut long);
    }
    assert!(long.is_empty(), "comment runs past two lines, rewrite them:\n{}", long.join("\n"));
}

fn walk(dir: &Path, long: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, long);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        let script = path.file_name().is_some_and(|name| name == "vm");
        match ext {
            "rs" => check(&path, "//", long),
            "sh" | "ps1" | "yml" | "toml" | "py" => check(&path, "#", long),
            _ if script => check(&path, "#", long),
            _ => {}
        }
    }
}

/// Every third consecutive line starting with `mark`; a `#!` shebang is no comment.
fn check(path: &Path, mark: &str, long: &mut Vec<String>) {
    let text = fs::read_to_string(path).unwrap();
    let mut run = 0;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        run = if trimmed.starts_with(mark) && !trimmed.starts_with("#!") { run + 1 } else { 0 };
        if run == 3 {
            long.push(format!("{}:{}", path.display(), i - 1));
        }
    }
}
