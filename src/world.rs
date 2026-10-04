//! The world snapshot: the derived state one refresh produces, built from git alone.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use anyhow::Result;

use crate::app::Tab;
use crate::file_list::{Annotation, Entry};
use crate::git;
use crate::herdr::AgentSample;
use crate::model::{ChangedFile, CommitPick, Scope};
use crate::turn::{TurnTracker, WorktreeState};

/// Everything the build reads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldInput {
    pub repo: PathBuf,
    pub tab: Tab,
    pub scope: Scope,
    /// The `--base` flag, resolved fresh per build.
    pub base: Option<String>,
    /// Bumped by this pane's own pick.
    pub base_epoch: u64,
    /// The `last-turn` baseline tree the changed set diffs against; `None` before a turn.
    pub turn_baseline: Option<String>,
    /// The `commits` scope's pick.
    pub commit_pick: Option<CommitPick>,
    /// Expanded ignored directories whose children the `All files` tree loads.
    pub toggled_dirs: HashSet<String>,
}

/// The derived state one refresh produces.
#[derive(Debug)]
pub struct WorldSnapshot {
    pub changed: HashMap<String, Annotation>,
    pub entries: Vec<Entry>,
    pub branch_base: git::BaseStatus,
    /// The `commits` scope's pick verdict, from the same build as the changeset it heads `None` on
    /// every other scope.
    pub pick_status: Option<PickStatus>,
    /// The two ends the changeset was diffed between, which a file's diff reads too.
    pub ends: Option<DiffEnds>,
    /// The commit `HEAD` named when the build ran, the commit picker's universe key `None` in an
    /// unborn repository.
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

/// A build's changeset and the base or pick it diffs against, landed together.
#[derive(Debug)]
pub struct ScopeBuild {
    pub branch_base: git::BaseStatus,
    pub pick_status: Option<PickStatus>,
    pub ends: Option<DiffEnds>,
    pub changed: Vec<ChangedFile>,
}

/// Build the snapshot for `input`; the changeset is built on every tab.
pub fn build(input: &WorldInput) -> Result<WorldSnapshot> {
    // Outside a git repo, an empty snapshot paints the quiet empty state rather than a failing
    // status line every poll.
    if !git::is_repo(&input.repo) {
        return Ok(WorldSnapshot {
            changed: HashMap::new(),
            entries: Vec::new(),
            branch_base: git::BaseStatus::default(),
            pick_status: None,
            ends: None,
            head: None,
        });
    }
    let ScopeBuild { branch_base, pick_status, ends, changed } = build_changed(input)?;
    let head = git::head_oid(&input.repo);
    let changed_map = annotate(&changed);
    let entries = match input.tab {
        // The whole worktree (ignored included), with expanded ignored dirs loaded lazily.
        Tab::AllFiles => all_files_entries(input, &changed_map)?,
        // `Changes` (the `PR` tab never builds a snapshot).
        _ => changed.iter().map(Entry::from_changed).collect(),
    };
    Ok(WorldSnapshot { changed: changed_map, entries, branch_base, pick_status, ends, head })
}

/// The active scope's changeset and, on `branch`, its base.
pub fn build_changed(input: &WorldInput) -> Result<ScopeBuild> {
    let plain = |changed| ScopeBuild {
        branch_base: git::BaseStatus::default(),
        pick_status: None,
        ends: None,
        changed,
    };
    let from = |old: String, new: Option<String>| Some(DiffEnds { old, new });
    if !git::is_repo(&input.repo) {
        return Ok(plain(Vec::new()));
    }
    match input.scope {
        Scope::LastTurn => match input.turn_baseline.as_deref() {
            Some(t) => {
                let now = git::snapshot_worktree(&input.repo)?;
                let changed = git::changed_between(&input.repo, t, &now)?;
                Ok(ScopeBuild { ends: from(t.to_string(), Some(now)), ..plain(changed) })
            }
            None => Ok(plain(Vec::new())),
        },
        Scope::Uncommitted => {
            let base = git::diff_base(&input.repo);
            let changed = git::changed_from(&input.repo, &base)?;
            Ok(ScopeBuild { ends: from(base, None), ..plain(changed) })
        }
        Scope::Branch => {
            // A resolve failure fails the build, keeping the stale frame.
            let resolution = git::resolve_base(&input.repo, input.base.as_deref())
                .map_err(|e| anyhow::anyhow!("{}", e.0))?;
            let merge_base = resolution
                .status
                .winner
                .as_ref()
                .and_then(|w| git::merge_base(&input.repo, w.oid()));
            let (changed, ends) = match merge_base {
                Some(base) => (git::changed_from(&input.repo, &base)?, from(base, None)),
                None => (Vec::new(), None),
            };
            Ok(ScopeBuild { branch_base: resolution.status, ends, ..plain(changed) })
        }
        Scope::Commits => {
            // A tag without a pick builds the empty changeset.
            let Some(pick) = &input.commit_pick else { return Ok(plain(Vec::new())) };
            let (status, changed, old) = build_pick(&input.repo, pick)?;
            let ends = old.and_then(|old| from(old, Some(pick.newest.clone())));
            Ok(ScopeBuild { pick_status: Some(status), ends, ..plain(changed) })
        }
    }
}

/// The pick's changeset, verdict and old end in one pass; a `gone` pick has none.
fn build_pick(
    repo: &Path,
    pick: &CommitPick,
) -> Result<(PickStatus, Vec<ChangedFile>, Option<String>)> {
    let gone = |sha: &str| {
        (
            PickStatus {
                verdict: PickVerdict::Gone(sha.to_string()),
                subject: String::new(),
                count: 0,
            },
            Vec::new(),
            None,
        )
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
    let changed = git::changed_between(repo, &old, &pick.newest)?;
    // The oldest is an ancestor of the newest, so one reachability check covers the run.
    let verdict = if git::is_reachable(repo, &pick.newest) {
        PickVerdict::Live
    } else {
        PickVerdict::OffBranch
    };
    Ok((PickStatus { verdict, subject, count }, changed, Some(old)))
}

/// The changed-files map every consumer keys by path.
pub fn annotate(changed: &[ChangedFile]) -> HashMap<String, Annotation> {
    changed.iter().map(|f| (f.path.clone(), Annotation::from(f))).collect()
}

/// The persisted turn baseline for `repo`, if any.
pub fn seed_baseline(repo: &std::path::Path) -> Option<String> {
    git::read_baseline_ref(repo)
}

/// The `All files` entries: every worktree path (ignored dimmed), with the children of expanded
/// ignored directories loaded lazily.
pub(crate) fn all_files_entries(
    input: &WorldInput,
    changed: &HashMap<String, Annotation>,
) -> Result<Vec<Entry>> {
    let to_entry = |w: git::WorktreeEntry| Entry {
        annotation: changed.get(&w.path).cloned(),
        path: w.path,
        previous_path: None,
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

/// Turn tracking, owned by the worker.
#[derive(Debug)]
pub struct TurnHost {
    tracker: TurnTracker,
    repo: PathBuf,
    /// Each agent `cwd` resolved to whether it is a member of the reviewed worktree.
    resolved: HashMap<String, bool>,
}

/// One sample's outcome: whether it ended a turn, and what it saw of membership.
#[derive(Clone, Debug)]
pub struct TurnReport {
    pub ended: bool,
    /// `None` when the sample could not see the whole worktree.
    pub agents_present: Option<bool>,
}

/// An agent's relationship to the reviewed worktree, as [`TurnHost::membership`] resolves it.
enum Membership {
    Member,
    NotMember,
    Unknown,
}

/// The absolute cwd an agent names, or `None` for a blank or relative one.
fn worktree_cwd(cwd: Option<&str>) -> Option<&str> {
    cwd.filter(|c| Path::new(c).is_absolute())
}

/// Whether two top levels name one worktree: case-insensitive on Windows, exact elsewhere.
fn same_root(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let same = |x: &std::ffi::OsStr, y: &std::ffi::OsStr| match (x.to_str(), y.to_str()) {
            (Some(x), Some(y)) => x.to_lowercase() == y.to_lowercase(),
            _ => x == y,
        };
        a.components().count() == b.components().count()
            && a.components().zip(b.components()).all(|(x, y)| same(x.as_os_str(), y.as_os_str()))
    } else {
        a == b
    }
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
    /// Resume any persisted turn baseline for this worktree.
    pub fn open(repo: PathBuf) -> Self {
        let tracker = TurnTracker::with_baseline(seed_baseline(&repo));
        Self { tracker, repo, resolved: HashMap::new() }
    }

    pub fn baseline(&self) -> Option<&str> {
        self.tracker.baseline()
    }

    /// Sample the agents over the herdr CLI and advance the baseline.
    pub fn sample(&mut self) -> TurnReport {
        self.observe_agents(crate::herdr::agent_samples().ok().as_deref())
    }

    /// Advance the baseline from one enumeration.
    pub fn observe_agents(&mut self, samples: Option<&[AgentSample]>) -> TurnReport {
        let Some(samples) = samples else {
            return TurnReport { ended: false, agents_present: None };
        };
        // An undetermined member holds the sample like a failed enumeration.
        let Some((present, state)) = classify(samples, |s| self.membership(s.cwd.as_deref()))
        else {
            return TurnReport { ended: false, agents_present: None };
        };
        let ended = self.observe(state);
        TurnReport { ended, agents_present: Some(present) }
    }

    /// An agent's relationship to the reviewed worktree.
    fn membership(&mut self, cwd: Option<&str>) -> Membership {
        let Some(cwd) = worktree_cwd(cwd) else {
            return Membership::NotMember;
        };
        if let Some(&member) = self.resolved.get(cwd) {
            return if member { Membership::Member } else { Membership::NotMember };
        }
        match git::worktree_of(Path::new(cwd)) {
            // A resolved root is stable, so record whether it is a member and never shell out for
            // this cwd again. git canonicalizes it, so the worktree root itself matches too.
            git::Worktree::Root(top) => {
                let member = same_root(&top, &self.repo);
                self.resolved.insert(cwd.to_string(), member);
                if member { Membership::Member } else { Membership::NotMember }
            }
            // git ran and found no worktree.
            git::Worktree::Outside => Membership::NotMember,
            // git could not run, so nothing is known this poll.
            git::Worktree::Unknown => Membership::Unknown,
        }
    }

    /// Advance the baseline from one folded worktree state, returning whether a turn ended.
    fn observe(&mut self, state: WorktreeState) -> bool {
        let transition = self.tracker.observe(state);
        if transition.started {
            match git::snapshot_worktree(&self.repo) {
                // The candidate is this worktree as of a moment ago, so it cannot have diverged from it yet.
                Ok(sha) => {
                    self.tracker.set_candidate(sha);
                    return transition.ended;
                }
                Err(e) => logln!("turn snapshot failed: {e}"),
            }
        }
        // Promote the pending candidate once the turn has changed a file.
        let Some(candidate) = self.tracker.candidate().map(str::to_string) else {
            return transition.ended;
        };
        match git::snapshot_worktree(&self.repo) {
            Ok(now) if now != candidate => {
                self.tracker.promote();
                if let Err(e) = git::write_baseline_ref(&self.repo, &candidate) {
                    logln!("turn baseline ref write failed: {e}");
                }
            }
            Ok(_) => {}
            Err(e) => logln!("turn divergence check failed: {e}"),
        }
        transition.ended
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

/// One refresh request.
#[derive(Debug)]
pub struct WorldJob {
    pub generation: u64,
    pub input: WorldInput,
    /// Poll-driven requests sample the agents in the worktree.
    pub sample_turn: bool,
    /// A user-initiated switch re-reveals the cursor when its result lands; a poll never does.
    pub reveal: bool,
}

/// A finished job: its tag, sample outcome, and snapshot.
#[derive(Debug)]
pub struct WorldCompletion {
    pub generation: u64,
    pub input: WorldInput,
    pub reveal: bool,
    pub turn: Option<TurnReport>,
    pub snapshot: Option<Result<WorldSnapshot>>,
}

/// Run the world worker until the request channel closes.
pub fn spawn(
    mut host: TurnHost,
    rx: Receiver<WorldJob>,
    tx: Sender<WorldCompletion>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("world".into())
        .spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(next) = rx.try_recv() {
                    job = WorldJob {
                        sample_turn: job.sample_turn || next.sample_turn,
                        reveal: job.reveal || next.reveal,
                        ..next
                    };
                }
                let turn = job.sample_turn.then(|| host.sample());
                job.input.turn_baseline = host.baseline().map(str::to_string);
                let snapshot = job.input.tab.is_file_tab().then(|| build(&job.input));
                let completion = WorldCompletion {
                    generation: job.generation,
                    input: job.input,
                    reveal: job.reveal,
                    turn,
                    snapshot,
                };
                if tx.send(completion).is_err() {
                    break;
                }
            }
        })
        .expect("spawn world worker")
}

#[cfg(test)]
mod tests {
    use super::{Membership, classify, same_root, worktree_cwd};
    use crate::herdr::AgentSample;
    use crate::turn::{Status, WorktreeState};
    use std::path::Path;

    fn working_at(cwd: &str) -> AgentSample {
        AgentSample { cwd: Some(cwd.into()), status: Status::Working }
    }

    #[test]
    fn only_an_absolute_cwd_can_name_a_worktree() {
        // A blank or relative cwd would resolve against reviewr's own cwd (the reviewed worktree).
        let abs = if cfg!(windows) { r"C:\abs\path" } else { "/abs/path" };
        assert_eq!(worktree_cwd(Some(abs)), Some(abs));
        assert_eq!(worktree_cwd(Some("relative/path")), None);
        assert_eq!(worktree_cwd(Some("")), None);
        assert_eq!(worktree_cwd(None), None);
    }

    #[test]
    fn a_root_in_another_case_is_the_same_worktree_only_on_windows() {
        // The literal pair, not git's output, pins the comparison itself.
        let same = |a: &str, b: &str| same_root(Path::new(a), Path::new(b));
        assert!(same("C:/Work/Repo", "C:/Work/Repo"));
        assert!(!same("C:/Work/Repo", "C:/Work/Other"));
        assert!(!same("C:/Work/Repo", "C:/Work/Repo/sub"), "a subdirectory is not the root");
        assert_eq!(same("C:/Work/Repo", "c:/work/REPO"), cfg!(windows));
        assert_eq!(same("C:/Users/Jürgen", "C:/Users/JÜRGEN"), cfg!(windows), "beyond ASCII");
        assert_eq!(same("/work/Repo", "/work/repo"), cfg!(windows));
    }

    /// An agent whose cwd spells the root in another case is a member.
    #[cfg(windows)]
    #[test]
    fn an_agent_at_the_root_in_another_case_is_a_member() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(status.success());
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
        // A member resting and a non-member (a sibling worktree) working.
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
