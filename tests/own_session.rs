//! Own fork: the `Session` tab — its list, its read pane, and its place among the file tabs.

mod common;

use common::{Repo, app_on, enter_tab, land_world};
use herdr_reviewr::app::{App, Tab};
use herdr_reviewr::session::{Agent, Artifact, SessionView, Tier};
use herdr_reviewr::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer.cell((x, y)).map_or(" ", |c| c.symbol()));
        }
        out.push('\n');
    }
    out
}

fn view(repo: &Repo) -> SessionView {
    let artifact = |key: &str, tier, letter| Artifact {
        key: key.into(),
        abs: repo.path_buf().join(key),
        tier,
        letter,
        note: None,
    };
    SessionView {
        agent: Some(Agent {
            kind: "claude".into(),
            session: "s1".into(),
            name: "own".into(),
            cwd: repo.path_buf(),
            status: "idle".into(),
            found_by: "tab",
        }),
        header: agent_desk::desk::header("Задача: #1 · Сейчас: жду · Нужно от тебя: принять план"),
        artifacts: vec![
            artifact("docs/plan.md", Tier::NewDoc, Some('A')),
            artifact("src/main.rs", Tier::Code, Some('M')),
        ],
        ..SessionView::default()
    }
}

fn session_app(r: &Repo) -> App {
    r.write("docs/plan.md", "# План\n\nшаг один\n");
    r.write("src/main.rs", "fn main() {}\n");
    r.commit_all("init");
    let mut app = app_on(r);
    enter_tab(&mut app, Tab::Session);
    app.land_session(view(r));
    land_world(&mut app);
    app
}

#[test]
fn the_tab_lists_the_summary_then_artifacts_by_tier_with_code_folded() {
    let r = Repo::init();
    let app = session_app(&r);
    let out = render(&app);
    assert!(out.contains("4 Сессия"), "the active tab is named:\n{out}");
    assert!(out.contains("Нужно от вас: принять план"), "the need line tops the tab:\n{out}");
    assert!(out.contains("Сводка"), "the summary leads the list:\n{out}");
    assert!(out.contains("Новые документы · 1"), "a tier group:\n{out}");
    assert!(out.contains("plan.md"), "its file:\n{out}");
    assert!(out.contains("Код и конфигурация · 1"), "code has a group:\n{out}");
    assert!(!out.contains("main.rs"), "code starts folded:\n{out}");
}

#[test]
fn the_summary_reads_as_markdown_and_an_artifact_as_its_content() {
    let r = Repo::init();
    let mut app = session_app(&r);
    let out = render(&app);
    assert!(out.contains("Сводка сессии"), "the summary opens first:\n{out}");
    assert!(
        out.contains("`A` строит") || out.contains("A строит"),
        "it says how to build it:\n{out}"
    );
    // Down past the group row to the plan.
    app.move_cursor(1).unwrap();
    app.move_cursor(1).unwrap();
    let out = render(&app);
    assert!(out.contains("шаг один"), "the artifact's content shows:\n{out}");
}

#[test]
fn the_session_tab_keeps_its_place_across_the_other_file_tabs() {
    let r = Repo::init();
    let mut app = session_app(&r);
    app.move_cursor(2).unwrap();
    let cursor = app.file_cursor;
    enter_tab(&mut app, Tab::AllFiles);
    enter_tab(&mut app, Tab::Changes);
    assert!(!render(&app).contains("4 Сессия"), "the tab hides while inactive");
    enter_tab(&mut app, Tab::Session);
    assert_eq!(app.file_cursor, cursor, "the cursor returns where it was");
    assert!(render(&app).contains("шаг один"));
}
