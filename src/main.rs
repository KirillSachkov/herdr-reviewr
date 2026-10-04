use herdr_reviewr::actions::{self, NonUiRun};

fn main() -> anyhow::Result<()> {
    // `NonUiRun` decides both dispatch and the actions' read of which panes run the UI.
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
