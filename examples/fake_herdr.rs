//! A fake herdr CLI for the plugin-action and send tests (`tests/pane_actions.rs`,
//! `tests/send_flow.rs`), answering in the live envelope shapes (docs/herdr-api-notes.md). It
//! is Rust rather than a shell script so the tests run on every OS, Windows included.
//!
//! It serves the fixture directory named by `FAKE_HERDR_DIR`, appending each invocation's
//! arguments as one line to `herdr.log`:
//!
//! - `pane list` serves `panes.json`, else one plain pane `w1:p1`, plus the pane `plugin pane
//!   open` creates while it is open. It first hangs for 10 s when `list-hang` exists, deleting
//!   that file, so only one listing hangs.
//! - `pane process-info --pane <id>` fails with `procfail-<id>.json` on stderr, else serves
//!   `procinfo-<id>.json`. Without either, the pane `plugin pane open` creates (`w1:p9`) runs
//!   the review UI, after answering no processes for as many reads as `opened-empty-reads`
//!   holds. Every other pane is a plain shell, which is not a reviewr pane.
//! - `pane close <id>` fails with `closefail-<id>` on stderr, else succeeds, and closes the
//!   opened pane when it names it.
//! - `plugin config-dir` names the fixture directory itself, after a 5 s hang when
//!   `configdir-hang` exists.
//! - `plugin pane open` fails with `openfail` on stderr, else opens pane `w1:p9` in tab `w1:t9`,
//!   which then stays open (the `opened` file) until a `pane close` names it.
//! - `agent list` fails with `agentsfail` on stderr, else serves `agents.json`, else no agents.
//! - `tab list` serves `tabs.json`, else no tabs.
//! - Everything else succeeds.
//!
//! A pane id's `:` is spelled `_` in a fixture file name, since Windows forbids `:` there.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

/// The pane every successful `plugin pane open` creates.
const OPENED: &str = "w1:p9";

fn main() -> ExitCode {
    let dir = PathBuf::from(std::env::var_os("FAKE_HERDR_DIR").expect("FAKE_HERDR_DIR is set"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let line = args.join(" ");
    let mut log =
        fs::OpenOptions::new().create(true).append(true).open(dir.join("herdr.log")).unwrap();
    writeln!(log, "{line}").unwrap();
    drop(log);

    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["pane", "list", ..] => {
            if fs::remove_file(dir.join("list-hang")).is_ok() {
                thread::sleep(Duration::from_secs(10));
            }
            let opened = if dir.join("opened").exists() {
                format!(r#",{{"pane_id":"{OPENED}"}}"#)
            } else {
                String::new()
            };
            let default = format!(r#"{{"result":{{"panes":[{{"pane_id":"w1:p1"}}{opened}]}}}}"#);
            answer(&read(&dir.join("panes.json")).unwrap_or(default))
        }
        ["pane", "process-info", "--pane", pane] => {
            if let Some(failure) = read(&fixture(&dir, "procfail", pane, ".json")) {
                return fail(&failure);
            }
            answer(
                &read(&fixture(&dir, "procinfo", pane, ".json"))
                    .unwrap_or_else(|| process_info(&dir, pane, &line)),
            )
        }
        ["pane", "close", pane] => {
            if let Some(failure) = read(&fixture(&dir, "closefail", pane, "")) {
                return fail(&failure);
            }
            if *pane == OPENED {
                let _ = fs::remove_file(dir.join("opened"));
            }
            answer(r#"{"result":{}}"#)
        }
        ["plugin", "config-dir", ..] => {
            if dir.join("configdir-hang").exists() {
                thread::sleep(Duration::from_secs(5));
            }
            answer(&dir.display().to_string())
        }
        ["plugin", "pane", "open", ..] => {
            if let Some(failure) = read(&dir.join("openfail")) {
                return fail(&failure);
            }
            fs::write(dir.join("opened"), "").unwrap();
            answer(&format!(
                r#"{{"result":{{"type":"plugin_pane_opened","plugin_pane":{{"pane":{{"pane_id":"{OPENED}","tab_id":"w1:t9"}}}}}}}}"#
            ))
        }
        ["agent", "list"] => match read(&dir.join("agentsfail")) {
            Some(failure) => fail(&failure),
            None => answer(
                &read(&dir.join("agents.json"))
                    .unwrap_or_else(|| r#"{"result":{"agents":[]}}"#.to_owned()),
            ),
        },
        ["tab", "list", ..] => answer(
            &read(&dir.join("tabs.json")).unwrap_or_else(|| r#"{"result":{"tabs":[]}}"#.to_owned()),
        ),
        _ => answer(r#"{"result":{}}"#),
    }
}

/// The default process-info answer for `pane`, whose read is the logged `line`.
fn process_info(dir: &Path, pane: &str, line: &str) -> String {
    let process = if pane == OPENED {
        let empty_reads: usize = read(&dir.join("opened-empty-reads"))
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0);
        let reads = read(&dir.join("herdr.log")).unwrap_or_default();
        if reads.lines().filter(|logged| *logged == line).count() <= empty_reads {
            // herdr omits the key for an empty list, as it serializes a pane its process
            // snapshot has not caught up with yet.
            return format!(
                r#"{{"result":{{"process_info":{{"pane_id":"{pane}","shell_pid":1}}}}}}"#
            );
        }
        r#"{"pid":9,"name":"herdr-reviewr","argv0":"herdr-reviewr","argv":["/plugin/bin/herdr-reviewr"],"cwd":"/w"}"#
    } else {
        r#"{"pid":7,"name":"zsh","argv0":"zsh","argv":["-zsh"],"cwd":"/"}"#
    };
    format!(
        r#"{{"result":{{"process_info":{{"foreground_process_group_id":7,"foreground_processes":[{process}],"pane_id":"{pane}","shell_pid":1}}}}}}"#
    )
}

/// The fixture file `<kind>-<pane><suffix>`, with the pane id's `:` spelled `_`.
fn fixture(dir: &Path, kind: &str, pane: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{kind}-{}{suffix}", pane.replace(':', "_")))
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn answer(stdout: &str) -> ExitCode {
    println!("{}", stdout.trim_end());
    ExitCode::SUCCESS
}

fn fail(stderr: &str) -> ExitCode {
    eprintln!("{}", stderr.trim_end());
    ExitCode::FAILURE
}
