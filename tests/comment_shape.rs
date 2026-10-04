//! Comments stay short: a run of comment lines is two at most.

use std::fs;
use std::path::Path;

#[test]
fn no_comment_runs_past_two_lines() {
    let mut long = Vec::new();
    for dir in ["src", "tests", "examples"] {
        walk(Path::new(env!("CARGO_MANIFEST_DIR")).join(dir).as_path(), &mut long);
    }
    assert!(long.is_empty(), "comment runs past two lines, rewrite them:\n{}", long.join("\n"));
}

fn walk(dir: &Path, long: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, long);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).unwrap();
            let mut run = 0;
            for (i, line) in text.lines().enumerate() {
                run = if line.trim_start().starts_with("//") { run + 1 } else { 0 };
                if run == 3 {
                    long.push(format!("{}:{}", path.display(), i - 1));
                }
            }
        }
    }
}
