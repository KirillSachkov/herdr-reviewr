//! Own fork: the `Session` tab — this Herdr tab's agent session, its artifacts, and summary.
//! Decisions 3, 6, 7, 11 in `docs/own/decisions.md`; the data comes from agent-desk.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::process::Command;

use agent_desk::desk;
use agent_desk::pick::{self, Candidate, Origin};
use serde_json::Value;

use crate::file_list::{Entry, Row, RowKind};
use crate::model::{ChangeKind, ChangedFile};

/// The summary's pseudo path: first in the list, rendered as markdown, never on disk.
pub const SUMMARY: &str = "session:summary.md";
/// The path prefix of a tier's group row.
pub const GROUP: &str = "session:group:";

/// Why a file is an artifact, strongest first (decision 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Report,
    Declared,
    Plan,
    NewDoc,
    Named,
    EditedDoc,
    /// Named in the agent's last answer, not changed by the session: a link to follow.
    Linked,
    Code,
}

impl Tier {
    /// The list group a tier shows under (decision 15).
    pub fn group(self) -> Group {
        match self {
            Tier::Report => Group::Report,
            Tier::Declared | Tier::Plan | Tier::NewDoc | Tier::Named => Group::Artifacts,
            Tier::EditedDoc => Group::Other,
            Tier::Linked => Group::Links,
            Tier::Code => Group::Code,
        }
    }
}

/// The groups of the `Session` list, in order (decision 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Report,
    Artifacts,
    Other,
    Links,
    Code,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Report => "Отчёт",
            Group::Artifacts => "Артефакты",
            Group::Other => "Прочее",
            Group::Links => "Ссылки из ответа",
            Group::Code => "Код",
        }
    }

    pub fn group_path(self) -> String {
        format!("{GROUP}{}", self as u8)
    }
}

/// One artifact: its navigator key (repo-relative inside the repo, else absolute).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub key: String,
    pub abs: PathBuf,
    pub tier: Tier,
    /// `A` created, `M` modified, `U` new in Git; `None` when only declared.
    pub letter: Option<char>,
    /// The agent's «зачем», for a declared file.
    pub note: Option<String>,
}

/// One file the session changed, keyed like an artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionChange {
    pub key: String,
    pub kind: ChangeKind,
    pub additions: u32,
    pub deletions: u32,
    /// From the own worktree's Git, diffed against `base`; else from the transcript.
    pub git: bool,
    /// The text before the session's first edit, when the transcript holds it.
    pub before: Option<String>,
}

/// The agent the tab shows, as Herdr lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub kind: String,
    pub session: String,
    pub name: String,
    pub cwd: PathBuf,
    pub status: String,
    /// How the agent was found: `tab` or `workspace`.
    pub found_by: &'static str,
}

/// What one load found; lands on the frame loop and is shared with the world worker.
#[derive(Clone, Debug, Default)]
pub struct SessionView {
    /// Bumped by each landing, so a world build of an older view never lands.
    pub epoch: u64,
    pub agent: Option<Agent>,
    pub header: desk::Header,
    pub first_prompt: String,
    pub last_answer: String,
    pub prs: Vec<String>,
    /// The agent's unanswered `AskUserQuestion`.
    pub question: Option<String>,
    pub artifacts: Vec<Artifact>,
    /// Every file the session changed, for the `session` scope (decision 5).
    pub changes: Vec<SessionChange>,
    /// The own worktree's merge-base, the old side of its Git changes.
    pub base: Option<String>,
    pub summary: Option<desk::Summary>,
    /// Why there is nothing to show, when the load found no session.
    pub problem: Option<String>,
    /// Whether a load is running, for the summary's wording.
    pub loading: bool,
}

impl PartialEq for SessionView {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch
    }
}
impl Eq for SessionView {}

impl SessionView {
    /// The navigator entries: the summary, then artifacts by tier.
    pub fn entries(&self) -> Vec<Entry> {
        let summary =
            Entry { path: SUMMARY.into(), annotation: None, ignored: false, is_dir: false };
        let mut out = vec![summary];
        out.extend(self.artifacts.iter().map(|a| Entry {
            path: a.key.clone(),
            annotation: a.letter.map(|l| annotation(&a.key, l)),
            ignored: false,
            is_dir: false,
        }));
        out
    }

    /// The line the tab bar shows: what the agent needs from the owner.
    pub fn need(&self) -> String {
        if let Some(problem) = &self.problem {
            return problem.clone();
        }
        if let Some(question) = &self.question {
            return format!("Нужно от вас: ответить агенту — {question}");
        }
        if let Some(need) = self.header.need.as_deref() {
            return format!("Нужно от вас: {need}");
        }
        // The summary may be older than the last answer, so its `need` stays in the summary.
        let first = self.last_answer.lines().map(str::trim).find(|l| !l.is_empty());
        match (self.agent.as_ref().map(|a| a.status.as_str()), first) {
            (None, _) if self.loading => "Ищу сессию…".into(),
            (Some("working"), _) => "Агент работает".into(),
            (Some("blocked"), _) => "Агент ждёт ответа в своей панели".into(),
            (Some(_), Some(first)) => format!("Агент ответил: {first}"),
            _ => "Нужно от вас: —".into(),
        }
    }

    /// The `session` scope's changeset; it carries no ends, its sides come from [`Self::sides`].
    pub fn changeset(&self) -> crate::world::Changeset {
        let files = self
            .changes
            .iter()
            .map(|c| {
                let file = ChangedFile {
                    path: c.key.clone(),
                    kind: c.kind,
                    additions: c.additions,
                    deletions: c.deletions,
                    previous_path: None,
                    binary: false,
                    old_size: 0,
                    new_size: None,
                };
                (c.key.clone(), file)
            })
            .collect();
        crate::world::Changeset { files, ends: None }
    }

    /// The `session` scope's two sides of `key`: before the session, and on disk now.
    pub fn sides(&self, repo: &Path, key: &str) -> Result<(String, String), crate::diff::Notice> {
        let Some(change) = self.changes.iter().find(|c| c.key == key) else {
            return Ok((String::new(), String::new()));
        };
        if change.git
            && let Some(base) = &self.base
        {
            return match crate::git::diff_sides(repo, base, None, key, None) {
                Ok(crate::git::DiffSides::Text { old, new }) => Ok((old, new)),
                Ok(crate::git::DiffSides::Binary) => Err(crate::diff::Notice::Binary),
                Err(_) => Err(crate::diff::Notice::Unreadable),
            };
        }
        let new = read_text(&repo.join(key))?;
        let old = match &change.before {
            Some(before) => before.clone(),
            // No state before the session (Codex): `HEAD` stands in, inside the repo.
            None => head_text(repo, key).unwrap_or_default(),
        };
        Ok((old, new))
    }

    /// The text behind a pseudo path, `None` for a real file.
    pub fn text(&self, path: &str) -> Option<String> {
        (path == SUMMARY).then(|| self.summary_markdown())
    }

    fn summary_markdown(&self) -> String {
        let mut out = String::from("# Сводка сессии\n\n");
        let Some(agent) = &self.agent else {
            out.push_str(self.problem.as_deref().unwrap_or("Ищу агента этой вкладки…"));
            out.push('\n');
            return out;
        };
        let _ = writeln!(out, "**{}**\n", self.need());
        if let Some(task) = &self.header.task {
            let _ = writeln!(out, "- **Задача:** {task}");
        }
        if let Some(now) = &self.header.now {
            let _ = writeln!(out, "- **Сейчас:** {now}");
        }
        let name = if agent.name.is_empty() { &agent.kind } else { &agent.name };
        let _ = writeln!(out, "- **Агент:** {name} · {} · {}", agent.kind, agent.status);
        for pr in &self.prs {
            let _ = writeln!(out, "- **PR:** {pr}");
        }
        out.push('\n');
        if let Some(s) = &self.summary {
            let _ = write!(out, "## О чём\n\n{}\n\n## Что сделано\n\n", s.about);
            for done in &s.done {
                let files: Vec<String> = done
                    .files
                    .iter()
                    .map(|f| format!("[{}](<{}>)", short_path(f), f.display()))
                    .collect();
                let tail = if files.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", files.join(", "))
                };
                let _ = writeln!(out, "- {}{tail}", done.text);
            }
            let _ = write!(out, "\n## Сейчас\n\n{}\n\n", s.now);
            let _ = writeln!(
                out,
                "*Сводка: {} · {} · {:.0} с. `A` — построить заново.*\n",
                s.at, s.model, s.seconds
            );
        } else {
            out.push_str("AI-сводка ещё не построена: `A` строит её за 10–30 секунд.\n\n");
            if !self.first_prompt.is_empty() {
                let _ =
                    write!(out, "## Поручение\n\n{}\n\n", quote(&clip(&self.first_prompt, 1200)));
            }
        }
        if !self.last_answer.is_empty() {
            let _ = writeln!(
                out,
                "## Последний ответ агента\n\n{}",
                quote(&clip(&self.last_answer, 2000))
            );
        }
        out
    }
}

/// A navigator badge for an artifact; the session's diff stats live in the `a` scope.
fn annotation(key: &str, letter: char) -> ChangedFile {
    let kind = match letter {
        'A' => ChangeKind::Added,
        'U' => ChangeKind::Untracked,
        _ => ChangeKind::Modified,
    };
    ChangedFile {
        path: key.to_string(),
        kind,
        additions: 0,
        deletions: 0,
        previous_path: None,
        binary: false,
        old_size: 0,
        new_size: None,
    }
}

/// The navigator rows: the summary, then one group row per tier with its files.
/// A group is expanded unless `collapsed` holds it.
pub fn rows<S: BuildHasher>(
    view: &SessionView,
    entries: &[Entry],
    collapsed: &HashSet<String, S>,
) -> Vec<Row> {
    let tiers: BTreeMap<&str, &Artifact> =
        view.artifacts.iter().map(|a| (a.key.as_str(), a)).collect();
    let mut out = Vec::new();
    let mut groups: BTreeMap<Group, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.path == SUMMARY {
            out.push(Row {
                depth: 0,
                name: "Сводка".into(),
                kind: RowKind::File { index },
                ignored: false,
            });
        } else if let Some(a) = tiers.get(entry.path.as_str()) {
            groups.entry(a.tier.group()).or_default().push(index);
        }
    }
    for (group, indices) in groups {
        let path = group.group_path();
        let expanded = !collapsed.contains(&path);
        out.push(Row {
            depth: 0,
            name: format!("{} · {}", group.label(), indices.len()),
            kind: RowKind::Dir { path, expanded, has_change: false },
            ignored: false,
        });
        if expanded {
            for index in indices {
                let a = tiers[entries[index].path.as_str()];
                let mut name = short_path(&entries[index].path);
                if let Some(note) = &a.note {
                    name = format!("{name} — {note}");
                }
                out.push(Row { depth: 1, name, kind: RowKind::File { index }, ignored: false });
            }
        }
    }
    out
}

/// `~/…` for a path under the home directory.
pub fn short_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path.is_absolute() && path.starts_with(&home) => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

fn quote(text: &str) -> String {
    text.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
}

// ---- loading, off the frame loop ----

/// Find this tab's agent and read its session; blocking (Herdr calls, the transcript).
pub fn load(repo: &Path) -> SessionView {
    let mut view = SessionView::default();
    let agent = match find_agent(repo) {
        Ok(agent) => agent,
        Err(problem) => {
            view.problem = Some(problem);
            return view;
        }
    };
    let trace = desk::trace(&agent.kind, &agent.session, &agent.cwd).unwrap_or_default();
    view.header = desk::header(&trace.last_answer);
    view.first_prompt.clone_from(&trace.first_prompt);
    view.last_answer.clone_from(&trace.last_answer);
    view.prs.clone_from(&trace.prs);
    view.question.clone_from(&trace.question);
    view.summary = desk::cached_summary(&agent.session);
    view.artifacts = artifacts(repo, &agent, &trace);
    view.base = desk::own_worktree(&agent.cwd).and_then(|_| merge_base(repo));
    view.changes = changes(repo, &agent, &trace);
    view.agent = Some(agent);
    view
}

/// Build the AI summary for the view's session; blocking for 10–30 s (decision 4).
pub fn summarize(view: &SessionView) -> Result<desk::Summary, String> {
    let agent = view.agent.as_ref().ok_or("нет сессии")?;
    let trace =
        desk::trace(&agent.kind, &agent.session, &agent.cwd).ok_or("нет записи разговора")?;
    let files = desk::changes(&trace, &agent.cwd);
    desk::summarize(&agent.session, &trace, &files)
}

/// The artifacts by tier, strongest signal first, then by path.
fn artifacts(repo: &Path, agent: &Agent, trace: &desk::Trace) -> Vec<Artifact> {
    let transcript = pick::transcript(&agent.kind, &agent.session);
    let mut roots = pick::Roots::default();
    let main = agent_desk::git::worktree_paths(repo)
        .ok()
        .and_then(|paths| paths.into_iter().next())
        .unwrap_or_else(|| repo.to_path_buf());
    let reports = pick::report_signals(&main, &agent.session);
    // Git proves authorship only in the session's own worktree (agent-desk decision 129).
    let git =
        desk::own_worktree(&agent.cwd).map(|root| pick::git_signals(&root)).unwrap_or_default();
    let signals = pick::session_signals(transcript, "", reports, git);
    let candidates = pick::collect(&signals, &agent.cwd, &mut roots);
    let marks: BTreeMap<PathBuf, String> = trace
        .artifacts
        .iter()
        .filter_map(|m| Some((m.path.canonicalize().ok()?, m.note.clone())))
        .collect();
    let root = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let since = trace.started.as_deref().and_then(parse_time);
    let mut out: Vec<Artifact> = candidates
        .iter()
        .filter_map(|c| {
            let mut note = marks.get(&c.path).cloned();
            let tier = match tier_of(c, note.is_some()) {
                Some(tier) => tier,
                // Named and changed during the session: likely written by a shell command.
                None if c.origins.contains(&Origin::Named) && changed_since(&c.path, since) => {
                    note = Some("изменён командой, по времени".into());
                    Tier::Named
                }
                None => return None,
            };
            Some(Artifact {
                key: key_of(&root, &c.path),
                abs: c.path.clone(),
                tier,
                letter: c.letter(),
                note,
            })
        })
        .collect();
    // A mark the transcript never wrote still counts: the agent declared it.
    for (path, note) in &marks {
        if path.is_file() && !out.iter().any(|a| &a.abs == path) {
            out.push(Artifact {
                key: key_of(&root, path),
                abs: path.clone(),
                tier: Tier::Declared,
                letter: None,
                note: Some(note.clone()),
            });
        }
    }
    // Files the last answer names but the session never changed: links to follow.
    let git_root = agent_desk::git::root(&agent.cwd).ok();
    for token in pick::path_tokens(&trace.last_answer) {
        let Some(path) = pick::resolve(&token, &agent.cwd, git_root.as_deref()) else { continue };
        if !out.iter().any(|a| a.abs == path) {
            let key = key_of(&root, &path);
            out.push(Artifact { key, abs: path, tier: Tier::Linked, letter: None, note: None });
        }
    }
    out.sort_by(|a, b| a.tier.cmp(&b.tier).then_with(|| a.key.cmp(&b.key)));
    out
}

/// The session's changed files; one changed and changed back is left out (decision 48).
fn changes(repo: &Path, agent: &Agent, trace: &desk::Trace) -> Vec<SessionChange> {
    let root = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let mut out: Vec<SessionChange> = desk::changes(trace, &agent.cwd)
        .into_iter()
        .filter_map(|f| {
            let abs = f.path.canonicalize().unwrap_or_else(|_| f.path.clone());
            let git = f.source == "git";
            let now = std::fs::read_to_string(&abs).ok();
            if !git && f.before.is_some() && f.before == now {
                return None;
            }
            let kind = match f.letter {
                'A' => ChangeKind::Added,
                'D' => ChangeKind::Deleted,
                _ if now.is_none() => ChangeKind::Deleted,
                _ => ChangeKind::Modified,
            };
            let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            Some(SessionChange {
                key: key_of(&root, &abs),
                kind,
                additions: count(f.added),
                deletions: count(f.removed),
                git,
                before: f.before,
            })
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out.dedup_by(|a, b| a.key == b.key);
    out
}

/// The branch scope's merge-base, which the own worktree's Git changes diff against.
fn merge_base(repo: &Path) -> Option<String> {
    let resolution = crate::git::resolve_base(repo, None).ok()?;
    crate::git::merge_base(repo, resolution.status.winner.as_ref()?.oid())
}

fn read_text(path: &Path) -> Result<String, crate::diff::Notice> {
    match std::fs::read(path) {
        Ok(bytes) if crate::diff::over_byte_budget(bytes.len()) => {
            Err(crate::diff::Notice::TooLarge)
        }
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| crate::diff::Notice::Binary),
        Err(_) => Ok(String::new()),
    }
}

/// `key` at `HEAD`, for a file inside the repo.
fn head_text(repo: &Path, key: &str) -> Option<String> {
    if Path::new(key).is_absolute() {
        return None;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("HEAD:{key}")])
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The tier by decision 11; `None` for a path the session only mentioned or showed.
fn tier_of(c: &Candidate, marked: bool) -> Option<Tier> {
    let has = |o| c.origins.contains(&o);
    let written = has(Origin::Created)
        || has(Origin::Edited)
        || has(Origin::GitNew)
        || has(Origin::GitModified);
    if has(Origin::Report) {
        return Some(Tier::Report);
    }
    if marked || has(Origin::Declared) {
        return Some(Tier::Declared);
    }
    if !written {
        return None;
    }
    let document = c.is_document();
    if document && is_plan_place(&c.path) {
        return Some(Tier::Plan);
    }
    if !document {
        return Some(Tier::Code);
    }
    if has(Origin::Created) || has(Origin::GitNew) {
        return Some(Tier::NewDoc);
    }
    if has(Origin::Named) {
        return Some(Tier::Named);
    }
    Some(Tier::EditedDoc)
}

fn parse_time(text: &str) -> Option<std::time::SystemTime> {
    let at =
        time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()?;
    Some(at.into())
}

/// Whether `path` changed on disk after the session started.
fn changed_since(path: &Path, since: Option<std::time::SystemTime>) -> bool {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    since.zip(modified).is_some_and(|(since, modified)| modified >= since)
}

/// Well-known places of plans and specs (research/artifacts.md, tier 2).
fn is_plan_place(path: &Path) -> bool {
    let text = path.to_string_lossy();
    [
        "/.claude/plans/",
        "/.kiro/specs/",
        "/.factory/docs/",
        "/.cursor/plans/",
        "/specs/",
        "/docs/plans/",
        "/docs/superpowers/",
        "/plans/",
    ]
    .iter()
    .any(|place| text.contains(place))
}

/// Repo-relative inside the repo, so comments match the other tabs; absolute outside.
fn key_of(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// This tab's agent; a pane outside any agent tab falls back to the workspace's agent in this repo.
fn find_agent(repo: &Path) -> Result<Agent, String> {
    let me = std::env::var("HERDR_PANE_ID")
        .map_err(|_| "reviewr открыт вне Herdr: сессии нет".to_string())?;
    let tab = herdr(&["pane", "get", &me])
        .and_then(|v| v["result"]["pane"]["tab_id"].as_str().map(str::to_string))
        .or_else(|| std::env::var("HERDR_TAB_ID").ok());
    let workspace = me.split(':').next().unwrap_or_default().to_string();
    let listed = herdr(&["agent", "list"]).ok_or("Herdr не ответил на agent list")?;
    let agents: Vec<&Value> = listed["result"]["agents"]
        .as_array()
        .map(|a| a.iter().filter(|v| v["pane_id"].as_str() != Some(me.as_str())).collect())
        .unwrap_or_default();
    let session_of = |v: &Value| {
        v["agent_session"]["value"].as_str().filter(|s| !s.is_empty()).map(str::to_string)
    };
    let in_tab: Vec<&&Value> = agents
        .iter()
        .filter(|v| {
            tab.is_some() && v["tab_id"].as_str() == tab.as_deref() && session_of(v).is_some()
        })
        .collect();
    let root = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let in_repo = |v: &&&Value| {
        v["workspace_id"].as_str() == Some(workspace.as_str())
            && session_of(v).is_some()
            && v["cwd"]
                .as_str()
                .is_some_and(|c| Path::new(c).canonicalize().is_ok_and(|c| c.starts_with(&root)))
    };
    let (chosen, found_by) = match in_tab.as_slice() {
        [one] => (**one, "tab"),
        [] => {
            let near: Vec<&&Value> = agents.iter().filter(|v| in_repo(v)).collect();
            match near.as_slice() {
                [one] => (**one, "workspace"),
                [] => return Err("В этой вкладке нет агента с сессией".into()),
                _ => return Err("Во вкладке нет агента, а в workspace их несколько".into()),
            }
        }
        _ => return Err("В этой вкладке несколько агентов".into()),
    };
    Ok(Agent {
        kind: chosen["agent"].as_str().unwrap_or_default().to_string(),
        session: session_of(chosen).unwrap_or_default(),
        name: chosen["name"].as_str().unwrap_or_default().to_string(),
        cwd: PathBuf::from(chosen["cwd"].as_str().unwrap_or_default()),
        status: chosen["agent_status"].as_str().unwrap_or_default().to_string(),
        found_by,
    })
}

/// The tab `pane` sits in now, read live since a pane can move; the launch tab as a fallback.
pub fn pane_tab(pane: &str) -> Option<String> {
    herdr(&["pane", "get", pane])
        .and_then(|v| v["result"]["pane"]["tab_id"].as_str().map(str::to_string))
        .or_else(|| std::env::var("HERDR_TAB_ID").ok())
}

/// Submit `text` to the agent in `pane` as a message (`S`, decision 8); refused while it asks.
pub fn submit(pane: &str, text: &str) -> anyhow::Result<()> {
    let bin = std::env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "herdr".into());
    let out = Command::new(bin).args(["agent", "prompt", pane, text]).output()?;
    if out.status.success() {
        return Ok(());
    }
    let answer = String::from_utf8_lossy(&out.stdout);
    if answer.contains("agent_blocked") {
        anyhow::bail!("answer the agent's question first");
    }
    anyhow::bail!("herdr agent prompt failed: {}", String::from_utf8_lossy(&out.stderr).trim())
}

/// The `S` destination: the comments go in as a submitted message.
#[derive(Debug)]
pub struct Prompt {
    pub pane: String,
    pub name: String,
}

impl crate::export::ExportTarget for Prompt {
    fn export(&self, text: &str) -> anyhow::Result<()> {
        submit(&self.pane, &format!("Замечания ревью:\n\n{text}"))
    }
    fn label(&self) -> &'static str {
        "agent prompt"
    }
    fn success_message(&self, count: usize) -> String {
        format!("submitted {} to {}", crate::export::counted_comments(count), self.name)
    }
    fn failure_message(&self, error: &anyhow::Error, copy: &str) -> String {
        format!("{error}, press {copy} to copy")
    }
}

/// A link to a local file: its path and line, from `a.md`, `~/x.md:12`, `/abs/y.rs#L3`.
/// `None` for a URL with a scheme or a path that names no file.
pub fn resolve_link(target: &str, base: &Path, repo: &Path) -> Option<(PathBuf, Option<usize>)> {
    let target = target.strip_prefix("file://").unwrap_or(target).trim();
    let scheme = target
        .split_once(':')
        .is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphabetic()));
    if scheme || target.starts_with('#') {
        return None;
    }
    let (path, line) = match target.split_once("#L").or_else(|| target.rsplit_once(':')) {
        Some((path, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            (path, n.parse().ok())
        }
        _ => (target, None),
    };
    pick::resolve(&percent_decode(path), base, Some(repo)).map(|p| (p, line))
}

/// `%20` and the like, as markdown link destinations spell spaces.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Add `path` to the list's links, for a link followed to a file outside the repo.
pub fn with_link(view: &SessionView, repo: &Path, path: &Path) -> SessionView {
    let mut view = view.clone();
    let root = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let key = key_of(&root, path);
    if !view.artifacts.iter().any(|a| a.key == key) {
        let abs = path.to_path_buf();
        view.artifacts.push(Artifact { key, abs, tier: Tier::Linked, letter: None, note: None });
    }
    view
}

/// The mark an open request carries in its paste, so a draft never swallows it.
const OPEN_MARK: &str = "⟦reviewr-open⟧ ";

/// The paste that asks a running reviewr to open `path`.
pub fn open_request(path: &str) -> String {
    format!("{OPEN_MARK}{path}")
}

/// The path an open request pastes, `None` for any other paste.
pub fn parse_open_request(text: &str) -> Option<&str> {
    text.strip_prefix(OPEN_MARK).map(str::trim)
}

/// A clicked link or selected text as an absolute path with its line, resolved from `cwd`.
pub fn request_path(raw: &str, cwd: Option<&str>) -> Option<String> {
    let raw = raw.trim().trim_matches(|c| matches!(c, '`' | '"' | '\'' | '(' | ')' | '<' | '>'));
    // `file:///abs`, `file://host/abs`, and Codex's `vscode://file/abs:12`.
    let raw = match raw.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None => raw.strip_prefix("vscode://file").unwrap_or(raw),
    };
    let base = cwd.map(PathBuf::from).unwrap_or_default();
    let (path, line) = resolve_link(raw, &base, &base)?;
    Some(match line {
        Some(line) => format!("{}:{line}", path.display()),
        None => path.display().to_string(),
    })
}

/// Whether this pane runs inside Herdr with its CLI on, where a session can exist.
/// Tests and the idle check switch the CLI off with `HERDR_BIN_PATH=false`.
pub fn available() -> bool {
    let cli_off = std::env::var("HERDR_BIN_PATH").is_ok_and(|b| b.is_empty() || b == "false");
    std::env::var_os("HERDR_PANE_ID").is_some_and(|p| !p.is_empty()) && !cli_off
}

/// One Herdr CLI call's JSON answer.
fn herdr(args: &[&str]) -> Option<Value> {
    let bin = std::env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "herdr".into());
    let out = Command::new(bin).args(args).output().ok()?;
    out.status.success().then(|| serde_json::from_slice(&out.stdout).ok()).flatten()
}

/// Open `path` with the system: HTML in the browser, PDF and images in their viewer.
pub fn open_external(path: &Path) -> Result<(), String> {
    let tool = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut child = Command::new(tool)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{tool}: {e}"))?;
    std::thread::spawn(move || drop(child.wait()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, origins: &[Origin]) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            origins: origins.iter().copied().collect(),
            repo: None,
        }
    }

    #[test]
    fn tiers_follow_decision_11() {
        let t = |path, origins: &[Origin]| tier_of(&candidate(path, origins), false);
        assert_eq!(t("/r/.reports/x/a.html", &[Origin::Report]), Some(Tier::Report));
        assert_eq!(t("/r/docs/a.md", &[Origin::Declared]), Some(Tier::Declared));
        assert_eq!(t("/h/.claude/plans/p.md", &[Origin::Created]), Some(Tier::Plan));
        assert_eq!(t("/r/docs/new.md", &[Origin::Created]), Some(Tier::NewDoc));
        assert_eq!(t("/r/docs/old.md", &[Origin::Edited, Origin::Named]), Some(Tier::Named));
        assert_eq!(t("/r/docs/old.md", &[Origin::Edited]), Some(Tier::EditedDoc));
        assert_eq!(t("/r/src/a.rs", &[Origin::Created, Origin::Named]), Some(Tier::Code));
        // Only mentioned or read: not the session's work.
        assert_eq!(t("/r/docs/read.md", &[Origin::Named]), None);
        assert_eq!(
            tier_of(&candidate("/r/src/a.rs", &[Origin::Edited]), true),
            Some(Tier::Declared)
        );
    }

    #[test]
    fn rows_group_by_tier_and_fold_collapsed_groups() {
        let view = SessionView {
            artifacts: vec![
                Artifact {
                    key: "docs/a.md".into(),
                    abs: "/r/docs/a.md".into(),
                    tier: Tier::NewDoc,
                    letter: Some('A'),
                    note: None,
                },
                Artifact {
                    key: "src/a.rs".into(),
                    abs: "/r/src/a.rs".into(),
                    tier: Tier::Code,
                    letter: Some('M'),
                    note: None,
                },
            ],
            ..SessionView::default()
        };
        let entries = view.entries();
        let collapsed: HashSet<String> = [Group::Code.group_path()].into();
        let names: Vec<String> =
            rows(&view, &entries, &collapsed).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Сводка", "Артефакты · 1", "docs/a.md", "Код · 1"]);
    }

    #[test]
    fn a_clicked_or_selected_path_resolves_from_the_agents_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/plan.md"), "x\n").unwrap();
        let cwd = root.to_str();
        let abs = root.join("docs/plan.md").display().to_string();
        assert_eq!(request_path("docs/plan.md", cwd), Some(abs.clone()));
        assert_eq!(request_path("`docs/plan.md:12`", cwd), Some(format!("{abs}:12")));
        assert_eq!(request_path(&format!("file://{abs}"), None), Some(abs.clone()));
        assert_eq!(request_path(&format!("file://host{abs}"), None), Some(abs.clone()));
        assert_eq!(request_path(&format!("vscode://file{abs}:3"), None), Some(format!("{abs}:3")));
        assert_eq!(request_path("docs/none.md", cwd), None);
        let pasted = open_request(&abs);
        assert_eq!(parse_open_request(&pasted), Some(abs.as_str()));
        assert_eq!(parse_open_request("plain paste"), None);
    }

    #[test]
    fn keys_are_repo_relative_inside_and_absolute_outside() {
        assert_eq!(key_of(Path::new("/r"), Path::new("/r/docs/a.md")), "docs/a.md");
        assert_eq!(key_of(Path::new("/r"), Path::new("/tmp/x.md")), "/tmp/x.md");
    }
}
