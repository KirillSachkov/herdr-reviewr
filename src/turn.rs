//! Turn tracking: a turn is the worktree's rest→work edge, its baseline promoted once a file changes.

/// The agent status reported by `herdr agent list` (`agent_status`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    Unknown,
}

impl Status {
    /// A status between turns; `blocked` and `unknown` are mid-turn.
    fn is_resting(self) -> bool {
        matches!(self, Status::Idle | Status::Done)
    }

    /// The status an `agent_status` spelling means; an unknown one is mid-turn, never an edge.
    pub fn from_wire(wire: &str) -> Self {
        match wire {
            "idle" => Status::Idle,
            "working" => Status::Working,
            "blocked" => Status::Blocked,
            "done" => Status::Done,
            _ => Status::Unknown,
        }
    }
}

/// The worktree's work state, folded from every agent in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorktreeState {
    /// Every agent rests, or there is none.
    #[default]
    Resting,
    /// At least one agent works.
    Working,
    /// An agent is `blocked` or `unknown` and none works: an open turn stays open.
    Neither,
}

impl WorktreeState {
    /// Fold the members' statuses; any `working` wins.
    pub fn fold(statuses: impl IntoIterator<Item = Status>) -> Self {
        let mut held = false;
        for status in statuses {
            if status == Status::Working {
                return Self::Working;
            }
            held |= !status.is_resting();
        }
        if held { Self::Neither } else { Self::Resting }
    }
}

/// The lifecycle edges produced by one sample of the worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnTransition {
    pub started: bool,
    pub ended: bool,
}

/// The turn baseline lifecycle: a candidate snapshot, and the live baseline `last-turn` reads.
#[derive(Default, Debug)]
pub struct TurnTracker {
    /// Whether the previous sample rested; `false` first, so the first sample starts nothing.
    prev_resting: bool,
    /// Whether the worktree worked since its last rest: an end can come through `Neither`,
    /// where a start never does, so the two edges keep separate memories.
    worked: bool,
    candidate: Option<String>,
    baseline: Option<String>,
}

impl TurnTracker {
    /// Seed from the persisted baseline ref at startup (`None` until a turn is observed).
    pub fn with_baseline(baseline: Option<String>) -> Self {
        Self { baseline, ..Self::default() }
    }

    /// The live baseline tree the `last-turn` diff reads against.
    pub fn baseline(&self) -> Option<&str> {
        self.baseline.as_deref()
    }

    pub fn has_baseline(&self) -> bool {
        self.baseline.is_some()
    }

    /// The candidate snapshot awaiting promotion, if a turn is in flight.
    pub fn candidate(&self) -> Option<&str> {
        self.candidate.as_deref()
    }

    /// Record one sample: a start is `Resting → Working`, an end any return to rest after work.
    pub fn observe(&mut self, state: WorktreeState) -> TurnTransition {
        let transition = TurnTransition {
            started: state == WorktreeState::Working && self.prev_resting,
            ended: self.worked && state == WorktreeState::Resting,
        };
        self.prev_resting = state == WorktreeState::Resting;
        self.worked = match state {
            WorktreeState::Working => true,
            WorktreeState::Resting => false,
            WorktreeState::Neither => self.worked,
        };
        transition
    }

    /// Store a turn start's snapshot as the candidate, replacing an unpromoted one.
    pub fn set_candidate(&mut self, sha: String) {
        self.candidate = Some(sha);
    }

    /// Promote the candidate to the baseline, returned for the host to persist.
    pub fn promote(&mut self) -> Option<&str> {
        if self.candidate.is_some() {
            self.baseline = self.candidate.take();
        }
        self.baseline.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{Status, TurnTracker, WorktreeState};

    #[test]
    fn from_wire_reads_herdrs_four_spellings_and_folds_the_rest_to_unknown() {
        // herdr's spellings, pinned literally.
        assert_eq!(Status::from_wire("idle"), Status::Idle);
        assert_eq!(Status::from_wire("working"), Status::Working);
        assert_eq!(Status::from_wire("blocked"), Status::Blocked);
        assert_eq!(Status::from_wire("done"), Status::Done);
        assert_eq!(Status::from_wire("unknown"), Status::Unknown);
        // A new herdr state is unknown, never resting, so it starts no turn.
        assert_eq!(Status::from_wire("compacting"), Status::Unknown);
        assert!(!Status::from_wire("compacting").is_resting());
    }

    #[test]
    fn an_empty_worktree_rests_so_its_first_working_agent_starts_a_turn() {
        // No agents rests, so a fresh pane tracks the next turn it sees.
        assert_eq!(WorktreeState::fold([]), WorktreeState::Resting);
        let mut t = TurnTracker::default();
        t.observe(WorktreeState::fold([]));
        assert!(t.observe(WorktreeState::fold([Status::Working])).started);
    }

    #[test]
    fn one_working_agent_makes_the_whole_worktree_work() {
        // `working` wins over every peer.
        assert_eq!(WorktreeState::fold([Status::Idle, Status::Working]), WorktreeState::Working);
        assert_eq!(WorktreeState::fold([Status::Blocked, Status::Working]), WorktreeState::Working);
        assert_eq!(WorktreeState::fold([Status::Idle, Status::Done]), WorktreeState::Resting);
    }

    #[test]
    fn a_held_agent_with_no_worker_leaves_the_worktree_neither() {
        // `blocked` and `unknown` never rest.
        assert_eq!(WorktreeState::fold([Status::Blocked, Status::Idle]), WorktreeState::Neither);
        assert_eq!(WorktreeState::fold([Status::Unknown]), WorktreeState::Neither);
    }

    #[test]
    fn a_turn_starts_when_the_worktree_works_after_resting() {
        let mut t = TurnTracker::default();
        assert!(!t.observe(WorktreeState::Resting).started, "the first sample never starts a turn");
        assert!(t.observe(WorktreeState::Working).started, "resting → working starts a turn");
    }

    #[test]
    fn a_held_worktree_returning_to_work_is_a_continuation() {
        let mut t = TurnTracker::default();
        t.observe(WorktreeState::Resting);
        t.observe(WorktreeState::Working); // turn started
        t.observe(WorktreeState::Neither); // permission prompt mid-turn
        assert!(
            !t.observe(WorktreeState::Working).started,
            "neither → working resumes the same turn"
        );
    }

    #[test]
    fn a_turn_ends_only_on_a_working_to_resting_edge() {
        let mut t = TurnTracker::default();
        assert!(!t.observe(WorktreeState::Resting).ended, "no prior work, so no turn to end");
        assert!(!t.observe(WorktreeState::Working).ended, "resting → working starts, never ends");
        assert!(
            !t.observe(WorktreeState::Neither).ended,
            "working → neither is a mid-turn pause, not an end"
        );
        t.observe(WorktreeState::Working);
        assert!(t.observe(WorktreeState::Resting).ended, "working → resting ends the turn");
    }

    #[test]
    fn a_turn_held_by_a_prompt_still_ends_when_the_worktree_rests() {
        // Working → prompt → idle: the end comes through `Neither`.
        let mut t = TurnTracker::default();
        t.observe(WorktreeState::Resting);
        t.observe(WorktreeState::Working);
        assert!(!t.observe(WorktreeState::Neither).ended, "the prompt holds the turn open");
        assert!(t.observe(WorktreeState::Resting).ended, "resting after it ends the turn");
        assert!(
            !t.observe(WorktreeState::Resting).ended,
            "a turn ends once, not on every resting sample after it"
        );
    }

    #[test]
    fn a_lone_first_working_sample_never_starts_a_turn() {
        let mut t = TurnTracker::default();
        assert!(!t.observe(WorktreeState::Working).started, "we did not observe this turn's start");
    }

    #[test]
    fn promote_moves_the_candidate_to_the_baseline() {
        let mut t = TurnTracker::with_baseline(None);
        assert!(!t.has_baseline());
        t.set_candidate("tree-a".into());
        assert_eq!(t.candidate(), Some("tree-a"));
        assert_eq!(t.promote(), Some("tree-a"));
        assert_eq!(t.baseline(), Some("tree-a"));
        assert_eq!(t.candidate(), None, "promotion consumes the candidate");
    }

    #[test]
    fn promote_without_a_candidate_keeps_the_baseline() {
        let mut t = TurnTracker::with_baseline(Some("tree-a".into()));
        assert_eq!(t.promote(), Some("tree-a"), "no candidate leaves the baseline intact");
        assert_eq!(t.baseline(), Some("tree-a"));
    }

    #[test]
    fn a_question_only_turn_keeps_the_previous_baseline() {
        // Turn A edits a file: candidate captured then promoted.
        let mut t = TurnTracker::default();
        t.observe(WorktreeState::Resting);
        t.observe(WorktreeState::Working);
        t.set_candidate("turn-a".into());
        t.promote();
        // Turn B is question-only: candidate captured at its start, never promoted.
        t.observe(WorktreeState::Resting);
        t.observe(WorktreeState::Working);
        t.set_candidate("turn-b".into());
        assert_eq!(t.baseline(), Some("turn-a"), "the unpromoted turn keeps A's baseline");
    }
}
