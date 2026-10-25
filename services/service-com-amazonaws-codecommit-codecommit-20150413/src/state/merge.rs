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
    blob_id, Commit, FileEntry, Repository, FILE_MODE_NORMAL, MAX_FILES_PER_TREE,
    MAX_PROJECTION_SECONDS,
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
    fn content(&mut self, bytes: usize) -> Result<(), MergeProblem> {
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
/// paths either side changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastForward {
    /// The destination tip is the merge base, so the destination's history is a
    /// prefix of the source's and the reference can simply advance to the source.
    /// The same condition [`MergeEvaluation::is_fast_forward`] reports.
    Advances,
    /// The source tip is the merge base, so its history is already part of the
    /// destination's: there is nothing left to advance, and the destination tip
    /// stands as the merged commit.
    AlreadyMerged,
    /// Neither tip is the other's ancestor: each history holds commits the other
    /// does not, so honouring both needs a merge commit, which no reference move
    /// can make.
    Diverged,
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
fn same_entry(left: Option<&FileEntry>, right: Option<&FileEntry>) -> bool {
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
