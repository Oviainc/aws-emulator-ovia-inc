// SPDX-License-Identifier: CC0-1.0

//! Mergeability: the merge base of two commits, and which of the strategies a
//! merge of a source commit into a destination commit admits.
//!
//! Every answer here is computed from the stored history and the stored object
//! bytes of one repository — the parents recorded on each [`Commit`] and the
//! blobs the two tree snapshots name — so a merge question and a later read of
//! the same repository cannot disagree. Nothing in this module writes: asking
//! whether two commits merge leaves the repository exactly as it was.
//!
//! The three-way content merge a line-level answer needs is
//! [`diffy::merge_bytes`], the same three-way merge git performs over a common
//! ancestor and two sides, rather than a line comparison written here.
//!
//! Every phase of an evaluation is charged to a [`MergeBudget`], because its
//! cost is a PRODUCT: a request names two tips whose histories it walks to find
//! their merge base, two snapshots whose union can carry a tree's worth of
//! paths, and each path both sides changed can demand a three-way merge of
//! caller-written file content. Caps on the stored size bound each factor and
//! none of the product, so the budget bounds the commits walked, the paths
//! compared, the content bytes loaded and the time one request may spend, and
//! the evaluation is refused rather than run to completion once any of them is
//! spent.

use std::collections::BTreeSet;

use super::{Commit, FileEntry, Repository, MAX_FILES_PER_TREE, MAX_PROJECTION_SECONDS};

/// `MergeOptionTypeEnum` values the model declares
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`): the strategies a
/// merge can be performed with, reported in that declared order.
pub const MERGE_OPTION_FAST_FORWARD: &str = "FAST_FORWARD_MERGE";
pub const MERGE_OPTION_SQUASH: &str = "SQUASH_MERGE";
pub const MERGE_OPTION_THREE_WAY: &str = "THREE_WAY_MERGE";

/// `ConflictDetailLevelTypeEnum` values the model declares
/// (`smithy_query(query="shape",shape="ConflictDetailLevelTypeEnum")`), and the
/// one a request that names none is evaluated at. Documented on
/// `conflictDetailLevel`: "If unspecified, the default FILE_LEVEL is used, which
/// returns a not-mergeable result if the same file has differences in both
/// branches. If LINE_LEVEL is specified, a conflict is considered not mergeable
/// if the same file in both branches has differences on the same line."
pub const CONFLICT_DETAIL_FILE_LEVEL: &str = "FILE_LEVEL";
pub const CONFLICT_DETAIL_LINE_LEVEL: &str = "LINE_LEVEL";
pub const CONFLICT_DETAIL_LEVELS: [&str; 2] = [CONFLICT_DETAIL_FILE_LEVEL, CONFLICT_DETAIL_LINE_LEVEL];

/// `ConflictResolutionStrategyTypeEnum` values the model declares
/// (`smithy_query(query="shape",shape="ConflictResolutionStrategyTypeEnum")`),
/// and the one a request that names none is evaluated with. Documented on
/// `conflictResolutionStrategy`: "Specifies which branch to use when resolving
/// conflicts, or whether to attempt automatically merging two versions of a
/// file. The default is NONE, which requires any conflicts to be resolved
/// manually before the merge operation is successful."
pub const CONFLICT_RESOLUTION_NONE: &str = "NONE";
pub const CONFLICT_RESOLUTION_ACCEPT_SOURCE: &str = "ACCEPT_SOURCE";
pub const CONFLICT_RESOLUTION_ACCEPT_DESTINATION: &str = "ACCEPT_DESTINATION";
pub const CONFLICT_RESOLUTION_AUTOMERGE: &str = "AUTOMERGE";
pub const CONFLICT_RESOLUTION_STRATEGIES: [&str; 4] = [
    CONFLICT_RESOLUTION_NONE,
    CONFLICT_RESOLUTION_ACCEPT_SOURCE,
    CONFLICT_RESOLUTION_ACCEPT_DESTINATION,
    CONFLICT_RESOLUTION_AUTOMERGE,
];

/// How a merge of one commit into another stands in stored state: the two tips
/// it was evaluated between, their merge base, and what the evaluation found.
#[derive(Debug, Clone)]
pub struct MergeEvaluation {
    /// The full id of the commit the source specifier resolved to.
    pub source_commit_id: String,
    /// The full id of the commit the destination specifier resolved to.
    pub destination_commit_id: String,
    /// The full id of the merge base: the most recent commit both tips reach.
    pub base_commit_id: String,
    /// The paths whose two sides the requested detail level and resolution
    /// strategy cannot reconcile, in path order. Empty means the merge needs no
    /// manual resolution.
    pub conflicts: Vec<String>,
}

impl MergeEvaluation {
    /// Can the destination simply advance to the source, with no merge commit?
    ///
    /// That is what a fast-forward is: the destination tip is already an
    /// ancestor of the source tip, so the merge base IS the destination commit
    /// and the destination's history is a prefix of the source's.
    pub fn is_fast_forward(&self) -> bool {
        self.base_commit_id == self.destination_commit_id
    }

    /// Does this merge need no manual conflict resolution?
    pub fn is_mergeable(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// The strategies this merge admits, in the order `MergeOptionTypeEnum`
    /// declares them.
    ///
    /// A fast-forward is available only where the destination can advance to the
    /// source; a squash or a three-way merge is available where the two sides
    /// carry no conflict the request's detail level and resolution strategy
    /// leaves unresolved.
    pub fn merge_options(&self) -> Vec<String> {
        let mut options: Vec<String> = Vec::new();
        if self.is_fast_forward() {
            options.push(MERGE_OPTION_FAST_FORWARD.to_string());
        }
        if self.is_mergeable() {
            options.push(MERGE_OPTION_SQUASH.to_string());
            options.push(MERGE_OPTION_THREE_WAY.to_string());
        }
        options
    }
}

/// How many paths one request's merge evaluations may compare in total.
///
/// The emulator's OWN bound, not an AWS limit: the comparison walks the union of
/// the base snapshot and the two sides, and three snapshots a write path
/// produced hold at most [`MAX_FILES_PER_TREE`] entries each, so this is the
/// largest union they can present. A request past it is comparing snapshots no
/// write path could have built, and the figure bounds the walk whatever produced
/// the stored state.
pub const MAX_ITEMS_TO_COMPARE_PER_REQUEST: usize = 3 * MAX_FILES_PER_TREE;

/// The largest ONE version of a file this emulator merges line by line, in
/// bytes, and the most lines it may hold.
///
/// The emulator's OWN bounds, and the ones the per-request total cannot give:
/// a three-way line merge is not linear in its input (a line diff costs on the
/// order of the product of the two sides' differing lines), and a single merge
/// is one synchronous unit no deadline can interrupt once it starts. Bounding
/// each version's bytes AND its line count bounds the work ONE merge can cost,
/// so the deadline is checked between merges that are each short. Content past
/// them is not loaded: the request is refused with the modelled
/// `MaximumFileContentToLoadExceededException` rather than answered after an
/// unbounded span.
pub const MAX_MERGE_FILE_BYTES: usize = 512 * 1024;
pub const MAX_MERGE_FILE_LINES: usize = 8 * 1024;

/// How many bytes of file content one request's merge evaluations may load into
/// three-way merges in total.
///
/// The emulator's OWN bound, not an AWS limit, and the one that caps the PRODUCT
/// the paths cap alone leaves open: content is caller-written and a single file
/// may be megabytes, so without this a request could demand a line-level merge
/// of a tree's worth of large files. It is charged per merge over the ancestor
/// and both sides, so a file pair too large to merge within what is left is
/// refused instead of merged.
pub const MAX_CONTENT_BYTES_PER_REQUEST: usize = 8 * 1024 * 1024;

/// How long one request's merge evaluations may take before they are refused.
/// The same figure, for the same reason, as the projection's own deadline: the
/// caps above bound the work and this bounds the time it may take on any
/// machine, so the store lock the evaluation runs under is never held for an
/// unbounded span. The evaluation also runs off the async worker — see
/// `crate::service`'s `off_the_async_worker`.
pub const MAX_MERGE_SECONDS: u64 = MAX_PROJECTION_SECONDS;

/// How many commits one request's merge evaluations may visit while resolving
/// merge bases.
///
/// The emulator's OWN bound, not an AWS limit, charged per commit the history
/// walk reaches. Each commit is visited at most once per walk, and a repository
/// retains at most [`super::MAX_COMMITS_RETAINED`] commits (whose parent lists
/// can also name commits pruning has dropped), so this is well above what a
/// written history can present while still bounding the walk whatever produced
/// the stored state.
pub const MAX_COMMITS_WALKED_PER_REQUEST: usize = 16 * super::MAX_COMMITS_RETAINED;

/// Why a merge could not be evaluated: what the caller asked about, or what the
/// evaluation would have cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeProblem {
    /// The two tips share no commit, so there is no common ancestor to compare
    /// either side's changes against.
    NoMergeBase,
    /// More commits or paths than one request compares.
    ItemsToCompare,
    /// More file content than one request loads into three-way merges.
    ContentToLoad,
    /// Longer than one request spends comparing.
    Deadline,
}

/// What ONE request's whole merge evaluation may still spend: commits walked,
/// paths compared, content bytes loaded and time.
///
/// One budget is built per request and threaded through every evaluation that
/// request drives — and through every phase of one, base resolution included —
/// so neither a long history nor a second question can multiply the work,
/// exactly as [`super::ProjectionBudget`] does for a push's projections.
#[derive(Debug)]
pub struct MergeBudget {
    commits_left: usize,
    items_left: usize,
    content_bytes_left: usize,
    deadline: std::time::Instant,
}

impl Default for MergeBudget {
    fn default() -> Self {
        Self::for_one_request()
    }
}

impl MergeBudget {
    /// The allowance one request gets.
    pub fn for_one_request() -> Self {
        Self {
            commits_left: MAX_COMMITS_WALKED_PER_REQUEST,
            items_left: MAX_ITEMS_TO_COMPARE_PER_REQUEST,
            content_bytes_left: MAX_CONTENT_BYTES_PER_REQUEST,
            deadline: std::time::Instant::now()
                + std::time::Duration::from_secs(MAX_MERGE_SECONDS),
        }
    }

    /// Charge one commit visited by a history walk.
    fn commit(&mut self) -> Result<(), MergeProblem> {
        self.commits_left = self
            .commits_left
            .checked_sub(1)
            .ok_or(MergeProblem::ItemsToCompare)?;
        if self.commits_left.is_multiple_of(256) {
            self.check_deadline()?;
        }
        Ok(())
    }

    /// Charge one path compared.
    fn item(&mut self) -> Result<(), MergeProblem> {
        self.items_left = self
            .items_left
            .checked_sub(1)
            .ok_or(MergeProblem::ItemsToCompare)?;
        // Checked as the paths are walked, not only per merge: a comparison that
        // loads no content at all still has to end.
        if self.items_left.is_multiple_of(256) {
            self.check_deadline()?;
        }
        Ok(())
    }

    /// Charge the bytes one three-way merge reads.
    fn content(&mut self, bytes: usize) -> Result<(), MergeProblem> {
        self.content_bytes_left = self
            .content_bytes_left
            .checked_sub(bytes)
            .ok_or(MergeProblem::ContentToLoad)?;
        self.check_deadline()
    }

    fn check_deadline(&self) -> Result<(), MergeProblem> {
        if std::time::Instant::now() > self.deadline {
            return Err(MergeProblem::Deadline);
        }
        Ok(())
    }
}

/// Every commit id reachable from `commit_id` by following parents, including
/// `commit_id` itself, charged to `budget`.
///
/// [`Repository::ancestry`] answers the same question uncharged; this walk is
/// driven by a caller's request, so every commit it visits is charged and the
/// deadline is checked as it goes. Each commit is visited at most once.
fn charged_ancestry(
    repository: &Repository,
    commit_id: &str,
    budget: &mut MergeBudget,
) -> Result<BTreeSet<String>, MergeProblem> {
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<String> = vec![commit_id.to_string()];
    while let Some(id) = pending.pop() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        budget.commit()?;
        if let Some(commit) = repository.commits.get(&id) {
            pending.extend(commit.parents.iter().cloned());
        }
    }
    Ok(reachable)
}

/// The merge base of two commits: the most recent commit both reach by
/// following parents.
///
/// Where several common ancestors are maximal (neither reaches the other, as
/// after two merges crossed), the newest written one is the base, so the answer
/// is the same for every caller and for every repeat of the same question.
///
/// `Err(MergeProblem::NoMergeBase)` when the two histories share no commit at
/// all; every commit the three walks visit is charged to `budget`, so resolving
/// a base over a long history is bounded exactly as the comparison that follows
/// it is.
pub fn merge_base(
    repository: &Repository,
    source_commit_id: &str,
    destination_commit_id: &str,
    budget: &mut MergeBudget,
) -> Result<String, MergeProblem> {
    let source_history = charged_ancestry(repository, source_commit_id, budget)?;
    let destination_history = charged_ancestry(repository, destination_commit_id, budget)?;
    let common: BTreeSet<String> = source_history
        .intersection(&destination_history)
        .cloned()
        .collect();
    if common.is_empty() {
        return Err(MergeProblem::NoMergeBase);
    }

    // A common ancestor that another common ancestor reaches through its parents
    // is not the MOST RECENT one: only the candidates nothing else reaches can
    // be the base. ONE walk from every candidate's parents answers that for all
    // of them at once — each commit expanded at most once, rather than a fresh
    // ancestry walk per candidate.
    let mut superseded: BTreeSet<String> = BTreeSet::new();
    let mut expanded: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<String> = common
        .iter()
        .filter_map(|candidate| repository.commits.get(candidate))
        .flat_map(|commit| commit.parents.iter().cloned())
        .collect();
    while let Some(id) = pending.pop() {
        if !expanded.insert(id.clone()) {
            continue;
        }
        budget.commit()?;
        if common.contains(&id) {
            superseded.insert(id.clone());
        }
        if let Some(commit) = repository.commits.get(&id) {
            pending.extend(commit.parents.iter().cloned());
        }
    }
    common
        .iter()
        .filter(|candidate| !superseded.contains(*candidate))
        // Newest write first, then by id, so the choice never depends on
        // iteration luck.
        .max_by_key(|candidate| {
            (
                repository
                    .commits
                    .get(*candidate)
                    .map_or(0u64, |commit| commit.sequence),
                (*candidate).clone(),
            )
        })
        // Every candidate superseded by another cannot happen (the reachability
        // is acyclic), but the newest common ancestor is still an answer rather
        // than a panic.
        .or_else(|| common.iter().next_back())
        .cloned()
        .ok_or(MergeProblem::NoMergeBase)
}

/// Evaluate a merge of `source` into `destination` in this repository at the
/// given conflict detail level and resolution strategy, charging the work to
/// `budget`.
///
/// `Err` names what stopped it: two tips with no common ancestor, or the
/// allowance this request had left.
pub fn evaluate(
    repository: &Repository,
    source: &Commit,
    destination: &Commit,
    conflict_detail_level: &str,
    conflict_resolution_strategy: &str,
    budget: &mut MergeBudget,
) -> Result<MergeEvaluation, MergeProblem> {
    let base_commit_id =
        merge_base(repository, &source.commit_id, &destination.commit_id, budget)?;
    let base = repository.commits.get(&base_commit_id);
    let paths: BTreeSet<&String> = base
        .map(|base| base.files.keys().collect::<BTreeSet<&String>>())
        .unwrap_or_default()
        .into_iter()
        .chain(source.files.keys())
        .chain(destination.files.keys())
        .collect();
    let mut conflicts: Vec<String> = Vec::new();
    for path in paths {
        budget.item()?;
        if conflicts_at(
            repository,
            base.and_then(|base| base.files.get(path)),
            source.files.get(path),
            destination.files.get(path),
            conflict_detail_level,
            conflict_resolution_strategy,
            budget,
        )? {
            conflicts.push(path.clone());
        }
    }
    Ok(MergeEvaluation {
        source_commit_id: source.commit_id.clone(),
        destination_commit_id: destination.commit_id.clone(),
        base_commit_id,
        conflicts,
    })
}

/// Do the two sides of ONE path conflict?
///
/// A path is a conflict only where BOTH sides changed it away from the merge
/// base and into different states: a path only one side touched carries that
/// side's change, and a path both sides changed the same way is already agreed.
/// A file's mode and its content are changed independently, so they are judged
/// independently: a mode only one side moved and content only the other side
/// moved reconcile into one file. Of what remains, a resolution strategy that
/// names a side resolves every path, and a line-level evaluation (or an
/// automerge, which attempts "to automatically merge two versions of a file")
/// resolves the content whose two sides changed different lines.
fn conflicts_at(
    repository: &Repository,
    base: Option<&FileEntry>,
    source: Option<&FileEntry>,
    destination: Option<&FileEntry>,
    conflict_detail_level: &str,
    conflict_resolution_strategy: &str,
    budget: &mut MergeBudget,
) -> Result<bool, MergeProblem> {
    if same_entry(source, destination) || same_entry(source, base) || same_entry(destination, base) {
        return Ok(false);
    }
    if matches!(
        conflict_resolution_strategy,
        CONFLICT_RESOLUTION_ACCEPT_SOURCE | CONFLICT_RESOLUTION_ACCEPT_DESTINATION
    ) {
        // One side is taken whole, so no path is left to resolve manually.
        return Ok(false);
    }
    // One side no longer holds a path the other changed: the side that deleted it
    // has no content or mode left to carry forward, so the two sides cannot both
    // be honoured whatever the detail level says about lines.
    let (Some(source), Some(destination)) = (source, destination) else {
        return Ok(true);
    };

    // A facet both sides moved, and moved differently, is the conflict; a facet
    // only one side moved away from the base is that side's change.
    let mode_conflict = source.file_mode != destination.file_mode
        && base.is_none_or(|base| {
            base.file_mode != source.file_mode && base.file_mode != destination.file_mode
        });
    // "If LINE_LEVEL is specified, a conflict is considered not mergeable if the
    // same file in both branches has differences on the same line", and an
    // automerge attempts the same reconciliation whatever detail level was asked
    // for.
    let by_line = conflict_detail_level == CONFLICT_DETAIL_LINE_LEVEL
        || conflict_resolution_strategy == CONFLICT_RESOLUTION_AUTOMERGE;
    let content_changed_on_both_sides = source.blob_id != destination.blob_id
        && base.is_none_or(|base| {
            base.blob_id != source.blob_id && base.blob_id != destination.blob_id
        });
    let content_conflict = content_changed_on_both_sides
        && !(by_line && merges_by_line(repository, base, source, destination, budget)?);
    Ok(mode_conflict || content_conflict)
}

/// Are two sides of a path in the same state? Two absences are, since neither
/// side holds the path at all.
fn same_entry(left: Option<&FileEntry>, right: Option<&FileEntry>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.blob_id == right.blob_id && left.file_mode == right.file_mode
        }
        _ => false,
    }
}

/// Does a three-way content merge of this path reconcile the two sides without
/// leaving a conflict?
///
/// A path neither side's history held before carries no common ancestor content,
/// so its two sides are merged against empty content — which reconciles only
/// where each side adds different lines in different places. Content this
/// repository does not hold the bytes of cannot be compared at all, which is a
/// conflict rather than a merge.
///
/// The bytes of all three versions are charged to `budget` BEFORE the merge
/// runs, and no single version past [`MAX_MERGE_FILE_BYTES`] or
/// [`MAX_MERGE_FILE_LINES`] is merged at all, so both the content one request
/// loads in total and the cost of each individual merge are capped.
fn merges_by_line(
    repository: &Repository,
    base: Option<&FileEntry>,
    source: &FileEntry,
    destination: &FileEntry,
    budget: &mut MergeBudget,
) -> Result<bool, MergeProblem> {
    let ancestor: &[u8] = match base {
        None => &[],
        Some(base) => match repository.blob(&base.blob_id) {
            Some(bytes) => bytes,
            None => return Ok(false),
        },
    };
    let (Some(ours), Some(theirs)) = (
        repository.blob(&destination.blob_id),
        repository.blob(&source.blob_id),
    ) else {
        return Ok(false);
    };
    budget.content(
        ancestor
            .len()
            .saturating_add(ours.len())
            .saturating_add(theirs.len()),
    )?;
    // Counting the lines of each version is linear in bytes the budget has just
    // been charged for, and it is what bounds the merge below: one version of
    // hundreds of thousands of short, differing lines would cost orders of
    // magnitude more than its byte count suggests.
    for version in [ancestor, ours, theirs] {
        if version.len() > MAX_MERGE_FILE_BYTES
            || version.iter().filter(|byte| **byte == b'\n').count() >= MAX_MERGE_FILE_LINES
        {
            return Err(MergeProblem::ContentToLoad);
        }
    }
    Ok(diffy::merge_bytes(ancestor, ours, theirs).is_ok())
}
