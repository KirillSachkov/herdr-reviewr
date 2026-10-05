//! External command-line tools: located, named, and run within a bound.

use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use process_wrap::std::CommandWrap;

/// Host bin dirs a stripped PATH may omit; none on Windows, where `\\usr\\bin` is plantable.
#[cfg(unix)]
const COMMON_BINS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];
#[cfg(not(unix))]
const COMMON_BINS: &[&str] = &[];

/// The host PATH: the common bins, then the inherited PATH, built once.
fn host_path() -> &'static OsString {
    host().path()
}

/// The host's lookup, built once.
fn host() -> &'static Lookup {
    static HOST: OnceLock<Lookup> = OnceLock::new();
    HOST.get_or_init(|| Lookup::on(prepended_path(env::var_os("PATH").as_deref())))
}

/// `name` on the host PATH.
fn resolve_on_host(name: &OsStr) -> Option<PathBuf> {
    host().resolve(name)
}

/// Program lookup on one PATH, a bare name's hit kept while it still exists.
struct Lookup {
    path: OsString,
    found: Mutex<HashMap<OsString, PathBuf>>,
}

impl Lookup {
    fn on(path: OsString) -> Self {
        Self { path, found: Mutex::default() }
    }

    fn path(&self) -> &OsString {
        &self.path
    }

    fn resolve(&self, name: &OsStr) -> Option<PathBuf> {
        let bare = Path::new(name).components().count() == 1 && !Path::new(name).is_absolute();
        let found = || self.found.lock().unwrap_or_else(PoisonError::into_inner);
        if bare
            && let Some(hit) = found().get(name)
            && hit.is_file()
        {
            return Some(hit.clone());
        }
        let hit = resolve_on(&self.path, name)?;
        if bare {
            found().insert(name.into(), hit.clone());
        }
        Some(hit)
    }
}

fn common_bins() -> impl Iterator<Item = PathBuf> {
    COMMON_BINS.iter().map(PathBuf::from)
}

/// The inherited PATH's entries; set-but-empty is none, never the cwd.
fn inherited_dirs(inherited: Option<&OsStr>) -> Vec<PathBuf> {
    inherited.filter(|p| !p.is_empty()).map(|p| env::split_paths(p).collect()).unwrap_or_default()
}

/// Join PATH entries; none holds a separator, so this cannot fail.
fn joined(dirs: impl Iterator<Item = PathBuf>) -> OsString {
    env::join_paths(dirs).expect("entries split from a PATH join back into one")
}

fn prepended_path(inherited: Option<&OsStr>) -> OsString {
    joined(common_bins().chain(inherited_dirs(inherited)))
}

fn appended_path(inherited: Option<&OsStr>) -> OsString {
    joined(inherited_dirs(inherited).into_iter().chain(common_bins()))
}

/// Resolve `name` as a shell would: PATHEXT on Windows, the cwd only for a path.
fn resolve_on(path: &OsStr, name: &OsStr) -> Option<PathBuf> {
    let cwd = env::current_dir().unwrap_or_default();
    which::which_in(name, Some(path), cwd).ok()
}

/// Resolve one of reviewr's own tools: the common host bins first, then the inherited PATH.
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    let mut cmd = resolve_on_host(program).map_or_else(|| Command::new(program), Command::new);
    cmd.env("PATH", host_path());
    cmd
}

/// Resolve the reviewer's own program as their shell would: their PATH first, the bins last.
pub(crate) fn user_command(program: impl AsRef<OsStr>) -> Option<Command> {
    let program = program.as_ref();
    let path = appended_path(env::var_os("PATH").as_deref());
    let mut cmd = Command::new(resolve_on(&path, program)?);
    cmd.env("PATH", path);
    Some(cmd)
}

/// A program's name: the base name after `/` or `\\`, a Windows extension dropped.
pub(crate) fn program_name(path: &str) -> &str {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((stem, ext)) if ["exe", "cmd", "bat"].iter().any(|e| ext.eq_ignore_ascii_case(e)) => {
            stem
        }
        _ => base,
    }
}

/// Whether `name` resolves to an executable on the host PATH.
#[must_use]
pub fn on_path(name: &str) -> bool {
    resolve_on_host(OsStr::new(name)).is_some()
}

/// How one bounded run of a tool failed.
#[derive(Debug)]
pub(crate) enum RunError {
    /// The program is not there to run.
    NotFound,
    /// It ran and exited non-zero; `stderr` carries its diagnostic.
    Failed { stderr: String },
    /// Spawning or waiting failed at the OS level.
    Io(String),
    /// The caller cancelled it mid-flight.
    Cancelled,
    /// It outlived its deadline.
    TimedOut,
}

/// Run `cmd` in its own process tree to its stdout; a cancel or the deadline ends the whole tree.
pub(crate) fn run_tree(
    cmd: Command,
    cancelled: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<String, RunError> {
    let mut cmd = CommandWrap::from(cmd);
    // No stdin: the terminal belongs to the pane.
    cmd.command_mut().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    cmd.wrap(process_wrap::std::JobObject);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(RunError::NotFound),
        Err(error) => return Err(RunError::Io(error.to_string())),
    };
    // Drained while polling, so a large answer cannot block the child.
    let stdout = read_all(child.stdout().take());
    let stderr = read_all(child.stderr().take());
    // Done once the tool exits and its pipes close; a lingering descendant is ended once.
    let mut ended = false;
    let status = loop {
        let stop = if cancelled.load(Ordering::Acquire) {
            Some(RunError::Cancelled)
        } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Some(RunError::TimedOut)
        } else {
            None
        };
        if let Some(stop) = stop {
            // The readers are left to finish as the ended tree closes their pipes.
            let _ = child.start_kill();
            // Once ended, the tool was reaped already; a job's wait would block on drained news.
            if !ended {
                let _ = child.wait();
            }
            return Err(stop);
        }
        match child.try_wait() {
            Ok(Some(status)) if stdout.is_finished() && stderr.is_finished() => break status,
            Ok(Some(_)) if !ended => {
                ended = true;
                let _ = child.start_kill();
            }
            Ok(_) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait();
                return Err(RunError::Io(error.to_string()));
            }
        }
    };
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    if status.success() {
        return Ok(String::from_utf8_lossy(&stdout).into_owned());
    }
    Err(RunError::Failed { stderr: String::from_utf8_lossy(&stderr).into_owned() })
}

/// Read `pipe` to its end on a thread of its own.
fn read_all(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

#[cfg(test)]
mod tests {
    use super::{
        COMMON_BINS, RunError, appended_path, prepended_path, program_name, resolve_on, run_tree,
    };
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    fn path_of(dirs: &[&str]) -> OsString {
        env::join_paths(dirs).unwrap()
    }

    fn common() -> Vec<PathBuf> {
        COMMON_BINS.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn prepended_path_puts_the_common_bins_in_front_of_the_inherited_path() {
        let got = prepended_path(Some(&path_of(&["inherited-a", "inherited-b"])));
        let parts: Vec<PathBuf> = env::split_paths(&got).collect();
        let mut expected = common();
        expected.extend([PathBuf::from("inherited-a"), PathBuf::from("inherited-b")]);
        assert_eq!(parts, expected);
    }

    #[test]
    fn prepended_path_keeps_the_common_bins_when_nothing_is_inherited() {
        let got = prepended_path(None);
        let parts: Vec<PathBuf> =
            env::split_paths(&got).filter(|p| !p.as_os_str().is_empty()).collect();
        assert_eq!(parts, common());
    }

    #[test]
    fn appended_path_leaves_the_reviewers_own_entries_in_front() {
        // The reviewer's shim wins; the common bins only backstop.
        let got = appended_path(Some(&path_of(&["mise-shims", "system-bin"])));
        let parts: Vec<PathBuf> = env::split_paths(&got).collect();
        let mut expected = vec![PathBuf::from("mise-shims"), PathBuf::from("system-bin")];
        expected.extend(common());
        assert_eq!(parts, expected);

        // An empty entry would put the reviewed repo's cwd first.
        assert_eq!(appended_path(Some(OsStr::new(""))), appended_path(None));
    }

    #[test]
    fn a_program_name_drops_the_directory_on_either_separator_and_a_windows_extension() {
        let rows = [
            ("target/debug/herdr-reviewr", "herdr-reviewr"),
            (r"C:\Users\me\plugin\bin\herdr-reviewr.exe", "herdr-reviewr"),
            (r"C:\plugin\bin\herdr-reviewr.EXE", "herdr-reviewr"),
            (r"C:\Program Files\Microsoft VS Code\Code.exe", "Code"),
            (r"C:\Users\me\AppData\Roaming\npm\code.CMD", "code"),
            ("C:/tools/edit.bat", "edit"),
            ("herdr-reviewr", "herdr-reviewr"),
            ("/usr/bin/herdr-reviewr-helper", "herdr-reviewr-helper"),
            ("/usr/bin/notepad++", "notepad++"),
            ("/opt/app.d/run.sh", "run.sh"),
            ("é.exe", "é"),
            ("exe", "exe"),
        ];
        for (path, want) in rows {
            assert_eq!(program_name(path), want, "{path:?}");
        }
    }

    /// An executable named `name` in `dir`, spelled the way the platform spells programs.
    #[cfg(unix)]
    fn program(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join(name);
        std::fs::write(&bin, []).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[cfg(windows)]
    fn program(dir: &Path, name: &str) -> PathBuf {
        let bin = dir.join(format!("{name}.exe"));
        std::fs::write(&bin, []).unwrap();
        bin
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
    }

    #[test]
    fn a_remembered_program_that_moved_is_looked_up_again() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = env::join_paths([first.path(), second.path()]).unwrap();
        let lookup = super::Lookup::on(path);
        let was = program(first.path(), "tool");
        assert!(same_file(&lookup.resolve(OsStr::new("tool")).unwrap(), &was));
        // Reinstalled elsewhere on the PATH: the remembered hit is gone, so the lookup runs again.
        std::fs::remove_file(&was).unwrap();
        let now = program(second.path(), "tool");
        assert!(same_file(&lookup.resolve(OsStr::new("tool")).unwrap(), &now));
    }

    #[test]
    fn resolve_on_finds_a_bare_name_in_a_path_directory() {
        let dir = tempfile::tempdir().unwrap();
        let bin = program(dir.path(), "gh");
        let path = env::join_paths([dir.path()]).unwrap();
        let found = resolve_on(&path, OsStr::new("gh")).expect("gh resolves");
        assert!(same_file(&found, &bin), "{found:?} is not {bin:?}");
        assert!(resolve_on(&path, OsStr::new("missing")).is_none());
    }

    /// A program named by its path resolves as itself, PATH unread.
    #[test]
    fn a_program_named_by_its_path_resolves_as_itself() {
        let dir = tempfile::tempdir().unwrap();
        let bin = program(dir.path(), "editor");
        let elsewhere = env::join_paths([std::path::Path::new("nowhere")]).unwrap();
        let found = resolve_on(&elsewhere, bin.as_os_str()).expect("the path resolves");
        assert!(same_file(&found, &bin), "{found:?} is not {bin:?}");
        assert!(resolve_on(&elsewhere, dir.path().join("missing").as_os_str()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_the_executable_bit_is_not_a_program() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes"), []).unwrap();
        let path = env::join_paths([dir.path()]).unwrap();
        assert!(resolve_on(&path, OsStr::new("notes")).is_none());
    }

    /// A tool whose grandchild holds its pipes, and that waits on it when `waits`.
    #[cfg(unix)]
    fn tool_with_grandchild(_dir: &Path, ready: &Path, waits: bool) -> Command {
        let script = if waits {
            r#"echo answer; sleep 60 & touch "$1"; wait"#
        } else {
            r#"echo answer; sleep 60 & touch "$1""#
        };
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script, "tool"]).arg(ready);
        cmd
    }

    /// The same tool as a batch file, the shape `az.cmd` has.
    #[cfg(windows)]
    fn tool_with_grandchild(dir: &Path, ready: &Path, waits: bool) -> Command {
        let mut script = String::from(
            "@echo off\r\necho answer\r\nstart /b \"\" ping -n 61 127.0.0.1 >nul\r\n\
             echo ready> \"%~1\"\r\n",
        );
        if waits {
            script.push_str("ping -n 61 127.0.0.1 >nul\r\n");
        }
        let path = dir.join("tool.cmd");
        std::fs::write(&path, script).unwrap();
        let mut cmd = Command::new(path);
        cmd.arg(ready);
        cmd
    }

    /// Run `cmd` on its own thread, cancelled through `cancelled`.
    fn spawn_run(
        cmd: Command,
        cancelled: Arc<AtomicBool>,
        deadline: Option<Instant>,
    ) -> mpsc::Receiver<Result<String, RunError>> {
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || done_tx.send(run_tree(cmd, &cancelled, deadline)));
        done_rx
    }

    /// Wait until the tool's grandchild exists.
    fn await_ready(ready: &Path) {
        let started = Instant::now();
        while !ready.exists() {
            assert!(started.elapsed() < Duration::from_secs(10), "never started");
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(300));
    }

    #[test]
    fn a_cancel_ends_the_tools_whole_process_tree() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let cancelled = Arc::new(AtomicBool::new(false));
        let done =
            spawn_run(tool_with_grandchild(dir.path(), &ready, true), cancelled.clone(), None);
        await_ready(&ready);
        cancelled.store(true, Ordering::Release);
        let result = done.recv_timeout(Duration::from_secs(5)).expect("the cancel ends the run");
        assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    }

    #[test]
    fn the_deadline_ends_a_tool_that_never_exits() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let deadline = Instant::now() + Duration::from_secs(2);
        let cmd = tool_with_grandchild(dir.path(), &ready, true);
        let done = spawn_run(cmd, Arc::default(), Some(deadline));
        let result = done.recv_timeout(Duration::from_secs(7)).expect("the deadline ends the run");
        assert!(matches!(result, Err(RunError::TimedOut)), "{result:?}");
    }

    #[test]
    fn a_tool_that_exits_is_done_though_a_grandchild_holds_its_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = tool_with_grandchild(dir.path(), &dir.path().join("ready"), false);
        // Never cancelled and no deadline: the tool's own exit has to end the run.
        let done = spawn_run(cmd, Arc::default(), None);
        let result = done.recv_timeout(Duration::from_secs(10)).expect("the tool's exit ends it");
        assert_eq!(result.map(|out| out.trim().to_string()).ok().as_deref(), Some("answer"));
    }

    /// A bare name reaches a batch shim through PATHEXT.
    #[cfg(windows)]
    #[test]
    fn a_batch_shim_resolves_through_pathext() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("az.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        let path = env::join_paths([dir.path()]).unwrap();
        let found = resolve_on(&path, OsStr::new("az")).expect("az resolves");
        assert!(same_file(&found, &shim), "{found:?} is not {shim:?}");
    }
}
