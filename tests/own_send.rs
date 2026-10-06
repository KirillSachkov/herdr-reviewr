//! Own fork: `S` submits to the agent in this pane's own tab, with no picker (decisions 8, 9).

mod common;

use std::env;
use std::fs;
use std::process::Command;

use common::{Repo, app_on, fake_herdr, herdr_calls};
use herdr_reviewr::app::{Focus, Mode};
use herdr_reviewr::handle_key;
use herdr_reviewr::keymap::Keymap;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use tempfile::TempDir;

// Two agents in the workspace, one of them in our tab `w8:t1`.
const SPLIT_AGENTS: &str = r#"{"result":{"agents":[
  {"agent":"claude","agent_status":"idle","pane_id":"w8:p1","tab_id":"w8:t1","workspace_id":"w8","cwd":"/w"},
  {"agent":"codex","agent_status":"idle","pane_id":"w8:p2","tab_id":"w8:t2","workspace_id":"w8","cwd":"/w"}
]}}"#;

#[test]
fn submit_goes_to_this_tabs_agent_as_a_message() {
    if env::var("OWN_SEND_CHILD").is_err() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("agents.json"), SPLIT_AGENTS).unwrap();
        let out = Command::new(env::current_exe().unwrap())
            .args(["--exact", "submit_goes_to_this_tabs_agent_as_a_message", "--nocapture"])
            .env("OWN_SEND_CHILD", "1")
            .env("FAKE_HERDR_DIR", dir.path())
            .env("HERDR_BIN_PATH", fake_herdr())
            .env("HERDR_WORKSPACE_ID", "w8")
            .env("HERDR_PANE_ID", "w8:p9")
            .env("HERDR_TAB_ID", "w8:t1")
            .env_remove("HERDR_SOCKET_PATH")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let calls = herdr_calls(dir.path());
        assert!(
            calls.contains("agent prompt w8:p1 Замечания ревью:"),
            "submitted to w8:p1:\n{calls}"
        );
        assert!(!calls.contains("w8:p2"), "the other tab's agent is never asked:\n{calls}");
        return;
    }
    let r = Repo::init();
    r.write("a.rs", "alpha\n");
    r.commit_all("init");
    r.write("a.rs", "alpha\nbeta\n");
    let mut app = app_on(&r);
    app.focus = Focus::Diff;
    app.diff_cursor = app.visible.iter().position(|row| row.marker() == '+').unwrap();
    app.start_comment();
    app.input = "rename this".into();
    app.submit_comment();
    let key = KeyEvent::from(KeyCode::Char('S'));
    handle_key(&mut app, key, Rect::new(0, 0, 80, 24), &Keymap::default()).unwrap();
    assert_ne!(app.mode, Mode::Picker, "our tab's agent needs no picker");
    assert!(app.store.is_empty(), "a submitted review is consumed: {}", app.status);
    assert!(app.status.starts_with("submitted 1 comment"), "{}", app.status);
}
