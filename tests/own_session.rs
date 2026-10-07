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
    assert!(out.contains("own (claude, idle)"), "the bar names the agent:\n{out}");
    assert!(out.contains("Нужно от вас: принять план"), "the summary says what is needed:\n{out}");
    assert!(out.contains("Сводка"), "the summary leads the list:\n{out}");
    assert!(out.contains("Артефакты · 1"), "a tier group:\n{out}");
    assert!(out.contains("plan.md"), "its file:\n{out}");
    assert!(out.contains("Код · 1"), "code has a group:\n{out}");
    assert!(!out.contains("main.rs"), "code starts folded:\n{out}");
}

#[test]
fn the_summary_reads_as_markdown_and_an_artifact_as_its_content() {
    let r = Repo::init();
    let mut app = session_app(&r);
    let out = render(&app);
    assert!(out.contains("Сводка сессии"), "the summary opens first:\n{out}");
    assert!(
        out.contains("`i` строит") || out.contains("i строит"),
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

#[test]
fn the_session_scope_diffs_from_before_the_session_and_reaches_outside_the_repo() {
    use herdr_reviewr::model::{ChangeKind, Scope};
    use herdr_reviewr::session::SessionChange;
    let r = Repo::init();
    r.write("notes.md", "before session\n");
    r.commit_all("init");
    r.write("notes.md", "after session\n");
    let outside = tempfile::tempdir().unwrap();
    let elsewhere = outside.path().canonicalize().unwrap().join("profile.md");
    std::fs::write(&elsewhere, "new rule\n").unwrap();
    let mut app = app_on(&r);
    app.set_scope(Scope::Session).unwrap();
    assert!(app.session_request, "the scope asks for the session");
    let change = |key: String, kind, before: &str| SessionChange {
        key,
        kind,
        additions: 1,
        deletions: 1,
        git: false,
        before: Some(before.into()),
    };
    app.land_session(SessionView {
        changes: vec![
            change("notes.md".into(), ChangeKind::Modified, "before session\n"),
            change(elsewhere.display().to_string(), ChangeKind::Added, ""),
        ],
        ..view(&r)
    });
    land_world(&mut app);
    let out = render(&app);
    assert!(out.contains("[session]"), "the scope chip:\n{out}");
    assert!(out.contains("profile.md"), "a file outside the repo:\n{out}");
    assert!(out.contains("new rule"), "its content, all new:\n{out}");
    app.move_cursor(1).unwrap();
    let out = render(&app);
    assert!(out.contains("before session") && out.contains("after session"), "both sides:\n{out}");
}

#[test]
fn a_diagram_wider_than_the_pane_clips_instead_of_wrapping() {
    let t = herdr_reviewr::theme::resolve(Some("catppuccin"));
    let hl = herdr_reviewr::highlight::Highlighter::new(t.syntax);
    let wide = format!("┌{}┐", "─".repeat(60));
    let text = format!("```text\n{wide}\n│ box │\n```\n");
    let out = herdr_reviewr::markdown::render(&text, 40, &hl, &t.palette);
    let lines: Vec<String> = out
        .lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .filter(|l: &String| !l.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 2, "one row per source line, never wrapped: {lines:?}");
    assert!(lines[0].ends_with('›'), "the cut is marked: {lines:?}");
    assert_eq!(lines[1].trim_end(), "  │ box │", "a line that fits is whole: {lines:?}");
    let narrow = herdr_reviewr::markdown::render("```text\n│ box │\n```\n", 40, &hl, &t.palette);
    assert!(!narrow.lines.iter().any(|l| l.spans.iter().any(|s| s.content.contains('›'))));
}

#[test]
fn an_alert_shows_its_label_and_drops_the_marker() {
    let t = herdr_reviewr::theme::resolve(Some("catppuccin"));
    let hl = herdr_reviewr::highlight::Highlighter::new(t.syntax);
    let out =
        herdr_reviewr::markdown::render("> [!WARNING]\n> Нужно решение.\n", 60, &hl, &t.palette);
    let text: Vec<String> =
        out.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
    assert!(text.iter().any(|l| l.contains("⚠ Warning")), "{text:?}");
    assert!(text.iter().any(|l| l.contains("Нужно решение.")), "{text:?}");
    assert!(!text.iter().any(|l| l.contains("[!WARNING]")), "{text:?}");
}

#[test]
fn a_local_link_opens_its_file_in_files_or_under_the_session_links() {
    let r = Repo::init();
    let mut app = session_app(&r);
    // A repo file opens in `All files`, at its line.
    assert!(app.open_local("docs/plan.md:3"));
    assert_eq!(app.tab, Tab::AllFiles);
    assert_eq!(app.diff_path.as_deref(), Some("docs/plan.md"));
    // A file outside the repo joins the session's links and opens there.
    let outside = tempfile::tempdir().unwrap();
    let note = outside.path().canonicalize().unwrap().join("note.md");
    std::fs::write(&note, "# Заметка\n\nвне репозитория\n").unwrap();
    assert!(app.open_local(&format!("file://{}", note.display())));
    assert_eq!(app.tab, Tab::Session);
    let out = render(&app);
    assert!(out.contains("Ссылки из ответа · 1"), "{out}");
    assert!(out.contains("вне репозитория"), "{out}");
    // A URL and a missing path are not local files.
    assert!(!app.open_local("https://example.com/a.md"));
    assert!(!app.open_local("no/such/file.md"));
}

#[test]
fn keys_work_on_the_russian_layout_and_the_summary_key_shows_in_the_footer() {
    use herdr_reviewr::keymap::{Action, Key, Keymap};
    let keys = Keymap::default();
    assert_eq!(keys.action_for(Key::plain('ш')), Some(Action::Summarize), "ш is i");
    assert_eq!(keys.action_for(Key::plain('ф')), Some(Action::ScopeSession), "ф is a");
    assert_eq!(keys.action_for(Key::plain('Ы')), Some(Action::Submit), "Ы is S");
    assert_eq!(keys.action_for(Key::plain('с')), Some(Action::Comment), "с is c");
    assert_eq!(keys.action_for(Key::plain('.')), Some(Action::Search), "the / key types . there");
    let r = Repo::init();
    let app = session_app(&r);
    let out = render(&app);
    assert!(out.contains("i сводка"), "the footer offers the summary:\n{out}");
    assert!(out.contains("Сводка · не построена"), "the row says it is not built:\n{out}");
}

#[test]
fn a_building_summary_shows_in_the_list_the_bar_and_the_summary() {
    let r = Repo::init();
    let mut app = session_app(&r);
    app.summary_building(true);
    let out = render(&app);
    assert!(out.contains("Сводка · ⟳ строится"), "{out}");
    assert!(out.contains("⟳ сводка строится 0 с"), "{out}");
    assert!(out.contains("Сводка строится, 5–30 секунд"), "{out}");
    app.summary_building(false);
    assert!(!render(&app).contains("строится"));
}
