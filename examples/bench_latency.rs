//! Component attribution for the perceived-latency work.

use std::path::PathBuf;
use std::time::Instant;

use herdr_reviewr::diff::DiffCache;
use herdr_reviewr::git;
use herdr_reviewr::highlight::Highlighter;
use herdr_reviewr::theme;

fn ms(f: impl FnOnce()) -> f64 {
    let t = Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

/// Run `f` `n` times, return (first, min, median) in ms.
fn sample(n: usize, mut f: impl FnMut()) -> (f64, f64, f64) {
    let mut times: Vec<f64> = (0..n).map(|_| ms(&mut f)).collect();
    let first = times[0];
    times.sort_by(f64::total_cmp);
    (first, times[0], times[times.len() / 2])
}

fn row(name: &str, (first, min, med): (f64, f64, f64)) {
    println!("{name:<46} first {first:>8.1}ms   min {min:>8.1}ms   median {med:>8.1}ms");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let repo = PathBuf::from(args.next().expect("usage: bench_latency <repo> [label]"));
    let label = args.next().unwrap_or_else(|| repo.display().to_string());
    assert!(git::is_repo(&repo), "not a git repo: {}", repo.display());
    let hl = Highlighter::new(theme::resolve(None).syntax);
    println!("== {label} ==");

    // --- Components of reload() -------------------------------------------------
    let changed = git::changed_from(&repo, &git::diff_base(&repo)).unwrap();
    row(
        "changed_from (uncommitted)",
        sample(5, || {
            git::changed_from(&repo, &git::diff_base(&repo)).unwrap();
        }),
    );
    row(
        "changed_from (branch, incl. resolve)",
        sample(5, || {
            let base = git::resolve_base(&repo, None).ok().and_then(|r| r.status.winner);
            if let Some(base) = base.and_then(|b| git::merge_base(&repo, b.oid())) {
                git::changed_from(&repo, &base).unwrap();
            }
        }),
    );
    let all = git::all_files(&repo).unwrap();
    row(
        "all_files (ls-files+untracked+status --ignored)",
        sample(5, || {
            git::all_files(&repo).unwrap();
        }),
    );
    row(
        "snapshot_worktree (poll during turn)",
        sample(3, || {
            git::snapshot_worktree(&repo).unwrap();
        }),
    );

    // --- File opens -------------------------------------------------------------
    // Pick representative text files by on-disk size: the median, and the largest
    // comfortably under the 2 MB diff byte budget so the open exercises a full
    // highlight instead of the too-large notice.
    let mut sized: Vec<(u64, String)> = all
        .iter()
        .filter(|e| !e.is_dir && !e.ignored)
        .filter_map(|e| {
            let m = std::fs::metadata(repo.join(&e.path)).ok()?;
            let bytes = std::fs::read(repo.join(&e.path)).ok()?;
            if bytes.contains(&0) {
                return None; // binary
            }
            Some((m.len(), e.path.clone()))
        })
        .collect();
    sized.sort();
    if sized.is_empty() {
        println!("no text files; skipping file opens");
        return;
    }
    let median_file = sized[sized.len() / 2].1.clone();
    let large_file = sized
        .iter()
        .rev()
        .find(|(s, _)| *s < 1_000_000)
        .map_or_else(|| median_file.clone(), |(_, p)| p.clone());
    let large_kb = sized.iter().find(|(_, p)| *p == large_file).unwrap().0 / 1024;

    // All files tab: set_file_view = fs read + highlight (cold), cache hit (warm).
    for (tag, path) in [("median", &median_file), (&format!("large {large_kb}KB"), &large_file)] {
        let content = std::fs::read_to_string(repo.join(path)).unwrap_or_default();
        row(
            &format!("file open, All files COLD ({tag})"),
            sample(3, || {
                let mut cache = DiffCache::new(); // cold: fresh cache each run
                let c = std::fs::read_to_string(repo.join(path)).unwrap_or_default();
                cache.get_file(path.clone(), &c, &hl);
            }),
        );
        let mut warm = DiffCache::new();
        warm.get_file(path.clone(), &content, &hl);
        row(
            &format!("file open, All files WARM ({tag})"),
            sample(5, || {
                let c = std::fs::read_to_string(repo.join(path)).unwrap_or_default();
                warm.get_file(path.clone(), &c, &hl);
            }),
        );
    }

    // Changes tab: set_diff = git show HEAD:path + fs read + two-side highlight + diff.
    if let Some(cf) = changed.first() {
        let path = cf.path.clone();
        row(
            &format!("diff open, Changes COLD ({path})"),
            sample(3, || {
                let mut cache = DiffCache::new();
                let old = git::file_content(&repo, "HEAD", &path);
                let new = std::fs::read_to_string(repo.join(&path)).unwrap_or_default();
                cache.get(path.clone(), None, &old, &new, &hl);
            }),
        );
        let mut warm = DiffCache::new();
        let old0 = git::file_content(&repo, "HEAD", &path);
        let new0 = std::fs::read_to_string(repo.join(&path)).unwrap_or_default();
        warm.get(path.clone(), None, &old0, &new0, &hl);
        row(
            "diff open, Changes WARM (same file re-poll)",
            sample(5, || {
                let old = git::file_content(&repo, "HEAD", &path);
                let new = std::fs::read_to_string(repo.join(&path)).unwrap_or_default();
                warm.get(path.clone(), None, &old, &new, &hl);
            }),
        );
    } else {
        // No uncommitted changes: still time the git-show side against the median file.
        row(
            "diff open sides only (git show, clean repo)",
            sample(5, || {
                git::file_content(&repo, "HEAD", &median_file);
            }),
        );
    }

    // --- Composite: one All-files reload.
    row(
        "TAB SWITCH -> All files (reload, no reopen)",
        sample(3, || {
            git::changed_from(&repo, &git::diff_base(&repo)).unwrap();
            git::all_files(&repo).unwrap();
        }),
    );
    println!();
}
