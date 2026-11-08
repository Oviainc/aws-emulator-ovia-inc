// SPDX-License-Identifier: CC0-1.0

//! Merging two commits of one repository: the merge base of their histories, the
//! three-way merge of their tree snapshots, and the conflicts that merge could
//! not settle on its own.
//!
//! Every CodeCommit merge API answers the same question — "what does merging this
//! source commit into this destination commit produce?" — so the answer is
//! computed ONCE, here, over the stored snapshots ([`super::Commit::files`]) and
//! the stored object bytes of one repository, and the operations differ only in
//! what they do with it: `GetMergeOptions` reports which strategies the merge
//! admits, `CreateUnreferencedMergeCommit` writes the merged tree as a commit no
//! reference names, the `MergeBranchesBy*` family moves a branch onto it, and the
//! `*MergeConflicts` family reports the conflicts. A merge question and a later
//! read of the same repository therefore cannot disagree, and nothing in this
//! module writes: asking whether two commits merge leaves the repository exactly
//! as it was, while a caller that wants the merge STORED commits the snapshot it
//! is handed back.
//!
//! The merge itself is git's: the tree of the merge base (the most recent commit
//! both tips reach) is the base of a three-way merge, a path only one side
//! changed takes that side's version, a path both sides changed the same way
//! takes the agreed version, and a path the two sides changed differently is a
//! conflict — `conflictDetailLevel` says how precisely a difference is looked
//! for, and `conflictResolutionStrategy` what to do with the differences that
//! remain. A file's mode and its content move independently, so they are judged
//! independently. The three-way content merge a line-level answer needs is
//! [`diffy::merge_bytes`], the same three-way merge git performs over a common
//! ancestor and two sides, rather than a line comparison written here.
//!
//! Every phase of a merge is charged to a [`MergeBudget`], because its cost is a
//! PRODUCT: a request names two tips whose histories it walks to find their merge
//! base, two snapshots whose union can carry a tree's worth of paths, and each
//! path both sides changed can demand a three-way merge of caller-written file
//! content. Caps on the stored size bound each factor and none of the product, so
//! the budget bounds the commits walked, the paths compared, the content bytes
//! loaded and the time one request may spend, and the merge is refused rather
//! than run to completion once any of them is spent.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    blob_id, normalize_path, Commit, FileEntry, PathProblem, Repository, FILE_MODE_NORMAL,
    MAX_FILES_PER_TREE, MAX_PROJECTION_SECONDS,
};
use crate::ops::types::{
    ConflictResolution, ReplaceContentEntry, FILE_MODE_TYPE_ENUM_VALUES,
    REPLACEMENT_TYPE_ENUM_VALUES,
};

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

/// How many paths one request's merge may compare in total.
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

/// How many bytes of file content one request's merge may load into three-way
/// merges in total.
///
/// The emulator's OWN bound, not an AWS limit, and the one that caps the PRODUCT
/// the paths cap alone leaves open: content is caller-written and a single file
/// may be megabytes, so without this a request could demand a line-level merge
/// of a tree's worth of large files. It is charged per merge over the ancestor
/// and both sides, so a file pair too large to merge within what is left is
/// refused instead of merged.
pub const MAX_CONTENT_BYTES_PER_REQUEST: usize = 8 * 1024 * 1024;

/// How many `conflictResolution` entries one merge request may carry before it
/// is refused with the modelled
/// `MaximumConflictResolutionEntriesExceededException`.
///
/// The emulator's OWN bound, not a published AWS figure: a resolution entry is
/// caller-supplied content applied to one path of the merge result, so this is
/// what bounds the work and the content one merge request can ask for. It is
/// well above any conflict set a merge of two trees bounded by
/// [`MAX_FILES_PER_TREE`] presents in practice.
pub const MAX_CONFLICT_RESOLUTION_ENTRIES: usize = 100;

/// How long one request's merge may take before it is refused.
/// The same figure, for the same reason, as the projection's own deadline: the
/// caps above bound the work and this bounds the time it may take on any
/// machine, so the store lock the merge runs under is never held for an
/// unbounded span. The merge also runs off the async worker — see
/// `crate::service`'s `off_the_async_worker`.
pub const MAX_MERGE_SECONDS: u64 = MAX_PROJECTION_SECONDS;

/// How many commits one request's merge may visit while resolving merge bases.
///
/// The emulator's OWN bound, not an AWS limit, charged per commit the history
/// walk reaches. Each commit is visited at most once per walk, and a repository
/// retains at most [`super::MAX_COMMITS_RETAINED`] commits (whose parent lists
/// can also name commits pruning has dropped), so this is well above what a
/// written history can present while still bounding the walk whatever produced
/// the stored state.
pub const MAX_COMMITS_WALKED_PER_REQUEST: usize = 16 * super::MAX_COMMITS_RETAINED;

/// Why a merge could not be computed: what the caller asked about, or what the
/// merge would have cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeProblem {
    /// The two tips share no commit, so there is no common ancestor to compare
    /// either side's changes against.
    NoMergeBase,
    /// More commits or paths than one request compares.
    ItemsToCompare,
    /// More file content than one request loads into three-way merges.
    ContentToLoad,
    /// Longer than one request spends merging.
    Deadline,
}

/// What ONE request's whole merge may still spend: commits walked, paths
/// compared, content bytes loaded and time.
///
/// One budget is built per request and threaded through every merge that request
/// drives — and through every phase of one, base resolution included — so
/// neither a long history nor a second question can multiply the work, exactly
/// as [`super::ProjectionBudget`] does for a push's projections. The deadline
/// itself is the crate's one [`super::Deadline`]; what differs from a
/// projection's budget is the refusal a merge answers with ([`MergeProblem`]).
#[derive(Debug)]
pub struct MergeBudget {
    commits_left: usize,
    items_left: usize,
    content_bytes_left: usize,
    deadline: super::Deadline,
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
            deadline: super::Deadline::in_seconds(MAX_MERGE_SECONDS),
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
    ///
    /// `pub(crate)` because the report a conflicting path's hunks are read out of
    /// ([`super::conflict_report::merge_hunks`]) loads the same three versions of
    /// a file, and one request's content allowance covers every byte it loads,
    /// whichever question asked for them.
    pub(crate) fn content(&mut self, bytes: usize) -> Result<(), MergeProblem> {
        self.content_bytes_left = self
            .content_bytes_left
            .checked_sub(bytes)
            .ok_or(MergeProblem::ContentToLoad)?;
        self.check_deadline()
    }

    fn check_deadline(&self) -> Result<(), MergeProblem> {
        if self.deadline.passed() {
            return Err(MergeProblem::Deadline);
        }
        Ok(())
    }
}

/// One file of a merged snapshot.
#[derive(Debug, Clone)]
pub struct MergedFile {
    /// The tree entry the merged snapshot holds for this path.
    pub entry: FileEntry,
    /// The bytes behind `entry.blob_id` when the merge PRODUCED them — a file
    /// merged line by line, or content a conflict resolution supplied — which a
    /// caller that stores the merge writes into the object store
    /// ([`Repository::insert_blob`]) before committing the snapshot. `None` when
    /// the entry names a blob the repository already holds.
    pub content: Option<Vec<u8>>,
}

impl MergedFile {
    /// A merged file whose blob the repository already holds.
    pub fn stored(entry: &FileEntry) -> Self {
        Self {
            entry: entry.clone(),
            content: None,
        }
    }

    /// A merged file the merge produced the bytes of.
    pub fn written(content: Vec<u8>, file_mode: &str) -> Self {
        let entry = FileEntry {
            blob_id: blob_id(&content),
            file_mode: file_mode.to_string(),
            size: i64::try_from(content.len()).unwrap_or(i64::MAX),
        };
        Self {
            entry,
            content: Some(content),
        }
    }

    /// The same merged file under `file_mode` — the mode the two sides' modes
    /// reconciled to, which is not always the mode of the side the CONTENT came
    /// from.
    fn under_mode(mut self, file_mode: &str) -> Self {
        self.entry.file_mode = file_mode.to_string();
        self
    }
}

/// What merging a source commit into a destination commit produces.
#[derive(Debug, Default)]
pub struct Merge {
    /// The merged snapshot: every path the merge settled, with the entry the
    /// result holds for it. A path the merge resolved to "deleted" is simply
    /// absent.
    pub files: BTreeMap<String, MergedFile>,
    /// Paths the two sides changed differently and the strategy did not settle.
    /// A caller may still settle each one with a `conflictResolution` entry;
    /// anything left is a merge that cannot be completed automatically.
    pub conflicts: BTreeSet<String>,
    /// Paths one side holds as a FILE while the other holds it as a FOLDER. A git
    /// tree cannot carry both under one name and no replacement content can make
    /// it carry both, so these are conflicts no strategy and no
    /// `conflictResolution` entry settles: the branches have to be merged
    /// manually.
    pub object_type_conflicts: BTreeSet<String>,
}

impl Merge {
    /// Every path still blocking this merge.
    pub fn unresolved(&self) -> BTreeSet<String> {
        self.conflicts
            .union(&self.object_type_conflicts)
            .cloned()
            .collect()
    }

    /// Record every path this result holds as a FILE while it also holds paths
    /// under it: a git tree holds one or the other under a name, never both.
    ///
    /// Recomputed from the result each time it is called, so the verdict is
    /// always about the CURRENT result: the merge itself calls it, and so does a
    /// caller that changed the result afterwards — a `conflictResolution` entry
    /// can create such a collision (a file written at a path the result holds as
    /// a folder) and can equally settle one (deleting either side of it). Only
    /// the deadline is charged, because the paths walked here were already
    /// charged as they were compared.
    ///
    /// The colliding entries are LEFT in [`Merge::files`]: a result with any
    /// unresolved path is refused ([`Merge::unresolved`]) rather than committed,
    /// and keeping them is what lets a later call see — and clear — the collision
    /// instead of silently dropping one side of it.
    pub fn detect_object_type_conflicts(
        &mut self,
        budget: &MergeBudget,
    ) -> Result<(), MergeProblem> {
        let files = &self.files;
        let conflicts = &mut self.conflicts;
        let object_type_conflicts = &mut self.object_type_conflicts;
        object_type_conflicts.clear();
        for path in files.keys() {
            budget.check_deadline()?;
            let folder = format!("{path}/");
            // The snapshot is SORTED by path, so the first entry at or after
            // `<path>/` either lives under that folder or nothing does: one range
            // lookup per path rather than a scan of every other path.
            if files
                .range(folder.clone()..)
                .next()
                .is_some_and(|(other, _)| other.starts_with(&folder))
            {
                // Defensive: a path this result HOLDS cannot also be in
                // `conflicts` as the merge records them (a conflicting path is
                // left out of the result), but a caller that settled a path and
                // then wrote a colliding entry could leave both, and a path must
                // never be reported in two conflict sets at once.
                conflicts.remove(path);
                object_type_conflicts.insert(path.clone());
            }
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
                    .map_or(0u64, |candidate| candidate.sequence),
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

/// The tree snapshot a commit id names, empty where the repository no longer
/// holds that commit's projection.
fn snapshot_of(repository: &Repository, commit_id: &str) -> BTreeMap<String, FileEntry> {
    repository
        .commits
        .get(commit_id)
        .map(|commit| commit.files.clone())
        .unwrap_or_default()
}

/// The tree snapshot of the merge base of two commits: the base of the three-way
/// merge.
///
/// `Err(MergeProblem::NoMergeBase)` where the two tips share no commit, exactly
/// as [`merge_base`] reports it — there is no common ancestor to compare either
/// side's changes against.
pub fn merge_base_files(
    repository: &Repository,
    source_commit_id: &str,
    destination_commit_id: &str,
    budget: &mut MergeBudget,
) -> Result<BTreeMap<String, FileEntry>, MergeProblem> {
    let base = merge_base(repository, source_commit_id, destination_commit_id, budget)?;
    Ok(snapshot_of(repository, &base))
}

/// How two tips are related by ancestry — the one question a fast-forward asks.
///
/// A fast-forward merge moves a reference rather than merging content, so what
/// decides it is which of the two tips the merge base IS, and nothing about the
/// paths either side changed. Only [`FastForward::Advances`] is a merge this
/// strategy can make: see [`FastForward::advances`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastForward {
    /// The destination tip is the merge base, so the destination's history is a
    /// prefix of the source's and the reference can simply advance to the source.
    /// The same condition [`MergeEvaluation::is_fast_forward`] reports.
    Advances,
    /// The source tip is the merge base, so its history is already part of the
    /// destination's and the destination holds commits the source does not: the
    /// reference would have to move BACKWARDS onto an ancestor to take the
    /// source tip, which is not an advance, so this strategy cannot make the
    /// merge.
    AlreadyMerged,
    /// Neither tip is the other's ancestor: each history holds commits the other
    /// does not, so honouring both needs a merge commit, which no reference move
    /// can make.
    Diverged,
}

impl FastForward {
    /// Can this merge be made by advancing the destination reference, which is
    /// the only merge a fast-forward strategy performs?
    ///
    /// True only where the destination tip IS the merge base — the same reading
    /// of the relation [`MergeEvaluation::is_fast_forward`] reports, so what
    /// `GetMergeOptions` offers as a strategy and what a fast-forward merge
    /// performs cannot disagree. Two tips that are the same commit advance by
    /// nothing and are still an advance.
    pub fn advances(self) -> bool {
        matches!(self, FastForward::Advances)
    }
}

/// How the two tips are related by ancestry in this repository, resolving the
/// merge base alone.
///
/// Every commit the history walk visits is charged to `budget`, exactly as a
/// path-comparing evaluation charges its own work. Two tips that are the SAME
/// commit are [`FastForward::Advances`], where advancing has nothing to do.
pub fn fast_forward(
    repository: &Repository,
    source_commit_id: &str,
    destination_commit_id: &str,
    budget: &mut MergeBudget,
) -> Result<FastForward, MergeProblem> {
    let base = merge_base(repository, source_commit_id, destination_commit_id, budget)?;
    if base == destination_commit_id {
        return Ok(FastForward::Advances);
    }
    if base == source_commit_id {
        return Ok(FastForward::AlreadyMerged);
    }
    Ok(FastForward::Diverged)
}

/// Evaluate a merge of `source` into `destination` in this repository at the
/// given conflict detail level and resolution strategy, charging the work to
/// `budget`.
///
/// The merge itself is [`merge_snapshots`] — the one reading of a CodeCommit
/// merge this crate has — so what this reports as conflicting is exactly what a
/// merge that WRITES its result would be unable to settle.
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
    let base_commit_id = merge_base(repository, &source.commit_id, &destination.commit_id, budget)?;
    let merged = merge_snapshots(
        repository,
        &snapshot_of(repository, &base_commit_id),
        &source.files,
        &destination.files,
        conflict_detail_level,
        conflict_resolution_strategy,
        budget,
    )?;
    Ok(MergeEvaluation {
        source_commit_id: source.commit_id.clone(),
        destination_commit_id: destination.commit_id.clone(),
        base_commit_id,
        conflicts: merged.unresolved().into_iter().collect(),
    })
}

/// Merge `source` into `destination` against the merge base snapshot `base`,
/// path by path.
///
/// `Err` is the allowance the merge ran into: more paths than
/// [`MAX_ITEMS_TO_COMPARE_PER_REQUEST`] (reported as
/// `MaximumItemsToCompareExceededException`), more content than
/// [`MAX_CONTENT_BYTES_PER_REQUEST`], or `budget`'s deadline, charged as the
/// paths are walked so the content merge of a pathological file cannot hold the
/// store lock for an unbounded span.
pub fn merge_snapshots(
    repository: &Repository,
    base: &BTreeMap<String, FileEntry>,
    source: &BTreeMap<String, FileEntry>,
    destination: &BTreeMap<String, FileEntry>,
    detail_level: &str,
    strategy: &str,
    budget: &mut MergeBudget,
) -> Result<Merge, MergeProblem> {
    let paths: BTreeSet<&str> = base
        .keys()
        .chain(source.keys())
        .chain(destination.keys())
        .map(String::as_str)
        .collect();

    let mut merge = Merge::default();
    for path in paths {
        budget.item()?;
        match resolve_path(
            repository,
            base.get(path),
            source.get(path),
            destination.get(path),
            detail_level,
            strategy,
            budget,
        )? {
            Resolved::File(file) => {
                merge.files.insert(path.to_string(), file);
            }
            // Neither side's version survives the merge, so the result simply
            // does not hold the path.
            Resolved::Deleted => {}
            Resolved::Conflict => {
                merge.conflicts.insert(path.to_string());
            }
        }
    }

    // A path the result holds as a file while it also holds paths UNDER it is a
    // file in one side and a folder in the other: a git tree holds one or the
    // other under a name, never both.
    merge.detect_object_type_conflicts(budget)?;

    Ok(merge)
}

/// What the merge holds for ONE path.
enum Resolved {
    /// The version the merged snapshot holds.
    File(MergedFile),
    /// The merge resolved the path to "deleted": the result holds no version.
    Deleted,
    /// The two sides were changed differently and neither the detail level nor
    /// the resolution strategy settled them.
    Conflict,
}

/// A side's version of one path, as the merged result would hold it.
fn taken(entry: Option<&FileEntry>) -> Resolved {
    match entry {
        Some(entry) => Resolved::File(MergedFile::stored(entry)),
        None => Resolved::Deleted,
    }
}

/// Merge the two sides of ONE path.
///
/// A path is a conflict only where BOTH sides moved it away from the merge base
/// and into different states: a path only one side touched carries that side's
/// change, and a path both sides changed the same way is already agreed. A
/// file's mode and its content are changed independently, so they are merged
/// independently: a mode only one side moved and content only the other side
/// moved reconcile into one file. Of what remains, a resolution strategy that
/// names a side takes that side whole, and a line-level merge (or an automerge,
/// which attempts "to automatically merge two versions of a file") reconciles
/// content whose two sides changed different lines. A path one side DELETED and
/// the other changed has no two versions to reconcile at all, so it needs a
/// decision.
fn resolve_path(
    repository: &Repository,
    base: Option<&FileEntry>,
    source: Option<&FileEntry>,
    destination: Option<&FileEntry>,
    detail_level: &str,
    strategy: &str,
    budget: &mut MergeBudget,
) -> Result<Resolved, MergeProblem> {
    if same_entry(source, destination) || same_entry(destination, base) {
        // Both sides agree about the path (including both having deleted it), or
        // only the source changed it.
        return Ok(taken(source));
    }
    if same_entry(source, base) {
        // Only the destination changed it.
        return Ok(taken(destination));
    }
    // Both sides changed it, differently.
    match strategy {
        // One side is taken whole, so nothing is left to resolve.
        CONFLICT_RESOLUTION_ACCEPT_SOURCE => return Ok(taken(source)),
        CONFLICT_RESOLUTION_ACCEPT_DESTINATION => return Ok(taken(destination)),
        _ => {}
    }
    // One side no longer holds a path the other changed: the side that deleted it
    // has no content or mode left to carry forward, so the two sides cannot both
    // be honoured whatever the detail level says about lines.
    let (Some(source), Some(destination)) = (source, destination) else {
        return Ok(Resolved::Conflict);
    };

    // The mode, which is a facet of its own: a mode both sides moved, and moved
    // differently, is a conflict, while a mode only one side moved away from the
    // base is that side's change.
    let Some(file_mode) = merged_file_mode(base, source, destination) else {
        return Ok(Resolved::Conflict);
    };

    // Then the content, the other facet.
    if source.blob_id == destination.blob_id {
        return Ok(Resolved::File(
            MergedFile::stored(destination).under_mode(&file_mode),
        ));
    }
    if base.is_some_and(|base| base.blob_id == source.blob_id) {
        return Ok(Resolved::File(
            MergedFile::stored(destination).under_mode(&file_mode),
        ));
    }
    if base.is_some_and(|base| base.blob_id == destination.blob_id) {
        return Ok(Resolved::File(
            MergedFile::stored(source).under_mode(&file_mode),
        ));
    }
    // Content both sides changed, and changed differently. "If LINE_LEVEL is
    // specified, a conflict is considered not mergeable if the same file in both
    // branches has differences on the same line", and an automerge attempts the
    // same reconciliation whatever detail level was asked for; under FILE_LEVEL
    // detail with any other strategy a file differing in both branches is simply
    // not mergeable.
    if detail_level != CONFLICT_DETAIL_LINE_LEVEL && strategy != CONFLICT_RESOLUTION_AUTOMERGE {
        return Ok(Resolved::Conflict);
    }
    Ok(
        match merged_content(repository, base, source, destination, budget)? {
            Some(content) => Resolved::File(MergedFile::written(content, &file_mode)),
            None => Resolved::Conflict,
        },
    )
}

/// The mode a merged file takes, or `None` where the two sides moved it
/// differently.
fn merged_file_mode(
    base: Option<&FileEntry>,
    source: &FileEntry,
    destination: &FileEntry,
) -> Option<String> {
    if source.file_mode == destination.file_mode {
        return Some(source.file_mode.clone());
    }
    let base = base?;
    if base.file_mode == source.file_mode {
        // Only the destination moved the mode.
        return Some(destination.file_mode.clone());
    }
    if base.file_mode == destination.file_mode {
        return Some(source.file_mode.clone());
    }
    None
}

/// Are two sides of a path in the same state? Two absences are, since neither
/// side holds the path at all.
///
/// `pub(crate)` because the handlers that REPORT on a path the evaluation named
/// (`crate::ops::get_merge_conflicts`) ask the same question of the same two
/// sides, and two readings of "the same thing here" must not drift.
pub(crate) fn same_entry(left: Option<&FileEntry>, right: Option<&FileEntry>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.blob_id == right.blob_id && left.file_mode == right.file_mode
        }
        _ => false,
    }
}

/// The three-way content merge of one path, or `None` where the two sides cannot
/// be reconciled without a decision from the caller.
///
/// A path neither side's history held before carries no common ancestor content,
/// so its two sides are merged against empty content — which reconciles only
/// where each side adds different lines in different places. Content this
/// repository does not hold the bytes of cannot be merged at all, which is a
/// conflict rather than a merge.
///
/// The bytes of all three versions are charged to `budget` BEFORE the merge
/// runs, and no single version past [`MAX_MERGE_FILE_BYTES`] or
/// [`MAX_MERGE_FILE_LINES`] is merged at all, so both the content one request
/// loads in total and the cost of each individual merge are capped.
fn merged_content(
    repository: &Repository,
    base: Option<&FileEntry>,
    source: &FileEntry,
    destination: &FileEntry,
    budget: &mut MergeBudget,
) -> Result<Option<Vec<u8>>, MergeProblem> {
    let ancestor: &[u8] = match base {
        None => &[],
        Some(base) => match repository.blob(&base.blob_id) {
            Some(bytes) => bytes,
            None => return Ok(None),
        },
    };
    let (Some(ours), Some(theirs)) = (
        repository.blob(&destination.blob_id),
        repository.blob(&source.blob_id),
    ) else {
        return Ok(None);
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
    // The destination is the side being merged INTO, so it is "ours"; the source
    // is "theirs". `Err` is the conflicted rendering, which means the two sides
    // changed the same lines.
    Ok(diffy::merge_bytes(ancestor, ours, theirs).ok())
}

/// The file mode a resolved path takes when the caller named none: the
/// destination's mode wherever the destination holds the path (which is also the
/// agreed mode when the two sides agree), the source's where only the source
/// holds it, and `NORMAL` where neither does.
pub fn resolved_file_mode(
    in_source: Option<&FileEntry>,
    in_destination: Option<&FileEntry>,
) -> String {
    in_destination
        .or(in_source)
        .map_or_else(|| FILE_MODE_NORMAL.to_string(), |entry| entry.file_mode.clone())
}

/// The folder of a merged tree this emulator would not write, with the bytes it
/// holds: the first folder the merge CHANGES whose immediate entries exceed
/// CodeCommit's documented 6 MB folder limit
/// ([`super::MAX_FOLDER_CONTENT_BYTES`]), reported for the modelled
/// `FolderContentSizeLimitExceededException`.
///
/// Charged per FOLDER of the resulting tree, not per changed path: a folder is
/// over the limit whatever combination of paths took it there, so a folder whose
/// growth comes from paths that are each unchanged relative to the destination is
/// charged too. Only a folder the merge CHANGES is charged, because a folder whose
/// immediate entries the destination already held exactly is one the merge commit
/// writes nothing into.
pub fn oversized_changed_folder<'tree>(
    merged: &'tree BTreeMap<String, FileEntry>,
    destination: &BTreeMap<String, FileEntry>,
) -> Option<(&'tree str, i64)> {
    /// One folder's immediate entries, as what identifies them in a git tree.
    fn entries_by_folder(
        tree: &BTreeMap<String, FileEntry>,
    ) -> BTreeMap<&str, Vec<(&str, &str, &str)>> {
        let mut folders: BTreeMap<&str, Vec<(&str, &str, &str)>> = BTreeMap::new();
        for (path, entry) in tree {
            folders
                .entry(super::folder_of(path))
                .or_default()
                .push((
                    path.as_str(),
                    entry.blob_id.as_str(),
                    entry.file_mode.as_str(),
                ));
        }
        folders
    }
    let merged_entries = entries_by_folder(merged);
    let destination_entries = entries_by_folder(destination);
    super::folder_content_bytes_by_folder(merged)
        .into_iter()
        .filter(|(folder, _)| destination_entries.get(folder) != merged_entries.get(folder))
        .find(|(_, bytes)| *bytes > super::MAX_FOLDER_CONTENT_BYTES)
}

/// `replacementType` values the model's `ReplacementTypeEnum` declares
/// (`smithy_query(query="shape",shape="ReplacementTypeEnum")`): which version of a
/// conflicting file a resolution entry keeps, or that it supplies new content.
pub const REPLACEMENT_KEEP_BASE: &str = "KEEP_BASE";
pub const REPLACEMENT_KEEP_SOURCE: &str = "KEEP_SOURCE";
pub const REPLACEMENT_KEEP_DESTINATION: &str = "KEEP_DESTINATION";
pub const REPLACEMENT_USE_NEW_CONTENT: &str = "USE_NEW_CONTENT";

/// Why a `conflictResolution` cannot be honoured, read once here for every
/// operation that accepts one.
///
/// Each of those operations models its OWN errors for these — they are not the
/// same set for all of them — so this names what is wrong with the resolution and
/// the operation chooses the modelled failure and its sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionProblem {
    /// An entry named no path at all.
    PathRequired,
    /// An entry named a path no stored tree addresses.
    InvalidPath(String),
    /// An entry named an empty file mode.
    FileModeRequired,
    /// An entry named a file mode `FileModeTypeEnum` does not declare.
    InvalidFileMode(String),
    /// More entries than [`MAX_CONFLICT_RESOLUTION_ENTRIES`], carrying the count
    /// the request held.
    TooManyEntries(usize),
    /// Two entries addressed one path: "A conflict can have only one conflict
    /// resolution entry."
    MultipleEntries(String),
    /// A content replacement for this path carried no replacement type.
    ReplacementTypeRequired(String),
    /// A content replacement carried a type `ReplacementTypeEnum` does not
    /// declare.
    InvalidReplacementType(String),
    /// `USE_NEW_CONTENT` was named for this path with no content to use.
    ReplacementContentRequired(String),
    /// Content was supplied for this path under a replacement type that keeps a
    /// stored version instead.
    InvalidReplacementContent {
        path: String,
        replacement_type: String,
    },
    /// A file mode was set for a path the resolved merge result does not hold.
    PathNotInResult(String),
}

/// Is this request asking for a conflict resolution its strategy cannot honour?
///
/// A resolution is "a list of inputs to use when resolving conflicts during a
/// merge" only under `AUTOMERGE`: under any other strategy the version each
/// conflicting path takes is already decided, so a list supplied with one cannot
/// be honoured and the request is refused rather than silently ignored. The
/// combination is what fails, so it is judged BEFORE the entries themselves and
/// whatever one of them says — an entry of a list the merge will not read is
/// never examined.
pub fn resolution_cannot_be_honoured(
    strategy: &str,
    resolution: Option<&ConflictResolution>,
) -> bool {
    resolution_supplied(resolution) && strategy != CONFLICT_RESOLUTION_AUTOMERGE
}

/// `InvalidConflictResolutionException`'s message for a `conflictResolution` the
/// request's strategy cannot honour: the sentence CodeCommit answers that
/// combination with, naming the strategy the merge is being evaluated with — the
/// one the request asked for, or `NONE` where it asked for none.
///
/// The one place this crate words that combination, so every operation that
/// takes a `conflictResolution` states it identically, and the one the
/// pre-dispatch layer
/// ([`crate::service::conflict_resolution`]) states it with for a request whose
/// entries the model's own constraints would otherwise refuse first.
pub fn invalid_conflict_resolution_message(strategy: &str) -> String {
    format!(
        "Conflict resolution parameter cannot be used together with {strategy} conflict \
         resolution strategy"
    )
}

/// Does this `conflictResolution` ask for anything at all? An absent block, and
/// one whose three lists are all absent or empty, ask for nothing.
pub fn resolution_supplied(resolution: Option<&ConflictResolution>) -> bool {
    resolution.is_some_and(|resolution| {
        [
            resolution.replace_contents.as_ref().map(Vec::len),
            resolution.delete_files.as_ref().map(Vec::len),
            resolution.set_file_modes.as_ref().map(Vec::len),
        ]
        .iter()
        .any(|entries| entries.is_some_and(|count| count > 0))
    })
}

/// A caller's `conflictResolution`, checked once and keyed by the path each entry
/// addresses — "a conflict can have only one conflict resolution entry", so a
/// path appears at most once across the content replacements, the deletions and
/// the mode changes.
#[derive(Debug, Default)]
pub struct CheckedResolution {
    /// The version (or the new content) each named path takes.
    pub replacements: BTreeMap<String, ReplaceContentEntry>,
    /// The paths the resolution removes from the merge result.
    pub deletes: BTreeSet<String>,
    /// The file mode each named path takes.
    pub modes: BTreeMap<String, String>,
}

/// The canonical form of one `conflictResolution` path. Every path an entry names
/// is canonicalized the way a stored tree addresses its files, so an entry
/// addresses the merge result rather than a spelling of it.
fn resolution_path(raw: &str) -> Result<String, ResolutionProblem> {
    match normalize_path(raw) {
        Ok(path) => Ok(path),
        Err(PathProblem::Missing) => Err(ResolutionProblem::PathRequired),
        Err(PathProblem::Invalid) => Err(ResolutionProblem::InvalidPath(raw.to_string())),
    }
}

/// A `fileMode` this service stores. A value outside `FileModeTypeEnum` is
/// refused at the request boundary where the member is modelled as that enum;
/// this is the same check for a mode that reached a handler.
fn resolution_file_mode(mode: &str) -> Result<String, ResolutionProblem> {
    if mode.is_empty() {
        return Err(ResolutionProblem::FileModeRequired);
    }
    if !FILE_MODE_TYPE_ENUM_VALUES.contains(&mode) {
        return Err(ResolutionProblem::InvalidFileMode(mode.to_string()));
    }
    Ok(mode.to_string())
}

impl CheckedResolution {
    /// The caller's `conflictResolution` with every entry checked, or the first
    /// entry that is not usable.
    pub fn check(resolution: Option<&ConflictResolution>) -> Result<Self, ResolutionProblem> {
        let mut checked = Self::default();
        let Some(resolution) = resolution else {
            return Ok(checked);
        };
        let replacements = resolution.replace_contents.as_deref().unwrap_or_default();
        let deletions = resolution.delete_files.as_deref().unwrap_or_default();
        let mode_changes = resolution.set_file_modes.as_deref().unwrap_or_default();

        let entries = replacements
            .len()
            .saturating_add(deletions.len())
            .saturating_add(mode_changes.len());
        if entries > MAX_CONFLICT_RESOLUTION_ENTRIES {
            return Err(ResolutionProblem::TooManyEntries(entries));
        }

        for entry in replacements {
            let path = resolution_path(&entry.file_path)?;
            let replacement_type = entry.replacement_type.as_str();
            if replacement_type.is_empty() {
                return Err(ResolutionProblem::ReplacementTypeRequired(path));
            }
            if !REPLACEMENT_TYPE_ENUM_VALUES.contains(&replacement_type) {
                return Err(ResolutionProblem::InvalidReplacementType(
                    replacement_type.to_string(),
                ));
            }
            // Content belongs to USE_NEW_CONTENT and to nothing else: "The
            // base-64 encoded content to use when the replacement type is
            // USE_NEW_CONTENT."
            if replacement_type == REPLACEMENT_USE_NEW_CONTENT && entry.content.is_none() {
                return Err(ResolutionProblem::ReplacementContentRequired(path));
            }
            if replacement_type != REPLACEMENT_USE_NEW_CONTENT && entry.content.is_some() {
                return Err(ResolutionProblem::InvalidReplacementContent {
                    path,
                    replacement_type: replacement_type.to_string(),
                });
            }
            let mut stored = entry.clone();
            stored.file_path = path.clone();
            stored.file_mode = match &entry.file_mode {
                Some(mode) => Some(resolution_file_mode(mode)?),
                None => None,
            };
            if checked.deletes.contains(&path)
                || checked.replacements.insert(path.clone(), stored).is_some()
            {
                return Err(ResolutionProblem::MultipleEntries(path));
            }
        }

        for entry in deletions {
            let path = resolution_path(&entry.file_path)?;
            if checked.replacements.contains_key(&path) || !checked.deletes.insert(path.clone()) {
                return Err(ResolutionProblem::MultipleEntries(path));
            }
        }

        for entry in mode_changes {
            let path = resolution_path(&entry.file_path)?;
            let mode = resolution_file_mode(&entry.file_mode)?;
            if checked.modes.insert(path.clone(), mode).is_some() {
                return Err(ResolutionProblem::MultipleEntries(path));
            }
        }

        Ok(checked)
    }

    /// Apply this resolution to `merged`, the result of merging `source` into
    /// `destination` against `base`.
    ///
    /// An entry is applied to the path it names whether or not the merge found
    /// that path conflicting: `conflictResolution` names the content the merge
    /// RESULT holds for a path, and the conflict set a merge computed is not
    /// reported by the operations that WRITE a merge, so a caller cannot know
    /// which paths this service found conflicting and an entry for a settled path
    /// is honoured rather than refused.
    ///
    /// The file-against-folder verdict is NOT recomputed here: a resolution can
    /// create such a collision and can equally settle one, so the caller runs
    /// [`Merge::detect_object_type_conflicts`] over the resolved result, which is
    /// where that walk's budget is charged.
    pub fn apply(
        &self,
        merged: &mut Merge,
        base: &BTreeMap<String, FileEntry>,
        source: &BTreeMap<String, FileEntry>,
        destination: &BTreeMap<String, FileEntry>,
    ) -> Result<(), ResolutionProblem> {
        for (path, entry) in &self.replacements {
            let replacement_type = entry.replacement_type.as_str();
            let resolved = if replacement_type == REPLACEMENT_USE_NEW_CONTENT {
                let content = entry.content.clone().unwrap_or_default();
                let file_mode = entry.file_mode.clone().unwrap_or_else(|| {
                    resolved_file_mode(source.get(path), destination.get(path))
                });
                Some(MergedFile::written(content, &file_mode))
            } else {
                let kept = match replacement_type {
                    REPLACEMENT_KEEP_BASE => base.get(path),
                    REPLACEMENT_KEEP_SOURCE => source.get(path),
                    REPLACEMENT_KEEP_DESTINATION => destination.get(path),
                    // No other value reaches here: the type was checked against
                    // `ReplacementTypeEnum` by [`CheckedResolution::check`].
                    _ => None,
                };
                kept.map(|in_tree| {
                    let mut file = MergedFile::stored(in_tree);
                    // "The file mode to apply during conflict resolution."
                    if let Some(mode) = &entry.file_mode {
                        file.entry.file_mode = mode.clone();
                    }
                    file
                })
            };
            match resolved {
                Some(file) => {
                    merged.files.insert(path.clone(), file);
                }
                // The version this entry keeps does not hold the path, so neither
                // does the resolved merge.
                None => {
                    merged.files.remove(path);
                }
            }
            merged.conflicts.remove(path);
        }
        for path in &self.deletes {
            merged.files.remove(path);
            merged.conflicts.remove(path);
        }
        for (path, mode) in &self.modes {
            match merged.files.get_mut(path) {
                Some(file) => file.entry.file_mode = mode.clone(),
                None => return Err(ResolutionProblem::PathNotInResult(path.clone())),
            }
        }
        Ok(())
    }
}
