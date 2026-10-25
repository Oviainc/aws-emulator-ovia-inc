// SPDX-License-Identifier: CC0-1.0

//! `GetMergeConflicts` over the wire, driven as awsJson1_1 requests through the router
//! that serves the real emulator.
//!
//! The rule under test: `GetMergeConflicts` evaluates merging the source commit into the
//! destination commit against their merge base — the nearest common ancestor of the two
//! in stored state — and reports one `ConflictMetadata` per path BOTH sides changed away
//! from that base in differing ways, with `mergeable` true exactly when no such path
//! remains. `conflictDetailLevel` decides how a file both sides edited is judged
//! (`FILE_LEVEL`, the default, conflicts on any difference; `LINE_LEVEL` only where the
//! two sides' hunks collide), `conflictResolutionStrategy` decides which conflicts a
//! merge settles on its own (`ACCEPT_SOURCE`/`ACCEPT_DESTINATION` every one,
//! `AUTOMERGE` a text file whose hunks do not collide, `NONE` none), and
//! `FAST_FORWARD_MERGE` asks only whether the destination is already in the source's
//! history — its conflict list is always empty.
//!
//! The tables below span that class rather than one recorded request: which side changed
//! a path and how (content, mode, object type, deletion, addition), whether the two
//! changes agree, how far apart the two sides' edits sit in the file, whether its content
//! can be merged line by line at all, the detail level, the resolution strategy, the
//! merge option, the commit specifier form, and the page size.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The bounds the shared merge evaluation (`crate::state::merge`) publishes, imported
// rather than copied: `MAX_MERGE_FILE_BYTES` is the largest ONE version of a file it
// merges line by line and `MAX_MERGE_FILE_LINES` the most lines such a version may hold.
// Content past either is not loaded at all — the request is refused with the modelled
// `MaximumFileContentToLoadExceededException` — because a three-way line merge costs more
// than its input's size and grows with its line count.
use service_com_amazonaws_codecommit_codecommit_20150413::state::merge::{
    MAX_MERGE_FILE_BYTES, MAX_MERGE_FILE_LINES,
};

// The awsJson1_1 wire harness lives once for this test crate in the suite that first
// needed it.
use crate::repository_lifecycle::{base64, emulator, error, ok, string_at};

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

/// `conflictResolutionStrategy` values, from
/// `smithy_query(query="shape",shape="ConflictResolutionStrategyTypeEnum")`.
const NONE: &str = "NONE";
const ACCEPT_SOURCE: &str = "ACCEPT_SOURCE";
const ACCEPT_DESTINATION: &str = "ACCEPT_DESTINATION";
const AUTOMERGE: &str = "AUTOMERGE";

/// `fileMode` values, from `smithy_query(query="shape",shape="FileModeTypeEnum")`.
const NORMAL: &str = "NORMAL";
const EXECUTABLE: &str = "EXECUTABLE";
const SYMLINK: &str = "SYMLINK";

const ALPHA: &[u8] = b"alpha\n";
const ALPHA_FROM_SOURCE: &[u8] = b"alpha, from the source\n";
const ALPHA_FROM_DESTINATION: &[u8] = b"alpha, from the destination\n";
const BEE: &[u8] = b"bee\n";
const BEE_FROM_SOURCE: &[u8] = b"bee, from the source\n";
const BEE_FROM_DESTINATION: &[u8] = b"bee, from the destination\n";
const SCRIPT: &[u8] = b"#!/bin/sh\necho hi\n";
const SCRIPT_FROM_SOURCE: &[u8] = b"#!/bin/sh\necho source\n";
const SCRIPT_FROM_DESTINATION: &[u8] = b"#!/bin/sh\necho destination\n";
/// What the base tree's symlink points at: its content, as a symlink's bytes are.
const LINK_TARGET: &[u8] = b"a.txt";
/// Bytes with a NUL in them: binary content, which has no lines to merge.
const BINARY: &[u8] = b"\x00\x01\x02gif\x00\xff";
const BINARY_FROM_SOURCE: &[u8] = b"\x00source";
const BINARY_FROM_DESTINATION: &[u8] = b"\x00destination\x00";
/// A file long enough for the two sides to edit different parts of it.
const SEVEN_LINES: &[u8] = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
const FIRST_LINE_CHANGED: &[u8] = b"ONE\ntwo\nthree\nfour\nfive\nsix\nseven\n";
const LAST_LINE_CHANGED: &[u8] = b"one\ntwo\nthree\nfour\nfive\nsix\nSEVEN\n";
const MIDDLE_LINE_CHANGED_ONE_WAY: &[u8] = b"one\ntwo\nthree\nFOUR\nfive\nsix\nseven\n";
const MIDDLE_LINE_CHANGED_ANOTHER_WAY: &[u8] = b"one\ntwo\nthree\nfour and a half\nfive\nsix\nseven\n";

/// One change a commit carries.
#[derive(Debug, Clone, Copy)]
enum Change<'a> {
    /// Write `path` with this content at this modelled `fileMode`.
    Put(&'a str, &'a [u8], &'a str),
    /// Remove `path` from the tree.
    Delete(&'a str),
    /// Change only `path`'s mode.
    SetMode(&'a str, &'a str),
}

/// The three commits one scenario sets up: the merge base, and the tip of each side.
#[derive(Debug, Clone)]
struct Merge {
    base: String,
    source: String,
    destination: String,
}

/// Commit `changes` on `branch`, and answer the new commit id. `CreateCommit` is the
/// only implemented write that can delete a path and set a mode, which is what lets a
/// scenario span every change kind the rule covers.
async fn commit(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    parent: Option<&str>,
    changes: &[Change<'_>],
) -> String {
    let mut puts: Vec<Value> = Vec::new();
    let mut deletes: Vec<Value> = Vec::new();
    let mut modes: Vec<Value> = Vec::new();
    for change in changes {
        match *change {
            Change::Put(path, content, mode) => puts.push(json!({
                "filePath": path,
                "fileContent": base64(content),
                "fileMode": mode,
            })),
            Change::Delete(path) => deletes.push(json!({ "filePath": path })),
            Change::SetMode(path, mode) => {
                modes.push(json!({ "filePath": path, "fileMode": mode }));
            }
        }
    }
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "commitMessage": format!("a commit on {branch}"),
        "authorName": "Ada Lovelace",
        "email": "ada@example.com",
    });
    if !puts.is_empty() {
        input["putFiles"] = Value::Array(puts);
    }
    if !deletes.is_empty() {
        input["deleteFiles"] = Value::Array(deletes);
    }
    if !modes.is_empty() {
        input["setFileModes"] = Value::Array(modes);
    }
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    let created = ok(router, "CreateCommit", &input).await;
    string_at(&created, "/commitId")
}

/// Build a repository whose two branches diverge from one base commit: `base` is the
/// commit both sides share, and each of `source`/`destination` is a LIST of commits made
/// on that side, so a side can take several steps.
async fn scenario(
    router: &MultiServiceRouter,
    repository: &str,
    base: &[Change<'_>],
    source: &[&[Change<'_>]],
    destination: &[&[Change<'_>]],
) -> Merge {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base_commit = commit(router, repository, DESTINATION_BRANCH, None, base).await;
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
    let mut destination_tip = base_commit.clone();
    for changes in destination {
        destination_tip = commit(
            router,
            repository,
            DESTINATION_BRANCH,
            Some(&destination_tip),
            changes,
        )
        .await;
    }
    let mut source_tip = base_commit.clone();
    for changes in source {
        source_tip = commit(router, repository, SOURCE_BRANCH, Some(&source_tip), changes).await;
    }
    Merge {
        base: base_commit,
        source: source_tip,
        destination: destination_tip,
    }
}

/// The base commit every scenario below starts from: a text file, a file in a folder, an
/// executable, a symlink and a binary blob, so each change kind has something to act on.
fn base_tree() -> Vec<Change<'static>> {
    vec![
        Change::Put("a.txt", ALPHA, NORMAL),
        Change::Put("dir/b.txt", BEE, NORMAL),
        Change::Put("run.sh", SCRIPT, EXECUTABLE),
        Change::Put("link", LINK_TARGET, SYMLINK),
        Change::Put("bin.dat", BINARY, NORMAL),
        Change::Put("multi.txt", SEVEN_LINES, NORMAL),
    ]
}

/// A `GetMergeConflicts` request naming the two branches, with only the members a case
/// names.
fn request(
    repository: &str,
    merge_option: &str,
    detail_level: Option<&str>,
    strategy: Option<&str>,
) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "sourceCommitSpecifier": SOURCE_BRANCH,
        "destinationCommitSpecifier": DESTINATION_BRANCH,
        "mergeOption": merge_option,
    });
    if let Some(detail_level) = detail_level {
        input["conflictDetailLevel"] = json!(detail_level);
    }
    if let Some(strategy) = strategy {
        input["conflictResolutionStrategy"] = json!(strategy);
    }
    input
}

/// The `filePath` of every conflict in one response, in the order the service returned
/// them.
fn conflicting_paths(payload: &Value) -> Vec<String> {
    payload
        .get("conflictMetadataList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a conflictMetadataList in {payload}"))
        .iter()
        .map(|conflict| {
            conflict
                .get("filePath")
                .and_then(Value::as_str)
                .unwrap_or("<none>")
                .to_string()
        })
        .collect()
}

fn mergeable(payload: &Value) -> bool {
    payload
        .get("mergeable")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("expected a mergeable flag in {payload}"))
}

/// The conflict one response reports for `path`.
fn conflict_for(payload: &Value, path: &str) -> Value {
    payload
        .get("conflictMetadataList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a conflictMetadataList in {payload}"))
        .iter()
        .find(|conflict| conflict.get("filePath").and_then(Value::as_str) == Some(path))
        .unwrap_or_else(|| panic!("no conflict for {path:?} in {payload}"))
        .clone()
}

/// One row of the conflict table: a label, the repository to build it in, the changes
/// each side makes, and the paths expected back as conflicting.
struct Case<'a> {
    label: &'a str,
    repository: &'a str,
    source: Vec<&'a [Change<'a>]>,
    destination: Vec<&'a [Change<'a>]>,
    conflicts: Vec<&'a str>,
}

#[tokio::test]
async fn conflicts_are_the_paths_both_sides_changed_in_differing_ways() {
    let router = emulator();

    // Rows differ along which side changed a path, how it changed it (content, mode,
    // object type, deletion, addition), whether the two changes agree, and how many
    // commits a side took to get there.
    let cases: Vec<Case<'_>> = vec![
        Case {
            label: "both sides change the same file differently",
            repository: "both-edit",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec!["a.txt"],
        },
        Case {
            label: "both sides make the identical change",
            repository: "same-edit",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            conflicts: vec![],
        },
        Case {
            label: "only the source changes a file",
            repository: "source-only",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![],
            conflicts: vec![],
        },
        Case {
            label: "only the destination changes a file",
            repository: "destination-only",
            source: vec![],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec![],
        },
        Case {
            label: "the two sides change different files",
            repository: "disjoint-edits",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL)]],
            conflicts: vec![],
        },
        Case {
            label: "both sides add the same new path with different content",
            repository: "both-add-differently",
            source: vec![&[Change::Put("new.txt", b"from the source\n", NORMAL)]],
            destination: vec![&[Change::Put("new.txt", b"from the destination\n", NORMAL)]],
            conflicts: vec!["new.txt"],
        },
        Case {
            label: "both sides add the same new path with the same content",
            repository: "both-add-alike",
            source: vec![&[Change::Put("new.txt", b"identical\n", NORMAL)]],
            destination: vec![&[Change::Put("new.txt", b"identical\n", NORMAL)]],
            conflicts: vec![],
        },
        Case {
            label: "the source deletes a file the destination changed",
            repository: "source-deletes",
            source: vec![&[Change::Delete("a.txt")]],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec!["a.txt"],
        },
        Case {
            label: "the destination deletes a file the source changed",
            repository: "destination-deletes",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Delete("a.txt")]],
            conflicts: vec!["a.txt"],
        },
        Case {
            label: "both sides delete the same file",
            repository: "both-delete",
            source: vec![&[Change::Delete("a.txt")]],
            destination: vec![&[Change::Delete("a.txt")]],
            conflicts: vec![],
        },
        Case {
            label: "both sides change only the file mode, differently",
            repository: "mode-clash",
            source: vec![&[Change::SetMode("link", NORMAL)]],
            destination: vec![&[Change::SetMode("link", EXECUTABLE)]],
            conflicts: vec!["link"],
        },
        Case {
            label: "both sides change only the file mode, the same way",
            repository: "mode-agreement",
            source: vec![&[Change::SetMode("link", NORMAL)]],
            destination: vec![&[Change::SetMode("link", NORMAL)]],
            conflicts: vec![],
        },
        Case {
            label: "a binary file changed on both sides",
            repository: "binary-clash",
            source: vec![&[Change::Put("bin.dat", BINARY_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Put("bin.dat", BINARY_FROM_DESTINATION, NORMAL)]],
            conflicts: vec!["bin.dat"],
        },
        Case {
            label: "the source replaces a file with a folder the destination changed",
            repository: "file-against-folder",
            source: vec![
                &[Change::Delete("a.txt")],
                &[Change::Put("a.txt/inner.txt", b"inside\n", NORMAL)],
            ],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec!["a.txt"],
        },
        Case {
            label: "several paths conflict at once, reported in path order",
            repository: "many-conflicts",
            source: vec![
                &[
                    Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL),
                    Change::Put("dir/b.txt", BEE_FROM_SOURCE, NORMAL),
                ],
                &[Change::Put("run.sh", SCRIPT_FROM_SOURCE, EXECUTABLE)],
            ],
            destination: vec![&[
                Change::Put("run.sh", SCRIPT_FROM_DESTINATION, EXECUTABLE),
                Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL),
                Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL),
            ]],
            conflicts: vec!["a.txt", "dir/b.txt", "run.sh"],
        },
        Case {
            label: "a folder replacing a file, with a sibling path sorting between the two",
            repository: "folder-with-close-sibling",
            // "a.txt.bak" sorts AFTER "a.txt" and BEFORE "a.txt/inner.txt" ('.' is 0x2e,
            // '/' is 0x2f), so the folder below "a.txt" is only seen by a walk that looks
            // past the nearest following path rather than at it alone.
            source: vec![
                &[
                    Change::Delete("a.txt"),
                    Change::Put("a.txt.bak", ALPHA, NORMAL),
                ],
                &[Change::Put("a.txt/inner.txt", b"inside\n", NORMAL)],
            ],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec!["a.txt"],
        },
        Case {
            label: "neither side moved from the base",
            repository: "no-divergence",
            source: vec![],
            destination: vec![],
            conflicts: vec![],
        },
        Case {
            label: "the source is only ahead of the destination",
            repository: "source-ahead",
            source: vec![
                &[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)],
                &[Change::Put("new.txt", b"and another\n", NORMAL)],
            ],
            destination: vec![],
            conflicts: vec![],
        },
    ];

    for case in cases {
        let label = case.label;
        let merge = scenario(
            &router,
            case.repository,
            &base_tree(),
            &case.source,
            &case.destination,
        )
        .await;
        let input = request(case.repository, THREE_WAY_MERGE, None, None);
        let payload = ok(&router, "GetMergeConflicts", &input).await;

        let got = conflicting_paths(&payload);
        let expected: Vec<String> = case.conflicts.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            got, expected,
            "case {label:?}: expected conflicts {expected:?}, got {got:?}"
        );
        assert_eq!(
            mergeable(&payload),
            expected.is_empty(),
            "case {label:?}: mergeable must be true exactly when nothing conflicts, got {payload}"
        );
        assert_eq!(
            string_at(&payload, "/baseCommitId"),
            merge.base,
            "case {label:?}: baseCommitId must be the merge base of the two tips"
        );
        assert_eq!(
            string_at(&payload, "/sourceCommitId"),
            merge.source,
            "case {label:?}: sourceCommitId must be the commit the source specifier resolved to"
        );
        assert_eq!(
            string_at(&payload, "/destinationCommitId"),
            merge.destination,
            "case {label:?}: destinationCommitId must be the commit the destination specifier \
             resolved to"
        );
        assert!(
            payload.get("nextToken").is_none_or(Value::is_null),
            "case {label:?}: a single unpaged page must not carry a continuation token, got \
             {payload}"
        );

        // A read never changes what the next caller sees.
        let again = ok(&router, "GetMergeConflicts", &input).await;
        assert_eq!(
            again, payload,
            "case {label:?}: GetMergeConflicts must be repeatable"
        );
        let tip = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": case.repository, "branchName": SOURCE_BRANCH }),
        )
        .await;
        assert_eq!(
            string_at(&tip, "/branch/commitId"),
            merge.source,
            "case {label:?}: evaluating a merge must not move a branch"
        );
    }
}

/// One detail-level row: a label, the repository, the content each side writes to
/// `multi.txt`, and whether the path conflicts at `FILE_LEVEL` and at `LINE_LEVEL`.
struct DetailCase<'a> {
    label: &'a str,
    repository: &'a str,
    source: &'a [u8],
    destination: &'a [u8],
    conflicts_at_file_level: bool,
    conflicts_at_line_level: bool,
}

#[tokio::test]
async fn the_conflict_detail_level_decides_how_a_file_both_sides_edited_is_judged() {
    let router = emulator();

    // Rows differ along where in the file the two sides edited: the same line, lines far
    // apart, opposite ends, and the same end.
    let cases: Vec<DetailCase<'_>> = vec![
        DetailCase {
            label: "both sides change the same line",
            repository: "same-line",
            source: MIDDLE_LINE_CHANGED_ONE_WAY,
            destination: MIDDLE_LINE_CHANGED_ANOTHER_WAY,
            conflicts_at_file_level: true,
            conflicts_at_line_level: true,
        },
        DetailCase {
            label: "the two sides change lines at opposite ends of the file",
            repository: "far-apart",
            source: FIRST_LINE_CHANGED,
            destination: LAST_LINE_CHANGED,
            conflicts_at_file_level: true,
            conflicts_at_line_level: false,
        },
        DetailCase {
            label: "one side prepends and the other appends",
            repository: "prepend-append",
            source: b"zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            destination: b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n",
            conflicts_at_file_level: true,
            conflicts_at_line_level: false,
        },
        DetailCase {
            label: "both sides append a different line at the end",
            repository: "both-append",
            source: b"one\ntwo\nthree\nfour\nfive\nsix\nseven\nfrom the source\n",
            destination: b"one\ntwo\nthree\nfour\nfive\nsix\nseven\nfrom the destination\n",
            conflicts_at_file_level: true,
            conflicts_at_line_level: true,
        },
        DetailCase {
            label: "the two sides write the same new content",
            repository: "identical-lines",
            source: FIRST_LINE_CHANGED,
            destination: FIRST_LINE_CHANGED,
            conflicts_at_file_level: false,
            conflicts_at_line_level: false,
        },
    ];

    for case in cases {
        let label = case.label;
        scenario(
            &router,
            case.repository,
            &base_tree(),
            &[&[Change::Put("multi.txt", case.source, NORMAL)]],
            &[&[Change::Put("multi.txt", case.destination, NORMAL)]],
        )
        .await;

        // The default detail level is FILE_LEVEL, documented on the member, so an
        // absent one must answer exactly as the explicit value does.
        for detail_level in [None, Some(FILE_LEVEL), Some(LINE_LEVEL)] {
            let conflicts = if detail_level == Some(LINE_LEVEL) {
                case.conflicts_at_line_level
            } else {
                case.conflicts_at_file_level
            };
            let mut expected: Vec<String> = Vec::new();
            if conflicts {
                expected.push("multi.txt".to_string());
            }
            let payload = ok(
                &router,
                "GetMergeConflicts",
                &request(case.repository, THREE_WAY_MERGE, detail_level, None),
            )
            .await;
            let got = conflicting_paths(&payload);
            assert_eq!(
                got, expected,
                "case {label:?} at {detail_level:?}: expected conflicts {expected:?}, got {got:?}"
            );
            assert_eq!(
                mergeable(&payload),
                expected.is_empty(),
                "case {label:?} at {detail_level:?}: mergeable must be true exactly when nothing \
                 conflicts, got {payload}"
            );
        }
    }
}

/// One resolution-strategy row: a label, the strategy, and the paths still conflicting
/// under it at `FILE_LEVEL`.
struct StrategyCase<'a> {
    label: &'a str,
    strategy: Option<&'a str>,
    conflicts: Vec<&'a str>,
}

/// A text file of `lines` numbered lines, with `marker` written on line `at` — so two
/// versions of the same file can differ at one chosen place and nowhere else.
fn numbered_lines(lines: usize, at: usize, marker: &str) -> Vec<u8> {
    let mut content = String::new();
    for line in 1..=lines {
        if line == at {
            content.push_str(&format!("line {line} {marker}\n"));
        } else {
            content.push_str(&format!("line {line}\n"));
        }
    }
    content.into_bytes()
}

#[tokio::test]
async fn a_long_shared_history_is_still_compared() {
    let router = emulator();
    let repository = "long-history";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    // A history far longer than the number of files it carries. Finding the merge base
    // walks every one of those commits, and that walk must not be billed to the published
    // count of FILES a merge compares, so a repository of this shape still answers the
    // comparison rather than a limit refusal. 80 commits is comfortably past the point a
    // per-pair walk would have cost thousands of steps, and still inside the store's own
    // retention.
    let mut tip = commit(
        &router,
        repository,
        DESTINATION_BRANCH,
        None,
        &[Change::Put("a.txt", ALPHA, NORMAL)],
    )
    .await;
    for step in 1..=80usize {
        let content = format!("alpha, step {step}\n").into_bytes();
        tip = commit(
            &router,
            repository,
            DESTINATION_BRANCH,
            Some(&tip),
            &[Change::Put("a.txt", &content, NORMAL)],
        )
        .await;
    }
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE_BRANCH,
            "commitId": tip,
        }),
    )
    .await;
    // One step on each side from that shared tip, so exactly one path conflicts.
    let source_tip = commit(
        &router,
        repository,
        SOURCE_BRANCH,
        Some(&tip),
        &[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)],
    )
    .await;
    commit(
        &router,
        repository,
        DESTINATION_BRANCH,
        Some(&tip),
        &[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)],
    )
    .await;

    for merge_option in [FAST_FORWARD_MERGE, SQUASH_MERGE, THREE_WAY_MERGE] {
        let payload = ok(
            &router,
            "GetMergeConflicts",
            &request(repository, merge_option, None, None),
        )
        .await;
        let expected: Vec<String> = if merge_option == FAST_FORWARD_MERGE {
            Vec::new()
        } else {
            vec!["a.txt".to_string()]
        };
        let got = conflicting_paths(&payload);
        assert_eq!(
            got, expected,
            "case {merge_option}: expected conflicts {expected:?} over a long history, got \
             {got:?}"
        );
        assert!(
            !mergeable(&payload),
            "case {merge_option}: two sides that changed the same file are not mergeable, got \
             {payload}"
        );
        assert_eq!(
            string_at(&payload, "/baseCommitId"),
            tip,
            "case {merge_option}: the merge base is the commit the two sides diverged from"
        );
        assert_eq!(
            string_at(&payload, "/sourceCommitId"),
            source_tip,
            "case {merge_option}: the source tip is the commit the source branch points at"
        );
    }
}

/// A text file of `lines` lines each padded to `width` bytes, with `marker` written on
/// line `at` — so a file can be made large in BYTES while holding few lines.
fn wide_lines(lines: usize, width: usize, at: usize, marker: &str) -> Vec<u8> {
    let mut content = String::new();
    for line in 1..=lines {
        let mut text = if line == at {
            format!("line {line} {marker}")
        } else {
            format!("line {line}")
        };
        while text.len() < width {
            text.push('.');
        }
        content.push_str(&text);
        content.push('\n');
    }
    content.into_bytes()
}

/// What a `LINE_LEVEL` evaluation of one file answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineLevel {
    /// The two sides cannot both be taken, so the path is reported.
    Conflicts,
    /// The two sides reconcile, so nothing is reported.
    Merges,
    /// The content is past what ONE merge may load, so the request is refused with
    /// `MaximumFileContentToLoadExceededException` instead of being merged.
    Refused,
}

/// One row of the incomparable-content table: the file's three versions and what a
/// `LINE_LEVEL` evaluation answers for them.
struct IncomparableCase<'a> {
    label: &'a str,
    repository: &'a str,
    base: &'a [u8],
    source: &'a [u8],
    destination: &'a [u8],
    at_line_level: LineLevel,
}

#[tokio::test]
async fn content_whose_lines_cannot_be_compared_is_not_merged_line_by_line() {
    let router = emulator();

    // Three versions of a file past the BYTE bound on one merge, differing only at
    // opposite ends: a line-by-line merge would take both changes, but content this large
    // is not loaded into a merge at all.
    let wide = MAX_MERGE_FILE_BYTES / 256 + 2;
    let huge_base = wide_lines(wide, 255, 0, "");
    assert!(
        huge_base.len() > MAX_MERGE_FILE_BYTES,
        "the oversized case must really exceed {MAX_MERGE_FILE_BYTES} bytes, got {}",
        huge_base.len()
    );
    let huge_from_source = wide_lines(wide, 255, 1, "from the source");
    let huge_from_destination = wide_lines(wide, 255, wide, "from the destination");

    // Past the LINE bound but well inside the byte bound: a file of many short lines,
    // again edited at opposite ends. The line count is what a line merge's cost follows,
    // so this file is not merged either, even though its bytes would be allowed.
    let many = MAX_MERGE_FILE_LINES;
    let many_lines_base = numbered_lines(many, 0, "");
    assert!(
        many_lines_base.len() <= MAX_MERGE_FILE_BYTES,
        "the many-lines case must stay inside {MAX_MERGE_FILE_BYTES} bytes so that only the \
         line bound stops it, got {}",
        many_lines_base.len()
    );
    let many_lines_from_source = numbered_lines(many, 1, "from the source");
    let many_lines_from_destination = numbered_lines(many, many, "from the destination");

    // One line inside the bound, the same shape: the control that proves the line count is
    // the only thing separating the two rows.
    let at_bound = MAX_MERGE_FILE_LINES - 1;
    let at_bound_base = numbered_lines(at_bound, 0, "");
    let at_bound_from_source = numbered_lines(at_bound, 1, "from the source");
    let at_bound_from_destination = numbered_lines(at_bound, at_bound, "from the destination");

    // The same shape, far smaller, is the control: its far-apart edits merge at LINE_LEVEL.
    let small_base = numbered_lines(64, 0, "");
    let small_from_source = numbered_lines(64, 1, "from the source");
    let small_from_destination = numbered_lines(64, 64, "from the destination");

    // Rows differ along what stops the line merge: sheer size, a line count, a NUL byte,
    // bytes that are not text at all — against the two comparable controls.
    let cases: Vec<IncomparableCase<'_>> = vec![
        IncomparableCase {
            label: "a file past the bytes one merge loads, edited at opposite ends",
            repository: "too-large",
            base: &huge_base,
            source: &huge_from_source,
            destination: &huge_from_destination,
            at_line_level: LineLevel::Refused,
        },
        IncomparableCase {
            label: "a file past the lines one merge compares, edited at opposite ends",
            repository: "too-many-lines",
            base: &many_lines_base,
            source: &many_lines_from_source,
            destination: &many_lines_from_destination,
            at_line_level: LineLevel::Refused,
        },
        IncomparableCase {
            label: "the same file one line inside the line bound",
            repository: "at-the-line-bound",
            base: &at_bound_base,
            source: &at_bound_from_source,
            destination: &at_bound_from_destination,
            at_line_level: LineLevel::Merges,
        },
        IncomparableCase {
            label: "a far smaller file edited at opposite ends",
            repository: "small-enough",
            base: &small_base,
            source: &small_from_source,
            destination: &small_from_destination,
            at_line_level: LineLevel::Merges,
        },
        IncomparableCase {
            label: "a binary file (a NUL byte in its leading bytes)",
            repository: "binary-lines",
            base: BINARY,
            source: BINARY_FROM_SOURCE,
            destination: BINARY_FROM_DESTINATION,
            at_line_level: LineLevel::Conflicts,
        },
        IncomparableCase {
            label: "bytes that are not UTF-8 text",
            repository: "not-text",
            base: b"\xff\xfe nonsense\n",
            source: b"\xff\xfe nonsense, from the source\n",
            destination: b"\xff\xfe nonsense, from the destination\n",
            at_line_level: LineLevel::Conflicts,
        },
    ];

    for case in cases {
        let IncomparableCase {
            label,
            repository,
            base,
            source,
            destination,
            at_line_level,
        } = case;
        scenario(
            &router,
            repository,
            &[Change::Put("big.txt", base, NORMAL)],
            &[&[Change::Put("big.txt", source, NORMAL)]],
            &[&[Change::Put("big.txt", destination, NORMAL)]],
        )
        .await;
        let line_level = request(repository, THREE_WAY_MERGE, Some(LINE_LEVEL), None);
        match at_line_level {
            LineLevel::Refused => {
                let code = error(&router, "GetMergeConflicts", &line_level, 400).await;
                assert_eq!(
                    code, "MaximumFileContentToLoadExceededException",
                    "case {label:?} at LINE_LEVEL: content past what one merge loads must be \
                     refused, got {code}"
                );
            }
            LineLevel::Conflicts | LineLevel::Merges => {
                let payload = ok(&router, "GetMergeConflicts", &line_level).await;
                let got = conflicting_paths(&payload);
                let expected: Vec<String> = if at_line_level == LineLevel::Conflicts {
                    vec!["big.txt".to_string()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    got, expected,
                    "case {label:?} at LINE_LEVEL: expected conflicts {expected:?}, got {got:?}"
                );
                assert_eq!(
                    mergeable(&payload),
                    expected.is_empty(),
                    "case {label:?} at LINE_LEVEL: mergeable must be true exactly when nothing \
                     conflicts, got {payload}"
                );
            }
        }

        // FILE_LEVEL loads no content at all — the stored blob ids already answer whether
        // the two sides differ — so every row conflicts there, whatever its size.
        let payload = ok(
            &router,
            "GetMergeConflicts",
            &request(repository, THREE_WAY_MERGE, Some(FILE_LEVEL), None),
        )
        .await;
        assert_eq!(
            conflicting_paths(&payload),
            vec!["big.txt".to_string()],
            "case {label:?} at FILE_LEVEL: two sides that differ conflict without any content \
             being loaded, got {payload}"
        );
    }
}

#[tokio::test]
async fn the_resolution_strategy_decides_which_conflicts_a_merge_settles_itself() {
    let router = emulator();
    let repository = "strategies";
    // Four disagreements of different kinds: an edit of the same line, an edit of lines
    // far apart in the same file, a deletion against a change, and a mode clash.
    scenario(
        &router,
        repository,
        &base_tree(),
        &[&[
            Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL),
            Change::Put("multi.txt", FIRST_LINE_CHANGED, NORMAL),
            Change::Delete("dir/b.txt"),
            Change::SetMode("link", NORMAL),
        ]],
        &[&[
            Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL),
            Change::Put("multi.txt", LAST_LINE_CHANGED, NORMAL),
            Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL),
            Change::SetMode("link", EXECUTABLE),
        ]],
    )
    .await;

    // Rows differ along the strategy, which is what decides how much of the same
    // evaluation a merge could settle without a caller.
    let cases: Vec<StrategyCase<'_>> = vec![
        StrategyCase {
            label: "no strategy named: the documented NONE default settles nothing",
            strategy: None,
            conflicts: vec!["a.txt", "dir/b.txt", "link", "multi.txt"],
        },
        StrategyCase {
            label: "NONE settles nothing",
            strategy: Some(NONE),
            conflicts: vec!["a.txt", "dir/b.txt", "link", "multi.txt"],
        },
        StrategyCase {
            label: "ACCEPT_SOURCE takes the source's whole file everywhere",
            strategy: Some(ACCEPT_SOURCE),
            conflicts: vec![],
        },
        StrategyCase {
            label: "ACCEPT_DESTINATION takes the destination's whole file everywhere",
            strategy: Some(ACCEPT_DESTINATION),
            conflicts: vec![],
        },
        StrategyCase {
            label: "AUTOMERGE settles only the text file whose hunks do not collide",
            strategy: Some(AUTOMERGE),
            conflicts: vec!["a.txt", "dir/b.txt", "link"],
        },
    ];

    for case in cases {
        let label = case.label;
        let payload = ok(
            &router,
            "GetMergeConflicts",
            &request(repository, THREE_WAY_MERGE, Some(FILE_LEVEL), case.strategy),
        )
        .await;
        let got = conflicting_paths(&payload);
        let expected: Vec<String> = case.conflicts.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            got, expected,
            "case {label:?}: expected conflicts {expected:?}, got {got:?}"
        );
        assert_eq!(
            mergeable(&payload),
            expected.is_empty(),
            "case {label:?}: mergeable must be true exactly when nothing conflicts, got {payload}"
        );
    }
}

/// One merge-option row: a label, the repository, the commits each side makes, the paths
/// the two merge-commit options report as conflicting, and whether the pair can
/// fast-forward.
struct OptionCase<'a> {
    label: &'a str,
    repository: &'a str,
    source: Vec<&'a [Change<'a>]>,
    destination: Vec<&'a [Change<'a>]>,
    conflicts: Vec<&'a str>,
    fast_forward: bool,
}

#[tokio::test]
async fn the_merge_option_decides_which_question_is_asked_of_the_two_commits() {
    let router = emulator();

    // Each row is a scenario and, per merge option, whether it is mergeable. A
    // fast-forward is possible exactly when the destination is already in the source's
    // history; the two merge-commit options answer the three-way comparison.
    let cases: Vec<OptionCase<'_>> = vec![
        OptionCase {
            label: "the source is ahead and the destination has not moved",
            repository: "ff-ahead",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![],
            conflicts: vec![],
            fast_forward: true,
        },
        OptionCase {
            label: "neither side has moved",
            repository: "ff-equal",
            source: vec![],
            destination: vec![],
            conflicts: vec![],
            fast_forward: true,
        },
        OptionCase {
            label: "the destination is ahead of the source",
            repository: "ff-behind",
            source: vec![],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec![],
            fast_forward: false,
        },
        OptionCase {
            label: "both sides moved, with no path in common",
            repository: "ff-diverged",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL)]],
            conflicts: vec![],
            fast_forward: false,
        },
        OptionCase {
            label: "both sides changed the same file",
            repository: "ff-conflicting",
            source: vec![&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
            destination: vec![&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
            conflicts: vec!["a.txt"],
            fast_forward: false,
        },
    ];

    for case in cases {
        let OptionCase {
            label,
            repository,
            source,
            destination,
            conflicts,
            fast_forward,
        } = case;
        scenario(&router, repository, &base_tree(), &source, &destination).await;

        // FAST_FORWARD_MERGE: "If the specified merge strategy is FAST_FORWARD_MERGE,
        // this list is always empty."
        let payload = ok(
            &router,
            "GetMergeConflicts",
            &request(repository, FAST_FORWARD_MERGE, None, None),
        )
        .await;
        let got = conflicting_paths(&payload);
        assert!(
            got.is_empty(),
            "case {label:?}: FAST_FORWARD_MERGE reports no conflict metadata, got {got:?}"
        );
        assert_eq!(
            mergeable(&payload),
            fast_forward,
            "case {label:?}: expected a fast forward to be {fast_forward}, got {payload}"
        );

        // SQUASH_MERGE and THREE_WAY_MERGE differ in the commit a merge WRITES, not in
        // what conflicts, so both answer the same three-way comparison.
        let expected: Vec<String> = conflicts.iter().map(|path| (*path).to_string()).collect();
        for merge_option in [SQUASH_MERGE, THREE_WAY_MERGE] {
            let payload = ok(
                &router,
                "GetMergeConflicts",
                &request(repository, merge_option, None, None),
            )
            .await;
            let got = conflicting_paths(&payload);
            assert_eq!(
                got, expected,
                "case {label:?} under {merge_option}: expected conflicts {expected:?}, got {got:?}"
            );
            assert_eq!(
                mergeable(&payload),
                expected.is_empty(),
                "case {label:?} under {merge_option}: mergeable must be true exactly when nothing \
                 conflicts, got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn every_commit_specifier_form_resolves_to_the_same_evaluation() {
    let router = emulator();
    let repository = "specifier-forms";
    let merge = scenario(
        &router,
        repository,
        &base_tree(),
        &[&[Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL)]],
        &[&[Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL)]],
    )
    .await;

    // `CommitName` is "the branch, tag, HEAD, or other fully qualified reference used to
    // identify a commit (for example, a branch name or a full commit ID)". `HEAD` names
    // this repository's default branch, which is the destination side.
    let destination_forms = [
        DESTINATION_BRANCH.to_string(),
        format!("refs/heads/{DESTINATION_BRANCH}"),
        "HEAD".to_string(),
        merge.destination.clone(),
    ];
    let source_forms = [
        SOURCE_BRANCH.to_string(),
        format!("refs/heads/{SOURCE_BRANCH}"),
        merge.source.clone(),
    ];
    for destination in &destination_forms {
        for source in &source_forms {
            let payload = ok(
                &router,
                "GetMergeConflicts",
                &json!({
                    "repositoryName": repository,
                    "sourceCommitSpecifier": source,
                    "destinationCommitSpecifier": destination,
                    "mergeOption": THREE_WAY_MERGE,
                }),
            )
            .await;
            let got = conflicting_paths(&payload);
            assert_eq!(
                got,
                vec!["a.txt".to_string()],
                "case {source:?} into {destination:?}: expected a.txt to conflict, got {got:?}"
            );
            assert_eq!(
                string_at(&payload, "/sourceCommitId"),
                merge.source,
                "case {source:?} into {destination:?}: every form of the source specifier names \
                 the same commit"
            );
            assert_eq!(
                string_at(&payload, "/destinationCommitId"),
                merge.destination,
                "case {source:?} into {destination:?}: every form of the destination specifier \
                 names the same commit"
            );
            assert_eq!(
                string_at(&payload, "/baseCommitId"),
                merge.base,
                "case {source:?} into {destination:?}: the merge base is the same commit"
            );
        }
    }
}

/// One metadata row: the conflicting path and every member the rule fixes for it.
struct MetadataCase<'a> {
    path: &'a str,
    content_conflict: bool,
    file_mode_conflict: bool,
    object_type_conflict: bool,
    source_operation: &'a str,
    destination_operation: &'a str,
    /// The `objectTypes` of the source, destination and base sides.
    object_types: Value,
    /// The `fileSizes` of the same three sides.
    file_sizes: Value,
    binary: Option<bool>,
}

#[tokio::test]
async fn one_conflict_reports_each_side_of_the_path() {
    let router = emulator();
    let repository = "metadata";
    scenario(
        &router,
        repository,
        &base_tree(),
        &[&[
            Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL),
            Change::Put("bin.dat", BINARY_FROM_SOURCE, NORMAL),
            Change::Delete("dir/b.txt"),
            Change::SetMode("link", NORMAL),
            Change::Put("added.txt", SCRIPT_FROM_SOURCE, SYMLINK),
        ]],
        &[&[
            Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL),
            Change::Put("bin.dat", BINARY_FROM_DESTINATION, NORMAL),
            Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL),
            Change::SetMode("link", EXECUTABLE),
            Change::Put("added.txt", SCRIPT_FROM_DESTINATION, EXECUTABLE),
        ]],
    )
    .await;
    let payload = ok(
        &router,
        "GetMergeConflicts",
        &request(repository, THREE_WAY_MERGE, Some(FILE_LEVEL), None),
    )
    .await;

    // Each row is one conflicting path and the members the rule fixes for it. `binary` is
    // the source side's `isBinaryFile`, absent where that side holds no file at the path.
    let cases: Vec<MetadataCase<'_>> = vec![
        MetadataCase {
            path: "a.txt",
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            source_operation: "M",
            destination_operation: "M",
            object_types: json!({ "source": "FILE", "destination": "FILE", "base": "FILE" }),
            file_sizes: json!({
                "source": ALPHA_FROM_SOURCE.len(),
                "destination": ALPHA_FROM_DESTINATION.len(),
                "base": ALPHA.len(),
            }),
            binary: Some(false),
        },
        MetadataCase {
            path: "bin.dat",
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            source_operation: "M",
            destination_operation: "M",
            object_types: json!({ "source": "FILE", "destination": "FILE", "base": "FILE" }),
            file_sizes: json!({
                "source": BINARY_FROM_SOURCE.len(),
                "destination": BINARY_FROM_DESTINATION.len(),
                "base": BINARY.len(),
            }),
            binary: Some(true),
        },
        // A deletion against a change: the content is what the two sides disagree about —
        // the destination's edit cannot be carried into a file the source deleted — and
        // the two merge operations say which side did which.
        MetadataCase {
            path: "dir/b.txt",
            content_conflict: true,
            file_mode_conflict: false,
            object_type_conflict: false,
            source_operation: "D",
            destination_operation: "M",
            object_types: json!({ "source": null, "destination": "FILE", "base": "FILE" }),
            file_sizes: json!({
                "source": 0,
                "destination": BEE_FROM_DESTINATION.len(),
                "base": BEE.len(),
            }),
            binary: None,
        },
        // The same bytes under two different modes.
        MetadataCase {
            path: "link",
            content_conflict: false,
            file_mode_conflict: true,
            object_type_conflict: false,
            source_operation: "M",
            destination_operation: "M",
            object_types: json!({
                "source": "FILE",
                "destination": "FILE",
                "base": "SYMBOLIC_LINK",
            }),
            file_sizes: json!({
                "source": LINK_TARGET.len(),
                "destination": LINK_TARGET.len(),
                "base": LINK_TARGET.len(),
            }),
            binary: Some(false),
        },
        // A path neither side's base held, added by both as a different kind of object,
        // at a different mode and with different content: all three facets disagree,
        // because neither side can claim the base's value for any of them.
        MetadataCase {
            path: "added.txt",
            content_conflict: true,
            file_mode_conflict: true,
            object_type_conflict: true,
            source_operation: "A",
            destination_operation: "A",
            object_types: json!({
                "source": "SYMBOLIC_LINK",
                "destination": "FILE",
                "base": null,
            }),
            file_sizes: json!({
                "source": SCRIPT_FROM_SOURCE.len(),
                "destination": SCRIPT_FROM_DESTINATION.len(),
                "base": 0,
            }),
            binary: Some(false),
        },
    ];

    for case in cases {
        let MetadataCase {
            path,
            content_conflict: content,
            file_mode_conflict: mode,
            object_type_conflict: object_type,
            source_operation: source_op,
            destination_operation: destination_op,
            object_types: types,
            file_sizes: sizes,
            binary,
        } = case;
        let conflict = conflict_for(&payload, path);
        assert_eq!(
            conflict.get("contentConflict").and_then(Value::as_bool),
            Some(content),
            "case {path:?}: expected contentConflict {content}, got {conflict}"
        );
        assert_eq!(
            conflict.get("fileModeConflict").and_then(Value::as_bool),
            Some(mode),
            "case {path:?}: expected fileModeConflict {mode}, got {conflict}"
        );
        assert_eq!(
            conflict.get("objectTypeConflict").and_then(Value::as_bool),
            Some(object_type),
            "case {path:?}: expected objectTypeConflict {object_type}, got {conflict}"
        );
        assert_eq!(
            conflict.pointer("/mergeOperations/source").and_then(Value::as_str),
            Some(source_op),
            "case {path:?}: expected the source operation {source_op}, got {conflict}"
        );
        assert_eq!(
            conflict
                .pointer("/mergeOperations/destination")
                .and_then(Value::as_str),
            Some(destination_op),
            "case {path:?}: expected the destination operation {destination_op}, got {conflict}"
        );
        for side in ["source", "destination", "base"] {
            let expected = types.get(side).expect("the row names every side");
            let got = conflict
                .pointer(&format!("/objectTypes/{side}"))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                &got, expected,
                "case {path:?}: expected the {side} object type {expected}, got {conflict}"
            );
            let expected = sizes
                .get(side)
                .and_then(Value::as_i64)
                .expect("the row names every side's size");
            let got = conflict
                .pointer(&format!("/fileSizes/{side}"))
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            assert_eq!(
                got, expected,
                "case {path:?}: expected the {side} file size {expected}, got {conflict}"
            );
        }
        assert_eq!(
            conflict.pointer("/isBinaryFile/source").and_then(Value::as_bool),
            binary,
            "case {path:?}: expected the source side's binary status {binary:?}, got {conflict}"
        );
        let conflicts = conflict
            .get("numberOfConflicts")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        // "The number of conflicts, including both hunk conflicts and metadata
        // conflicts": the facets this row says disagree, and never fewer than the one
        // disagreement that put the path in the list. The rows differ here — one facet
        // for most of them, three for the path both sides added differently.
        let expected_conflicts = i64::from(content) + i64::from(mode) + i64::from(object_type);
        let expected_conflicts = expected_conflicts.max(1);
        assert_eq!(
            conflicts, expected_conflicts,
            "case {path:?}: expected numberOfConflicts {expected_conflicts}, got {conflict}"
        );
    }

    // The file modes each side records, read off the same response.
    let link = conflict_for(&payload, "link");
    for (side, expected) in [
        ("source", NORMAL),
        ("destination", EXECUTABLE),
        ("base", SYMLINK),
    ] {
        let got = link
            .pointer(&format!("/fileModes/{side}"))
            .and_then(Value::as_str)
            .unwrap_or("<none>");
        assert_eq!(
            got, expected,
            "case \"link\": expected the {side} file mode {expected}, got {link}"
        );
    }
}

#[tokio::test]
async fn max_conflict_files_pages_the_ordered_conflicts() {
    let router = emulator();
    let repository = "paged";
    scenario(
        &router,
        repository,
        &base_tree(),
        &[&[
            Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL),
            Change::Put("bin.dat", BINARY_FROM_SOURCE, NORMAL),
            Change::Put("dir/b.txt", BEE_FROM_SOURCE, NORMAL),
            Change::Put("multi.txt", MIDDLE_LINE_CHANGED_ONE_WAY, NORMAL),
        ]],
        &[&[
            Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL),
            Change::Put("bin.dat", BINARY_FROM_DESTINATION, NORMAL),
            Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL),
            Change::Put("multi.txt", MIDDLE_LINE_CHANGED_ANOTHER_WAY, NORMAL),
        ]],
    )
    .await;

    let whole = vec![
        "a.txt".to_string(),
        "bin.dat".to_string(),
        "dir/b.txt".to_string(),
        "multi.txt".to_string(),
    ];
    let unpaged = ok(
        &router,
        "GetMergeConflicts",
        &request(repository, THREE_WAY_MERGE, None, None),
    )
    .await;
    let got = conflicting_paths(&unpaged);
    assert_eq!(
        got, whole,
        "case \"unpaged\": expected {whole:?}, got {got:?}"
    );

    // Page sizes below, at and above the conflict count: every page is at most
    // `maxConflictFiles` long, the pages concatenate to exactly the unpaged result, only
    // a page with more to come carries a token, and `mergeable` is about the whole
    // evaluation rather than the page.
    for max_conflict_files in [1usize, 2, 3, 4, 10] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut input = request(repository, THREE_WAY_MERGE, None, None);
            input["maxConflictFiles"] = json!(max_conflict_files);
            if let Some(token) = &token {
                input["nextToken"] = json!(token);
            }
            let payload = ok(&router, "GetMergeConflicts", &input).await;
            let page = conflicting_paths(&payload);
            assert!(
                page.len() <= max_conflict_files,
                "case maxConflictFiles={max_conflict_files}: a page holds at most \
                 {max_conflict_files} conflicts, got {page:?}"
            );
            assert!(
                !mergeable(&payload),
                "case maxConflictFiles={max_conflict_files}: mergeable is about the whole \
                 evaluation, got {payload}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case maxConflictFiles={max_conflict_files}: paging must terminate, got \
                 {collected:?}"
            );
            token = payload
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let more = collected.len() < whole.len();
            assert_eq!(
                token.is_some(),
                more,
                "case maxConflictFiles={max_conflict_files}: a continuation token is present \
                 exactly while conflicts remain, got {payload}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, whole,
            "case maxConflictFiles={max_conflict_files}: the pages concatenate to the whole \
             result, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_names_the_modeled_error() {
    let router = emulator();
    let repository = "refusals";
    let merge = scenario(
        &router,
        repository,
        &base_tree(),
        &[&[
            Change::Put("a.txt", ALPHA_FROM_SOURCE, NORMAL),
            Change::Put("dir/b.txt", BEE_FROM_SOURCE, NORMAL),
        ]],
        &[&[
            Change::Put("a.txt", ALPHA_FROM_DESTINATION, NORMAL),
            Change::Put("dir/b.txt", BEE_FROM_DESTINATION, NORMAL),
        ]],
    )
    .await;

    // A token this evaluation really issued: the first page of its two conflicts. One row
    // below replays it under a different detail level, so that row is about the token's
    // SCOPE rather than about its form.
    let mut first_page = request(repository, THREE_WAY_MERGE, None, None);
    first_page["maxConflictFiles"] = json!(1);
    let paged = ok(&router, "GetMergeConflicts", &first_page).await;
    let issued = string_at(&paged, "/nextToken");
    // That token continues THIS evaluation, which is what makes the scope row meaningful.
    let mut second_page = first_page.clone();
    second_page["nextToken"] = json!(issued);
    let continued = ok(&router, "GetMergeConflicts", &second_page).await;
    assert_eq!(
        conflicting_paths(&continued),
        vec!["dir/b.txt".to_string()],
        "the issued token continues the evaluation it was issued for, got {continued}"
    );

    // Rows differ along which part of the request is at fault: the repository, either
    // commit specifier, each enum member, the page size and the continuation token.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository the store does not hold",
            request("no-such-repository", THREE_WAY_MERGE, None, None),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name of nothing but space names no repository",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["repositoryName"] = json!("   ");
                input
            },
            "InvalidRepositoryNameException",
        ),
        (
            "a source commit id the repository does not hold",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["sourceCommitSpecifier"] = json!("0".repeat(40));
                input
            },
            "CommitDoesNotExistException",
        ),
        (
            "a destination specifier that names no branch",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["destinationCommitSpecifier"] = json!("refs/heads/nowhere");
                input
            },
            "CommitDoesNotExistException",
        ),
        (
            "a source specifier carrying nothing specifies no commit",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["sourceCommitSpecifier"] = json!("");
                input
            },
            "CommitRequiredException",
        ),
        (
            // Space is not a reference name, so a specifier of spaces is a
            // specifier that cannot name a commit rather than an absent one.
            "a source specifier of nothing but space cannot name a commit",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["sourceCommitSpecifier"] = json!("   ");
                input
            },
            "InvalidSourceCommitSpecifierException",
        ),
        (
            "a detail level that differs from the enum value only by surrounding space",
            request(repository, THREE_WAY_MERGE, Some(" LINE_LEVEL "), None),
            "InvalidConflictDetailLevelException",
        ),
        (
            "a source specifier that cannot name a commit at all",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["sourceCommitSpecifier"] = json!("not a ref name");
                input
            },
            "InvalidSourceCommitSpecifierException",
        ),
        (
            "a destination specifier that cannot name a commit at all",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["destinationCommitSpecifier"] = json!("refs/heads/..");
                input
            },
            "InvalidDestinationCommitSpecifierException",
        ),
        (
            "no merge option at all",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input.as_object_mut().expect("an object").remove("mergeOption");
                input
            },
            "MergeOptionRequiredException",
        ),
        (
            "a merge option outside the enum",
            request(repository, "REBASE_MERGE", None, None),
            "InvalidMergeOptionException",
        ),
        (
            "a conflict detail level outside the enum",
            request(repository, THREE_WAY_MERGE, Some("WORD_LEVEL"), None),
            "InvalidConflictDetailLevelException",
        ),
        (
            "a conflict resolution strategy outside the enum",
            request(repository, THREE_WAY_MERGE, None, Some("ACCEPT_BOTH")),
            "InvalidConflictResolutionStrategyException",
        ),
        (
            "zero is not a number of conflict files",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["maxConflictFiles"] = json!(0);
                input
            },
            "InvalidMaxConflictFilesException",
        ),
        (
            "a negative number of conflict files",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["maxConflictFiles"] = json!(-3);
                input
            },
            "InvalidMaxConflictFilesException",
        ),
        (
            "more conflict files than the published limit",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["maxConflictFiles"] = json!(1001);
                input
            },
            "InvalidMaxConflictFilesException",
        ),
        (
            "a continuation token this operation never issued",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["nextToken"] = json!("not-a-token");
                input
            },
            "InvalidContinuationTokenException",
        ),
        (
            "a continuation token past the end of the evaluation",
            {
                let mut input = request(repository, THREE_WAY_MERGE, None, None);
                input["nextToken"] = json!(hex::encode("codecommit-page:GetMergeConflicts:4096"));
                input
            },
            "InvalidContinuationTokenException",
        ),
        (
            "a continuation token this evaluation really issued, replayed against another \
             detail level",
            {
                let mut input = request(repository, THREE_WAY_MERGE, Some(LINE_LEVEL), None);
                input["maxConflictFiles"] = json!(1);
                input["nextToken"] = json!(issued);
                input
            },
            "InvalidContinuationTokenException",
        ),
        (
            // A fast-forward reports no conflict metadata, but it pages the same
            // way every other merge option does, so it refuses the same tokens.
            "a continuation token under FAST_FORWARD_MERGE",
            {
                let mut input = request(repository, FAST_FORWARD_MERGE, None, None);
                input["nextToken"] = json!("not-a-token");
                input
            },
            "InvalidContinuationTokenException",
        ),
        (
            "zero is not a number of conflict files under FAST_FORWARD_MERGE",
            {
                let mut input = request(repository, FAST_FORWARD_MERGE, None, None);
                input["maxConflictFiles"] = json!(0);
                input
            },
            "InvalidMaxConflictFilesException",
        ),
    ];

    for (label, input, expected) in cases {
        let code = error(&router, "GetMergeConflicts", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }

    // None of the refusals, and none of the reads above, changed the repository.
    for (branch, tip) in [
        (SOURCE_BRANCH, &merge.source),
        (DESTINATION_BRANCH, &merge.destination),
    ] {
        let read = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": branch }),
        )
        .await;
        assert_eq!(
            &string_at(&read, "/branch/commitId"),
            tip,
            "evaluating a merge must leave the {branch} tip where it was"
        );
    }
}

/// One enum row: the shape, the values the service names for it, and the
/// model-generated catalog they must agree with.
struct EnumCase<'a> {
    shape: &'a str,
    named: Vec<&'a str>,
    catalog: &'a [&'a str],
}

#[test]
fn every_enum_value_the_service_names_is_one_the_model_declares() {
    use service_com_amazonaws_codecommit_codecommit_20150413::ops::{
        get_merge_conflicts as merge_conflicts_op, types,
    };
    use service_com_amazonaws_codecommit_codecommit_20150413::state::merge;

    // Each row pairs the values this suite sends AND the ones the service's own code names
    // for its branching — `crate::state::merge` for the merge evaluation every merge
    // question shares, this operation's own module for what only its report names — with
    // the generated `*_ENUM_VALUES` catalog for that shape, so no copy can drift from the
    // model.
    let cases: Vec<EnumCase<'_>> = vec![
        EnumCase {
            shape: "MergeOptionTypeEnum",
            named: vec![
                FAST_FORWARD_MERGE,
                SQUASH_MERGE,
                THREE_WAY_MERGE,
                merge::MERGE_OPTION_FAST_FORWARD,
                merge::MERGE_OPTION_SQUASH,
                merge::MERGE_OPTION_THREE_WAY,
            ],
            catalog: types::MERGE_OPTION_TYPE_ENUM_VALUES,
        },
        EnumCase {
            shape: "ConflictDetailLevelTypeEnum",
            named: vec![
                FILE_LEVEL,
                LINE_LEVEL,
                merge::CONFLICT_DETAIL_FILE_LEVEL,
                merge::CONFLICT_DETAIL_LINE_LEVEL,
            ],
            catalog: types::CONFLICT_DETAIL_LEVEL_TYPE_ENUM_VALUES,
        },
        EnumCase {
            shape: "ConflictResolutionStrategyTypeEnum",
            named: vec![
                NONE,
                ACCEPT_SOURCE,
                ACCEPT_DESTINATION,
                AUTOMERGE,
                merge::CONFLICT_RESOLUTION_NONE,
                merge::CONFLICT_RESOLUTION_ACCEPT_SOURCE,
                merge::CONFLICT_RESOLUTION_ACCEPT_DESTINATION,
                merge::CONFLICT_RESOLUTION_AUTOMERGE,
            ],
            catalog: types::CONFLICT_RESOLUTION_STRATEGY_TYPE_ENUM_VALUES,
        },
        EnumCase {
            shape: "ChangeTypeEnum",
            named: vec![
                merge_conflicts_op::CHANGE_TYPE_ADDED,
                merge_conflicts_op::CHANGE_TYPE_MODIFIED,
                merge_conflicts_op::CHANGE_TYPE_DELETED,
            ],
            catalog: types::CHANGE_TYPE_ENUM_VALUES,
        },
        EnumCase {
            shape: "ObjectTypeEnum",
            named: vec![
                merge_conflicts_op::OBJECT_TYPE_FILE,
                merge_conflicts_op::OBJECT_TYPE_DIRECTORY,
                merge_conflicts_op::OBJECT_TYPE_SYMBOLIC_LINK,
            ],
            catalog: types::OBJECT_TYPE_ENUM_VALUES,
        },
        EnumCase {
            shape: "FileModeTypeEnum",
            named: vec![NORMAL, EXECUTABLE, SYMLINK],
            catalog: types::FILE_MODE_TYPE_ENUM_VALUES,
        },
    ];

    for EnumCase {
        shape,
        named,
        catalog,
    } in cases
    {
        for value in &named {
            assert!(
                catalog.contains(value),
                "case {shape:?}: {value:?} is not a value the model declares, which are {catalog:?}"
            );
        }
        // And the other way, except for `ObjectTypeEnum`'s GIT_LINK — a submodule, which
        // no stored tree holds, so the service names no constant for it.
        if shape != "ObjectTypeEnum" {
            for value in catalog {
                assert!(
                    named.contains(value),
                    "case {shape:?}: the model declares {value:?}, which the service names \
                     nowhere; it names {named:?}"
                );
            }
        }
    }
}
