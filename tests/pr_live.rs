//! A manual live probe against real `gh`: `REVIEWR_LIVE_REPO=… cargo test --test pr_live -- --ignored`.

use herdr_reviewr::config::PluginConfig;
use herdr_reviewr::forge::{fetch, fetch_input};

#[test]
#[ignore = "live network; run with REVIEWR_LIVE_REPO set"]
fn resolve_one_live_worktree() {
    let repo = std::env::var("REVIEWR_LIVE_REPO").expect("set REVIEWR_LIVE_REPO");
    let repo = std::path::PathBuf::from(repo);
    let input = fetch_input(&repo, None, &PluginConfig::default()).expect("fetch input");
    eprintln!("input: {input:#?}");
    let view = fetch(&repo, &input);
    eprintln!("view: {view:#?}");
}
