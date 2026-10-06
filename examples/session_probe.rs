//! Own fork: print what the `Session` tab loads for a pane, e.g.
//! `HERDR_PANE_ID=<ws>:pX HERDR_TAB_ID=<tab> cargo run --example session_probe -- <repo>`.

fn main() {
    let repo = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let started = std::time::Instant::now();
    let view = herdr_reviewr::session::load(std::path::Path::new(&repo));
    println!("loaded in {:?}", started.elapsed());
    println!("agent: {:?}", view.agent);
    println!("{}", view.need());
    for a in &view.artifacts {
        println!("{:?} {:?} {} {}", a.tier, a.letter, a.key, a.note.as_deref().unwrap_or(""));
    }
    if std::env::args().any(|a| a == "--summary") {
        let started = std::time::Instant::now();
        let mut view = view.clone();
        view.summary = Some(herdr_reviewr::session::summarize(&view).expect("summary"));
        println!("summary in {:?}", started.elapsed());
        println!("---\n{}", view.text(herdr_reviewr::session::SUMMARY).unwrap_or_default());
    } else {
        println!("---\n{}", view.text(herdr_reviewr::session::SUMMARY).unwrap_or_default());
    }
}
