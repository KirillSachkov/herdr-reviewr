use herdr_reviewr::actions::{self, NonUiRun};

fn main() -> anyhow::Result<()> {
    // One rule decides both halves of "a flag run is not the review UI": this dispatch, and
    // the plugin actions' read of which panes run the review UI (`actions::is_review_ui`).
    // Both go through `NonUiRun::from_args`, so a process started with a non-UI flag never
    // runs the review UI, and the actions never count it as a live reviewr pane.
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match NonUiRun::from_args(&args) {
        Some(NonUiRun::ResolvePluginConfig) => {
            if let Err(error) = herdr_reviewr::config::print_plugin_config() {
                eprintln!("reviewr: {error}");
                std::process::exit(1);
            }
            Ok(())
        }
        Some(NonUiRun::Action(name)) => std::process::exit(actions::run(name.as_deref())),
        None => herdr_reviewr::run(),
    }
}
