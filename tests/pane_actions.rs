//! The plugin actions end to end: the real binary run as `--action <name>` against a fake
//! herdr (`examples/fake_herdr.rs`, which documents its fixture files). This file is its own
//! test process, and every run gets its herdr environment set explicitly, so no test reaches
//! a live herdr.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use common::fake_herdr;
use serde_json::{Value, json};

fn reviewr_bin() -> &'static str {
    env!("CARGO_BIN_EXE_herdr-reviewr")
}

/// The fixture file for `pane`, spelled the way the fake reads it: `:` becomes `_`, since
/// Windows forbids `:` in a file name.
fn fixture(dir: &Path, kind: &str, pane: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{kind}-{}{suffix}", pane.replace(':', "_")))
}

/// One `pane process-info` answer for `pane`: a foreground holding `processes`, in the live
/// envelope shape (docs/herdr-api-notes.md).
fn procinfo(dir: &Path, pane: &str, processes: &Value) {
    let answer = json!({"result": {"process_info": {
        "foreground_process_group_id": 7,
        "foreground_processes": processes,
        "pane_id": pane,
        "shell_pid": 1,
    }}});
    fs::write(fixture(dir, "procinfo", pane, ".json"), answer.to_string()).unwrap();
}

/// One foreground process, as herdr reports it.
fn process(argv0: &str, argv: &[&str]) -> Value {
    json!({"pid": 8, "name": "some-title", "argv0": argv0, "argv": argv, "cwd": "/w"})
}

/// The review UI as a plugin pane runs it.
fn review_ui() -> Value {
    process("herdr-reviewr", &["/plugin/bin/herdr-reviewr"])
}

/// herdr's error envelope for `code`, as a failed call writes it to stderr.
fn herdr_error(code: &str) -> String {
    json!({"error": {"code": code, "message": "boom"}, "id": "cli:request"}).to_string()
}

/// One `pane list` answer holding `panes`.
fn panes(dir: &Path, panes: &Value) {
    fs::write(dir.join("panes.json"), json!({"result": {"panes": panes}}).to_string()).unwrap();
}

/// One `pane list` answer: a single pane whose entry carries a live `foreground_cwd`.
fn pane_with_cwd(dir: &Path, pane: &str, foreground_cwd: &Path) {
    panes(dir, &json!([{"pane_id": pane, "foreground_cwd": foreground_cwd}]));
}

/// Every herdr call the runs in `dir` made, one per line. Empty when herdr was never called.
fn calls(dir: &Path) -> String {
    fs::read_to_string(dir.join("herdr.log")).unwrap_or_default()
}

/// Forget every call the fake logged and close the pane it opened, for the next run in `dir`.
fn reset(dir: &Path) {
    let _ = fs::remove_file(dir.join("herdr.log"));
    let _ = fs::remove_file(dir.join("opened"));
}

fn herdr_called(dir: &Path) -> bool {
    dir.join("herdr.log").exists()
}

/// A fresh git repo at `dir/name`, for tests that need a real second repo beside the
/// crate's own.
fn init_repo(dir: &Path, name: &str) -> PathBuf {
    let repo = dir.join(name);
    fs::create_dir(&repo).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap()
            .success()
    );
    repo
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The action `mode`, run the way herdr runs it from workspace `workspace-1`, with `dir` as
/// the plugin config dir, the plugin state dir, and the fake's fixture dir. Every other herdr
/// variable is cleared, so the run sees exactly what the test sets.
fn action(mode: &str, dir: &Path) -> Command {
    let mut command = Command::new(reviewr_bin());
    command
        .args(["--action", mode])
        .env("HERDR_PLUGIN_CONFIG_DIR", dir)
        .env("HERDR_PLUGIN_STATE_DIR", dir)
        .env("HERDR_BIN_PATH", fake_herdr())
        .env("FAKE_HERDR_DIR", dir)
        .env("HERDR_WORKSPACE_ID", "workspace-1");
    for name in [
        "HERDR_PANE_ID",
        "HERDR_PLUGIN_ID",
        "HERDR_PLUGIN_ROOT",
        "HERDR_PLUGIN_CONTEXT_JSON",
        "HERDR_PLUGIN_EVENT_JSON",
    ] {
        command.env_remove(name);
    }
    command
}

fn run(mode: &str, dir: &Path) -> Output {
    action(mode, dir).output().unwrap()
}

/// An `open` with the workspace context a focused pane provides, so the run reaches the
/// placement and `plugin pane open` stages.
fn run_open(dir: &Path) -> Output {
    run_with_context("open", dir, &repo_context())
}

/// The action context of a focused pane in this crate's repo, so an open can proceed.
fn repo_context() -> String {
    json!({"focused_pane_cwd": env!("CARGO_MANIFEST_DIR")}).to_string()
}

/// Any mode with a caller-shaped action context, invoked from pane `w1:p1`.
fn run_with_context(mode: &str, dir: &Path, context: &str) -> Output {
    with_context(mode, dir, context).output().unwrap()
}

fn with_context(mode: &str, dir: &Path, context: &str) -> Command {
    let mut command = action(mode, dir);
    command.env("HERDR_PANE_ID", "w1:p1").env("HERDR_PLUGIN_CONTEXT_JSON", context);
    command
}

/// The event hook, as herdr fires it: no workspace or pane of its own, only the payload.
fn run_auto_open(dir: &Path, event: &str, context: Option<&str>) -> Output {
    let mut command = action("auto-open", dir);
    command.env_remove("HERDR_WORKSPACE_ID").env("HERDR_PLUGIN_EVENT_JSON", event);
    if let Some(context) = context {
        command.env("HERDR_PLUGIN_CONTEXT_JSON", context);
    }
    command.output().unwrap()
}

/// A worktree event payload in the live shape (docs/herdr-api-notes.md).
fn worktree_event(
    name: &str,
    workspace: &str,
    checkout: &str,
    already_open: Option<bool>,
) -> String {
    let mut data = json!({
        "type": name,
        "workspace": {"workspace_id": workspace, "worktree": {"checkout_path": checkout}},
        "worktree": {"path": checkout, "open_workspace_id": workspace},
    });
    if let Some(already_open) = already_open {
        data["already_open"] = json!(already_open);
    }
    json!({"event": name, "data": data}).to_string()
}

/// The `plugin pane open` call a run made.
fn open_call(dir: &Path) -> String {
    calls(dir)
        .lines()
        .find(|line| line.starts_with("plugin pane open"))
        .expect("a plugin pane open call")
        .to_owned()
}

// --- Config: the whole file validates before any herdr call.

#[test]
fn invalid_config_refuses_manual_action_before_herdr_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "theme = \"not-a-theme\"\n").unwrap();

    for mode in ["open", "close", "toggle"] {
        let output = run(mode, dir.path());
        assert_eq!(output.status.code(), Some(1), "{mode}");
        assert!(stderr(&output).contains("config.toml"), "{mode}: {}", stderr(&output));
        assert!(stderr(&output).contains("`theme`"), "{mode}: {}", stderr(&output));
    }
    assert!(!herdr_called(dir.path()), "herdr was invoked before validation");
}

#[test]
fn invalid_config_refuses_event_loudly_before_herdr_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "auto_open = \"sometimes\"\n").unwrap();

    let output = run("auto-open", dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("`auto_open`"), "{}", stderr(&output));
    assert!(!herdr_called(dir.path()), "herdr was invoked before validation");
}

#[test]
fn corrected_config_recovers_on_the_next_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    fs::write(&config, "unknown = true\n").unwrap();
    assert_eq!(run("close", dir.path()).status.code(), Some(1));
    assert!(!herdr_called(dir.path()));

    fs::write(&config, "theme = \"gruvbox\"\n").unwrap();
    let output = run("close", dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: nothing open in workspace-1\n");
    assert!(calls(dir.path()).contains("pane list --workspace workspace-1"));
}

#[test]
fn an_unknown_action_refuses_before_any_herdr_call() {
    let dir = tempfile::tempdir().unwrap();

    let output = run("frobnicate", dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "reviewr: unknown action 'frobnicate' (toggle | open | close | auto-open)\n"
    );
    assert!(output.stdout.is_empty());
    assert!(!herdr_called(dir.path()));

    // A bare `--action` names no action at all, and refuses the same way.
    let output = Command::new(reviewr_bin())
        .arg("--action")
        .env("HERDR_PLUGIN_CONFIG_DIR", dir.path())
        .env("HERDR_BIN_PATH", fake_herdr())
        .env("FAKE_HERDR_DIR", dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("unknown action ''"), "{}", stderr(&output));
}

// --- The event: gated by policy, silent on a runtime refusal.

#[test]
fn disabled_auto_open_stops_after_successful_validation() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "auto_open = false\n").unwrap();

    let output = run("auto-open", dir.path());

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!herdr_called(dir.path()));
}

#[test]
fn auto_open_skips_placements_that_are_not_split_or_tab() {
    let dir = tempfile::tempdir().unwrap();
    let event = worktree_event("worktree_created", "workspace-9", env!("CARGO_MANIFEST_DIR"), None);

    for placement in ["overlay", "zoomed"] {
        fs::write(dir.path().join("config.toml"), format!("toggle_placement = \"{placement}\"\n"))
            .unwrap();
        let output = run_auto_open(dir.path(), &event, None);
        assert!(output.status.success(), "{placement}: {}", stderr(&output));
        assert!(output.stdout.is_empty(), "{placement}");
        assert!(output.stderr.is_empty(), "{placement}");
    }
    assert!(!herdr_called(dir.path()), "{}", calls(dir.path()));
}

#[test]
fn valid_auto_open_runtime_refusal_remains_silent() {
    let dir = tempfile::tempdir().unwrap();

    let output = action("auto-open", dir.path()).env_remove("HERDR_WORKSPACE_ID").output().unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!herdr_called(dir.path()));
}

#[test]
fn auto_open_opened_live_exits_before_herdr_calls() {
    let dir = tempfile::tempdir().unwrap();
    let event =
        worktree_event("worktree_opened", "workspace-9", env!("CARGO_MANIFEST_DIR"), Some(true));

    let output = run_auto_open(dir.path(), &event, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!herdr_called(dir.path()), "opened-live inspected herdr before exiting");
}

#[test]
fn auto_open_birth_events_follow_shared_policy() {
    let dir = tempfile::tempdir().unwrap();
    let live_repo = init_repo(dir.path(), "live-repo");
    pane_with_cwd(dir.path(), "w1:p1", &live_repo);
    let context = json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": live_repo}).to_string();

    for (event_name, already_open) in [("worktree_created", None), ("worktree_opened", Some(false))]
    {
        for placement in ["split", "tab"] {
            fs::write(
                dir.path().join("config.toml"),
                format!("toggle_placement = \"{placement}\"\n"),
            )
            .unwrap();
            reset(dir.path());
            let workspace = format!("workspace-{event_name}-{placement}");
            let event =
                worktree_event(event_name, &workspace, env!("CARGO_MANIFEST_DIR"), already_open);

            let output = run_auto_open(dir.path(), &event, Some(&context));

            assert!(output.status.success(), "{event_name}/{placement}: {}", stderr(&output));
            // The event reports nothing on success either.
            assert!(output.stdout.is_empty(), "{event_name}/{placement}: {}", stdout(&output));
            let calls = calls(dir.path());
            assert!(calls.contains(&format!("pane list --workspace {workspace}")), "{calls}");
            let open = open_call(dir.path());
            let tokens = open.split_whitespace().collect::<Vec<_>>();
            assert!(tokens.contains(&"--no-focus"), "{open}");
            assert!(!tokens.contains(&"--focus"), "{open}");
            assert!(open.contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))), "{open}");
            assert!(open.contains(&format!("--placement {placement}")), "{open}");
            assert!(!open.contains(live_repo.to_str().unwrap()), "{open}");
        }
    }
}

#[test]
fn auto_open_without_its_payload_refuses_silently_before_any_herdr_call() {
    let dir = tempfile::tempdir().unwrap();
    // A focused workspace and pane are in reach, and opening there would stack a pane into
    // whatever workspace the user is looking at.
    for payload in [None, Some("")] {
        let mut command = with_context("auto-open", dir.path(), &repo_context());
        match payload {
            Some(json) => command.env("HERDR_PLUGIN_EVENT_JSON", json),
            None => command.env_remove("HERDR_PLUGIN_EVENT_JSON"),
        };

        let output = command.output().unwrap();

        assert!(output.status.success(), "{payload:?}: {}", stderr(&output));
        assert!(output.stdout.is_empty(), "{payload:?}: {}", stdout(&output));
        assert!(output.stderr.is_empty(), "{payload:?}: {}", stderr(&output));
    }
    assert!(!herdr_called(dir.path()), "{}", calls(dir.path()));
}

#[test]
fn auto_open_reads_each_payload_field_on_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let repo = env!("CARGO_MANIFEST_DIR");
    let data = |extra: Value| {
        let mut data = json!({
            "type": "worktree_opened",
            "workspace": {"workspace_id": "workspace-9", "worktree": {"checkout_path": repo}},
        });
        data.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({"event": "worktree_opened", "data": data}).to_string()
    };
    // A field of an unexpected type reads as absent and leaves the rest of the payload alone.
    // Only a boolean `true` marks the workspace as already open.
    let payloads = [
        data(json!({"worktree": {"path": 42, "open_workspace_id": ["w"]}})),
        data(json!({"already_open": "true"})),
    ];
    for payload in payloads {
        reset(dir.path());

        let output = run_auto_open(dir.path(), &payload, None);

        assert!(output.status.success(), "{payload}: {}", stderr(&output));
        let open = open_call(dir.path());
        assert!(open.contains(&format!("--cwd {repo}")), "{payload}: {open}");
        assert!(
            calls(dir.path()).contains("pane list --workspace workspace-9"),
            "{payload}: {}",
            calls(dir.path())
        );
    }
}

#[test]
fn auto_open_falls_back_to_the_worktree_fields_of_the_payload() {
    let dir = tempfile::tempdir().unwrap();
    // A payload without `data.workspace`: the hook targets `data.worktree`'s workspace and
    // checkout instead (docs/herdr-api-notes.md).
    let event = json!({"event": "worktree_created", "data": {
        "type": "worktree_created",
        "worktree": {"path": env!("CARGO_MANIFEST_DIR"), "open_workspace_id": "workspace-7"},
    }})
    .to_string();

    let output = run_auto_open(dir.path(), &event, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        calls(dir.path()).contains("pane list --workspace workspace-7"),
        "{}",
        calls(dir.path())
    );
    let open = open_call(dir.path());
    assert!(open.contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))), "{open}");
}

#[test]
fn a_failed_plugin_pane_open_refuses_an_action_and_stays_silent_for_the_event() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("openfail"), herdr_error("internal")).unwrap();

    let output = run_open(dir.path());
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "reviewr: herdr plugin pane open failed\n");

    let event = worktree_event("worktree_created", "workspace-9", env!("CARGO_MANIFEST_DIR"), None);
    let output = run_auto_open(dir.path(), &event, None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
}

#[test]
fn manifest_runs_the_binary_directly_for_every_pane_action_and_event() {
    let manifest: toml::Table =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("herdr-plugin.toml"))
            .unwrap()
            .parse()
            .unwrap();
    // herdr 0.9.0 is the first to resolve a relative pane `command[0]` against the plugin
    // root, which the pane command below relies on.
    assert_eq!(manifest["min_herdr_version"].as_str(), Some("0.9.0"));
    let commands = |section: &str| -> Vec<(toml::Table, Vec<String>)> {
        manifest[section]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                let table = entry.as_table().unwrap().clone();
                let command = table["command"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|part| part.as_str().unwrap().to_owned())
                    .collect();
                (table, command)
            })
            .collect()
    };

    let panes = commands("panes");
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].1, ["bin/herdr-reviewr"]);

    // Each action runs itself by its own id, so `persiyanov.reviewr.toggle` is one binding on
    // every OS.
    let mut ids = Vec::new();
    for (table, command) in commands("actions") {
        let id = table["id"].as_str().unwrap();
        assert_eq!(command, ["bin/herdr-reviewr", "--action", id]);
        ids.push(id.to_owned());
    }
    ids.sort_unstable();
    assert_eq!(ids, ["close", "open", "toggle"]);

    let mut auto_open_events = commands("events")
        .into_iter()
        .filter(|(_, command)| command == &["bin/herdr-reviewr", "--action", "auto-open"])
        .map(|(table, _)| table["on"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    auto_open_events.sort_unstable();
    assert_eq!(auto_open_events, ["worktree.created", "worktree.opened"]);

    // No runtime command starts a shell: only the build step may.
    for section in ["panes", "actions", "events"] {
        for (_, command) in commands(section) {
            assert!(
                !["bash", "sh", "powershell", "pwsh"].contains(&command[0].as_str()),
                "{section}: {command:?}"
            );
        }
    }
}

#[test]
fn manifest_builds_with_one_install_script_per_platform() {
    let manifest: toml::Table =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("herdr-plugin.toml"))
            .unwrap()
            .parse()
            .unwrap();
    let strings = |value: &toml::Value| -> Vec<String> {
        value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
    };
    assert_eq!(strings(&manifest["platforms"]), ["macos", "linux", "windows"]);

    let builds: Vec<(Vec<String>, Vec<String>)> = manifest["build"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (strings(&entry["platforms"]), strings(&entry["command"])))
        .collect();
    let expected: [(&[&str], &[&str]); 2] = [
        (&["macos", "linux"], &["bash", "herdr/install.sh"]),
        (
            &["windows"],
            &[
                "powershell",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "herdr/install.ps1",
            ],
        ),
    ];
    assert_eq!(builds.len(), expected.len());
    for ((platforms, command), (want_platforms, want_command)) in builds.iter().zip(expected) {
        assert_eq!(platforms, want_platforms);
        assert_eq!(command, want_command);
    }
    // herdr runs every build entry whose platforms match, so overlapping sets would run two
    // installers. A `bash` entry matching Windows could reach the WSL launcher and fetch the
    // Linux binary.
    for platform in ["macos", "linux", "windows"] {
        let matching: Vec<_> = builds
            .iter()
            .filter(|(platforms, _)| platforms.iter().any(|p| p == platform))
            .collect();
        assert_eq!(matching.len(), 1, "{platform}");
        assert!(platform != "windows" || matching[0].1[0] != "bash");
    }
}

/// PowerShell 5.1 reads a script without a BOM in the ANSI code page, so one non-ASCII byte (an
/// em dash in a comment) can turn into a quote that ends a string early. The install step runs
/// under 5.1, and so can the Windows CI scripts.
#[test]
fn every_powershell_script_is_ascii() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut scripts = vec![root.join("herdr/install.ps1")];
    for entry in fs::read_dir(root.join("scripts")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "ps1") {
            scripts.push(path);
        }
    }
    assert!(scripts.len() > 1, "{scripts:?}");
    for script in scripts {
        let bytes = fs::read(&script).unwrap();
        let offending: Vec<usize> = (0..bytes.len()).filter(|&at| !bytes[at].is_ascii()).collect();
        assert!(offending.is_empty(), "{}: non-ASCII bytes at {offending:?}", script.display());
    }
}

// --- Pane identity: the foreground process decides, never the label.

#[test]
fn a_pane_running_the_review_ui_counts_however_it_was_launched() {
    let dir = tempfile::tempdir().unwrap();
    // A wrapped launch: `cargo run` holds the group, its child is the review UI, and the
    // pane carries no `reviewr` label at all. The child's title (`name`) is rewritten, so
    // only the executable identifies it.
    procinfo(
        dir.path(),
        "w1:p1",
        &json!([
            process("cargo", &["cargo", "run"]),
            process("target/debug/herdr-reviewr", &["target/debug/herdr-reviewr"]),
        ]),
    );

    let output = run("open", dir.path());
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: already open (w1:p1) in workspace-1\n");
    assert!(
        !calls(dir.path()).contains("plugin pane open"),
        "an open over a live pane must not stack another"
    );

    // `close` sweeps the same pane by the same live read, with a plain `pane close`.
    let output = run("close", dir.path());
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: closed w1:p1 in workspace-1\n");
    let calls = calls(dir.path());
    assert!(calls.lines().any(|l| l == "pane close w1:p1"), "{calls}");
}

#[test]
fn a_windows_pane_counts_through_its_one_reported_process() {
    let dir = tempfile::tempdir().unwrap();
    // herdr on Windows reports exactly one process per pane, with a backslashed path and the
    // `.exe` suffix. The suffix arrives in either case: herdr's pty layer resolves the manifest's
    // extension-less command through PATHEXT, whose entries are uppercase.
    for exe in [
        r"C:\Users\me\.config\herdr\plugins\github\persiyanov.reviewr-1a2b\bin\herdr-reviewr.exe",
        r"C:\Users\me\.config\herdr\plugins\github\persiyanov.reviewr-1a2b\bin\herdr-reviewr.EXE",
    ] {
        procinfo(dir.path(), "w1:p1", &json!([process(exe, &[exe])]));

        let output = run("open", dir.path());

        assert!(output.status.success(), "{exe}: {}", stderr(&output));
        assert_eq!(stdout(&output), "reviewr: already open (w1:p1) in workspace-1\n", "{exe}");
    }
}

#[test]
fn a_review_ui_started_with_ui_flags_still_counts() {
    let dir = tempfile::tempdir().unwrap();
    procinfo(
        dir.path(),
        "w1:p1",
        &json!([process("herdr-reviewr", &["herdr-reviewr", "--base", "main", "/repo"])]),
    );

    let output = run("close", dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: closed w1:p1 in workspace-1\n");
}

#[test]
fn a_flag_run_never_counts_as_the_review_ui() {
    let dir = tempfile::tempdir().unwrap();
    // The review binary run for a non-UI flag is not the review UI, so `open` opens a fresh
    // pane over it: the config flag, and an action that is itself still running.
    let flag_runs: [&[&str]; 2] =
        [&["herdr-reviewr", "--resolve-plugin-config"], &["herdr-reviewr", "--action", "toggle"]];
    for argv in flag_runs {
        reset(dir.path());
        procinfo(dir.path(), "w1:p1", &json!([process("herdr-reviewr", argv)]));

        let output = run_open(dir.path());

        assert!(output.status.success(), "{argv:?}: {}", stderr(&output));
        assert!(
            calls(dir.path()).contains("plugin pane open"),
            "{argv:?}: a flag run must not read as open: {}",
            calls(dir.path())
        );
    }
}

#[test]
fn the_flag_dispatch_matches_the_actions_anywhere_in_argv() {
    // The other half of the flag-run contract, pinned in the binary itself: the actions
    // exclude a non-UI flag wherever it sits in argv, so the binary must dispatch it there too,
    // or a flag run would start the review UI while the actions refuse to count it.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "theme = \"gruvbox\"\n").unwrap();

    let output = Command::new(reviewr_bin())
        .args(["--some-future-arg", "--resolve-plugin-config"])
        .env("HERDR_PLUGIN_CONFIG_DIR", dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("\"theme\""), "expected config JSON: {}", stdout(&output));

    // The flag after a UI argument still dispatches to the action, never the review UI.
    let mut close = action("close", dir.path());
    let output = close.args(["/some/repo"]).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: nothing open in workspace-1\n");
    let output = Command::new(reviewr_bin())
        .args(["/some/repo", "--action", "close"])
        .env("HERDR_PLUGIN_CONFIG_DIR", dir.path())
        .env("HERDR_PLUGIN_STATE_DIR", dir.path())
        .env("HERDR_BIN_PATH", fake_herdr())
        .env("FAKE_HERDR_DIR", dir.path())
        .env("HERDR_WORKSPACE_ID", "workspace-1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: nothing open in workspace-1\n");
}

#[test]
fn close_sweeps_every_reviewr_pane_and_a_close_that_lost_the_race_still_converges() {
    let dir = tempfile::tempdir().unwrap();
    // w1:p2 is a plain shell wearing a stale `reviewr` label — a crashed binary's leftover.
    // The label is display only and never read, so the sweep below must not touch it.
    panes(
        dir.path(),
        &json!([{"pane_id": "w1:p1"}, {"pane_id": "w1:p2", "label": "reviewr"}, {"pane_id": "w1:p3"}]),
    );
    procinfo(dir.path(), "w1:p1", &json!([review_ui()]));
    procinfo(dir.path(), "w1:p3", &json!([review_ui()]));
    // w1:p3's close fails with the pane gone: it exited between the read and the close.
    // The sweep still exits 0 — the end state is the same.
    fs::write(fixture(dir.path(), "closefail", "w1:p3", ""), herdr_error("pane_not_found"))
        .unwrap();

    let output = run("close", dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: closed w1:p1 w1:p3 in workspace-1\n");
    let calls = calls(dir.path());
    // Whole log lines, so a `plugin pane close` could not satisfy the plain-`pane close`
    // contract these assert.
    assert!(calls.lines().any(|l| l == "pane close w1:p1"), "{calls}");
    assert!(calls.lines().any(|l| l == "pane close w1:p3"), "{calls}");
    assert!(
        !calls.contains("pane close w1:p2"),
        "a labeled plain shell must not be swept: {calls}"
    );
}

#[test]
fn open_over_several_reviewr_panes_names_them_all() {
    let dir = tempfile::tempdir().unwrap();
    panes(dir.path(), &json!([{"pane_id": "w1:p1"}, {"pane_id": "w1:p2"}, {"pane_id": "w1:p3"}]));
    procinfo(dir.path(), "w1:p1", &json!([review_ui()]));
    procinfo(dir.path(), "w1:p3", &json!([review_ui()]));

    let output = run("open", dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: already open (w1:p1 w1:p3) in workspace-1\n");
}

#[test]
fn a_close_that_fails_for_a_live_pane_sweeps_the_rest_then_refuses() {
    let dir = tempfile::tempdir().unwrap();
    panes(dir.path(), &json!([{"pane_id": "w1:p1"}, {"pane_id": "w1:p3"}]));
    procinfo(dir.path(), "w1:p1", &json!([review_ui()]));
    procinfo(dir.path(), "w1:p3", &json!([review_ui()]));
    // w1:p1's close fails with the pane still there — a wedged herdr, not the benign
    // exited-between-read-and-close race. Reporting it closed would leave a running pane
    // the user believes gone, so the sweep refuses.
    fs::write(fixture(dir.path(), "closefail", "w1:p1", ""), herdr_error("internal")).unwrap();

    let output = run("close", dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "reviewr: herdr pane close failed for w1:p1 in workspace-1\n");
    // The refusal comes after the sweep, so the panes herdr could close are closed.
    let calls = calls(dir.path());
    assert!(calls.lines().any(|l| l == "pane close w1:p3"), "{calls}");
}

#[test]
fn a_gone_pane_skips_and_an_unreadable_read_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let procfail = fixture(dir.path(), "procfail", "w1:p1", ".json");
    // The read reports the pane gone: it exited between the list and the read, so the
    // action converges — this close has nothing to sweep and exits 0.
    fs::write(&procfail, herdr_error("pane_not_found")).unwrap();
    let output = run("close", dir.path());
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: nothing open in workspace-1\n");

    // Any other read failure refuses, never reads as "no reviewr pane": an open would
    // stack a duplicate and a close would false-succeed.
    fs::write(&procfail, herdr_error("internal")).unwrap();
    for mode in ["open", "close", "toggle"] {
        let output = run(mode, dir.path());
        assert_eq!(output.status.code(), Some(1), "{mode}");
        assert_eq!(
            stderr(&output),
            "reviewr: herdr pane process-info failed in workspace-1\n",
            "{mode}"
        );
    }
}

#[test]
fn a_process_info_answer_missing_its_shape_refuses() {
    let dir = tempfile::tempdir().unwrap();
    // Exit 0 with an error envelope — no `.result.process_info`. A shape failure must refuse
    // like a failed pane list, never read as "no reviewr pane".
    fs::write(fixture(dir.path(), "procinfo", "w1:p1", ".json"), herdr_error("internal")).unwrap();
    for mode in ["open", "close", "toggle"] {
        let output = run(mode, dir.path());
        assert_eq!(output.status.code(), Some(1), "{mode}");
        assert!(stderr(&output).contains("process-info failed"), "{mode}: {}", stderr(&output));
    }
}

#[test]
fn a_failed_pane_list_refuses_rather_than_reading_as_no_pane() {
    let dir = tempfile::tempdir().unwrap();
    // `pane list` answering an error envelope (exit 0, no `.result.panes`) must refuse:
    // read as "no reviewr pane", an open would stack a duplicate and a close would
    // false-succeed with panes still running.
    fs::write(dir.path().join("panes.json"), herdr_error("internal")).unwrap();

    let output = run("close", dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "reviewr: herdr pane list failed for workspace-1\n");
}

#[cfg(unix)]
#[test]
fn an_action_repoints_the_stable_launch_paths_at_the_live_plugin_root() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    // The install's build step runs in a staging checkout herdr renames afterwards, so the
    // actions own the stable links: every valid invocation re-points them at the runtime
    // root. `~/.local/bin` only when it exists.
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("bin")).unwrap();
    fs::write(root.path().join("bin/herdr-reviewr"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(root.path().join("bin/herdr-reviewr"), fs::Permissions::from_mode(0o755))
        .unwrap();
    let run_close = || {
        action("close", dir.path())
            .env("HERDR_PLUGIN_ROOT", root.path())
            .env("HOME", home.path())
            .output()
            .unwrap()
    };

    let output = run_close();

    assert!(output.status.success(), "{}", stderr(&output));
    let state_link =
        home.path().join(".local/state/herdr/plugins/persiyanov.reviewr/bin/herdr-reviewr");
    assert_eq!(fs::read_link(&state_link).unwrap(), root.path().join("bin/herdr-reviewr"));
    let bin_link = home.path().join(".local/bin/herdr-reviewr");
    assert!(!bin_link.exists(), "~/.local/bin must not be created for the link");

    // With `~/.local/bin` present, the second link lands too — and an existing symlink
    // re-points rather than blocks.
    fs::create_dir_all(home.path().join(".local/bin")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/old", &bin_link).unwrap();
    let output = run_close();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fs::read_link(&bin_link).unwrap(), root.path().join("bin/herdr-reviewr"));

    // A link that already names the live binary is left in place.
    let inode =
        |path: &Path| std::os::unix::fs::MetadataExt::ino(&fs::symlink_metadata(path).unwrap());
    let before = inode(&bin_link);
    let output = run_close();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(inode(&bin_link), before, "a current link was replaced");

    // A re-point swaps the link in one step: a launch through it never finds the path
    // missing, and the swap leaves nothing else beside it.
    let other = tempfile::tempdir().unwrap();
    fs::create_dir_all(other.path().join("bin")).unwrap();
    fs::copy(root.path().join("bin/herdr-reviewr"), other.path().join("bin/herdr-reviewr"))
        .unwrap();
    let done = std::sync::atomic::AtomicBool::new(false);
    let missing = std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let mut missing = 0;
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                if fs::symlink_metadata(&bin_link).is_err() {
                    missing += 1;
                }
            }
            missing
        });
        for round in 0..20 {
            let live = if round % 2 == 0 { other.path() } else { root.path() };
            let output = action("close", dir.path())
                .env("HERDR_PLUGIN_ROOT", live)
                .env("HOME", home.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", stderr(&output));
            assert_eq!(fs::read_link(&bin_link).unwrap(), live.join("bin/herdr-reviewr"));
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap()
    });
    assert_eq!(missing, 0, "the link was missing mid-swap");
    let names: Vec<_> = fs::read_dir(home.path().join(".local/bin"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["herdr-reviewr"]);

    // Anything but a symlink at the path is a user's own, and survives.
    fs::remove_file(&bin_link).unwrap();
    fs::write(&bin_link, "mine").unwrap();
    let output = run_close();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(&bin_link).unwrap(), "mine");
}

// --- The config dir lookup the binary falls back to.

#[test]
fn the_cli_fallback_resolves_the_config_dir_when_the_env_names_none() {
    // The launcher-blind half of config resolution: with no `HERDR_PLUGIN_CONFIG_DIR`, the
    // binary asks `herdr plugin config-dir` and reads the directory it names. This is the one
    // test that exercises the real herdr-CLI path — the unit tests drive the resolver with an
    // injected closure.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "theme = \"gruvbox\"\n").unwrap();

    let output = Command::new(reviewr_bin())
        .arg("--resolve-plugin-config")
        .env_remove("HERDR_PLUGIN_CONFIG_DIR")
        .env("HERDR_BIN_PATH", fake_herdr())
        .env("FAKE_HERDR_DIR", dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("gruvbox"), "expected the CLI-named dir's config");
}

#[test]
fn a_wedged_config_dir_lookup_degrades_to_the_defaults_inside_the_bound() {
    // A herdr that does not answer resolves no directory, the missing-file outcome. The fake
    // hangs 5s, well past the binary's bound, so a success here can only come from giving the
    // lookup up.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "theme = \"gruvbox\"\n").unwrap();
    fs::write(dir.path().join("configdir-hang"), "").unwrap();

    let output = Command::new(reviewr_bin())
        .arg("--resolve-plugin-config")
        .env_remove("HERDR_PLUGIN_CONFIG_DIR")
        .env("HERDR_BIN_PATH", fake_herdr())
        .env("FAKE_HERDR_DIR", dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(!stdout.contains("gruvbox"), "a hung lookup must name no directory: {stdout}");
    assert!(stdout.contains("\"theme\""), "the defaults still print in full: {stdout}");
}

// --- Placement: the shape of the `plugin pane open` call.

#[test]
fn an_open_reports_the_pane_it_opened() {
    let dir = tempfile::tempdir().unwrap();

    let output = run_open(dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: opened w1:p9 (split) in workspace-1\n");
    let open = open_call(dir.path());
    assert_eq!(
        open,
        format!(
            "plugin pane open --plugin persiyanov.reviewr --entrypoint pane --placement split \
             --target-pane w1:p1 --direction right --cwd {} --focus",
            env!("CARGO_MANIFEST_DIR")
        )
    );
}

#[test]
fn an_open_names_the_plugin_herdr_runs_it_as() {
    let dir = tempfile::tempdir().unwrap();
    let context = json!({"focused_pane_cwd": env!("CARGO_MANIFEST_DIR")}).to_string();

    let output = action("open", dir.path())
        .env("HERDR_PLUGIN_ID", "someone.reviewr-fork")
        .env("HERDR_PLUGIN_CONTEXT_JSON", context)
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        open_call(dir.path()).contains("--plugin someone.reviewr-fork "),
        "{}",
        calls(dir.path())
    );
}

#[test]
fn valid_non_default_placement_and_direction_reach_herdr_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");

    let cases = [
        ("toggle_placement = \"overlay\"\n", "--placement overlay", None),
        (
            "toggle_placement = \"split\"\ntoggle_direction = \"down\"\n",
            "--placement split",
            Some("--direction down"),
        ),
    ];
    for (text, placement, direction) in cases {
        fs::write(&config, text).unwrap();
        reset(dir.path());
        let output = run_open(dir.path());
        assert!(output.status.success(), "{}", stderr(&output));
        let open = open_call(dir.path());
        assert!(open.contains(placement), "{open}");
        if let Some(direction) = direction {
            assert!(open.contains(direction), "{open}");
        }
    }
}

#[test]
fn zoomed_placement_attaches_to_the_focused_pane_else_the_first_pane() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "toggle_placement = \"zoomed\"\n").unwrap();
    panes(dir.path(), &json!([{"pane_id": "w1:p4"}, {"pane_id": "w1:p5"}]));
    let context = json!({"focused_pane_cwd": env!("CARGO_MANIFEST_DIR")}).to_string();

    // No focused pane (`HERDR_PANE_ID` unset): the workspace's first pane.
    let output =
        action("open", dir.path()).env("HERDR_PLUGIN_CONTEXT_JSON", &context).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let open = open_call(dir.path());
    assert!(open.contains("--placement zoomed --target-pane w1:p4 --cwd"), "{open}");
    assert!(!open.contains("--direction"), "only a split takes a direction: {open}");

    reset(dir.path());
    let output = action("open", dir.path())
        .env("HERDR_PANE_ID", "w1:p5")
        .env("HERDR_PLUGIN_CONTEXT_JSON", &context)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(open_call(dir.path()).contains("--target-pane w1:p5"), "{}", calls(dir.path()));
}

#[test]
fn a_split_with_no_pane_to_attach_to_refuses() {
    let dir = tempfile::tempdir().unwrap();
    panes(dir.path(), &json!([]));
    let context = json!({"focused_pane_cwd": env!("CARGO_MANIFEST_DIR")}).to_string();

    let output =
        action("open", dir.path()).env("HERDR_PLUGIN_CONTEXT_JSON", context).output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "reviewr: no pane to attach to in workspace-1\n");
    assert!(!calls(dir.path()).contains("plugin pane open"), "{}", calls(dir.path()));
}

#[test]
fn tab_placement_open_names_its_fresh_tab() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "toggle_placement = \"tab\"\n").unwrap();

    let output = run_open(dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(open_call(dir.path()).contains("--placement tab --workspace workspace-1"));
    let calls = calls(dir.path());
    assert!(calls.lines().any(|l| l == "tab rename w1:t9 reviewr"), "{calls}");
}

#[test]
fn split_placement_open_renames_no_tab() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("config.toml"), "toggle_placement = \"split\"\n").unwrap();

    let output = run_open(dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!calls(dir.path()).contains("tab rename"), "{}", calls(dir.path()));
}

#[test]
fn a_manual_open_passes_focus() {
    let dir = tempfile::tempdir().unwrap();
    let context = json!({"focused_pane_cwd": env!("CARGO_MANIFEST_DIR")}).to_string();

    for mode in ["open", "toggle"] {
        reset(dir.path());
        let output = run_with_context(mode, dir.path(), &context);
        assert!(output.status.success(), "{mode}: {}", stderr(&output));
        let open = open_call(dir.path());
        let tokens: Vec<&str> = open.split_whitespace().collect();
        assert!(tokens.contains(&"--focus"), "{mode} must pass --focus: {open}");
        assert!(!tokens.contains(&"--no-focus"), "{mode} must not pass --no-focus: {open}");
    }
}

// --- An open returns once its pane reads as reviewr.

/// The reads of the opened pane `w1:p9` a run made.
fn opened_pane_reads(dir: &Path) -> usize {
    calls(dir).lines().filter(|l| *l == "pane process-info --pane w1:p9").count()
}

#[test]
fn an_open_waits_until_its_pane_reads_as_reviewr() {
    let dir = tempfile::tempdir().unwrap();
    // herdr's Windows process snapshot lags a fresh pane: its first reads come back empty.
    fs::write(dir.path().join("opened-empty-reads"), "2").unwrap();

    let output = run_open(dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: opened w1:p9 (split) in workspace-1\n");
    // Two empty reads (few enough to fit the bound on a slow runner), then the read that
    // sees reviewr, and no read after it.
    assert_eq!(opened_pane_reads(dir.path()), 3, "{}", calls(dir.path()));
}

#[test]
fn an_open_whose_pane_never_reads_as_reviewr_succeeds_after_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("opened-empty-reads"), "1000000").unwrap();

    let started = Instant::now();
    let output = run_open(dir.path());
    let elapsed = started.elapsed();

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: opened w1:p9 (split) in workspace-1\n");
    assert!(elapsed >= Duration::from_millis(5900), "returned before the bound: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(15), "the wait is bounded: {elapsed:?}");
    assert!(opened_pane_reads(dir.path()) > 1, "{}", calls(dir.path()));
}

// --- Actions serialize on the lock in the plugin state dir.

/// Workspace `ws`'s action lock, held by the test process as another action would hold it.
fn hold_lock(dir: &Path, ws: &str) -> fs::File {
    let file = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(format!("action-{ws}.lock")))
        .unwrap();
    file.lock().unwrap();
    file
}

/// Start `mode` with a focused pane in this crate's repo, its output captured.
fn start(mode: &str, dir: &Path) -> Child {
    with_context(mode, dir, &repo_context())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn two_concurrent_toggles_open_then_close() {
    let dir = tempfile::tempdir().unwrap();
    // herdr's Windows process snapshot lags a fresh pane, so the opened pane first reads empty.
    fs::write(dir.path().join("opened-empty-reads"), "2").unwrap();

    let first = start("toggle", dir.path());
    let second = start("toggle", dir.path());
    let mut lines = [first, second].map(|child| {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        stdout(&output)
    });
    lines.sort();

    assert_eq!(
        lines,
        [
            "reviewr: closed w1:p9 in workspace-1\n".to_owned(),
            "reviewr: opened w1:p9 (split) in workspace-1\n".to_owned(),
        ]
    );
    let effects: Vec<_> = calls(dir.path())
        .lines()
        .filter(|line| line.starts_with("plugin pane open") || line.starts_with("pane close"))
        .map(|line| line.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(effects, ["plugin pane open", "pane close w1:p9"]);
}

#[test]
fn an_explicit_action_waits_for_a_held_lock_and_proceeds_once_released() {
    let dir = tempfile::tempdir().unwrap();
    let lock = hold_lock(dir.path(), "workspace-1");

    let mut child = start("toggle", dir.path());
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "the toggle did not wait for the lock");
    assert!(!herdr_called(dir.path()), "{}", calls(dir.path()));
    drop(lock);
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: opened w1:p9 (split) in workspace-1\n");
}

#[test]
fn an_explicit_action_refuses_once_the_lock_stays_held_past_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let _lock = hold_lock(dir.path(), "workspace-1");

    let started = Instant::now();
    let children = ["toggle", "open", "close"].map(|mode| (mode, start(mode, dir.path())));
    for (mode, child) in children {
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{mode}");
        assert_eq!(
            stderr(&output),
            "reviewr: another reviewr action in workspace-1 is still running after 15s\n",
            "{mode}"
        );
        assert!(output.stdout.is_empty(), "{mode}");
    }
    let elapsed = started.elapsed();

    assert!(elapsed >= Duration::from_millis(14500), "refused before the bound: {elapsed:?}");
    assert!(!herdr_called(dir.path()), "{}", calls(dir.path()));
}

#[test]
fn auto_open_yields_silently_to_a_held_lock() {
    let dir = tempfile::tempdir().unwrap();
    let _lock = hold_lock(dir.path(), "workspace-9");
    let event = worktree_event("worktree_created", "workspace-9", env!("CARGO_MANIFEST_DIR"), None);

    let started = Instant::now();
    let output = run_auto_open(dir.path(), &event, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty(), "{}", stdout(&output));
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
    assert!(started.elapsed() < Duration::from_secs(3), "the event waited for the lock");
    assert!(!herdr_called(dir.path()), "{}", calls(dir.path()));
}

#[test]
fn a_held_lock_holds_back_only_its_own_workspace() {
    // The race is per workspace, and so is the lock: an action in another workspace, or a new
    // worktree's auto-open, never waits on this one.
    let dir = tempfile::tempdir().unwrap();
    let _lock = hold_lock(dir.path(), "workspace-2");
    let event = worktree_event("worktree_created", "workspace-9", env!("CARGO_MANIFEST_DIR"), None);

    let started = Instant::now();
    let toggle = run_with_context("toggle", dir.path(), &repo_context());
    let born = run_auto_open(dir.path(), &event, None);

    assert!(started.elapsed() < Duration::from_secs(5), "an action waited on another workspace");
    assert!(toggle.status.success(), "{}", stderr(&toggle));
    assert_eq!(stdout(&toggle), "reviewr: opened w1:p9 (split) in workspace-1\n");
    assert!(born.status.success(), "{}", stderr(&born));
    // The event read its own workspace instead of yielding to the held lock.
    let log = calls(dir.path());
    assert!(log.contains("pane list --workspace workspace-9"), "{log}");
}

#[test]
fn auto_open_over_an_open_reviewr_pane_does_nothing_and_says_nothing() {
    // The workspace already shows reviewr, whoever opened it: the birth event neither stacks a
    // second pane nor closes the first, and stays silent.
    let dir = tempfile::tempdir().unwrap();
    procinfo(dir.path(), "w1:p1", &json!([review_ui()]));
    let event = worktree_event("worktree_created", "workspace-9", env!("CARGO_MANIFEST_DIR"), None);

    let output = run_auto_open(dir.path(), &event, None);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty(), "{}", stdout(&output));
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
    let log = calls(dir.path());
    assert!(!log.contains("plugin pane open") && !log.contains("pane close"), "{log}");
}

#[test]
fn a_lock_held_by_a_crashed_action_frees_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    // The first toggle takes the lock, then hangs in its pane listing until it is killed.
    fs::write(dir.path().join("list-hang"), "").unwrap();
    let mut crashed = start("toggle", dir.path());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !calls(dir.path()).contains("pane list") {
        assert!(Instant::now() < deadline, "the first toggle never listed panes");
        std::thread::sleep(Duration::from_millis(20));
    }
    crashed.kill().unwrap();
    crashed.wait().unwrap();

    let output = run_with_context("toggle", dir.path(), &repo_context());

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "reviewr: opened w1:p9 (split) in workspace-1\n");
}

#[test]
fn an_action_without_a_plugin_state_dir_refuses_before_any_herdr_call() {
    let dir = tempfile::tempdir().unwrap();

    let output = with_context("toggle", dir.path(), &repo_context())
        .env_remove("HERDR_PLUGIN_STATE_DIR")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr(&output), "reviewr: no plugin state dir (invoke as a herdr plugin action)\n");
    assert!(!herdr_called(dir.path()), "{}", calls(dir.path()));
}

// --- Open cwd: the focused pane's live foreground cwd, then the context's launch cwd.

#[test]
fn open_prefers_the_focused_panes_live_foreground_cwd() {
    let dir = tempfile::tempdir().unwrap();
    // The launch cwd is not a git repo: an open that trusted it would refuse. The pane's
    // live foreground cwd is the reviewed repo — the `claude -w <worktree>` shape, where
    // the agent chdirs into the worktree only inside its own process after launching
    // from the main checkout. The live cwd comes from the pane-list snapshot, so the
    // open pays no extra herdr call.
    let context = json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": dir.path()}).to_string();
    pane_with_cwd(dir.path(), "w1:p1", Path::new(env!("CARGO_MANIFEST_DIR")));

    let output = run_with_context("open", dir.path(), &context);

    assert!(output.status.success(), "{}", stderr(&output));
    let calls = calls(dir.path());
    assert!(
        open_call(dir.path()).contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))),
        "the open must use the live foreground cwd: {calls}"
    );
    // The live cwd comes from the run's one snapshot; a second listing would put a herdr
    // round-trip back on the keypress path.
    assert_eq!(
        calls.matches("pane list --workspace").count(),
        1,
        "the open must reuse the held pane-list snapshot: {calls}"
    );
}

#[test]
fn open_prefers_the_live_cwd_when_the_launch_cwd_is_also_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    // The motivating shape exactly: the launch cwd is itself a valid repo (the main
    // checkout `claude -w <worktree>` was launched from), so a fallback-only read would
    // pass every other test and still review the wrong repo. The live cwd must win.
    let launch_repo = init_repo(dir.path(), "main-checkout");
    let context = json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": launch_repo}).to_string();
    pane_with_cwd(dir.path(), "w1:p1", Path::new(env!("CARGO_MANIFEST_DIR")));

    let output = run_with_context("open", dir.path(), &context);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        open_call(dir.path()).contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))),
        "the live cwd must win over a launch cwd that is also a repo: {}",
        calls(dir.path())
    );
}

#[test]
fn open_keeps_the_context_cwd_without_a_live_foreground_cwd() {
    let dir = tempfile::tempdir().unwrap();
    // The fake's default pane-list entry carries no foreground cwd — a pane whose live
    // read has nothing to add keeps the context cwd instead of losing it.
    let context =
        json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": env!("CARGO_MANIFEST_DIR")})
            .to_string();

    let output = run_with_context("open", dir.path(), &context);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        open_call(dir.path()).contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))),
        "the open must fall back to the context cwd: {}",
        calls(dir.path())
    );
}

#[test]
fn open_falls_back_to_the_workspace_cwd_without_a_focused_pane_cwd() {
    let dir = tempfile::tempdir().unwrap();
    // A workspace-context invocation carries no focused pane cwd, only the workspace's.
    let context = json!({"workspace_cwd": env!("CARGO_MANIFEST_DIR")}).to_string();

    let output = run_with_context("open", dir.path(), &context);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        open_call(dir.path()).contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))),
        "{}",
        calls(dir.path())
    );
}

#[test]
fn a_toggle_open_falls_back_when_the_live_cwd_is_not_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    // The live foreground cwd sits outside any git repo — a shell that wandered off. The
    // live cwd wins only inside a repo; here it yields to the context cwd rather than
    // refusing an open the context alone could place. Run as a toggle, so the opening toggle
    // exercises the same path.
    let context =
        json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": env!("CARGO_MANIFEST_DIR")})
            .to_string();
    pane_with_cwd(dir.path(), "w1:p1", dir.path());

    let output = run_with_context("toggle", dir.path(), &context);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        open_call(dir.path()).contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))),
        "a non-repo live cwd must fall back to the context cwd: {}",
        calls(dir.path())
    );
}

#[test]
fn open_takes_the_focused_panes_cwd_not_another_panes() {
    let dir = tempfile::tempdir().unwrap();
    // Two panes, both in repos: a decoy listed first and the focused pane after it. The
    // lookup must key on the focused pane's id — a first-entry read would review the
    // decoy's repo.
    let decoy_repo = init_repo(dir.path(), "decoy-repo");
    panes(
        dir.path(),
        &json!([
            {"pane_id": "w1:p0", "foreground_cwd": decoy_repo},
            {"pane_id": "w1:p1", "foreground_cwd": env!("CARGO_MANIFEST_DIR")},
        ]),
    );
    let context = json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": dir.path()}).to_string();

    let output = run_with_context("open", dir.path(), &context);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        open_call(dir.path()).contains(&format!("--cwd {}", env!("CARGO_MANIFEST_DIR"))),
        "the open must use the focused pane's cwd, not the decoy's: {}",
        calls(dir.path())
    );
}

#[test]
fn a_refusal_names_the_rejected_live_cwd_too() {
    let dir = tempfile::tempdir().unwrap();
    // No context cwd and a non-repo live cwd: the open refuses, and the one stderr line
    // names the live directory it inspected and rejected — a refusal that hid it would
    // read as if no directory was ever tried.
    let context = json!({"focused_pane_id": "w1:p1"}).to_string();
    pane_with_cwd(dir.path(), "w1:p1", dir.path());

    let output = run_with_context("open", dir.path(), &context);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        format!("reviewr: not a git repo: '<no cwd>' (live cwd '{}')\n", dir.path().display())
    );
}
