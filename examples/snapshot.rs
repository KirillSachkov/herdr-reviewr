//! Paint reviewr's real frames, for every theme, as colored HTML: one fragment per scene and
//! theme, so two builds can be compared side by side by eye.
//!
//! Usage: `cargo run --example snapshot -- <out-dir>`. Run it with the herdr environment
//! stripped (`env -u HERDR_WORKSPACE_ID -u HERDR_PANE_ID HERDR_BIN_PATH=false`): nothing here
//! sends, but a scene must never reach a live agent pane.
//!
//! Each scene is a fixture repo driven through the app's public entry points, drawn by
//! `ui::render` into a `TestBackend`. The terminal's own background and foreground stand in as
//! the theme's `base` and `text`, the precondition reviewr's colors are designed against.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use herdr_reviewr::app::{App, Focus, Tab};
use herdr_reviewr::export::ExportTarget;
use herdr_reviewr::forge::{
    Check, CheckStatus, Comment, CommentKind, Merge, PrSnapshot, PrState, PrView, Sync,
};
use herdr_reviewr::keymap::Keymap;
use herdr_reviewr::model::Scope;
use herdr_reviewr::{handle_key, handle_mouse, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

const W: u16 = 120;
const H: u16 = 34;

const THEMES: &[&str] = &[
    "catppuccin",
    "catppuccin-frappe",
    "catppuccin-macchiato",
    "catppuccin-latte",
    "tokyo-night",
    "tokyo-night-day",
    "dracula",
    "nord",
    "gruvbox",
    "gruvbox-light",
    "one-dark",
    "one-light",
    "solarized",
    "solarized-light",
    "github-light",
    "monokai",
    "rose-pine",
    "rose-pine-dawn",
    "ayu",
    "everforest",
];

/// A scene: its name, and how to build its app on the fixture repo for one theme.
type Scene = (&'static str, fn(&Path, &str) -> App);

const SCENES: &[Scene] = &[
    ("diff", scene_diff),
    ("comments", scene_comment_list),
    ("find", scene_find),
    ("markdown", scene_markdown),
    ("selection", scene_selection),
    ("pr", scene_pr),
    ("quit", scene_quit),
];

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: snapshot <out-dir>"));
    let repo = fixture_repo();
    for theme in THEMES {
        let dir = out.join(theme);
        std::fs::create_dir_all(&dir).expect("out dir");
        for (name, scene) in SCENES {
            let app = scene(repo.path(), theme);
            let html = frame_html(&app);
            std::fs::write(dir.join(format!("{name}.html")), html).expect("write fragment");
        }
    }
    println!("{} themes × {} scenes → {}", THEMES.len(), SCENES.len(), out.display());
}

/// A temp repo with a Rust file and a README, both edited since the commit: modified, added and
/// removed lines in each.
fn fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let git = |args: &[&str]| {
        let ok = Command::new("git").arg("-C").arg(dir.path()).args(args).status().unwrap();
        assert!(ok.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "s@example.com"]);
    git(&["config", "user.name", "snapshot"]);
    std::fs::write(dir.path().join("src.rs"), RUST_BEFORE).unwrap();
    std::fs::write(dir.path().join("README.md"), MD_BEFORE).unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(dir.path().join("src.rs"), RUST_AFTER).unwrap();
    std::fs::write(dir.path().join("README.md"), MD_AFTER).unwrap();
    dir
}

const RUST_BEFORE: &str = r"use std::collections::HashMap;

/// Totals the line items of one order.
pub fn total(items: &[Item]) -> u64 {
    let mut sum = 0;
    for item in items {
        sum += item.price * item.qty;
    }
    sum
}

pub struct Item {
    pub price: u64,
    pub qty: u64,
}

// TODO: remove once callers migrate.
pub fn legacy_total(items: &[Item]) -> u64 {
    total(items)
}
";

const RUST_AFTER: &str = r"use std::collections::HashMap;

/// Totals the line items of one order, after discounts.
pub fn total(items: &[Item], discounts: &HashMap<String, u64>) -> u64 {
    let mut sum = 0;
    for item in items {
        let off = discounts.get(&item.sku).copied().unwrap_or(0);
        sum += item.price.saturating_sub(off) * item.qty;
    }
    sum
}

pub struct Item {
    pub sku: String,
    pub price: u64,
    pub qty: u64,
}
";

const MD_BEFORE: &str = "# Orders\n\nTotals line items for one order.\n\n## Usage\n\n- Call `total` with the items.\n- The result is in cents.\n\nLegacy callers use `legacy_total`.\n";

const MD_AFTER: &str = "# Orders\n\nTotals line items for one order, after discounts.\n\n## Usage\n\n- Call `total` with the items and a discount map.\n- The result is in cents.\n- Unknown SKUs get no discount.\n";

fn app_on(repo: &Path, theme: &str) -> App {
    let mut app = App::new(repo.to_path_buf(), Scope::Uncommitted, None);
    app.set_cli_theme(Some(theme.to_string()));
    app.reload().expect("reload");
    app
}

fn press(app: &mut App, code: KeyCode) {
    let area = Rect::new(0, 0, W, H);
    handle_key(app, KeyEvent::from(code), area, &Keymap::default()).expect("key");
}

fn open(app: &mut App, path: &str) {
    let row = app
        .file_rows
        .iter()
        .position(|r| r.file_index().is_some_and(|i| app.entries[i].path == path))
        .expect("file row");
    app.select_file(row).expect("select");
    app.focus = Focus::Diff;
}

/// The cursor on the first added line whose text contains `needle`.
fn cursor_to(app: &mut App, needle: &str) {
    app.diff_cursor =
        app.visible.iter().position(|r| r.text().contains(needle)).expect("row with needle");
}

fn comment(app: &mut App, needle: &str, text: &str) {
    cursor_to(app, needle);
    app.start_comment();
    for ch in text.chars() {
        app.input_push(ch);
    }
    app.submit_comment();
}

/// A diff with two comments, the cursor on a commented line.
fn scene_diff(repo: &Path, theme: &str) -> App {
    let mut app = app_on(repo, theme);
    open(&mut app, "src.rs");
    comment(&mut app, "discounts.get", "unwrap_or(0) hides a missing SKU");
    comment(&mut app, "saturating_sub", "say why a discount can exceed the price");
    cursor_to(&mut app, "discounts.get");
    app.status.clear();
    app
}

/// The comment list open over the diff.
fn scene_comment_list(repo: &Path, theme: &str) -> App {
    let mut app = scene_diff(repo, theme);
    app.open_list();
    app
}

/// A find band with matches lit.
fn scene_find(repo: &Path, theme: &str) -> App {
    let mut app = app_on(repo, theme);
    open(&mut app, "src.rs");
    app.open_find();
    for ch in "item".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    app
}

/// Rendered markdown with its change marks.
fn scene_markdown(repo: &Path, theme: &str) -> App {
    let mut app = app_on(repo, theme);
    open(&mut app, "README.md");
    app.toggle_rendered();
    app
}

/// A text selection dragged across two diff rows, settled after the release.
fn scene_selection(repo: &Path, theme: &str) -> App {
    let mut app = app_on(repo, theme);
    open(&mut app, "src.rs");
    let area = Rect::new(0, 0, W, H);
    let inner = ui::read_inner_rect(area, &app);
    // Hit-testing reads the last painted frame, so each event follows a draw, as in the loop.
    let mouse = |app: &mut App, kind, column, row| {
        let mut terminal = Terminal::new(TestBackend::new(W, H)).expect("terminal");
        terminal.draw(|f| ui::render(f, app)).expect("draw");
        let heights = ui::diff_row_heights(app, area);
        let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
        handle_mouse(app, event, area, &heights, &Keymap::default(), &NoClipboard).expect("mouse");
    };
    let (x0, y0) = (inner.x + 12, inner.y + 3);
    let (x1, y1) = (inner.x + 40, inner.y + 4);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x0, y0);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), x1, y1);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), x1, y1);
    app.status.clear();
    app
}

/// The PR tab with every check and comment kind.
fn scene_pr(repo: &Path, theme: &str) -> App {
    let mut app = app_on(repo, theme);
    app.set_tab(Tab::Pr).expect("pr tab");
    let check = |name: &str, status| Check { name: name.into(), status };
    let note = |author: &str, bot: bool, body: &str| Comment {
        kind: CommentKind::Comment,
        author: author.into(),
        author_is_bot: bot,
        anchor: "comment".into(),
        place: None,
        body: body.into(),
        snippet: None,
        created_at: "2026-10-01T12:00:00Z".into(),
        is_resolved: false,
        is_outdated: false,
        replies: Vec::new(),
    };
    app.pr = PrView::Pr(Box::new(PrSnapshot {
        number: 42,
        title: "Apply discounts to order totals".into(),
        body: "Adds per-SKU discounts.\n\n- [x] tests\n- [ ] docs".into(),
        url: "https://example.com/pr/42".into(),
        state: PrState::Open,
        is_draft: false,
        head_ref: "discounts".into(),
        head_is_fork: false,
        head_oid: String::new(),
        base_ref: "main".into(),
        merge: Merge::Clean,
        sync: Sync::Behind(2),
        checks: vec![
            check("build", CheckStatus::Success),
            check("lint", CheckStatus::Failure),
            check("e2e", CheckStatus::Running),
            check("deploy-preview", CheckStatus::Skipped),
        ],
        comments: vec![
            note("ann", false, "Looks right. One nit on the rounding."),
            note("ci-bot", true, "Coverage 87% (+1.2%)."),
        ],
        comments_truncated: false,
        checks_truncated: false,
    }));
    app
}

/// The quit question over a diff with comments.
fn scene_quit(repo: &Path, theme: &str) -> App {
    let mut app = scene_diff(repo, theme);
    app.request_quit();
    app
}

/// A copy target that keeps nothing: a scene's drag must never touch the real clipboard.
struct NoClipboard;

impl ExportTarget for NoClipboard {
    fn export(&self, _text: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn label(&self) -> &'static str {
        "none"
    }
    fn success_message(&self, _count: usize) -> String {
        String::new()
    }
    fn failure_message(&self, _error: &anyhow::Error) -> String {
        String::new()
    }
}

/// The frame as a `<pre>` of styled runs. `Color::Reset` is the terminal's own color, which
/// the precondition makes the theme's `base` and `text`.
fn frame_html(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(W, H)).expect("terminal");
    terminal.draw(|f| ui::render(f, app)).expect("draw");
    let buf: Buffer = terminal.backend().buffer().clone();
    let p = app.palette();
    let (base, text) = (hex(p.base, "#000"), hex(p.text, "#fff"));
    let mut html = format!("<pre class=\"frame\" style=\"background:{base};color:{text}\">");
    for y in 0..H {
        let mut run = String::new();
        let mut style = String::new();
        for x in 0..W {
            let cell = &buf[(x, y)];
            let mut s = String::new();
            if cell.fg != Color::Reset {
                let _ = write!(s, "color:{};", hex(cell.fg, &text));
            }
            if cell.bg != Color::Reset {
                let _ = write!(s, "background:{};", hex(cell.bg, &base));
            }
            if cell.modifier.contains(Modifier::BOLD) {
                s.push_str("font-weight:700;");
            }
            if cell.modifier.contains(Modifier::UNDERLINED) {
                s.push_str("text-decoration:underline;");
            }
            if cell.modifier.contains(Modifier::ITALIC) {
                s.push_str("font-style:italic;");
            }
            if s != style {
                flush(&mut html, &run, &style);
                run.clear();
                style = s;
            }
            run.push_str(cell.symbol());
        }
        flush(&mut html, &run, &style);
        html.push('\n');
    }
    html.push_str("</pre>");
    html
}

fn flush(html: &mut String, run: &str, style: &str) {
    if run.is_empty() {
        return;
    }
    let escaped = run.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    if style.is_empty() {
        html.push_str(&escaped);
    } else {
        let _ = write!(html, "<span style=\"{style}\">{escaped}</span>");
    }
}

fn hex(color: Color, fallback: &str) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.to_string(),
    }
}
