//! The plugin actions and the `auto-open` hook, run as `--action <name>`, one workspace lock each.

use std::env;
use std::ffi::OsStr;
use std::fs::{File, TryLockError};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::{PluginConfig, PluginConfigError, TogglePlacement};
use crate::herdr::{self, HerdrError, PaneList, Process, ProcessInfo};
use crate::logln;
use crate::proc::program_name;

/// A run that is not the review UI: dispatched by `main`, and never counted as a reviewr pane.
#[derive(Debug, PartialEq, Eq)]
pub enum NonUiRun {
    /// `--resolve-plugin-config`: print the normalized plugin config.
    ResolvePluginConfig,
    /// `--action <name>`. The name is `None` when the flag ends argv.
    Action(Option<String>),
}

impl NonUiRun {
    /// The non-UI run `args` asks for, anywhere in argv, or `None` for the review UI.
    pub fn from_args<S: AsRef<OsStr>>(args: &[S]) -> Option<Self> {
        let args: Vec<&OsStr> = args.iter().map(AsRef::as_ref).collect();
        if args.contains(&OsStr::new("--resolve-plugin-config")) {
            return Some(Self::ResolvePluginConfig);
        }
        let at = args.iter().position(|arg| *arg == "--action")?;
        Some(Self::Action(args.get(at + 1).map(|name| name.to_string_lossy().into_owned())))
    }
}

/// One plugin action. The manifest names each with the same spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Toggle,
    Open,
    Close,
    AutoOpen,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "toggle" => Some(Self::Toggle),
            "open" => Some(Self::Open),
            "close" => Some(Self::Close),
            "auto-open" => Some(Self::AutoOpen),
            _ => None,
        }
    }
}

/// Why an action stopped short.
#[derive(Debug)]
enum Stop {
    /// The plugin config is invalid; loud everywhere, so it reaches herdr's plugin log.
    Config(PluginConfigError),
    /// The action cannot proceed. Loud for an explicit action, silent for the event.
    Refused(String),
}

fn refused(why: impl Into<String>) -> Stop {
    Stop::Refused(why.into())
}

/// Run the action `name` and return the process exit code.
pub fn run(name: Option<&str>) -> i32 {
    crate::log::init();
    let Some(action) = name.and_then(Action::parse) else {
        eprintln!(
            "reviewr: unknown action '{}' (toggle | open | close | auto-open)",
            name.unwrap_or_default()
        );
        return 1;
    };
    match act(action) {
        // The event reports nothing, on success either.
        Ok(None) => 0,
        Ok(Some(line)) if action == Action::AutoOpen => {
            logln!("auto-open: {line}");
            0
        }
        Ok(Some(line)) => {
            println!("reviewr: {line}");
            0
        }
        Err(Stop::Config(error)) => {
            eprintln!("reviewr: {error}");
            1
        }
        Err(Stop::Refused(why)) if action == Action::AutoOpen => {
            logln!("auto-open refused: {why}");
            0
        }
        Err(Stop::Refused(why)) => {
            eprintln!("reviewr: {why}");
            1
        }
    }
}

/// One action, step by step. `Ok` holds the success line, if the action reports one.
fn act(action: Action) -> Result<Option<String>, Stop> {
    // The whole config validates before any workspace read or pane write.
    let config = crate::config::plugin_config_from_herdr().map_err(Stop::Config)?;

    #[cfg(unix)]
    repoint_launch_links();

    // Event policy gates only the event, before any read.
    let event = if action == Action::AutoOpen {
        if !config.auto_open()
            || !matches!(config.toggle_placement(), TogglePlacement::Split | TogglePlacement::Tab)
        {
            return Ok(None);
        }
        // Without a payload the only workspace in reach is the focused one, so the event refuses.
        let Some(json) = var("HERDR_PLUGIN_EVENT_JSON") else {
            return Err(refused("no event payload"));
        };
        let event: Value = serde_json::from_str(&json).unwrap_or_default();
        // `worktree.opened` on a live workspace is no birth: never resurrect a closed pane.
        if event.pointer("/data/already_open") == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        Some(event)
    } else {
        None
    };

    let target = Target::read(event)?;
    let ws = target.ws.as_str();
    // Held through the close or open, so a concurrent action sees this one's effect.
    let Some(_lock) = action_lock(action, ws)? else {
        return Ok(None);
    };

    // One listing serves the run; a failed one never reads as "no reviewr pane".
    let panes =
        PaneList::of(ws).map_err(|_| refused(format!("herdr pane list failed for {ws}")))?;
    let existing = reviewr_panes(&panes)
        .ok_or_else(|| refused(format!("herdr pane process-info failed in {ws}")))?;

    if !existing.is_empty() {
        return match action {
            Action::Close | Action::Toggle => close_all(&existing, ws).map(Some),
            Action::Open | Action::AutoOpen => {
                Ok(Some(format!("already open ({}) in {ws}", existing.join(" "))))
            }
        };
    }
    if action == Action::Close {
        return Ok(Some(format!("nothing open in {ws}")));
    }
    open(action, &config, &target, &panes).map(Some)
}

/// How long an explicit action waits for its workspace's lock: the slowest open, with room.
const LOCK_BOUND: Duration = Duration::from_secs(15);

/// The pause between two lock attempts while an explicit action waits.
const LOCK_POLL: Duration = Duration::from_millis(20);

/// Workspace `ws`'s action lock: an action waits up to [`LOCK_BOUND`], the event yields.
fn action_lock(action: Action, ws: &str) -> Result<Option<File>, Stop> {
    let Some(dir) = env::var_os("HERDR_PLUGIN_STATE_DIR").filter(|dir| !dir.is_empty()) else {
        return Err(refused("no plugin state dir (invoke as a herdr plugin action)"));
    };
    // Hex, so ids that differ only in case never share a lock file.
    let name = hex::encode(ws);
    let path = Path::new(&dir).join(format!("action-{name}.lock"));
    let unusable = |error| refused(format!("cannot lock {}: {error}", path.display()));
    // Windows locks need a handle with access; the content is never touched.
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(unusable)?;
    let deadline = Instant::now() + LOCK_BOUND;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if action == Action::AutoOpen => {
                logln!("auto-open yielded: another reviewr action holds {ws}'s lock");
                return Ok(None);
            }
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => thread::sleep(LOCK_POLL),
            Err(TryLockError::WouldBlock) => {
                return Err(refused(format!(
                    "another reviewr action in {ws} is still running after {LOCK_BOUND:?}"
                )));
            }
            Err(TryLockError::Error(error)) => return Err(unusable(error)),
        }
    }
}

/// A non-empty environment variable. herdr leaves context variables unset or empty alike.
fn var(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

/// The non-empty string at `pointer`; a missing or mistyped field reads as absent.
fn text(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(Value::as_str).filter(|text| !text.is_empty()).map(Into::into)
}

/// Where an action acts.
#[derive(Debug)]
struct Target {
    ws: String,
    /// The pane a split or zoomed open attaches to.
    pane: Option<String>,
    /// The launch cwd to review.
    cwd: Option<String>,
    /// The focused pane, whose live cwd a manual open prefers over `cwd`.
    focused: Option<String>,
}

impl Target {
    /// The event's target from its payload, else the explicit action's.
    fn read(event: Option<Value>) -> Result<Self, Stop> {
        let no_workspace = || refused("no workspace context (invoke from inside herdr)");
        // Target the event's fresh workspace, never a focused pane's cwd.
        if let Some(event) = event {
            return Ok(Self {
                ws: text(&event, "/data/workspace/workspace_id")
                    .or_else(|| text(&event, "/data/worktree/open_workspace_id"))
                    .ok_or_else(no_workspace)?,
                pane: None,
                cwd: text(&event, "/data/workspace/worktree/checkout_path")
                    .or_else(|| text(&event, "/data/worktree/path")),
                focused: None,
            });
        }
        let context: Value = var("HERDR_PLUGIN_CONTEXT_JSON")
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Ok(Self {
            ws: var("HERDR_WORKSPACE_ID").ok_or_else(no_workspace)?,
            pane: var("HERDR_PANE_ID"),
            cwd: text(&context, "/focused_pane_cwd").or_else(|| text(&context, "/workspace_cwd")),
            focused: text(&context, "/focused_pane_id"),
        })
    }
}

/// The workspace's reviewr panes, read concurrently; `None` when any read failed.
fn reviewr_panes(panes: &PaneList) -> Option<Vec<&str>> {
    thread::scope(|scope| {
        let probes: Vec<_> = panes
            .panes
            .iter()
            .map(|entry| {
                let pane = entry.pane_id.as_str();
                (pane, scope.spawn(move || runs_review_ui(pane)))
            })
            .collect();
        let mut existing = Vec::new();
        for (pane, probe) in probes {
            // A probe that panicked never settled, so it refuses like a failed read.
            match probe.join() {
                Ok(Ok(true)) => existing.push(pane),
                Ok(Ok(false)) => {}
                Ok(Err(_)) | Err(_) => return None,
            }
        }
        Some(existing)
    })
}

/// Whether pane `pane` runs the review UI; a pane gone since the list counts as closed.
fn runs_review_ui(pane: &str) -> Result<bool, HerdrError> {
    match ProcessInfo::of(pane) {
        Ok(info) => Ok(info.foreground_processes.iter().any(is_review_ui)),
        Err(error) if error.pane_gone() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether `process` is the review UI, by its executable name and a UI argv.
fn is_review_ui(process: &Process) -> bool {
    let argv = process.argv.as_slice();
    // Windows names ignore case, so `HERDR-REVIEWR.EXE` is the same program.
    let named = process
        .argv0
        .iter()
        .chain(argv.first())
        .any(|exe| program_name(exe).eq_ignore_ascii_case("herdr-reviewr"));
    named && NonUiRun::from_args(argv.get(1..).unwrap_or_default()).is_none()
}

/// Close every pane in `existing`; a pane already gone counts as closed.
fn close_all(existing: &[&str], ws: &str) -> Result<String, Stop> {
    let mut closed = Vec::new();
    let mut failed = Vec::new();
    for &pane in existing {
        match herdr::close_pane(pane) {
            Ok(()) => closed.push(pane),
            Err(error) if error.pane_gone() => closed.push(pane),
            Err(_) => failed.push(pane),
        }
    }
    if !failed.is_empty() {
        return Err(refused(format!("herdr pane close failed for {} in {ws}", failed.join(" "))));
    }
    Ok(format!("closed {} in {ws}", closed.join(" ")))
}

/// How long an open waits for its pane to read as reviewr: a cold Windows open, with room.
const VISIBLE_BOUND: Duration = Duration::from_secs(6);

/// The pause between two reads of the new pane while an open waits.
const VISIBLE_POLL: Duration = Duration::from_millis(50);

/// Open a reviewr pane in the target's workspace and return the success line.
fn open(
    action: Action,
    config: &PluginConfig,
    target: &Target,
    panes: &PaneList,
) -> Result<String, Stop> {
    let ws = target.ws.as_str();
    // The focused pane's live cwd wins over the launch cwd, when it is inside a repo.
    let live = target
        .focused
        .as_deref()
        .and_then(|focused| panes.pane(focused))
        .and_then(|entry| entry.foreground_cwd.clone());
    let cwd = match (&live, &target.cwd) {
        (Some(live), _) if has_worktree(live) => live,
        (_, Some(cwd)) if has_worktree(cwd) => cwd,
        // Name every rejected candidate.
        _ => {
            let live = live.map(|live| format!(" (live cwd '{live}')")).unwrap_or_default();
            let cwd = target.cwd.as_deref().unwrap_or("<no cwd>");
            return Err(refused(format!("not a git repo: '{cwd}'{live}")));
        }
    };

    let plugin = var("HERDR_PLUGIN_ID").unwrap_or_else(|| herdr::PLUGIN_ID.to_owned());
    let placement = config.toggle_placement();
    let mut spot = herdr::PaneOpen {
        plugin: &plugin,
        entrypoint: "pane",
        placement: placement.as_str(),
        target_pane: None,
        direction: None,
        workspace: None,
        cwd,
        // A manual open takes focus. The event never does.
        focus: action != Action::AutoOpen,
    };
    // A split or zoomed open attaches to the focused pane, else the workspace's first pane.
    match placement {
        TogglePlacement::Split | TogglePlacement::Zoomed => {
            spot.target_pane = Some(
                target
                    .pane
                    .as_deref()
                    .or_else(|| panes.panes.first().map(|entry| entry.pane_id.as_str()))
                    .ok_or_else(|| refused(format!("no pane to attach to in {ws}")))?,
            );
            if placement == TogglePlacement::Split {
                spot.direction = Some(config.toggle_direction().as_str());
            }
        }
        TogglePlacement::Tab => spot.workspace = Some(ws),
        TogglePlacement::Overlay => {}
    }

    let opened =
        herdr::open_plugin_pane(&spot).map_err(|_| refused("herdr plugin pane open failed"))?;

    // Name a fresh tab after the plugin; cosmetic, so its failure is ignored.
    if placement == TogglePlacement::Tab
        && let Some(tab) = opened.tab_id.as_deref()
    {
        let _ = herdr::rename_tab(tab, herdr::LABEL);
    }

    wait_until_visible(&opened.pane_id);
    Ok(format!("opened {} ({}) in {ws}", opened.pane_id, placement.as_str()))
}

/// Whether `dir` sits in a worktree; a `.git` dir or bare repository does not.
fn has_worktree(dir: &str) -> bool {
    crate::git::toplevel(Path::new(dir)).is_some()
}

/// Return once pane `pane` reads as reviewr, or past [`VISIBLE_BOUND`].
fn wait_until_visible(pane: &str) {
    let deadline = Instant::now() + VISIBLE_BOUND;
    loop {
        if runs_review_ui(pane).unwrap_or(false) {
            return;
        }
        if Instant::now() >= deadline {
            logln!("opened pane {pane} not visible as reviewr after {VISIBLE_BOUND:?}");
            return;
        }
        thread::sleep(VISIBLE_POLL);
    }
}

/// Re-point the stable launch links at the live plugin root; best effort, symlinks only.
#[cfg(unix)]
fn repoint_launch_links() {
    use std::os::unix::fs::PermissionsExt;

    let (Some(root), Some(home)) = (env::var_os("HERDR_PLUGIN_ROOT"), dirs::home_dir()) else {
        return;
    };
    if root.is_empty() {
        return;
    }
    let binary = Path::new(&root).join("bin").join("herdr-reviewr");
    let executable = std::fs::metadata(&binary)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if !executable {
        return;
    }
    let state_bin = home.join(".local/state/herdr/plugins").join(herdr::PLUGIN_ID).join("bin");
    let local_bin = home.join(".local/bin");
    // `~/.local/bin` only when it already exists: reviewr never creates a PATH directory.
    let dirs = [Some(state_bin), local_bin.is_dir().then_some(local_bin)];
    for dir in dirs.into_iter().flatten() {
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let link = dir.join("herdr-reviewr");
        match std::fs::symlink_metadata(&link) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if std::fs::read_link(&link).is_ok_and(|target| target == binary) {
                    continue;
                }
            }
            Ok(_) => continue,
            Err(_) => {}
        }
        // A fresh link renamed over the old one, so the path is never missing.
        let fresh = dir.join(format!(".herdr-reviewr.{}", std::process::id()));
        let _ = std::fs::remove_file(&fresh);
        if std::os::unix::fs::symlink(&binary, &fresh).is_ok()
            && std::fs::rename(&fresh, &link).is_err()
        {
            let _ = std::fs::remove_file(&fresh);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NonUiRun;

    #[test]
    fn a_non_ui_flag_is_recognized_anywhere_in_argv() {
        assert_eq!(
            NonUiRun::from_args(&["--some-future-arg", "--resolve-plugin-config"]),
            Some(NonUiRun::ResolvePluginConfig)
        );
        assert_eq!(
            NonUiRun::from_args(&["/repo", "--action", "toggle"]),
            Some(NonUiRun::Action(Some("toggle".into())))
        );
        assert_eq!(NonUiRun::from_args(&["--action"]), Some(NonUiRun::Action(None)));
        // UI flags and a repo path are the review UI.
        assert_eq!(NonUiRun::from_args(&["--base", "main", "/repo"]), None);
    }
}
