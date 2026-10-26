// SPDX-License-Identifier: CC0-1.0

//! `DescribeMergeConflicts` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: the operation evaluates merging the source commit into the
//! destination commit against their merge base — all three resolved out of stored
//! state — and describes the ONE path named by `filePath` that merge cannot reconcile:
//! its conflict metadata (which facet of the path the two sides disagree about, and what
//! each of the three sides holds there) and the hunks of the differences between the
//! path's two versions, at the granularity `conflictDetailLevel` names. `maxMergeHunks`
//! is the page size of those hunks and `nextToken` resumes after the last one reported.
//! A path this merge has no conflict at — including one no side of the merge holds — has
//! no conflict to describe.
//!
//! The tables below span that class rather than one recorded request: which side changed
//! the path and how (content, mode, deletion, addition), whether the file's content is
//! text at all, whether the two sides' line edits collide or sit apart, how many regions
//! they fall into, the detail level (named and defaulted), the merge option, the commit
//! specifier form, the path form, and the page size at each published bound.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The paging bounds this service publishes for a `maxResults`-style member, imported
// rather than copied: `maxMergeHunks` is this operation's page size.
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    DEFAULT_MAX_RESULTS, MAX_MAX_RESULTS, MIN_MAX_RESULTS,
};

// The awsJson1_1 wire harness lives once for this test crate in the suite that first
// needed it, and the commit writer in the suite that first needed that.
use crate::repository_lifecycle::{emulator, error, from_base64, ok, string_at};
use crate::unreferenced_merge_commits::{commit, Change, EXECUTABLE, NORMAL};

/// The branch a merge is evaluated INTO, and the branch merged in.
const DESTINATION_BRANCH: &str = "main";
const SOURCE_BRANCH: &str = "feature";

/// `mergeOption` values, from `smithy_query(query="shape",shape="MergeOptionTypeEnum")`.
const FAST_FORWARD_MERGE: &str = "FAST_FORWARD_MERGE";
const SQUASH_MERGE: &str = "SQUASH_MERGE";
const THREE_WAY_MERGE: &str = "THREE_WAY_MERGE";

/// `conflictDetailLevel` values, from
/// `smithy_query(query="shape",shape="ConflictDetailLevelTypeEnum")`.
const FILE_LEVEL: &str = "FILE_LEVEL";
const LINE_LEVEL: &str = "LINE_LEVEL";

/// The `fileMode` this suite needs beyond the two the commit writer names, from
/// `smithy_query(query="shape",shape="FileModeTypeEnum")`.
const SYMLINK: &str = "SYMLINK";

/// `objectType` values, from `smithy_query(query="shape",shape="ObjectTypeEnum")`: what
/// the three sides report holding at a path.
const OBJECT_TYPE_FILE: &str = "FILE";
const OBJECT_TYPE_SYMBOLIC_LINK: &str = "SYMBOLIC_LINK";

/// `changeType` values, from `smithy_query(query="shape",shape="ChangeTypeEnum")`: what
/// one side of the merge did to the path.
const CHANGE_ADDED: &str = "A";
const CHANGE_MODIFIED: &str = "M";
const CHANGE_DELETED: &str = "D";

const ALPHA: &str = "alpha\n";
const ALPHA_FROM_SOURCE: &str = "alpha, from the source\n";
const ALPHA_FROM_DESTINATION: &str = "alpha, from the destination\n";
const EDITED_BY_DESTINATION: &str = "edited by the destination\n";
const UNTOUCHED: &str = "untouched\n";
const ADDED_BY_SOURCE: &str = "added by the source\n";
const ADDED_BY_DESTINATION: &str = "added by the destination\n";

/// A file long enough for the two sides to edit regions that collide, regions that sit
/// apart, and two regions at once.
const SEVEN_LINES: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
const FIRST_FROM_SOURCE: &str = "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\n";
const FIRST_FROM_DESTINATION: &str = "uno\ntwo\nthree\nfour\nfive\nsix\nseven\n";
const LAST_FROM_DESTINATION: &str = "one\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n";
const BOTH_ENDS_FROM_SOURCE: &str = "ONE\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n";
const BOTH_ENDS_FROM_DESTINATION: &str = "uno\ntwo\nthree\nfour\nfive\nsix\nsiete\n";

/// What a symlink's bytes are: the path it points at.
const LINK_TARGET: &str = "a.txt";

/// Bytes with a NUL in them: binary content, which has no lines to report hunks about.
const BINARY: &str = "\u{0}\u{1}gif\u{0}";
const BINARY_FROM_SOURCE: &str = "\u{0}source";
const BINARY_FROM_DESTINATION: &str = "\u{0}destination\u{0}";

/// What one side holds at a path: its content and the mode it holds it at, or `None`
/// where that side holds nothing there.
type Side = Option<(&'static str, &'static str)>;

/// One row of the table: a path, what each of the three sides of the merge holds there,
/// and what the rule fixes about the report for it.
struct Row {
    case: &'static str,
    path: &'static str,
    base: Side,
    source: Side,
    destination: Side,
    /// The facets of the path the two sides disagree about.
    content_conflict: bool,
    file_mode_conflict: bool,
    object_type_conflict: bool,
    /// The detail levels at which this merge cannot reconcile the path at all; at any
    /// other level there is no conflict here to describe.
    conflicting_at: &'static [&'static str],
    /// How many hunks the report carries under `FILE_LEVEL` detail, which judges the
    /// file as a whole: one region per side, or none where the two sides have no two
    /// text versions to compare.
    file_level_hunks: usize,
    /// The source side's content of each hunk the report carries under `LINE_LEVEL`
    /// detail, in order: one entry per region of lines the two sides' changes fall into.
    line_level_regions: &'static [&'static str],
    /// Whether any reported hunk is marked a conflict.
    hunk_conflict: bool,
}

/// The paths the table covers, spanning which side changed the path and how, whether its
/// content is text, and how the two sides' line edits sit relative to each other.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "both sides change a one-line file differently",
            path: "a.txt",
            base: Some((ALPHA, NORMAL)),
            source: Some((ALPHA_FROM_SOURCE, NORMAL)),
            destination: Some((ALPHA_FROM_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 1,
            line_level_regions: &[ALPHA_FROM_SOURCE],
            hunk_conflict: true,
        },
        Row {
            case: "both sides change the same line of a seven-line file",
            path: "near.txt",
            base: Some((SEVEN_LINES, NORMAL)),
            source: Some((FIRST_FROM_SOURCE, NORMAL)),
            destination: Some((FIRST_FROM_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 1,
            line_level_regions: &["ONE\n"],
            hunk_conflict: true,
        },
        Row {
            case: "both sides change two separate lines, each differently",
            path: "two-regions.txt",
            base: Some((SEVEN_LINES, NORMAL)),
            source: Some((BOTH_ENDS_FROM_SOURCE, NORMAL)),
            destination: Some((BOTH_ENDS_FROM_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 1,
            line_level_regions: &["ONE\n", "SEVEN\n"],
            hunk_conflict: true,
        },
        // The two sides' edits sit apart, so the lines merge: at LINE_LEVEL there is no
        // conflict here to describe, while FILE_LEVEL judges the file as a whole.
        Row {
            case: "the two sides change lines that sit apart",
            path: "far.txt",
            base: Some((SEVEN_LINES, NORMAL)),
            source: Some((FIRST_FROM_SOURCE, NORMAL)),
            destination: Some((LAST_FROM_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL],
            file_level_hunks: 1,
            line_level_regions: &[],
            hunk_conflict: true,
        },
        // A deletion against a change: the destination's edit cannot be carried into a
        // file the source no longer holds, and there are no two versions to compare.
        Row {
            case: "the source deletes a file the destination changed",
            path: "gone-in-source.txt",
            base: Some((ALPHA, NORMAL)),
            source: None,
            destination: Some((EDITED_BY_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 0,
            line_level_regions: &[],
            hunk_conflict: false,
        },
        Row {
            case: "the destination deletes a file the source changed",
            path: "gone-in-destination.txt",
            base: Some((ALPHA, NORMAL)),
            source: Some((ALPHA_FROM_SOURCE, NORMAL)),
            destination: None,
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 0,
            line_level_regions: &[],
            hunk_conflict: false,
        },
        // A path no merge base held: neither side can claim the base's content, and the
        // hunk carries no base side at all.
        Row {
            case: "both sides add the same new path with different content",
            path: "added-by-both.txt",
            base: None,
            source: Some((ADDED_BY_SOURCE, NORMAL)),
            destination: Some((ADDED_BY_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 1,
            line_level_regions: &[ADDED_BY_SOURCE],
            hunk_conflict: true,
        },
        // Binary content has no lines to report, whatever the detail level.
        Row {
            case: "both sides change binary content differently",
            path: "bin.dat",
            base: Some((BINARY, NORMAL)),
            source: Some((BINARY_FROM_SOURCE, NORMAL)),
            destination: Some((BINARY_FROM_DESTINATION, NORMAL)),
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 0,
            line_level_regions: &[],
            hunk_conflict: false,
        },
        // The same bytes under two modes neither side can take from the other: the mode
        // is the facet that disagrees, and "file mode conflicts in a merge do not set
        // this to true" — no hunk is marked a conflict, and the two versions hold the
        // same lines, so there is no hunk at all.
        Row {
            case: "both sides move only the file mode, differently",
            path: "link",
            base: Some((LINK_TARGET, SYMLINK)),
            source: Some((LINK_TARGET, NORMAL)),
            destination: Some((LINK_TARGET, EXECUTABLE)),
            content_conflict: false,
            file_mode_conflict: true,
            object_type_conflict: false,
            conflicting_at: &[FILE_LEVEL, LINE_LEVEL],
            file_level_hunks: 0,
            line_level_regions: &[],
            hunk_conflict: false,
        },
        Row {
            case: "a path neither side touched",
            path: "quiet.txt",
            base: Some((UNTOUCHED, NORMAL)),
            source: Some((UNTOUCHED, NORMAL)),
            destination: Some((UNTOUCHED, NORMAL)),
            content_conflict: false,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[],
            file_level_hunks: 0,
            line_level_regions: &[],
            hunk_conflict: false,
        },
        Row {
            case: "a path no side of the merge holds",
            path: "nowhere.txt",
            base: None,
            source: None,
            destination: None,
            content_conflict: false,
            file_mode_conflict: false,
            object_type_conflict: false,
            conflicting_at: &[],
            file_level_hunks: 0,
            line_level_regions: &[],
            hunk_conflict: false,
        },
    ]
}

/// The three commits the table's fixture sets up: the merge base, and the tip of each
/// side.
#[derive(Debug, Clone)]
struct Fixture {
    base: String,
    source: String,
    destination: String,
}

/// The changes one side commits on top of the base: a path it holds differently is
/// written, a path it no longer holds is deleted, and a path it leaves exactly as the
/// base had it is not touched at all.
fn changes_for(rows: &[Row], side: impl Fn(&Row) -> Side) -> Vec<Change> {
    rows.iter()
        .filter_map(|row| {
            let held = side(row);
            if held == row.base {
                return None;
            }
            match held {
                Some((content, mode)) => Some(Change::PutMode(row.path, content, mode)),
                None => Some(Change::Delete(row.path)),
            }
        })
        .collect()
}

/// Build a repository whose two branches diverge from one base commit, holding exactly
/// what the table's rows say each side holds.
async fn fixture(router: &MultiServiceRouter, repository: &str, rows: &[Row]) -> Fixture {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base_changes: Vec<Change> = rows
        .iter()
        .filter_map(|row| {
            row.base
                .map(|(content, mode)| Change::PutMode(row.path, content, mode))
        })
        .collect();
    let base = commit(
        router,
        repository,
        DESTINATION_BRANCH,
        None,
        &base_changes,
        "the base commit",
    )
    .await;
    ok(
        router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE_BRANCH,
            "commitId": base,
        }),
    )
    .await;
    let destination = commit(
        router,
        repository,
        DESTINATION_BRANCH,
        Some(&base),
        &changes_for(rows, |row| row.destination),
        "a commit on the destination",
    )
    .await;
    let source = commit(
        router,
        repository,
        SOURCE_BRANCH,
        Some(&base),
        &changes_for(rows, |row| row.source),
        "a commit on the source",
    )
    .await;
    Fixture {
        base,
        source,
        destination,
    }
}

/// A `DescribeMergeConflicts` request for one path, naming the two branches.
fn request(repository: &str, path: &str, merge_option: &str, detail_level: Option<&str>) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "sourceCommitSpecifier": SOURCE_BRANCH,
        "destinationCommitSpecifier": DESTINATION_BRANCH,
        "mergeOption": merge_option,
        "filePath": path,
    });
    if let Some(detail_level) = detail_level {
        input["conflictDetailLevel"] = json!(detail_level);
    }
    input
}

/// The lines of `content`, each KEEPING its terminator and with no trailing empty line —
/// the lines a hunk's span counts.
fn lines(content: &str) -> Vec<&str> {
    content.split_inclusive('\n').collect()
}

/// Is this content binary rather than text? A NUL byte is git's own test, and content
/// with one has no lines to compare at all.
fn is_binary(content: &str) -> bool {
    content.contains('\u{0}')
}

/// The modelled `objectType` of a side holding content at `mode`.
fn object_type(mode: &str) -> &'static str {
    if mode == SYMLINK {
        OBJECT_TYPE_SYMBOLIC_LINK
    } else {
        OBJECT_TYPE_FILE
    }
}

/// An object carrying only the sides the row gives a value for: the wire omits a member
/// a side has none of.
fn present(source: Option<Value>, destination: Option<Value>, base: Option<Value>) -> Value {
    let mut map = Map::new();
    for (member, value) in [
        ("source", source),
        ("destination", destination),
        ("base", base),
    ] {
        if let Some(value) = value {
            map.insert(member.to_string(), value);
        }
    }
    Value::Object(map)
}

/// How one side changed the path away from the merge base, or `None` where it left the
/// path exactly as the base had it.
fn merge_operation(base: Side, side: Side) -> Option<&'static str> {
    if side == base {
        return None;
    }
    match (base, side) {
        (None, Some(_)) => Some(CHANGE_ADDED),
        (Some(_), None) => Some(CHANGE_DELETED),
        _ => Some(CHANGE_MODIFIED),
    }
}

/// The whole `conflictMetadata` the rule fixes for one row, built from what the row says
/// the three sides hold rather than from the response.
fn expected_metadata(row: &Row) -> Value {
    let size = |side: Side| json!(side.map_or(0, |(content, _)| content.len()));
    let mode = |side: Side| side.map(|(_, mode)| json!(mode));
    let kind = |side: Side| side.map(|(_, mode)| json!(object_type(mode)));
    let binary = |side: Side| side.map(|(content, _)| json!(is_binary(content)));
    // "The number of conflicts, including both hunk conflicts and metadata conflicts":
    // the facets this row says disagree, and never fewer than the one disagreement that
    // put the path in the report at all.
    let facets = i64::from(row.content_conflict)
        + i64::from(row.file_mode_conflict)
        + i64::from(row.object_type_conflict);
    json!({
        "filePath": row.path,
        "fileSizes": json!({
            "source": size(row.source),
            "destination": size(row.destination),
            "base": size(row.base),
        }),
        "fileModes": present(mode(row.source), mode(row.destination), mode(row.base)),
        "objectTypes": present(kind(row.source), kind(row.destination), kind(row.base)),
        "isBinaryFile": present(binary(row.source), binary(row.destination), binary(row.base)),
        "numberOfConflicts": facets.max(1),
        "contentConflict": row.content_conflict,
        "fileModeConflict": row.file_mode_conflict,
        "objectTypeConflict": row.object_type_conflict,
        "mergeOperations": present(
            merge_operation(row.base, row.source).map(|change| json!(change)),
            merge_operation(row.base, row.destination).map(|change| json!(change)),
            None,
        ),
    })
}

/// The hunks one response reports.
fn reported_hunks(payload: &Value) -> Vec<Value> {
    payload
        .get("mergeHunks")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a mergeHunks list in {payload}"))
        .clone()
}

/// Check one reported hunk against the content the row says the side holds: the bytes
/// `hunkContent` carries are exactly the side's lines in the half-open span the hunk
/// names, and the side is reported only where it has text lines to report.
fn check_side(case: &str, hunk: &Value, member: &str, held: Side) {
    let text = held
        .map(|(content, _)| content)
        .filter(|content| !is_binary(content));
    let Some(detail) = hunk.get(member) else {
        assert!(
            text.is_none(),
            "case {case:?}: expected a {member} side in the hunk {hunk}"
        );
        return;
    };
    let content = text.unwrap_or_else(|| {
        panic!("case {case:?}: expected no {member} side in the hunk {hunk}");
    });
    let start = detail
        .get("startLine")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a startLine in {detail}"));
    let end = detail
        .get("endLine")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("case {case:?}: expected an endLine in {detail}"));
    let held_lines = lines(content);
    assert!(
        start <= end && end as usize <= held_lines.len(),
        "case {case:?}: expected a span within the {member} side's {} lines, got {detail}",
        held_lines.len()
    );
    let expected: String = held_lines[start as usize..end as usize].concat();
    let reported = from_base64(&string_at(detail, "/hunkContent"));
    assert_eq!(
        reported,
        expected.as_bytes(),
        "case {case:?}: expected the {member} hunk content {expected:?}, got {:?}",
        String::from_utf8_lossy(&reported)
    );
}

/// The `isConflict` flag of every reported hunk.
fn conflict_flags(case: &str, hunks: &[Value]) -> Vec<bool> {
    hunks
        .iter()
        .map(|hunk| {
            hunk.get("isConflict")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| panic!("case {case:?}: expected an isConflict flag in {hunk}"))
        })
        .collect()
}

#[tokio::test]
async fn one_conflicting_path_is_described_out_of_stored_state() {
    let router = emulator();
    let rows = rows();
    let fixture = fixture(&router, "described-paths", &rows).await;

    // The detail level is sent, and also left out to exercise the FILE_LEVEL default the
    // member documents; the merge option is each of the two strategies this report
    // describes conflicts for, which must describe the same merge.
    for sent_level in [None, Some(FILE_LEVEL), Some(LINE_LEVEL)] {
        let level = sent_level.unwrap_or(FILE_LEVEL);
        for merge_option in [SQUASH_MERGE, THREE_WAY_MERGE] {
            for row in &rows {
                let case = format!("{} [{level}, {merge_option}]", row.case);
                let input = request("described-paths", row.path, merge_option, sent_level);
                if !row.conflicting_at.contains(&level) {
                    let answered = error(&router, "DescribeMergeConflicts", &input, 400).await;
                    assert_eq!(
                        answered, "FileDoesNotExistException",
                        "case {case:?}: a path this merge has no conflict at has no conflict \
                         to describe, got {answered}"
                    );
                    continue;
                }

                let payload = ok(&router, "DescribeMergeConflicts", &input).await;
                assert_eq!(
                    payload.get("conflictMetadata"),
                    Some(&expected_metadata(row)),
                    "case {case:?}: expected the metadata {}, got {payload}",
                    expected_metadata(row)
                );
                for (member, expected) in [
                    ("sourceCommitId", &fixture.source),
                    ("destinationCommitId", &fixture.destination),
                    ("baseCommitId", &fixture.base),
                ] {
                    assert_eq!(
                        payload.get(member).and_then(Value::as_str),
                        Some(expected.as_str()),
                        "case {case:?}: expected the {member} {expected}, got {payload}"
                    );
                }
                assert!(
                    payload.get("nextToken").is_none(),
                    "case {case:?}: a report that fits in one page resumes nowhere, got {payload}"
                );

                let hunks = reported_hunks(&payload);
                let expected_hunks = if level == LINE_LEVEL {
                    row.line_level_regions.len()
                } else {
                    row.file_level_hunks
                };
                assert_eq!(
                    hunks.len(),
                    expected_hunks,
                    "case {case:?}: expected {expected_hunks} hunk(s), got {payload}"
                );
                for hunk in &hunks {
                    check_side(&case, hunk, "source", row.source);
                    check_side(&case, hunk, "destination", row.destination);
                    check_side(&case, hunk, "base", row.base);
                }
                let flags = conflict_flags(&case, &hunks);
                assert_eq!(
                    flags.iter().any(|flag| *flag),
                    row.hunk_conflict && !hunks.is_empty(),
                    "case {case:?}: expected the conflict flags {:?}, got {flags:?}",
                    row.hunk_conflict
                );

                // FILE_LEVEL judges the file as a whole, so the region it reports is each
                // side's WHOLE file; LINE_LEVEL reports one region per group of differing
                // lines, in the order they sit in the file.
                if level == LINE_LEVEL {
                    let regions: Vec<String> = hunks
                        .iter()
                        .map(|hunk| {
                            String::from_utf8_lossy(&from_base64(&string_at(
                                hunk,
                                "/source/hunkContent",
                            )))
                            .to_string()
                        })
                        .collect();
                    assert_eq!(
                        regions, row.line_level_regions,
                        "case {case:?}: expected the source regions {:?}, got {regions:?}",
                        row.line_level_regions
                    );
                } else if let Some(hunk) = hunks.first() {
                    for (member, held) in [("source", row.source), ("destination", row.destination)]
                    {
                        let held = held.map(|(content, _)| content).unwrap_or_default();
                        assert_eq!(
                            hunk.pointer(&format!("/{member}/endLine"))
                                .and_then(Value::as_u64),
                            Some(lines(held).len() as u64),
                            "case {case:?}: expected the {member} region to span the whole \
                             file, got {hunk}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn max_merge_hunks_pages_the_reported_hunks() {
    let router = emulator();
    let repository = "paged-hunks";
    let rows = rows();
    fixture(&router, repository, &rows).await;
    // The path whose two sides changed two separate regions, so there is more than one
    // hunk to page through.
    let path = "two-regions.txt";
    let input = request(repository, path, THREE_WAY_MERGE, Some(LINE_LEVEL));
    let whole = reported_hunks(&ok(&router, "DescribeMergeConflicts", &input).await);
    assert!(
        whole.len() > 1,
        "case \"the paging fixture\": expected more than one hunk to page, got {whole:?}"
    );

    // Each published bound of the page size, and one past the top of the range.
    for (case, size, expected) in [
        ("the smallest page", MIN_MAX_RESULTS, 1usize),
        ("one hunk at a time", 1, 1),
        ("a page wider than the report", 50, whole.len()),
        ("the largest page", MAX_MAX_RESULTS, whole.len()),
    ] {
        let mut page_input = input.clone();
        page_input["maxMergeHunks"] = json!(size);
        let payload = ok(&router, "DescribeMergeConflicts", &page_input).await;
        let hunks = reported_hunks(&payload);
        assert_eq!(
            hunks.len(),
            expected.min(whole.len()),
            "case {case:?}: expected {expected} hunk(s) in the page, got {payload}"
        );
        assert_eq!(
            payload.get("nextToken").is_some(),
            hunks.len() < whole.len(),
            "case {case:?}: a page that stops short of the end resumes after its last \
             hunk, got {payload}"
        );
    }

    // The pages of any size together are the whole list, in the same order.
    for size in [1, 2, 3] {
        let mut paged: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut page_input = input.clone();
            page_input["maxMergeHunks"] = json!(size);
            if let Some(token) = &token {
                page_input["nextToken"] = json!(token);
            }
            let payload = ok(&router, "DescribeMergeConflicts", &page_input).await;
            let hunks = reported_hunks(&payload);
            assert!(
                hunks.len() <= size,
                "case \"pages of {size}\": expected at most {size} hunk(s), got {payload}"
            );
            paged.extend(hunks);
            token = payload
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            paged, whole,
            "case \"pages of {size}\": the pages together are the whole hunk list, got \
             {paged:?} and {whole:?}"
        );
    }

    // The page size a request that names none is served at: the whole report in one
    // page, which is what the default is.
    let mut defaulted = input.clone();
    defaulted["maxMergeHunks"] = json!(DEFAULT_MAX_RESULTS);
    let payload = ok(&router, "DescribeMergeConflicts", &defaulted).await;
    assert_eq!(
        reported_hunks(&payload),
        whole,
        "case \"the default page size\": expected the whole report in one page, got {payload}"
    );

    // A page size outside the published range, at each side of it.
    for (case, size) in [
        ("one below the range", MIN_MAX_RESULTS - 1),
        ("a negative page size", -1),
        ("one past the range", MAX_MAX_RESULTS + 1),
    ] {
        let mut page_input = input.clone();
        page_input["maxMergeHunks"] = json!(size);
        let answered = error(&router, "DescribeMergeConflicts", &page_input, 400).await;
        assert_eq!(
            answered, "InvalidMaxMergeHunksException",
            "case {case:?}: expected InvalidMaxMergeHunksException for a page size of \
             {size}, got {answered}"
        );
    }

    // A token this service did not mint.
    let mut bad_token = input.clone();
    bad_token["nextToken"] = json!("not-a-token-this-service-minted");
    let answered = error(&router, "DescribeMergeConflicts", &bad_token, 400).await;
    assert_eq!(
        answered, "InvalidContinuationTokenException",
        "case \"a token this service did not mint\": got {answered}"
    );
}

#[tokio::test]
async fn the_path_and_the_two_specifiers_are_read_out_of_stored_state() {
    let router = emulator();
    let repository = "read-specifiers";
    let rows = rows();
    let fixture = fixture(&router, repository, &rows).await;

    // The path is read in the canonical form every file read in this service uses, so
    // these forms all address the same file.
    let canonical = ok(
        &router,
        "DescribeMergeConflicts",
        &request(repository, "a.txt", THREE_WAY_MERGE, Some(LINE_LEVEL)),
    )
    .await;
    for form in ["/a.txt", "//a.txt"] {
        let payload = ok(
            &router,
            "DescribeMergeConflicts",
            &request(repository, form, THREE_WAY_MERGE, Some(LINE_LEVEL)),
        )
        .await;
        assert_eq!(
            payload, canonical,
            "case {form:?}: expected the same report as the canonical path form, got \
             {payload} and {canonical}"
        );
    }

    // A specifier is a branch name, a tag, or a full commit id, and the same commit
    // named either way is the same merge.
    let mut by_commit_id = request(repository, "a.txt", THREE_WAY_MERGE, Some(LINE_LEVEL));
    by_commit_id["sourceCommitSpecifier"] = json!(fixture.source);
    by_commit_id["destinationCommitSpecifier"] = json!(fixture.destination);
    let payload = ok(&router, "DescribeMergeConflicts", &by_commit_id).await;
    assert_eq!(
        payload, canonical,
        "case \"the tips named by commit id\": expected the same report as the branch \
         names, got {payload} and {canonical}"
    );

    // What the operation refuses, and with which modelled error.
    let bad_name = {
        let mut input = request(repository, "a.txt", THREE_WAY_MERGE, None);
        input["repositoryName"] = json!("not a repository name!");
        input
    };
    let no_such_source = {
        let mut input = request(repository, "a.txt", THREE_WAY_MERGE, None);
        input["sourceCommitSpecifier"] = json!("no-such-branch");
        input
    };
    let no_such_destination = {
        let mut input = request(repository, "a.txt", THREE_WAY_MERGE, None);
        input["destinationCommitSpecifier"] = json!("no-such-branch");
        input
    };
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository this account does not hold",
            request("no-such-repository", "a.txt", THREE_WAY_MERGE, None),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name outside the modelled constraints",
            bad_name,
            "InvalidRepositoryNameException",
        ),
        (
            "a source specifier naming no commit",
            no_such_source,
            "CommitDoesNotExistException",
        ),
        (
            "a destination specifier naming no commit",
            no_such_destination,
            "CommitDoesNotExistException",
        ),
        (
            "a fast-forward, which is not a strategy this report describes conflicts for",
            request(repository, "a.txt", FAST_FORWARD_MERGE, None),
            "InvalidMergeOptionException",
        ),
        (
            "a path that addresses no file",
            request(repository, "dir/../a.txt", THREE_WAY_MERGE, None),
            "InvalidPathException",
        ),
        (
            "an empty path",
            request(repository, "", THREE_WAY_MERGE, None),
            "PathRequiredException",
        ),
        (
            "a conflict detail level outside the enum",
            request(repository, "a.txt", THREE_WAY_MERGE, Some("WORD_LEVEL")),
            "InvalidConflictDetailLevelException",
        ),
    ];
    for (case, input, code) in cases {
        let answered = error(&router, "DescribeMergeConflicts", &input, 400).await;
        assert_eq!(
            answered, code,
            "case {case:?}: expected {code}, got {answered} for {input}"
        );
    }
}

/// What a later caller sees of one branch: its tip, and a file read at it.
async fn seen_by_a_later_caller(
    router: &MultiServiceRouter,
    repository: &str,
    specifier: &str,
) -> Value {
    let branch = ok(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": specifier }),
    )
    .await;
    let file = ok(
        router,
        "GetFile",
        &json!({
            "repositoryName": repository,
            "commitSpecifier": specifier,
            "filePath": "a.txt",
        }),
    )
    .await;
    json!({ "branch": branch, "file": file })
}

#[tokio::test]
async fn describing_a_merge_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let repository = "unchanged-by-describing";
    let rows = rows();
    let fixture = fixture(&router, repository, &rows).await;

    let before = vec![
        seen_by_a_later_caller(&router, repository, SOURCE_BRANCH).await,
        seen_by_a_later_caller(&router, repository, DESTINATION_BRANCH).await,
    ];
    let input = request(repository, "a.txt", THREE_WAY_MERGE, Some(LINE_LEVEL));
    let first = ok(&router, "DescribeMergeConflicts", &input).await;
    let second = ok(&router, "DescribeMergeConflicts", &input).await;
    let after = vec![
        seen_by_a_later_caller(&router, repository, SOURCE_BRANCH).await,
        seen_by_a_later_caller(&router, repository, DESTINATION_BRANCH).await,
    ];

    assert_eq!(
        first, second,
        "case \"described twice\": the same request must answer the same way, got {first} \
         and {second}"
    );
    assert_eq!(
        before, after,
        "case \"the branches and their files\": describing a merge writes nothing, got \
         {before:?} and {after:?}"
    );
    // The merge the report describes is committed nowhere: the two tips are where the
    // fixture left them.
    for (case, seen, expected) in [
        ("the source tip", &after[0], &fixture.source),
        ("the destination tip", &after[1], &fixture.destination),
    ] {
        let tip = string_at(seen, "/branch/branch/commitId");
        assert_eq!(
            &tip, expected,
            "case {case:?}: expected it where the fixture left it, got {seen}"
        );
    }
}

