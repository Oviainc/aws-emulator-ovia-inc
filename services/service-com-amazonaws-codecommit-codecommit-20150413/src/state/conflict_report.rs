// SPDX-License-Identifier: CC0-1.0

//! What the `*MergeConflicts` operations REPORT about one path a merge could not
//! reconcile: the three sides of the path and which of its facets they disagree
//! about (`ConflictMetadata`), and the regions of the path's two text versions
//! that each side moved away from the merge base (`MergeHunk`).
//!
//! WHICH paths a merge cannot reconcile is [`super::merge::evaluate`] — the one
//! merge evaluation every merge question in this service shares. What lives HERE
//! is the description of a path that evaluation already named, computed once for
//! every operation that reports it (`GetMergeConflicts`'s
//! `conflictMetadataList`, `BatchDescribeMergeConflicts`'s `conflicts`), so no
//! two of them can describe the same path differently.
//!
//! Nothing in this module writes: describing a merge leaves the repository
//! exactly as it was, and every figure it reports is read out of the stored
//! snapshots and object bytes of one repository.

use std::collections::BTreeMap;
use std::ops::Range;

use super::merge::{
    MergeBudget, MergeProblem, CONFLICT_DETAIL_LINE_LEVEL, MAX_MERGE_FILE_BYTES,
    MAX_MERGE_FILE_LINES,
};
use super::{FileEntry, Repository, FILE_MODE_SYMLINK};
use crate::ops::types::{
    ConflictMetadata, FileModes, FileSizes, IsBinaryFile, MergeHunk, MergeHunkDetail,
    MergeOperations, ObjectTypes,
};

/// `objectType` values the model's `ObjectTypeEnum` declares, catalogued by the
/// generated `OBJECT_TYPE_ENUM_VALUES`. A commit's tree in this store holds
/// files and the folders their paths imply, so `GIT_LINK` — a submodule — is a
/// type no stored tree reports.
pub const OBJECT_TYPE_FILE: &str = "FILE";
pub const OBJECT_TYPE_DIRECTORY: &str = "DIRECTORY";
pub const OBJECT_TYPE_SYMBOLIC_LINK: &str = "SYMBOLIC_LINK";

/// `changeType` values the model's `ChangeTypeEnum` declares, catalogued by the
/// generated `CHANGE_TYPE_ENUM_VALUES`: what one side of a merge did to a path —
/// added it, modified it, deleted it.
pub const CHANGE_TYPE_ADDED: &str = "A";
pub const CHANGE_TYPE_MODIFIED: &str = "M";
pub const CHANGE_TYPE_DELETED: &str = "D";

/// How many of a blob's leading bytes are examined before it is called binary:
/// git's own test, which looks for a NUL byte in the first 8000 bytes of a file.
const BINARY_SNIFF_BYTES: usize = 8_000;

/// Is this blob binary rather than text? A NUL byte in the leading
/// [`BINARY_SNIFF_BYTES`] is git's own test, and content whose leading bytes are
/// not valid UTF-8 is binary here too, because its lines cannot be compared at
/// all. Only that prefix is examined, whatever the file's size, so the test costs
/// the same for every blob.
pub fn is_binary(content: &[u8]) -> bool {
    let sniffed = &content[..content.len().min(BINARY_SNIFF_BYTES)];
    if sniffed.contains(&0) {
        return true;
    }
    match ::std::str::from_utf8(sniffed) {
        Ok(_) => false,
        // An error with no length is the prefix ending in the middle of a
        // character — the cut, not the content, so the content is still text.
        Err(error) => error.error_len().is_some(),
    }
}

/// What one commit's tree holds at one path: the file entry, if any, and whether
/// the tree holds entries BELOW the path, which is what makes that path a
/// directory in that commit.
#[derive(Debug, Clone, Copy)]
pub struct TreeSlot<'tree> {
    pub entry: Option<&'tree FileEntry>,
    pub directory: bool,
}

impl<'tree> TreeSlot<'tree> {
    /// Read what `files` holds at `path`.
    ///
    /// The directory test walks the keys just above `path` for as long as they
    /// still begin with it — those are the only keys that could sit BELOW it —
    /// and asks whether one of them continues with a separator. No prefix string
    /// is built, because this runs three times (base, source, destination) for
    /// every path a merge compares.
    pub fn at(files: &'tree BTreeMap<String, FileEntry>, path: &str) -> Self {
        let directory = files
            .range::<str, _>((::std::ops::Bound::Excluded(path), ::std::ops::Bound::Unbounded))
            .take_while(|(other, _)| other.starts_with(path))
            .any(|(other, _)| other.as_bytes().get(path.len()) == Some(&b'/'));
        Self {
            entry: files.get(path),
            directory,
        }
    }

    /// The modeled `ObjectTypeEnum` value for what sits here, or `None` where
    /// this commit holds nothing at the path at all.
    pub fn object_type(&self) -> Option<&'static str> {
        match self.entry {
            Some(entry) if entry.file_mode == FILE_MODE_SYMLINK => Some(OBJECT_TYPE_SYMBOLIC_LINK),
            Some(_) => Some(OBJECT_TYPE_FILE),
            None => self.directory.then_some(OBJECT_TYPE_DIRECTORY),
        }
    }

    /// Do two commits hold the very same thing here? The file entries are
    /// compared by [`super::merge::same_entry`], the same reading the shared
    /// merge evaluation decides "unchanged" by, plus whether the path is a
    /// directory in both.
    pub fn same_as(&self, other: &Self) -> bool {
        self.directory == other.directory && super::merge::same_entry(self.entry, other.entry)
    }

    /// The `ChangeTypeEnum` value for how this slot differs from the merge
    /// base's, or `None` where this side left the path exactly as the base had
    /// it.
    pub fn change_from(&self, base: &Self) -> Option<&'static str> {
        if self.same_as(base) {
            return None;
        }
        match (base.object_type().is_some(), self.object_type().is_some()) {
            (false, true) => Some(CHANGE_TYPE_ADDED),
            (true, false) => Some(CHANGE_TYPE_DELETED),
            _ => Some(CHANGE_TYPE_MODIFIED),
        }
    }

    /// The size this side records for the file, and `0` where it holds none —
    /// the default the model gives every `FileSizes` member.
    pub fn size(&self) -> i64 {
        self.entry.map_or(0, |entry| entry.size)
    }

    /// The modeled `fileMode` of the file this side holds, absent where it holds
    /// none.
    pub fn file_mode(&self) -> Option<String> {
        self.entry.map(|entry| entry.file_mode.clone())
    }

    /// The stored bytes of the TEXT file this side holds at the path: absent
    /// where the side holds no file, where the repository no longer holds the
    /// blob, and where the content is binary — content whose lines cannot be
    /// compared at all ([`is_binary`]).
    fn text<'blob>(&self, repository: &'blob Repository) -> Option<&'blob [u8]> {
        self.entry
            .and_then(|entry| repository.blob(&entry.blob_id))
            .filter(|content| !is_binary(content))
    }
}

/// Do the two sides disagree about the CONTENT of this path?
///
/// A facet is a disagreement only where both sides moved it away from the base
/// AND moved it differently; a facet only one side moved is that side's change
/// and reconciles, which is the same reading [`super::merge`] decides a conflict
/// by. Content is also a disagreement where one side no longer holds the path at
/// all, because the other side's change cannot be carried into a file that is
/// gone.
fn content_conflict(base: &TreeSlot<'_>, source: &TreeSlot<'_>, destination: &TreeSlot<'_>) -> bool {
    match (source.entry, destination.entry) {
        (Some(mine), Some(theirs)) => {
            mine.blob_id != theirs.blob_id
                && base.entry.is_none_or(|base| {
                    base.blob_id != mine.blob_id && base.blob_id != theirs.blob_id
                })
        }
        // One side no longer holds a file where the other does: that side has no
        // content left to carry the other side's change into.
        (Some(_), None) | (None, Some(_)) => true,
        // Neither side holds a file at the path — whatever these two sides
        // disagree about here, it is not the file's content.
        (None, None) => false,
    }
}

/// The metadata the `*MergeConflicts` operations report for ONE path a merge
/// evaluation could not reconcile: which facet of the path the two sides
/// disagree about, and what each of the three sides holds there.
///
/// WHETHER a path conflicts is [`super::merge::evaluate`]'s answer, shared with
/// every other merge question this service answers; this function only describes
/// a path that evaluation already named, by reading the three tree snapshots and
/// the stored blobs of the sides that hold a file.
///
/// At `LINE_LEVEL` a path is reported only when its lines could not be merged,
/// so a content difference on a reported path is a content conflict there too.
pub fn conflict_metadata(
    repository: &Repository,
    path: &str,
    base: TreeSlot<'_>,
    source: TreeSlot<'_>,
    destination: TreeSlot<'_>,
) -> ConflictMetadata {
    let content_of =
        |slot: &TreeSlot<'_>| slot.entry.and_then(|entry| repository.blob(&entry.blob_id));

    let object_type_conflict = match (source.object_type(), destination.object_type()) {
        (Some(mine), Some(theirs)) => {
            mine != theirs
                && base
                    .object_type()
                    .is_none_or(|base| base != mine && base != theirs)
        }
        _ => false,
    };
    let file_mode_conflict = match (source.entry, destination.entry) {
        (Some(mine), Some(theirs)) => {
            mine.file_mode != theirs.file_mode
                && base.entry.is_none_or(|base| {
                    base.file_mode != mine.file_mode && base.file_mode != theirs.file_mode
                })
        }
        _ => false,
    };
    let content_conflict = content_conflict(&base, &source, &destination);

    // "The number of conflicts, including both hunk conflicts and metadata
    // conflicts": the facets above that disagree, and never fewer than the one
    // disagreement that put this path in the list.
    let counted = usize::from(content_conflict)
        + usize::from(file_mode_conflict)
        + usize::from(object_type_conflict);
    let number_of_conflicts = i32::try_from(counted.max(1)).unwrap_or(i32::MAX);

    ConflictMetadata {
        file_path: Some(path.to_string()),
        file_sizes: Some(FileSizes {
            source: source.size(),
            destination: destination.size(),
            base: base.size(),
        }),
        file_modes: Some(FileModes {
            source: source.file_mode(),
            destination: destination.file_mode(),
            base: base.file_mode(),
        }),
        object_types: Some(ObjectTypes {
            source: source.object_type().map(str::to_string),
            destination: destination.object_type().map(str::to_string),
            base: base.object_type().map(str::to_string),
        }),
        number_of_conflicts,
        // A sniff of each side's leading bytes (see [`is_binary`]), so the test
        // costs the same whatever the file's size.
        is_binary_file: Some(IsBinaryFile {
            source: content_of(&source).map(is_binary),
            destination: content_of(&destination).map(is_binary),
            base: content_of(&base).map(is_binary),
        }),
        content_conflict,
        file_mode_conflict,
        object_type_conflict,
        merge_operations: Some(MergeOperations {
            source: source.change_from(&base).map(str::to_string),
            destination: destination.change_from(&base).map(str::to_string),
        }),
    }
}

/// One run of lines one side changed away from the merge base: the lines the
/// base holds there, and the lines this side holds instead.
#[derive(Debug, Clone)]
struct Edit {
    base: Range<usize>,
    side: Range<usize>,
}

/// How many lines this side's run holds beyond the base's, as a signed figure:
/// what the side's line numbering has drifted by once the run is passed.
fn drift(edits: &[Edit]) -> isize {
    edits
        .iter()
        .map(|edit| {
            let side = isize::try_from(edit.side.len()).unwrap_or(isize::MAX);
            let base = isize::try_from(edit.base.len()).unwrap_or(isize::MAX);
            side - base
        })
        .sum()
}

/// The lines of `content`, each KEEPING its terminator, with no trailing empty
/// line for content that ends in one — the lines [`diffy`] compares, so the two
/// agree about what a line is.
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut start = 0usize;
    for (index, byte) in content.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(&content[start..=index]);
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

/// The runs of lines one side changed away from the merge base, in base order.
///
/// The diff itself is [`diffy`]'s — the same crate the three-way content merge
/// uses — asked for a context wide enough that the whole comparison is ONE hunk,
/// so the edit script can be walked line by line and every position counted
/// rather than read out of a unified-diff header. `None` where that walk did not
/// account for every line of both versions, which is the one case the positions
/// could not be trusted.
fn edits_against_base(base: &[u8], side: &[u8], base_lines: usize, side_lines: usize) -> Option<Vec<Edit>> {
    let context = base.len().saturating_add(side.len()).saturating_add(1);
    let patch = ::diffy::DiffOptions::new()
        .set_context_len(context)
        .create_patch_bytes(base, side);

    let mut edits: Vec<Edit> = Vec::new();
    let mut base_at = 0usize;
    let mut side_at = 0usize;
    let mut run: Option<(usize, usize)> = None;
    for hunk in patch.hunks() {
        for line in hunk.lines() {
            match line {
                ::diffy::Line::Context(_) => {
                    if let Some((base_from, side_from)) = run.take() {
                        edits.push(Edit {
                            base: base_from..base_at,
                            side: side_from..side_at,
                        });
                    }
                    base_at += 1;
                    side_at += 1;
                }
                ::diffy::Line::Delete(_) => {
                    run.get_or_insert((base_at, side_at));
                    base_at += 1;
                }
                ::diffy::Line::Insert(_) => {
                    run.get_or_insert((base_at, side_at));
                    side_at += 1;
                }
            }
        }
    }
    if let Some((base_from, side_from)) = run {
        edits.push(Edit {
            base: base_from..base_at,
            side: side_from..side_at,
        });
    }
    (base_at == base_lines && side_at == side_lines).then_some(edits)
}

/// One side's span of a merge hunk: the half-open line span the region covers in
/// that side, and the region's bytes as the model carries them.
///
/// `hunkContent` is "the base-64 encoded content of the hunk merged region", so
/// it is base64 TEXT in a string member rather than a blob the protocol encodes,
/// and `startLine`/`endLine` are line positions counted from the start of that
/// side's file, the first line being line 0 and `endLine` one past the region's
/// last line — so `endLine - startLine` is how many lines the region holds and a
/// region a side holds no line of has `startLine == endLine`.
fn hunk_detail(lines: &[&[u8]], span: Range<usize>) -> MergeHunkDetail {
    let start = span.start.min(lines.len());
    let end = span.end.clamp(start, lines.len());
    let content: Vec<u8> = lines[start..end].concat();
    MergeHunkDetail {
        start_line: Some(i32::try_from(start).unwrap_or(i32::MAX)),
        end_line: Some(i32::try_from(end).unwrap_or(i32::MAX)),
        hunk_content: Some(::aws_smithy_types::base64::encode(&content)),
    }
}

/// The merge hunks of ONE path a merge could not reconcile: the regions of the
/// path's two versions that differ, at the granularity `detail_level` names.
///
/// * `FILE_LEVEL` — the level a request that names none is evaluated at — judges
///   a file as a whole ("returns a not-mergeable result if the same file has
///   differences in both branches"), so the region it reports is each side's
///   WHOLE file.
/// * `LINE_LEVEL` judges the lines ("a conflict is considered not mergeable if
///   the same file in both branches has differences on the same line"), so each
///   region is one group of lines the two sides' changes against the merge base
///   fall into: two changes that touch the same base lines, or that meet with no
///   unchanged line between them, are one region, since neither side's lines can
///   be taken there without dropping the other's.
///
/// `isConflict` is true exactly where both sides changed the region and changed
/// it to different lines — "true when the hunk represents a conflict and one or
/// more files contains a line conflict", and a region only one side changed is
/// that side's change, which merges. A mode-only disagreement sets it nowhere:
/// "file mode conflicts in a merge do not set this to true".
///
/// A path with no two text versions to compare — a side that holds no file at
/// all, content this repository no longer holds the bytes of, or content that is
/// binary — reports NO hunks: there are no lines there to report. The bytes of
/// the three versions are charged to `budget` and no version past
/// [`MAX_MERGE_FILE_BYTES`] or [`MAX_MERGE_FILE_LINES`] is compared at all, the
/// same bounds the three-way content merge keeps.
pub fn merge_hunks(
    repository: &Repository,
    base: TreeSlot<'_>,
    source: TreeSlot<'_>,
    destination: TreeSlot<'_>,
    detail_level: &str,
    budget: &mut MergeBudget,
) -> Result<Vec<MergeHunk>, MergeProblem> {
    let (Some(source_text), Some(destination_text)) =
        (source.text(repository), destination.text(repository))
    else {
        return Ok(Vec::new());
    };
    // The two sides hold the very same lines: there is no difference between
    // them to report a hunk about, whatever else about the path conflicts.
    if source_text == destination_text {
        return Ok(Vec::new());
    }
    // A base that holds no text version of the path is compared against as empty
    // content, exactly as the three-way content merge compares a path neither
    // history held before.
    let base_text = base.text(repository);
    let ancestor: &[u8] = base_text.unwrap_or_default();
    budget.content(
        ancestor
            .len()
            .saturating_add(source_text.len())
            .saturating_add(destination_text.len()),
    )?;
    for version in [ancestor, source_text, destination_text] {
        if version.len() > MAX_MERGE_FILE_BYTES
            || version.iter().filter(|byte| **byte == b'\n').count() >= MAX_MERGE_FILE_LINES
        {
            return Err(MergeProblem::ContentToLoad);
        }
    }

    let conflicting = content_conflict(&base, &source, &destination);
    let base_lines = split_lines(ancestor);
    let source_lines = split_lines(source_text);
    let destination_lines = split_lines(destination_text);

    // The whole file, as one region: what `FILE_LEVEL` detail judges, and the
    // answer for a comparison whose line positions could not be accounted for.
    let whole_file = || {
        vec![MergeHunk {
            is_conflict: conflicting,
            source: Some(hunk_detail(&source_lines, 0..source_lines.len())),
            destination: Some(hunk_detail(&destination_lines, 0..destination_lines.len())),
            base: base_text.map(|_| hunk_detail(&base_lines, 0..base_lines.len())),
        }]
    };
    if detail_level != CONFLICT_DETAIL_LINE_LEVEL {
        return Ok(whole_file());
    }

    let (Some(source_edits), Some(destination_edits)) = (
        edits_against_base(
            ancestor,
            source_text,
            base_lines.len(),
            source_lines.len(),
        ),
        edits_against_base(
            ancestor,
            destination_text,
            base_lines.len(),
            destination_lines.len(),
        ),
    ) else {
        return Ok(whole_file());
    };

    let mut hunks: Vec<MergeHunk> = Vec::new();
    let mut next_source = 0usize;
    let mut next_destination = 0usize;
    // How far each side's line numbering has drifted from the base's at the
    // start of the region being built: the sum of the drift of every run of that
    // side already reported.
    let mut source_drift = 0isize;
    let mut destination_drift = 0isize;
    while next_source < source_edits.len() || next_destination < destination_edits.len() {
        let starts = [
            source_edits.get(next_source).map(|edit| edit.base.start),
            destination_edits
                .get(next_destination)
                .map(|edit| edit.base.start),
        ];
        let Some(region_from) = starts.into_iter().flatten().min() else {
            break;
        };
        let mut region_to = region_from;
        let mut in_source: Vec<Edit> = Vec::new();
        let mut in_destination: Vec<Edit> = Vec::new();
        // A region grows while either side has a run that starts no later than
        // where it currently ends: such a run either overlaps the region or
        // abuts it with no unchanged line between, and in both cases the two
        // sides' lines cannot be taken independently.
        loop {
            let mut grew = false;
            while let Some(edit) = source_edits
                .get(next_source)
                .filter(|edit| edit.base.start <= region_to)
            {
                region_to = region_to.max(edit.base.end);
                in_source.push(edit.clone());
                next_source += 1;
                grew = true;
            }
            while let Some(edit) = destination_edits
                .get(next_destination)
                .filter(|edit| edit.base.start <= region_to)
            {
                region_to = region_to.max(edit.base.end);
                in_destination.push(edit.clone());
                next_destination += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }

        let source_span = side_span(region_from, region_to, source_drift, &in_source);
        let destination_span =
            side_span(region_from, region_to, destination_drift, &in_destination);
        source_drift += drift(&in_source);
        destination_drift += drift(&in_destination);

        let changed_by_both = !in_source.is_empty() && !in_destination.is_empty();
        let same_lines = source_lines.get(source_span.clone())
            == destination_lines.get(destination_span.clone());
        hunks.push(MergeHunk {
            is_conflict: conflicting && changed_by_both && !same_lines,
            source: Some(hunk_detail(&source_lines, source_span)),
            destination: Some(hunk_detail(&destination_lines, destination_span)),
            base: base_text.map(|_| hunk_detail(&base_lines, region_from..region_to)),
        });
    }
    Ok(hunks)
}

/// Where a region of the merge base sits in ONE side's line numbering: the base
/// span shifted by the drift this side carries into the region, and lengthened
/// by the drift of this side's own runs within it.
fn side_span(
    region_from: usize,
    region_to: usize,
    drift_before: isize,
    within: &[Edit],
) -> Range<usize> {
    let start = isize::try_from(region_from)
        .unwrap_or(isize::MAX)
        .saturating_add(drift_before);
    let length = isize::try_from(region_to.saturating_sub(region_from))
        .unwrap_or(isize::MAX)
        .saturating_add(drift(within));
    let start = usize::try_from(start).unwrap_or(0);
    let length = usize::try_from(length).unwrap_or(0);
    start..start.saturating_add(length)
}
