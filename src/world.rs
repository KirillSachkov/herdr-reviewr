//! The world snapshot: what one refresh derives from git alone, built on the caller or the worker.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use anyhow::Result;

use crate::app::Tab;
use crate::file_list::Entry;
use crate::git;
use crate::herdr::AgentSample;
use crate::model::{ChangedFile, CommitPick, Scope};
use crate::turn::{TurnTracker, WorktreeState};

/// Everything the build reads; a snapshot lands only while the view still matches it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldInput {
    pub repo: PathBuf,
    pub tab: Tab,
    pub scope: Scope,
    /// The `--base` flag. The pick is read at build time, so another pane's pick lands as content.
    pub base: Option<String>,
    /// Bumped by this pane's pick, so a build of the previous pick never lands.
    pub base_epoch: u64,
    /// The `last-turn` baseline tree the changed set diffs against; `None` before a turn.
    pub turn_baseline: Option<String>,
    /// The `commits` scope's pick, so a build of a replaced pick never lands.
    pub commit_pick: Option<CommitPick>,
    /// Expanded ignored directories whose children the `All files` tree loads.
    pub toggled_dirs: HashSet<String>,
}

/// One refresh's result; the base rides along so the header and its changeset land together.
#[derive(Debug)]
pub struct WorldSnapshot {
    pub changeset: Changeset,
    pub entries: Vec<Entry>,
    pub branch_base: git::BaseStatus,
    /// The `commits` scope's pick verdict; `None` on every other scope.
    pub pick_status: Option<PickStatus>,
    /// `HEAD` at build time, the commit picker's key; `None` when unborn.
    pub head: Option<String>,
}

/// What one build found the commit pick to be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PickVerdict {
    /// Every commit reachable from `HEAD`.
    Live,
    /// Some commit unreachable from `HEAD`. The run still paints.
    OffBranch,
    /// A needed commit is pruned, named here. The scope is empty.
    Gone(String),
}

/// The pick's verdict and the newest commit's subject, for the header.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PickStatus {
    pub verdict: PickVerdict,
    pub subject: String,
    /// How many commits the run spans, `0` when `gone` or not a run.
    pub count: usize,
}

/// The ends a changeset was diffed between (`new` `None` for the worktree); a file's diff reads these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEnds {
    pub old: String,
    pub new: Option<String>,
}

/// A scope's changed files by path and the ends they were diffed between, landed together.
#[derive(Debug, Default)]
pub struct Changeset {
    pub files: BTreeMap<String, ChangedFile>,
    /// `None` when the scope has nothing to diff: no baseline, base, or live pick.
    pub ends: Option<DiffEnds>,
}

/// A build's changeset and the base or pick it diffs against, landed together.
#[derive(Debug, Default)]
pub struct ScopeBuild {
    pub branch_base: git::BaseStatus,
    pub pick_status: Option<PickStatus>,
    pub changeset: Changeset,
}

/// Build the snapshot for `input`; the changeset is built on every tab.
pub fn build(input: &WorldInput) -> Result<WorldSnapshot> {
    build_at(input, None)
}

/// [`build`], reusing `written`, a worktree tree written this instant, as `last-turn`'s new end.
fn build_at(input: &WorldInput, written: Option<String>) -> Result<WorldSnapshot> {
    // Outside a repo, paint the quiet empty state, not an error every poll.
    if !git::is_repo(&input.repo) {
        return Ok(WorldSnapshot {
            changeset: Changeset::default(),
            entries: Vec::new(),
            branch_base: git::BaseStatus::default(),
            pick_status: None,
            head: None,
        });
    }
    // One read of HEAD serves the snapshot and the uncommitted diff's old end.
    let head = git::head_oid(&input.repo);
    let ScopeBuild { branch_base, pick_status, changeset } =
        scope_build(input, head.clone(), written)?;
    let entries = match input.tab {
        // The whole worktree (ignored included), with expanded ignored dirs loaded lazily.
        Tab::AllFiles => all_files_entries(input, &changeset.files)?,
        // `Changes` (the `PR` tab never builds a snapshot).
        _ => changeset.files.values().map(Entry::from_changed).collect(),
    };
    Ok(WorldSnapshot { changeset, entries, branch_base, pick_status, head })
}

/// The active scope's changeset and, on `branch`, its base.
pub fn build_changed(input: &WorldInput) -> Result<ScopeBuild> {
    if !git::is_repo(&input.repo) {
        return Ok(ScopeBuild::default());
    }
    scope_build(input, git::head_oid(&input.repo), None)
}

/// [`build_changed`] against `head`, as the caller read it, inside a repo; `written`, a tree of
/// the worktree written this instant, spares `last-turn` a second snapshot.
fn scope_build(
    input: &WorldInput,
    head: Option<String>,
    written: Option<String>,
) -> Result<ScopeBuild> {
    match input.scope {
        Scope::LastTurn => match input.turn_baseline.as_deref() {
            Some(t) => {
                let now = match written {
                    Some(tree) => tree,
                    None => git::snapshot_worktree(&input.repo)?,
                };
                at_ends(&input.repo, DiffEnds { old: t.to_string(), new: Some(now) })
            }
            None => Ok(ScopeBuild::default()),
        },
        Scope::Uncommitted => {
            let base = git::diff_base(head);
            at_ends(&input.repo, DiffEnds { old: base, new: None })
        }
        Scope::Branch => {
            // A resolve failure fails the build, keeping the stale frame.
            let resolution = git::resolve_base(&input.repo, input.base.as_deref())?;
            let merge_base = resolution
                .status
                .winner
                .as_ref()
                .and_then(|w| git::merge_base(&input.repo, w.oid()));
            let build = match merge_base {
                Some(base) => at_ends(&input.repo, DiffEnds { old: base, new: None })?,
                None => ScopeBuild::default(),
            };
            Ok(ScopeBuild { branch_base: resolution.status, ..build })
        }
        Scope::Commits => {
            // A tag without a pick builds the empty changeset.
            let Some(pick) = &input.commit_pick else { return Ok(ScopeBuild::default()) };
            build_pick(&input.repo, pick)
        }
    }
}

/// The changeset between `ends`, carried beside them: the ends a file's diff reads are its input.
fn at_ends(repo: &Path, ends: DiffEnds) -> Result<ScopeBuild> {
    let changed = match &ends.new {
        None => git::changed_from(repo, &ends.old)?,
        Some(new) => git::changed_between(repo, &ends.old, new)?,
    };
    let files = changed.into_iter().map(|f| (f.path.clone(), f)).collect();
    Ok(ScopeBuild { changeset: Changeset { files, ends: Some(ends) }, ..ScopeBuild::default() })
}

/// The pick's changeset, verdict and ends in one pass; a `gone` pick has neither.
fn build_pick(repo: &Path, pick: &CommitPick) -> Result<ScopeBuild> {
    let gone = |sha: &str| {
        let status = PickStatus {
            verdict: PickVerdict::Gone(sha.to_string()),
            subject: String::new(),
            count: 0,
        };
        ScopeBuild { pick_status: Some(status), ..ScopeBuild::default() }
    };
    if !git::commit_exists(repo, &pick.newest) {
        return Ok(gone(&pick.newest));
    }
    let Some(old) = git::parent_or_empty(repo, &pick.oldest) else {
        return Ok(gone(&pick.oldest));
    };
    if old != git::EMPTY_TREE && !git::commit_exists(repo, &old) {
        return Ok(gone(&old));
    }
    let subject = git::commit_subject(repo, &pick.newest).unwrap_or_default();
    let count = git::run_length_from(repo, &old, &pick.oldest, &pick.newest).unwrap_or(0);
    let at = at_ends(repo, DiffEnds { old, new: Some(pick.newest.clone()) })?;
    // The oldest is an ancestor of the newest, so one reachability check covers the run.
    let verdict = if git::is_reachable(repo, &pick.newest) {
        PickVerdict::Live
    } else {
        PickVerdict::OffBranch
    };
    Ok(ScopeBuild { pick_status: Some(PickStatus { verdict, subject, count }), ..at })
}

/// The persisted turn baseline for `repo`, if any.
pub fn seed_baseline(repo: &std::path::Path) -> Option<String> {
    git::read_baseline_ref(repo)
}

/// The `All files` entries; an ignored directory is walked only once expanded.
pub(crate) fn all_files_entries(
    input: &WorldInput,
    changed: &BTreeMap<String, ChangedFile>,
) -> Result<Vec<Entry>> {
    let to_entry = |w: git::WorktreeEntry| Entry {
        annotation: changed.get(&w.path).cloned(),
        path: w.path,
        ignored: w.ignored,
        is_dir: w.is_dir,
    };
    let mut entries: Vec<Entry> = git::all_files(&input.repo)?.into_iter().map(&to_entry).collect();
    let mut i = 0;
    while i < entries.len() {
        if entries[i].is_dir && input.toggled_dirs.contains(&entries[i].path) {
            let path = entries[i].path.clone();
            let children = git::list_ignored_dir(&input.repo, &path).into_iter().map(&to_entry);
            entries.extend(children);
        }
        i += 1;
    }
    Ok(entries)
}

/// Turn tracking on the worker, so a snapshot always rides the sample that saw its edge.
#[derive(Debug)]
pub struct TurnHost {
    tracker: TurnTracker,
    repo: PathBuf,
    /// The reviewed worktree's [`canonical`] root, which a member's top level equals.
    root: PathBuf,
    /// Each agent `cwd` with a resolved top level, mapped to whether it is a member.
    resolved: HashMap<String, bool>,
}

/// One sample's outcome: whether a turn ended, and whether agents are present.
#[derive(Clone, Debug, Default)]
pub struct TurnReport {
    pub ended: bool,
    /// `None` when the enumeration failed or a member didn't resolve, so the reader keeps what it knew.
    pub agents_present: Option<bool>,
    /// The worktree tree the sample wrote, which its job's `last-turn` build reuses.
    pub written: Option<String>,
}

/// An agent's place in the worktree; `Unknown` holds the poll instead of counting it out.
enum Membership {
    Member,
    NotMember,
    Unknown,
}

/// The agent's cwd when absolute: `git -C` would resolve a relative one against reviewr's own.
fn worktree_cwd(cwd: Option<&str>) -> Option<&str> {
    cwd.filter(|c| Path::new(c).is_absolute())
}

/// `path` as the OS resolves it, so two spellings of one directory compare equal.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Fold the members' statuses, or `None` when any membership is undetermined.
fn classify(
    samples: &[AgentSample],
    mut member: impl FnMut(&AgentSample) -> Membership,
) -> Option<(bool, WorktreeState)> {
    let mut members = Vec::new();
    for sample in samples {
        match member(sample) {
            Membership::Member => members.push(sample.status),
            Membership::NotMember => {}
            Membership::Unknown => return None,
        }
    }
    Some((!members.is_empty(), WorktreeState::fold(members)))
}

impl TurnHost {
    /// Resume the persisted baseline of `repo`, which must be the git top level.
    pub fn open(repo: PathBuf) -> Self {
        let tracker = TurnTracker::with_baseline(seed_baseline(&repo));
        Self { tracker, root: canonical(&repo), repo, resolved: HashMap::new() }
    }

    pub fn baseline(&self) -> Option<&str> {
        self.tracker.baseline()
    }

    /// Sample the agents over the herdr CLI and advance the baseline.
    pub fn sample(&mut self) -> TurnReport {
        self.observe_agents(crate::herdr::agent_samples().ok().as_deref())
    }

    /// Advance the baseline from one enumeration; `None`, a failed one, holds the last state.
    pub fn observe_agents(&mut self, samples: Option<&[AgentSample]>) -> TurnReport {
        let Some(samples) = samples else {
            return TurnReport::default();
        };
        // An unresolved member holds the sample, as a failed enumeration does.
        let Some((present, state)) = classify(samples, |s| self.membership(s.cwd.as_deref()))
        else {
            return TurnReport::default();
        };
        let (ended, written) = self.observe(state);
        TurnReport { ended, agents_present: Some(present), written }
    }

    /// An agent's place by git top level: a subdirectory is a member, a sibling worktree is not.
    fn membership(&mut self, cwd: Option<&str>) -> Membership {
        let Some(cwd) = worktree_cwd(cwd) else {
            return Membership::NotMember;
        };
        if let Some(&member) = self.resolved.get(cwd) {
            return if member { Membership::Member } else { Membership::NotMember };
        }
        match git::worktree_of(Path::new(cwd)) {
            // A resolved root never moves, so it is cached.
            git::Worktree::Root(top) => {
                let member = canonical(&top) == self.root;
                self.resolved.insert(cwd.to_string(), member);
                if member { Membership::Member } else { Membership::NotMember }
            }
            // Not cached: a directory can become a worktree later.
            git::Worktree::Outside => Membership::NotMember,
            // git could not run: hold, as a failed enumeration does.
            git::Worktree::Unknown => Membership::Unknown,
        }
    }

    /// Advance the baseline from one worktree state: whether a turn ended, and the tree written.
    fn observe(&mut self, state: WorktreeState) -> (bool, Option<String>) {
        let transition = self.tracker.observe(state);
        if transition.started {
            match git::snapshot_worktree(&self.repo) {
                // A fresh candidate cannot have diverged yet; the next poll checks.
                Ok(sha) => {
                    self.tracker.set_candidate(sha.clone());
                    return (transition.ended, Some(sha));
                }
                Err(e) => logln!("turn snapshot failed: {e}"),
            }
        }
        // Full snapshots compare, so a new untracked file counts as a change.
        let Some(candidate) = self.tracker.candidate().map(str::to_string) else {
            return (transition.ended, None);
        };
        match git::snapshot_worktree(&self.repo) {
            Ok(now) => {
                if now != candidate {
                    self.tracker.promote();
                    if let Err(e) = git::write_baseline_ref(&self.repo, &candidate) {
                        logln!("turn baseline ref write failed: {e}");
                    }
                }
                (transition.ended, Some(now))
            }
            Err(e) => {
                logln!("turn divergence check failed: {e}");
                (transition.ended, None)
            }
        }
    }
}

/// One queued refresh's attributes, accumulated on `App` until the loop dispatches it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldRequest {
    /// Sample the agents in the worktree — set by the poll alone.
    pub sample_turn: bool,
    /// Re-reveal the cursor when the result lands — user-initiated switches only.
    pub reveal: bool,
}

/// One refresh request; the completion echoes its tag.
#[derive(Debug)]
pub struct WorldJob {
    pub generation: u64,
    pub input: WorldInput,
    /// Only polls sample the agents, so herdr calls track the poll alone.
    pub sample_turn: bool,
    /// Whether the result re-reveals the cursor: a user's switch does, a poll never.
    pub reveal: bool,
}

/// A finished job; no turn without a sample, no snapshot on the `PR` tab.
#[derive(Debug)]
pub struct WorldCompletion {
    pub generation: u64,
    pub input: WorldInput,
    pub reveal: bool,
    pub turn: Option<TurnReport>,
    pub snapshot: Option<Result<WorldSnapshot>>,
}

/// Run the world worker; queued requests coalesce into the newest, keeping their flags.
pub fn spawn(
    mut host: TurnHost,
    rx: Receiver<WorldJob>,
    tx: Sender<WorldCompletion>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("world".into())
        .spawn(move || {
            git::sweep_dead_copies(&host.repo);
            while let Ok(mut job) = rx.recv() {
                while let Ok(next) = rx.try_recv() {
                    job = WorldJob {
                        sample_turn: job.sample_turn || next.sample_turn,
                        reveal: job.reveal || next.reveal,
                        ..next
                    };
                }
                let turn = job.sample_turn.then(|| host.sample());
                if tx.send(complete(&host, job, turn)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn world worker")
}

/// Finish `job` after its sample, `turn`: the build reuses the worktree tree that sample wrote.
fn complete(host: &TurnHost, mut job: WorldJob, turn: Option<TurnReport>) -> WorldCompletion {
    job.input.turn_baseline = host.baseline().map(str::to_string);
    let written = turn.as_ref().and_then(|t| t.written.clone());
    let snapshot = job.input.tab.is_file_tab().then(|| build_at(&job.input, written));
    WorldCompletion {
        generation: job.generation,
        input: job.input,
        reveal: job.reveal,
        turn,
        snapshot,
    }
}

#[cfg(test)]
mod tests {
    use super::{Membership, classify, worktree_cwd};
    use crate::herdr::AgentSample;
    use crate::turn::{Status, WorktreeState};

    #[test]
    fn a_last_turn_build_reuses_the_tree_its_sample_wrote() {
        let (dir, git) = crate::test_support::test_repo();
        let crate::git::Worktree::Root(root) = crate::git::worktree_of(dir.path()) else {
            panic!("a fresh repository resolves to a worktree root");
        };
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        let baseline = crate::git::snapshot_worktree(&root).unwrap();
        crate::git::write_baseline_ref(&root, &baseline).unwrap();
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        let mut host = super::TurnHost::open(root.clone());
        // A resting sample, then a working one: the turn's start snapshots the worktree.
        host.observe_agents(Some(&[]));
        let turn = host.observe_agents(Some(&[working_at(&root.to_string_lossy())]));
        std::fs::write(root.join("b.txt"), "later\n").unwrap();
        let job = |generation| super::WorldJob {
            generation,
            input: super::WorldInput {
                repo: root.clone(),
                tab: crate::app::Tab::Changes,
                scope: crate::model::Scope::LastTurn,
                base: None,
                base_epoch: 0,
                turn_baseline: None,
                commit_pick: None,
                toggled_dirs: std::collections::HashSet::new(),
            },
            sample_turn: true,
            reveal: false,
        };
        let paths = |done: super::WorldCompletion| -> Vec<String> {
            done.snapshot.unwrap().unwrap().changeset.files.into_keys().collect()
        };
        assert_eq!(paths(super::complete(&host, job(1), Some(turn))), ["a.txt"]);
        // A job without a sample has no tree to reuse, so it snapshots the worktree now.
        assert_eq!(paths(super::complete(&host, job(2), None)), ["a.txt", "b.txt"]);
    }

    fn working_at(cwd: &str) -> AgentSample {
        AgentSample { cwd: Some(cwd.into()), status: Status::Working }
    }

    #[test]
    fn only_an_absolute_cwd_can_name_a_worktree() {
        // A blank or relative cwd is rejected before any git call.
        let abs = if cfg!(windows) { r"C:\abs\path" } else { "/abs/path" };
        assert_eq!(worktree_cwd(Some(abs)), Some(abs));
        assert_eq!(worktree_cwd(Some("relative/path")), None);
        assert_eq!(worktree_cwd(Some("")), None);
        assert_eq!(worktree_cwd(None), None);
    }

    /// An agent whose cwd spells the root in another case is a member.
    #[cfg(windows)]
    #[test]
    fn an_agent_at_the_root_in_another_case_is_a_member() {
        let (dir, _) = crate::test_support::test_repo();
        let crate::git::Worktree::Root(root) = crate::git::worktree_of(dir.path()) else {
            panic!("a fresh repository resolves to a worktree root");
        };
        let mut host = super::TurnHost::open(root.clone());
        let recased = root.to_string_lossy().to_ascii_uppercase();
        assert_ne!(recased, root.to_string_lossy(), "the spelling really differs");
        assert!(matches!(host.membership(Some(&recased)), Membership::Member));
    }

    #[test]
    fn membership_decides_the_fold_and_undetermined_holds() {
        // One working agent, resolved three ways.
        let samples = [working_at("/w")];
        assert_eq!(classify(&samples, |_| Membership::Unknown), None);
        assert_eq!(
            classify(&samples, |_| Membership::Member),
            Some((true, WorktreeState::Working))
        );
        assert_eq!(
            classify(&samples, |_| Membership::NotMember),
            Some((false, WorktreeState::Resting))
        );
    }

    #[test]
    fn one_undetermined_member_holds_even_beside_a_resolved_one() {
        // An unknown member holds the whole sample.
        let samples = [working_at("/a"), working_at("/b")];
        let held = classify(&samples, |s| match s.cwd.as_deref() {
            Some("/b") => Membership::Unknown,
            _ => Membership::Member,
        });
        assert_eq!(held, None);
    }

    #[test]
    fn a_non_members_status_never_reaches_the_fold() {
        // A resting member and a working sibling: only the member's status folds.
        let samples = [
            AgentSample { cwd: Some("/mine".into()), status: Status::Idle },
            AgentSample { cwd: Some("/sibling".into()), status: Status::Working },
        ];
        let folded = classify(&samples, |s| match s.cwd.as_deref() {
            Some("/sibling") => Membership::NotMember,
            _ => Membership::Member,
        });
        assert_eq!(folded, Some((true, WorktreeState::Resting)));
    }
}
