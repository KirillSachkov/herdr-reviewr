//! The plugin's pane actions and event hook, run as `herdr-reviewr --action <name>`:
//!
//! - `toggle` opens a reviewr pane, or closes every one if any is open.
//! - `open` opens a reviewr pane, and is a no-op if one is open.
//! - `close` closes every reviewr pane, and is a no-op if none is.
//! - `auto-open` is the worktree workspace-birth hook, gated by `auto_open` and placement.
//!
//! A reviewr pane is any pane whose foreground runs the review UI, read live per pane. The
//! `reviewr` label is display only and never read. An action refuses loudly (exit 1, one
//! `reviewr:` line on stderr) and reports a success on stdout. A refused event stays silent,
//! except for a config error, which goes to stderr for herdr's plugin log.
//!
//! herdr runs plugin actions concurrently, so every action that reaches a workspace holds that
//! workspace's exclusive OS lock, `$HERDR_PLUGIN_STATE_DIR/action-<workspace>.lock`, from its
//! pane listing to its end (see [`action_lock`]). The file holds no state: it is never written
//! or deleted, and the OS releases the lock when a run exits or crashes.

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

/// A run of the binary that is not the review UI, read from its arguments after argv\[0\].
///
/// `main` dispatches on this, and [`is_review_ui`] reads every observed process through it.
/// That makes "a flag run never counts as the review UI, so it never runs the review UI
/// either" one rule: a non-UI flag added here is dispatched and excluded at once.
#[derive(Debug, PartialEq, Eq)]
pub enum NonUiRun {
    /// `--resolve-plugin-config`: print the normalized plugin config.
    ResolvePluginConfig,
    /// `--action <name>`. The name is `None` when the flag ends argv.
    Action(Option<String>),
}

impl NonUiRun {
    /// The non-UI run `args` asks for, recognized anywhere in argv, or `None` for the review
    /// UI. UI flags such as `--base` and a repo path never make a run non-UI.
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
    /// The plugin config is invalid. Loud on every action, the event included, so the error
    /// reaches herdr's plugin log.
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
    // The whole plugin config validates before any workspace read or pane write, so every
    // plugin entry point shares exactly one contract.
    let config = crate::config::plugin_config_from_herdr().map_err(Stop::Config)?;

    #[cfg(unix)]
    repoint_launch_links();

    // Event policy gates the event alone: explicit actions ignore it. This sits after
    // validation but before any workspace or pane read, so a disabled event does no work.
    let event = if action == Action::AutoOpen {
        if !config.auto_open()
            || !matches!(config.toggle_placement(), TogglePlacement::Split | TogglePlacement::Tab)
        {
            return Ok(None);
        }
        // The payload names the event's workspace. Without it, the only workspace in reach is
        // the focused one, whatever the user is looking at, so the event refuses.
        let Some(json) = var("HERDR_PLUGIN_EVENT_JSON") else {
            return Err(refused("no event payload"));
        };
        let event: Value = serde_json::from_str(&json).unwrap_or_default();
        // `worktree.opened` also fires when its workspace is already live. That is a
        // focus/open request, not a workspace birth: never resurrect a reviewr pane the user
        // closed there.
        if event.pointer("/data/already_open") == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        Some(event)
    } else {
        None
    };

    let target = Target::read(event)?;
    let ws = target.ws.as_str();
    // Held from the listing through the close or the open, so a concurrent action on this
    // workspace reads it only after this one's effect is visible.
    let Some(_lock) = action_lock(action, ws)? else {
        return Ok(None);
    };

    // One pane-list snapshot serves the whole run. A failed or unreadable listing must not
    // read as "no reviewr pane": that would stack a duplicate on toggle and false-succeed a
    // close.
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

/// How long an explicit action waits for another action on its workspace to release the lock.
/// One action holds it for a few herdr round trips plus, for an open, up to [`VISIBLE_BOUND`]:
/// under a second warm, and up to 5.5 s for a cold first open on a Windows VM. This waits out
/// the slowest holder with room to spare. The wait runs on herdr's action thread and blocks
/// nothing, so the bound only ends a wait on a wedged holder.
const LOCK_BOUND: Duration = Duration::from_secs(15);

/// The pause between two lock attempts while an explicit action waits.
const LOCK_POLL: Duration = Duration::from_millis(20);

/// Take workspace `ws`'s action lock, or `None` when the event finds it held and yields.
///
/// herdr spawns every action and event hook on its own thread with no per-plugin queue, so two
/// quick toggles, or a toggle and an auto-open, would both read "no reviewr pane" and both
/// open. An explicit action waits, bounded, so a double press opens and then closes. Past
/// [`LOCK_BOUND`] it refuses rather than act unguarded. The event tries once and yields:
/// whoever holds this workspace's lock is already acting on its reviewr panes, and a second
/// open is exactly what the lock exists to stop. The lock is per workspace, since the race is:
/// an action in one workspace never holds back another's. This is the pattern of
/// herdr-sidebar's launcher lock.
///
/// The wait polls `try_lock` against a deadline instead of blocking in `lock`, because Windows
/// can take a moment to release a crashed holder's lock. Without a usable state dir the action
/// refuses: herdr sets and creates the dir for every action, so a run without it is not a herdr
/// action, and an unguarded run is the race this lock closes.
fn action_lock(action: Action, ws: &str) -> Result<Option<File>, Stop> {
    let Some(dir) = env::var_os("HERDR_PLUGIN_STATE_DIR").filter(|dir| !dir.is_empty()) else {
        return Err(refused("no plugin state dir (invoke as a herdr plugin action)"));
    };
    // A workspace id names the file, one-to-one so two workspaces never share a lock: an ASCII
    // letter, digit or `-` stays, and every other byte is `_` and its two hex digits.
    let name: String = ws
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' {
                char::from(b).to_string()
            } else {
                format!("_{b:02x}")
            }
        })
        .collect();
    let path = Path::new(&dir).join(format!("action-{name}.lock"));
    let unusable = |error| refused(format!("cannot lock {}: {error}", path.display()));
    // Read and write without truncation: Windows locks need a handle with access, and the
    // file's (empty) content is never touched.
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

/// The non-empty string at JSON `pointer` in `value`. Each field of herdr's action context
/// (`HERDR_PLUGIN_CONTEXT_JSON`) and event payload (`HERDR_PLUGIN_EVENT_JSON`) reads on its own:
/// one missing or of another type reads as absent and leaves the rest of the payload usable.
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
    /// The target of the event with payload `event`, else of an explicit action. Refused
    /// without a workspace to act in.
    fn read(event: Option<Value>) -> Result<Self, Stop> {
        let no_workspace = || refused("no workspace context (invoke from inside herdr)");
        // The events fire without a focused pane: target the fresh workspace from their
        // payload, never a focused pane's cwd. The `worktree` fields are compatible fallbacks
        // (`docs/herdr-api-notes.md`).
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

/// The workspace's reviewr panes, in listing order: any tab, any placement, however they were
/// launched. The per-pane reads run concurrently, so the sweep costs one process-info round
/// trip of wall clock, not one per pane. `None` when any read failed, which the caller refuses
/// like a failed pane list.
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

/// Whether pane `pane` runs the review UI. A pane the read reports gone exited between the
/// list and this read, and converges like any observed-then-exited pane. Any other failed read
/// is an error.
fn runs_review_ui(pane: &str) -> Result<bool, HerdrError> {
    match ProcessInfo::of(pane) {
        Ok(info) => Ok(info.foreground_processes.iter().any(is_review_ui)),
        Err(error) if error.pane_gone() => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether `process` is the review UI: its executable is `herdr-reviewr`, and its argv asks for
/// no [`NonUiRun`]. A wrapped launch (`cargo run`) counts through its child. The executable
/// name in `argv0` or `argv[0]` decides, never `name`, which is a rewritable process title
/// (`docs/herdr-api-notes.md`).
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

/// Close every pane in `existing`, with plain `pane close` (see [`herdr::close_pane`]).
///
/// A close refused because the pane is gone lost a benign race: the pane exited between the read
/// and the close, the same end state, so the sweep still converges. A close failing any other
/// way names a pane that may still be running, so the sweep finishes the rest and then refuses
/// rather than reporting that pane closed.
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

/// How long an open waits for its new pane to read as reviewr. herdr caches its Windows process
/// snapshot for 250 ms, so a just-opened pane can read empty, and a toggle that returned then
/// would let the next toggle open a second pane instead of closing this one. A cold first open
/// on a Windows VM took up to 5.5 s to read as reviewr, so the bound waits that out. Past it
/// the open reports success anyway: the pane is open, only its read lags.
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
    // Prefer the focused pane's live `foreground_cwd`, read from the pane-list snapshot already
    // in hand, over the context's launch cwd (the launch-vs-live split is in
    // docs/herdr-api-notes.md). The live cwd wins only inside a repo.
    let live = target
        .focused
        .as_deref()
        .and_then(|focused| panes.pane(focused))
        .and_then(|entry| entry.foreground_cwd.clone());
    let cwd = match (&live, &target.cwd) {
        (Some(live), _) if has_worktree(live) => live,
        (_, Some(cwd)) if has_worktree(cwd) => cwd,
        // Name every candidate the check rejected, or a refusal over an inspected but unusable
        // live cwd would read as if no directory was ever tried.
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

    // A tab open lands in a fresh tab that herdr labels with a bare index: name it after the
    // plugin so the tab bar reads "reviewr". Cosmetic, so a failed rename never fails an open
    // that already succeeded.
    if placement == TogglePlacement::Tab
        && let Some(tab) = opened.tab_id.as_deref()
    {
        let _ = herdr::rename_tab(tab, herdr::LABEL);
    }

    wait_until_visible(&opened.pane_id);
    Ok(format!("opened {} ({}) in {ws}", opened.pane_id, placement.as_str()))
}

/// Whether `dir` sits in a worktree: git names its top level. Unlike `git::is_repo`, a `.git`
/// dir or a bare repository does not count, since reviewr reviews a worktree.
fn has_worktree(dir: &str) -> bool {
    crate::git::toplevel(Path::new(dir)).is_some()
}

/// Return once pane `pane` reads as a reviewr pane, or once [`VISIBLE_BOUND`] has passed. A
/// sequential toggle after this one then always sees the pane it opened.
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

/// Re-point the stable launch paths at the live plugin root.
///
/// They track the root from here, not from the install step: the build step runs in a staging
/// checkout that herdr renames afterwards, so only a runtime invocation knows the real root.
/// Best effort, so this never fails an action, and it never replaces anything but a symlink: a
/// user's own binary at the path (`cargo install --root ~/.local`) survives. Unix only, since
/// symlinks on Windows need Developer Mode or admin rights.
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
        // A layout may launch through the link at any moment, so the swap never leaves the
        // path missing: a fresh link beside it renames over the old one in one step.
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
