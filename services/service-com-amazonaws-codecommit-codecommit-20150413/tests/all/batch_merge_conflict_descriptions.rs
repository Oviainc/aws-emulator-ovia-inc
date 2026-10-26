// SPDX-License-Identifier: CC0-1.0

//! `BatchDescribeMergeConflicts` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: the operation evaluates merging the source commit into the
//! destination commit against their merge base — both resolved out of stored state — and
//! describes each path that merge cannot reconcile with the SAME conflict metadata the
//! per-path reads report, plus the hunks of the differences between the path's two
//! versions. A hunk is a region of the two versions at the granularity
//! `conflictDetailLevel` names (the whole file under `FILE_LEVEL`, one group of differing
//! lines under `LINE_LEVEL`), carrying per side the half-open line span the region covers
//! in that side and the base64 of the region's bytes, and `isConflict` is true exactly
//! where both sides changed the region into different lines. `filePaths` picks which
//! conflicting paths are described and sends the rest to `errors`, `maxConflictFiles`
//! pages those paths and `maxMergeHunks` bounds the hunks reported per path.
//!
//! The tables below span that class rather than one recorded request: which side changed
//! a path and how (content, deletion, addition), how many lines each side holds, whether
//! the file's content is text at all, whether the two sides' edits collide, how far their
//! regions sit from the file's start once one side has inserted lines, the detail level,
//! the page size and the hunk bound.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{emulator, error, from_base64, ok, string_at};
// The commit writer and its change kinds live once for this test crate in the suite that
// first needed them, so every merge suite builds its history the same way.
use crate::unreferenced_merge_commits::{commit, Change, NORMAL};

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

/// A file long enough for the two sides to edit different parts of it.
const SEVEN_LINES: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
/// Bytes with a NUL in them: binary content, which has no lines to report.
const BINARY: &str = "\u{0}\u{1}gif\u{0}";

/// The three commits one scenario sets up: the merge base, and the tip of each side.
#[derive(Debug, Clone)]
struct Fixture {
    base: String,
    source: String,
    destination: String,
}

/// Build a repository whose two branches diverge from one base commit.
async fn fixture(
    router: &MultiServiceRouter,
    repository: &str,
    base: &[Change],
    source: &[Change],
    destination: &[Change],
) -> Fixture {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base_commit = commit(
        router,
        repository,
        DESTINATION_BRANCH,
        None,
        base,
        "the base commit",
    )
    .await;
    ok(
        router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE_BRANCH,
            "commitId": base_commit,
        }),
    )
    .await;
    let destination_tip = if destination.is_empty() {
        base_commit.clone()
    } else {
        commit(
            router,
            repository,
            DESTINATION_BRANCH,
            Some(&base_commit),
            destination,
            "a commit on the destination",
        )
        .await
    };
    let source_tip = if source.is_empty() {
        base_commit.clone()
    } else {
        commit(
            router,
            repository,
            SOURCE_BRANCH,
            Some(&base_commit),
            source,
            "a commit on the source",
        )
        .await
    };
    Fixture {
        base: base_commit,
        source: source_tip,
        destination: destination_tip,
    }
}

/// A `BatchDescribeMergeConflicts` request naming the two branches.
fn request(repository: &str, merge_option: &str, detail_level: Option<&str>) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "sourceCommitSpecifier": SOURCE_BRANCH,
        "destinationCommitSpecifier": DESTINATION_BRANCH,
        "mergeOption": merge_option,
    });
    if let Some(detail_level) = detail_level {
        input["conflictDetailLevel"] = json!(detail_level);
    }
    input
}

/// Every conflict one response describes, in the order the service returned them.
fn conflicts(payload: &Value) -> Vec<Value> {
    payload
        .get("conflicts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a conflicts list in {payload}"))
        .clone()
}

/// The `filePath` of every conflict one response describes.
fn described_paths(payload: &Value) -> Vec<String> {
    conflicts(payload)
        .iter()
        .map(|conflict| string_at(conflict, "/conflictMetadata/filePath"))
        .collect()
}

/// The conflict one response describes for `path`.
fn conflict_for(payload: &Value, path: &str) -> Value {
    conflicts(payload)
        .into_iter()
        .find(|conflict| {
            conflict
                .pointer("/conflictMetadata/filePath")
                .and_then(Value::as_str)
                == Some(path)
        })
        .unwrap_or_else(|| panic!("no conflict for {path:?} in {payload}"))
}

/// One side of one reported hunk, as the wire carries it: the line span and the
/// bytes its base64 `hunkContent` decodes to.
#[derive(Debug, PartialEq, Eq)]
struct ReportedSide {
    start_line: i64,
    end_line: i64,
    content: Vec<u8>,
}

fn reported_side(hunk: &Value, side: &str) -> Option<ReportedSide> {
    let detail = hunk.get(side)?;
    Some(ReportedSide {
        start_line: detail
            .get("startLine")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("expected a startLine in {detail}")),
        end_line: detail
            .get("endLine")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("expected an endLine in {detail}")),
        content: from_base64(&string_at(detail, "/hunkContent")),
    })
}

fn reported_hunks(conflict: &Value) -> Vec<Value> {
    conflict
        .get("mergeHunks")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a mergeHunks list in {conflict}"))
        .clone()
}

fn is_conflict(hunk: &Value) -> bool {
    hunk.get("isConflict")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("expected an isConflict flag in {hunk}"))
}

/// The lines of some content, each keeping its terminator — what a line span
/// addresses, recomputed here rather than taken from the service.
fn lines_of(content: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&content[start..=index]);
            start = index + 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

/// The side a hunk must report for `span` of `text`: the span itself, and the
/// bytes of exactly those lines.
fn expected_side(text: &str, span: (usize, usize)) -> ReportedSide {
    let lines = lines_of(text);
    ReportedSide {
        start_line: span.0 as i64,
        end_line: span.1 as i64,
        content: lines[span.0..span.1].concat().into_bytes(),
    }
}

// ---------------------------------------------------------------------------
// FILE_LEVEL: the region is each side's whole file
// ---------------------------------------------------------------------------

/// One row of the `FILE_LEVEL` table: the history, the conflicting path, and the
/// text each side holds there — `None` where that side holds no text version at
/// all, which is a path with no lines to report.
struct FileLevelCase {
    case: &'static str,
    base: Vec<Change>,
    source: Vec<Change>,
    destination: Vec<Change>,
    path: &'static str,
    source_text: Option<&'static str>,
    destination_text: Option<&'static str>,
    base_text: Option<&'static str>,
}

fn file_level_cases() -> Vec<FileLevelCase> {
    vec![
        FileLevelCase {
            case: "both sides rewrote the one line the base held",
            base: vec![Change::Put("a.txt", "alpha\n")],
            source: vec![Change::Put("a.txt", "alpha, from the source\n")],
            destination: vec![Change::Put("a.txt", "alpha, from the destination\n")],
            path: "a.txt",
            source_text: Some("alpha, from the source\n"),
            destination_text: Some("alpha, from the destination\n"),
            base_text: Some("alpha\n"),
        },
        FileLevelCase {
            case: "neither side's file was in the base",
            base: vec![Change::Put("a.txt", "alpha\n")],
            source: vec![Change::Put("new.txt", "added by the source\n")],
            destination: vec![Change::Put("new.txt", "added by the destination\n")],
            path: "new.txt",
            source_text: Some("added by the source\n"),
            destination_text: Some("added by the destination\n"),
            // No version of the path in the merge base, so there is no base
            // region to report.
            base_text: None,
        },
        FileLevelCase {
            case: "the two sides hold different numbers of lines",
            base: vec![Change::Put("a.txt", "one\ntwo\nthree\n")],
            source: vec![Change::Put("a.txt", "ONE\nTWO\nTHREE\n")],
            destination: vec![Change::Put("a.txt", "only one line\n")],
            path: "a.txt",
            source_text: Some("ONE\nTWO\nTHREE\n"),
            destination_text: Some("only one line\n"),
            base_text: Some("one\ntwo\nthree\n"),
        },
        FileLevelCase {
            case: "a last line with no terminator",
            base: vec![Change::Put("a.txt", "alpha")],
            source: vec![Change::Put("a.txt", "alpha, from the source")],
            destination: vec![Change::Put("a.txt", "alpha, from the destination")],
            path: "a.txt",
            source_text: Some("alpha, from the source"),
            destination_text: Some("alpha, from the destination"),
            base_text: Some("alpha"),
        },
        FileLevelCase {
            case: "a file one side deleted and the other changed",
            base: vec![Change::Put("a.txt", "alpha\n")],
            source: vec![Change::Delete("a.txt")],
            destination: vec![Change::Put("a.txt", "alpha, from the destination\n")],
            path: "a.txt",
            // The source holds no file at all, so the two sides have no two text
            // versions to report regions of.
            source_text: None,
            destination_text: Some("alpha, from the destination\n"),
            base_text: Some("alpha\n"),
        },
        FileLevelCase {
            case: "a binary file both sides changed",
            base: vec![Change::Put("bin.dat", BINARY)],
            source: vec![Change::Put("bin.dat", "\u{0}from the source")],
            destination: vec![Change::Put("bin.dat", "\u{0}from the destination\u{0}")],
            path: "bin.dat",
            // Binary content has no lines to compare at all.
            source_text: None,
            destination_text: None,
            base_text: Some(BINARY),
        },
    ]
}

#[tokio::test]
async fn file_level_hunks_span_each_side_whole_file() {
    let router = emulator();
    for (index, case) in file_level_cases().into_iter().enumerate() {
        let label = case.case;
        let repository = format!("file-level-{index}");
        let built = fixture(
            &router,
            &repository,
            &case.base,
            &case.source,
            &case.destination,
        )
        .await;

        let payload = ok(
            &router,
            "BatchDescribeMergeConflicts",
            &request(&repository, THREE_WAY_MERGE, Some(FILE_LEVEL)),
        )
        .await;

        assert_eq!(
            string_at(&payload, "/sourceCommitId"),
            built.source,
            "case {label:?}: expected the source branch's tip, got {payload}"
        );
        assert_eq!(
            string_at(&payload, "/destinationCommitId"),
            built.destination,
            "case {label:?}: expected the destination branch's tip, got {payload}"
        );
        assert_eq!(
            string_at(&payload, "/baseCommitId"),
            built.base,
            "case {label:?}: expected the merge base, got {payload}"
        );

        let conflict = conflict_for(&payload, case.path);
        let hunks = reported_hunks(&conflict);
        let (Some(source_text), Some(destination_text)) =
            (case.source_text, case.destination_text)
        else {
            assert!(
                hunks.is_empty(),
                "case {label:?}: a path with no two text versions has no line regions to \
                 report, got {hunks:?}"
            );
            continue;
        };

        assert_eq!(
            hunks.len(),
            1,
            "case {label:?}: FILE_LEVEL detail judges the file as a whole, so it reports one \
             region, got {hunks:?}"
        );
        let hunk = &hunks[0];
        assert!(
            is_conflict(hunk),
            "case {label:?}: both sides changed the file's content, so the region is a \
             conflict, got {hunk}"
        );
        let whole = |text: &str| expected_side(text, (0, lines_of(text).len()));
        assert_eq!(
            reported_side(hunk, "source"),
            Some(whole(source_text)),
            "case {label:?}: expected the source's whole file as its region, got {hunk}"
        );
        assert_eq!(
            reported_side(hunk, "destination"),
            Some(whole(destination_text)),
            "case {label:?}: expected the destination's whole file as its region, got {hunk}"
        );
        assert_eq!(
            reported_side(hunk, "base"),
            case.base_text.map(whole),
            "case {label:?}: expected the merge base's whole file as its region, got {hunk}"
        );
    }
}

// ---------------------------------------------------------------------------
// LINE_LEVEL: the regions the two sides' changes group into
// ---------------------------------------------------------------------------

/// One hunk a `LINE_LEVEL` row must report: the half-open line span in each side
/// (`None` for a base that holds no version) and whether it is a conflict.
struct ExpectedHunk {
    base: Option<(usize, usize)>,
    source: (usize, usize),
    destination: (usize, usize),
    is_conflict: bool,
}

/// One row of the `LINE_LEVEL` table: what each side did to the seven-line base
/// file, and the regions the report must carry. An empty `hunks` is a merge the
/// two sides' lines reconcile, which has no conflicting path to describe at all.
struct LineLevelCase {
    case: &'static str,
    source_text: &'static str,
    destination_text: &'static str,
    hunks: Vec<ExpectedHunk>,
}

fn line_level_cases() -> Vec<LineLevelCase> {
    vec![
        LineLevelCase {
            case: "the two sides changed the same line",
            source_text: "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            destination_text: "uno\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            hunks: vec![ExpectedHunk {
                base: Some((0, 1)),
                source: (0, 1),
                destination: (0, 1),
                is_conflict: true,
            }],
        },
        LineLevelCase {
            case: "a line only one side changed, beside a line they both changed",
            source_text: "ONE\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n",
            destination_text: "uno\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            hunks: vec![
                ExpectedHunk {
                    base: Some((0, 1)),
                    source: (0, 1),
                    destination: (0, 1),
                    is_conflict: true,
                },
                ExpectedHunk {
                    base: Some((6, 7)),
                    source: (6, 7),
                    destination: (6, 7),
                    // Only the source moved these lines, so they merge.
                    is_conflict: false,
                },
            ],
        },
        LineLevelCase {
            case: "lines one side inserted above the line they both changed",
            source_text: "zero\nhalf\none\ntwo\nthree\nFOUR\nfive\nsix\nseven\n",
            destination_text: "one\ntwo\nthree\nfour and a half\nfive\nsix\nseven\n",
            hunks: vec![
                ExpectedHunk {
                    // An insertion sits between two base lines, so it covers no
                    // base line at all and none of the destination's.
                    base: Some((0, 0)),
                    source: (0, 2),
                    destination: (0, 0),
                    is_conflict: false,
                },
                ExpectedHunk {
                    base: Some((3, 4)),
                    // The two lines the source inserted move its region down.
                    source: (5, 6),
                    destination: (3, 4),
                    is_conflict: true,
                },
            ],
        },
        LineLevelCase {
            case: "each side changed a line the other left alone",
            source_text: "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            destination_text: "one\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n",
            // Lines that do not collide merge, so the merge has no conflict and
            // nothing to describe.
            hunks: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn line_level_hunks_are_the_regions_the_two_sides_changed() {
    let router = emulator();
    for (index, case) in line_level_cases().into_iter().enumerate() {
        let label = case.case;
        let repository = format!("line-level-{index}");
        fixture(
            &router,
            &repository,
            &[Change::Put("multi.txt", SEVEN_LINES)],
            &[Change::Put("multi.txt", case.source_text)],
            &[Change::Put("multi.txt", case.destination_text)],
        )
        .await;

        let payload = ok(
            &router,
            "BatchDescribeMergeConflicts",
            &request(&repository, SQUASH_MERGE, Some(LINE_LEVEL)),
        )
        .await;

        if case.hunks.is_empty() {
            assert!(
                described_paths(&payload).is_empty(),
                "case {label:?}: lines that do not collide merge, so no path is described, \
                 got {payload}"
            );
            continue;
        }

        let conflict = conflict_for(&payload, "multi.txt");
        let reported = reported_hunks(&conflict);
        assert_eq!(
            reported.len(),
            case.hunks.len(),
            "case {label:?}: expected {} region(s), got {reported:?}",
            case.hunks.len()
        );
        for (position, (hunk, expected)) in reported.iter().zip(&case.hunks).enumerate() {
            assert_eq!(
                reported_side(hunk, "source"),
                Some(expected_side(case.source_text, expected.source)),
                "case {label:?}: region {position}: expected the source's lines \
                 {:?}, got {hunk}",
                expected.source
            );
            assert_eq!(
                reported_side(hunk, "destination"),
                Some(expected_side(case.destination_text, expected.destination)),
                "case {label:?}: region {position}: expected the destination's lines \
                 {:?}, got {hunk}",
                expected.destination
            );
            assert_eq!(
                reported_side(hunk, "base"),
                expected.base.map(|span| expected_side(SEVEN_LINES, span)),
                "case {label:?}: region {position}: expected the base's lines {:?}, got {hunk}",
                expected.base
            );
            assert_eq!(
                is_conflict(hunk),
                expected.is_conflict,
                "case {label:?}: region {position}: expected isConflict {}, got {hunk}",
                expected.is_conflict
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Which paths are described, and which are errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_paths_pick_the_described_paths_and_the_rest_are_errors() {
    let router = emulator();
    let repository = "selected-paths";
    fixture(
        &router,
        repository,
        &[
            Change::Put("a.txt", "alpha\n"),
            Change::Put("dir/b.txt", "bee\n"),
            Change::Put("quiet.txt", "untouched\n"),
        ],
        &[
            Change::Put("a.txt", "alpha, from the source\n"),
            Change::Put("dir/b.txt", "bee, from the source\n"),
        ],
        &[
            Change::Put("a.txt", "alpha, from the destination\n"),
            Change::Put("dir/b.txt", "bee, from the destination\n"),
        ],
    )
    .await;

    // Every conflicting path, where the request names none.
    let all = ok(
        &router,
        "BatchDescribeMergeConflicts",
        &request(repository, THREE_WAY_MERGE, None),
    )
    .await;
    assert_eq!(
        described_paths(&all),
        vec!["a.txt".to_string(), "dir/b.txt".to_string()],
        "case \"no filePaths\": expected every conflicting path, got {all}"
    );
    assert_eq!(
        all.get("errors"),
        Some(&json!([])),
        "case \"no filePaths\": the errors list is reported for every call, and is empty \
         where nothing was asked for that could not be described, got {all}"
    );

    // One conflicting path, and four the report cannot describe: a path that
    // conflicts nowhere, a path no side holds, a path that addresses no file, and
    // an empty path.
    let mut selective = request(repository, THREE_WAY_MERGE, None);
    selective["filePaths"] = json!([
        "dir/b.txt",
        "quiet.txt",
        "nowhere.txt",
        "dir/../b.txt",
        "",
    ]);
    let payload = ok(&router, "BatchDescribeMergeConflicts", &selective).await;
    assert_eq!(
        described_paths(&payload),
        vec!["dir/b.txt".to_string()],
        "case \"filePaths\": expected only the conflicting path that was asked for, got \
         {payload}"
    );
    let errors: Vec<(String, String, String)> = payload
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an errors list in {payload}"))
        .iter()
        .map(|entry| {
            (
                string_at(entry, "/filePath"),
                string_at(entry, "/exceptionName"),
                string_at(entry, "/message"),
            )
        })
        .collect();
    assert_eq!(
        errors,
        vec![
            (
                "quiet.txt".to_string(),
                "FileDoesNotExistException".to_string(),
                "The specified file does not exist. Verify that you have used the correct \
                 file name, full path, and extension."
                    .to_string(),
            ),
            (
                "nowhere.txt".to_string(),
                "FileDoesNotExistException".to_string(),
                "The specified file does not exist. Verify that you have used the correct \
                 file name, full path, and extension."
                    .to_string(),
            ),
            (
                "dir/../b.txt".to_string(),
                "InvalidPathException".to_string(),
                "The specified path is not valid.".to_string(),
            ),
            (
                "".to_string(),
                "PathRequiredException".to_string(),
                "The folderPath for a location cannot be null.".to_string(),
            ),
        ],
        "case \"filePaths\": expected one error per path that could not be described, got \
         {payload}"
    );
}

// ---------------------------------------------------------------------------
// Agreement with the per-path reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_metadata_agrees_with_the_per_path_conflict_reads() {
    let router = emulator();
    let repository = "agreeing-reads";
    for detail_level in [FILE_LEVEL, LINE_LEVEL] {
        let repository = format!("{repository}-{}", detail_level.to_lowercase());
        fixture(
            &router,
            &repository,
            &[
                Change::Put("a.txt", "alpha\n"),
                Change::Put("dir/b.txt", "bee\n"),
                Change::Put("run.sh", "#!/bin/sh\necho hi\n"),
            ],
            &[
                Change::Put("a.txt", "alpha, from the source\n"),
                Change::Delete("dir/b.txt"),
                Change::PutMode("run.sh", "#!/bin/sh\necho source\n", NORMAL),
            ],
            &[
                Change::Put("a.txt", "alpha, from the destination\n"),
                Change::Put("dir/b.txt", "bee, from the destination\n"),
                Change::Put("run.sh", "#!/bin/sh\necho destination\n"),
            ],
        )
        .await;

        let batch = ok(
            &router,
            "BatchDescribeMergeConflicts",
            &request(&repository, THREE_WAY_MERGE, Some(detail_level)),
        )
        .await;
        let per_path = ok(
            &router,
            "GetMergeConflicts",
            &request(&repository, THREE_WAY_MERGE, Some(detail_level)),
        )
        .await;

        let read_metadata: Vec<Value> = per_path
            .get("conflictMetadataList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected a conflictMetadataList in {per_path}"))
            .clone();
        let described: Vec<Value> = conflicts(&batch)
            .iter()
            .map(|conflict| {
                conflict
                    .get("conflictMetadata")
                    .cloned()
                    .unwrap_or_else(|| panic!("expected conflictMetadata in {conflict}"))
            })
            .collect();
        assert_eq!(
            described, read_metadata,
            "case {detail_level:?}: the two reports describe the same conflicting paths, got \
             {batch} and {per_path}"
        );
        for member in ["sourceCommitId", "destinationCommitId", "baseCommitId"] {
            assert_eq!(
                batch.get(member),
                per_path.get(member),
                "case {detail_level:?}: expected the same {member} from both reads, got \
                 {batch} and {per_path}"
            );
        }
    }
}

#[tokio::test]
async fn the_per_path_read_describes_exactly_what_the_batch_describes() {
    let router = emulator();
    for detail_level in [FILE_LEVEL, LINE_LEVEL] {
        let repository = format!("per-path-{}", detail_level.to_lowercase());
        fixture(
            &router,
            &repository,
            &[
                Change::Put("a.txt", "alpha\n"),
                Change::Put("multi.txt", SEVEN_LINES),
                Change::Put("quiet.txt", "untouched\n"),
            ],
            &[
                Change::Put("a.txt", "alpha, from the source\n"),
                Change::Put("multi.txt", "ONE\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n"),
            ],
            &[
                Change::Put("a.txt", "alpha, from the destination\n"),
                Change::Put("multi.txt", "uno\ntwo\nthree\nfour\nfive\nsix\nseven\n"),
            ],
        )
        .await;

        let batch = ok(
            &router,
            "BatchDescribeMergeConflicts",
            &request(&repository, THREE_WAY_MERGE, Some(detail_level)),
        )
        .await;
        for path in described_paths(&batch) {
            let mut input = request(&repository, THREE_WAY_MERGE, Some(detail_level));
            input["filePath"] = json!(path);
            let one = ok(&router, "DescribeMergeConflicts", &input).await;
            let described = conflict_for(&batch, &path);
            assert_eq!(
                one.get("conflictMetadata"),
                described.get("conflictMetadata"),
                "case {detail_level:?}: the two reads describe {path:?} the same way, got \
                 {one} and {described}"
            );
            assert_eq!(
                one.get("mergeHunks"),
                described.get("mergeHunks"),
                "case {detail_level:?}: the two reads report the same hunks for {path:?}, got \
                 {one} and {described}"
            );
            for member in ["sourceCommitId", "destinationCommitId", "baseCommitId"] {
                assert_eq!(
                    one.get(member),
                    batch.get(member),
                    "case {detail_level:?}: expected the same {member} from both reads, got \
                     {one} and {batch}"
                );
            }
        }

        // `maxMergeHunks` is this read's page size, so its token resumes after the
        // last hunk reported; the pages together are the whole list.
        let mut input = request(&repository, THREE_WAY_MERGE, Some(LINE_LEVEL));
        input["filePath"] = json!("multi.txt");
        let whole = ok(&router, "DescribeMergeConflicts", &input).await;
        let all_hunks = whole
            .get("mergeHunks")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected a mergeHunks list in {whole}"))
            .clone();
        let mut paged: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut page_input = input.clone();
            page_input["conflictDetailLevel"] = json!(LINE_LEVEL);
            page_input["maxMergeHunks"] = json!(1);
            if let Some(token) = &token {
                page_input["nextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeMergeConflicts", &page_input).await;
            let hunks = page
                .get("mergeHunks")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("expected a mergeHunks list in {page}"))
                .clone();
            assert_eq!(
                hunks.len(),
                1,
                "case \"one hunk per page\": expected one hunk per page, got {page}"
            );
            paged.extend(hunks);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            paged, all_hunks,
            "case \"one hunk per page\": the pages together are the whole hunk list, got \
             {paged:?} and {all_hunks:?}"
        );

        // A path this merge has no conflict at has no conflict to describe.
        for (case, path, code) in [
            ("a path that conflicts nowhere", "quiet.txt", "FileDoesNotExistException"),
            ("a path no side holds", "nowhere.txt", "FileDoesNotExistException"),
            ("a path that addresses no file", "dir/../b.txt", "InvalidPathException"),
            ("an empty path", "", "PathRequiredException"),
        ] {
            let mut input = request(&repository, THREE_WAY_MERGE, Some(detail_level));
            input["filePath"] = json!(path);
            let answered = error(&router, "DescribeMergeConflicts", &input, 400).await;
            assert_eq!(
                answered, code,
                "case {case:?}: expected {code}, got {answered} for {input}"
            );
        }

        // "If the merge option for the attempted merge is specified as
        // FAST_FORWARD_MERGE, an exception is thrown."
        let mut fast_forward = request(&repository, FAST_FORWARD_MERGE, Some(detail_level));
        fast_forward["filePath"] = json!("a.txt");
        let answered = error(&router, "DescribeMergeConflicts", &fast_forward, 400).await;
        assert_eq!(
            answered, "InvalidMergeOptionException",
            "case \"a fast-forward is not a strategy this read describes conflicts for\": got \
             {answered}"
        );
    }
}

#[tokio::test]
async fn describing_a_merge_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let repository = "unchanged-by-reading";
    fixture(
        &router,
        repository,
        &[Change::Put("a.txt", "alpha\n")],
        &[Change::Put("a.txt", "alpha, from the source\n")],
        &[Change::Put("a.txt", "alpha, from the destination\n")],
    )
    .await;
    let read_file = json!({
        "repositoryName": repository,
        "commitSpecifier": DESTINATION_BRANCH,
        "filePath": "a.txt",
    });

    let before = ok(&router, "GetFile", &read_file).await;
    let first = ok(
        &router,
        "BatchDescribeMergeConflicts",
        &request(repository, THREE_WAY_MERGE, Some(LINE_LEVEL)),
    )
    .await;
    let second = ok(
        &router,
        "BatchDescribeMergeConflicts",
        &request(repository, THREE_WAY_MERGE, Some(LINE_LEVEL)),
    )
    .await;
    let after = ok(&router, "GetFile", &read_file).await;

    assert_eq!(
        first, second,
        "case \"read twice\": the same request must answer the same way, got {first} and \
         {second}"
    );
    assert_eq!(
        before, after,
        "case \"read twice\": describing a merge must not change what a later read of the \
         file returns, got {before} and {after}"
    );
}

// ---------------------------------------------------------------------------
// Paging the paths, and bounding the hunks per path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_conflict_files_pages_the_paths_and_max_merge_hunks_bounds_each_path() {
    let router = emulator();
    let repository = "paged-conflicts";
    fixture(
        &router,
        repository,
        &[
            Change::Put("a.txt", "alpha\n"),
            Change::Put("b.txt", "bee\n"),
            Change::Put("multi.txt", SEVEN_LINES),
        ],
        &[
            Change::Put("a.txt", "alpha, from the source\n"),
            Change::Put("b.txt", "bee, from the source\n"),
            Change::Put("multi.txt", "ONE\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n"),
        ],
        &[
            Change::Put("a.txt", "alpha, from the destination\n"),
            Change::Put("b.txt", "bee, from the destination\n"),
            Change::Put("multi.txt", "uno\ntwo\nthree\nfour\nfive\nsix\nseven\n"),
        ],
    )
    .await;

    // One path per page: the pages together are the whole list, in order, with no
    // path reported twice and a token only while a further page exists.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..3 {
        let mut input = request(repository, THREE_WAY_MERGE, Some(LINE_LEVEL));
        input["maxConflictFiles"] = json!(1);
        if let Some(token) = &token {
            input["nextToken"] = json!(token);
        }
        let payload = ok(&router, "BatchDescribeMergeConflicts", &input).await;
        let paths = described_paths(&payload);
        assert_eq!(
            paths.len(),
            1,
            "case \"page {page}\": expected one path per page, got {payload}"
        );
        seen.extend(paths);
        token = payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            token.is_some(),
            page < 2,
            "case \"page {page}\": a token is issued only while a further page exists, got \
             {payload}"
        );
    }
    assert_eq!(
        seen,
        vec![
            "a.txt".to_string(),
            "b.txt".to_string(),
            "multi.txt".to_string()
        ],
        "case \"paging\": expected every conflicting path exactly once, got {seen:?}"
    );
    assert_eq!(
        seen.iter().collect::<BTreeSet<_>>().len(),
        seen.len(),
        "case \"paging\": no path may be reported twice, got {seen:?}"
    );

    // `multi.txt` has two regions at LINE_LEVEL; `maxMergeHunks` bounds how many
    // of them each described path carries.
    for (bound, expected) in [(1usize, 1usize), (2, 2), (50, 2)] {
        let mut input = request(repository, THREE_WAY_MERGE, Some(LINE_LEVEL));
        input["filePaths"] = json!(["multi.txt"]);
        input["maxMergeHunks"] = json!(bound);
        let payload = ok(&router, "BatchDescribeMergeConflicts", &input).await;
        let hunks = reported_hunks(&conflict_for(&payload, "multi.txt"));
        assert_eq!(
            hunks.len(),
            expected,
            "case \"maxMergeHunks {bound}\": expected {expected} region(s), got {hunks:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One refusal row: the member the request carries, and the modelled error code.
struct RefusalCase {
    case: &'static str,
    input: Value,
    code: &'static str,
}

#[tokio::test]
async fn a_request_the_report_cannot_answer_is_refused_with_its_modelled_error() {
    let router = emulator();
    let repository = "refused-reports";
    fixture(
        &router,
        repository,
        &[Change::Put("a.txt", "alpha\n")],
        &[Change::Put("a.txt", "alpha, from the source\n")],
        &[Change::Put("a.txt", "alpha, from the destination\n")],
    )
    .await;

    let with = |member: &str, value: Value| {
        let mut input = request(repository, THREE_WAY_MERGE, None);
        input[member] = value;
        input
    };
    let cases = vec![
        RefusalCase {
            case: "a fast-forward is not a strategy this report describes conflicts for",
            input: request(repository, FAST_FORWARD_MERGE, None),
            code: "InvalidMergeOptionException",
        },
        RefusalCase {
            case: "no repository of that name",
            input: request("absent-repository", THREE_WAY_MERGE, None),
            code: "RepositoryDoesNotExistException",
        },
        RefusalCase {
            case: "a specifier that names no commit the repository holds",
            input: with("sourceCommitSpecifier", json!("no-such-branch")),
            code: "CommitDoesNotExistException",
        },
        RefusalCase {
            case: "a specifier no reference name could be",
            input: with("destinationCommitSpecifier", json!("a branch?")),
            code: "InvalidCommitException",
        },
        RefusalCase {
            case: "a page size outside the published range",
            input: with("maxConflictFiles", json!(0)),
            code: "InvalidMaxConflictFilesException",
        },
        RefusalCase {
            case: "a hunk bound outside the published range",
            input: with("maxMergeHunks", json!(0)),
            code: "InvalidMaxMergeHunksException",
        },
        RefusalCase {
            case: "a continuation token this service did not issue",
            input: with("nextToken", json!("not-a-token")),
            code: "InvalidContinuationTokenException",
        },
        RefusalCase {
            case: "a conflict detail level outside the enum",
            input: with("conflictDetailLevel", json!("WORD_LEVEL")),
            code: "InvalidConflictDetailLevelException",
        },
        RefusalCase {
            case: "a merge option outside the enum",
            input: with("mergeOption", json!("CHERRY_PICK")),
            code: "InvalidMergeOptionException",
        },
    ];
    for RefusalCase { case, input, code } in cases {
        let answered = error(&router, "BatchDescribeMergeConflicts", &input, 400).await;
        assert_eq!(
            answered, code,
            "case {case:?}: expected {code}, got {answered} for {input}"
        );
    }
}
