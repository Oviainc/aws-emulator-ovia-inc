// SPDX-License-Identifier: CC0-1.0

//! `ListFileCommitHistory` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: for one file, the revision DAG lists exactly the commits
//! reachable from the resolved commit specifier in whose tree that file's content
//! differs from the content in every parent — the commits that changed it — newest
//! first, each carrying that commit's stored metadata, the blob the file had there,
//! the file's path, and the ids of the revisions holding a more recent version of
//! it; the list is paged by `maxResults`/`nextToken` and nothing is written.
//!
//! The tables below span the class the rule covers — how many revisions, writes to
//! other files in between, path depth and leading separator, content kind and size,
//! page size, and which commit specifier bounds the history — rather than one
//! recorded request. The wire harness (`emulator`, `ok`, `error`, the base64 and
//! object-name helpers) is the one already used by the repository-lifecycle suite.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{
    base64, emulator, error, from_base64, git_blob_id, is_object_id, ok, string_at,
};

/// The identity every write in this suite is authored by, so the commit each
/// revision reports can be checked against what was written.
const AUTHOR_NAME: &str = "History Author";
const AUTHOR_EMAIL: &str = "author@example.com";

/// One row of the history table: a repository, the writes applied to it in order,
/// and the file whose history is read back.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    repository: &'static str,
    branch: &'static str,
    /// The writes, in order: `(path, content)`. Each one becomes one commit.
    writes: Vec<(&'static str, Vec<u8>)>,
    /// The `filePath` as sent to `ListFileCommitHistory`.
    sent_path: &'static str,
    /// The path the service must address the file by, and report back (leading
    /// separators removed).
    stored_path: &'static str,
}

/// The table. Rows differ along what the rule generalises over: the number of
/// versions (none, one, two, three, five), writes to OTHER files interleaved
/// between them, path depth and a leading separator on the read path, and content
/// kind and size (short text, every byte value, unicode, 4 KiB).
fn history_cases() -> Vec<Case> {
    vec![
        Case {
            label: "a single version",
            repository: "single",
            branch: "main",
            writes: vec![("README.md", b"# one\n".to_vec())],
            sent_path: "README.md",
            stored_path: "README.md",
        },
        Case {
            label: "three versions of one file",
            repository: "linear",
            branch: "main",
            writes: vec![
                ("f.txt", b"one".to_vec()),
                ("f.txt", b"two".to_vec()),
                ("f.txt", b"three".to_vec()),
            ],
            sent_path: "f.txt",
            stored_path: "f.txt",
        },
        Case {
            label: "versions interleaved with another file's",
            repository: "interleaved",
            branch: "trunk",
            writes: vec![
                ("a.txt", b"a1".to_vec()),
                ("b.txt", b"b1".to_vec()),
                ("a.txt", b"a2".to_vec()),
                ("b.txt", b"b2".to_vec()),
                ("a.txt", b"a3".to_vec()),
            ],
            sent_path: "a.txt",
            stored_path: "a.txt",
        },
        Case {
            // The same script read for the OTHER file: its versions are the
            // commits that changed IT, not the ones that changed "a.txt".
            label: "the other file of the interleaved script",
            repository: "interleaved-other",
            branch: "trunk",
            writes: vec![
                ("a.txt", b"a1".to_vec()),
                ("b.txt", b"b1".to_vec()),
                ("a.txt", b"a2".to_vec()),
                ("b.txt", b"b2".to_vec()),
                ("a.txt", b"a3".to_vec()),
            ],
            sent_path: "b.txt",
            stored_path: "b.txt",
        },
        Case {
            // A leading separator addresses the same file as none at all, and the
            // reported path is the canonical one.
            label: "a nested path read with a leading separator",
            repository: "nested",
            branch: "release-1.0",
            writes: vec![
                ("src/main/app.sh", b"#!/bin/sh\necho one\n".to_vec()),
                ("src/main/app.sh", b"#!/bin/sh\necho two\n".to_vec()),
            ],
            sent_path: "/src/main/app.sh",
            stored_path: "src/main/app.sh",
        },
        Case {
            label: "five versions spanning content kinds and sizes",
            repository: "wide-contents",
            branch: "main",
            writes: vec![
                ("data.bin", b"plain text".to_vec()),
                ("data.bin", (0..=255u8).collect()),
                ("data.bin", "héllo wörld — ünïcode".as_bytes().to_vec()),
                ("data.bin", (0..=255u8).cycle().take(4096).collect()),
                ("data.bin", vec![0u8; 7]),
            ],
            sent_path: "data.bin",
            stored_path: "data.bin",
        },
        Case {
            label: "a file no commit holds",
            repository: "absent-file",
            branch: "main",
            writes: vec![("other.txt", b"other".to_vec())],
            sent_path: "ghost.txt",
            stored_path: "ghost.txt",
        },
    ]
}

/// The list of strings at `pointer`, or the empty list where the member is absent:
/// an empty list may be omitted from the response.
fn strings_at(payload: &Value, pointer: &str) -> Vec<String> {
    match payload.pointer(pointer) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .unwrap_or_else(|| panic!("expected strings at {pointer} in {payload}"))
                    .to_string()
            })
            .collect(),
        Some(other) => panic!("expected a list at {pointer} in {payload}, got {other}"),
    }
}

/// The `revisionDag` of a response.
fn dag_of(payload: &Value) -> Vec<Value> {
    payload
        .get("revisionDag")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("expected a revisionDag in {payload}"))
}

/// Create the case's repository and apply its writes in order, each on the branch
/// tip, answering the commit id every write produced.
async fn apply(router: &MultiServiceRouter, case: &Case) -> Vec<String> {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": case.repository }),
    )
    .await;
    let mut commits: Vec<String> = Vec::new();
    for (index, (path, content)) in case.writes.iter().enumerate() {
        let mut input = json!({
            "repositoryName": case.repository,
            "branchName": case.branch,
            "filePath": path,
            "fileContent": base64(content),
            "commitMessage": format!("write {index}"),
            "name": AUTHOR_NAME,
            "email": AUTHOR_EMAIL,
        });
        if let Some(parent) = commits.last() {
            input["parentCommitId"] = json!(parent);
        }
        commits.push(string_at(&ok(router, "PutFile", &input).await, "/commitId"));
    }
    commits
}

/// The writes that changed the case's read path, as `(write index, content)` —
/// the rule's own answer, computed from the script rather than from the service.
fn expected_revisions(case: &Case) -> Vec<(usize, Vec<u8>)> {
    let mut revisions: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    for (index, (path, content)) in case.writes.iter().enumerate() {
        if *path != case.stored_path {
            continue;
        }
        if current.as_deref() != Some(content.as_slice()) {
            revisions.push((index, content.clone()));
            current = Some(content.clone());
        }
    }
    revisions
}

#[tokio::test]
async fn the_revision_dag_lists_every_commit_that_changed_the_file_newest_first() {
    let router = emulator();
    for case in history_cases() {
        let label = case.label;
        let commits = apply(&router, &case).await;
        let expected = expected_revisions(&case);

        let request = json!({
            "repositoryName": case.repository,
            "filePath": case.sent_path,
        });
        let listed = ok(&router, "ListFileCommitHistory", &request).await;
        let dag = dag_of(&listed);
        assert_eq!(
            dag.len(),
            expected.len(),
            "case {label:?}: expected {} version(s), got {} in {listed}",
            expected.len(),
            dag.len()
        );
        assert!(
            listed
                .get("nextToken")
                .is_none_or(|token| token.is_null()),
            "case {label:?}: the whole history fits one page, got {listed}"
        );

        for (position, entry) in dag.iter().enumerate() {
            // Newest first: the DAG is the reverse of the order the versions were
            // written in.
            let (write_index, content) = &expected[expected.len() - 1 - position];
            let commit_id = &commits[*write_index];
            assert_eq!(
                &string_at(entry, "/commit/commitId"),
                commit_id,
                "case {label:?} position {position}: expected commit {commit_id}, got {entry}"
            );
            assert_eq!(
                string_at(entry, "/path"),
                case.stored_path,
                "case {label:?} position {position}: expected path {:?}, got {entry}",
                case.stored_path
            );
            assert_eq!(
                string_at(entry, "/blobId"),
                git_blob_id(content),
                "case {label:?} position {position}: blobId must be the object name of the content written, got {entry}"
            );
            let tree_id = string_at(entry, "/commit/treeId");
            assert!(
                is_object_id(&tree_id),
                "case {label:?} position {position}: treeId must be an object name, got {tree_id}"
            );
            assert_eq!(
                string_at(entry, "/commit/message"),
                format!("write {write_index}"),
                "case {label:?} position {position}: the commit message that was written, got {entry}"
            );
            assert_eq!(
                string_at(entry, "/commit/author/name"),
                AUTHOR_NAME,
                "case {label:?} position {position}: the author that was written, got {entry}"
            );
            assert_eq!(
                string_at(entry, "/commit/committer/email"),
                AUTHOR_EMAIL,
                "case {label:?} position {position}: the committer that was written, got {entry}"
            );
            let expected_parents: Vec<String> = if *write_index == 0 {
                Vec::new()
            } else {
                vec![commits[write_index - 1].clone()]
            };
            assert_eq!(
                strings_at(entry, "/commit/parents"),
                expected_parents,
                "case {label:?} position {position}: the commit's stored parents, got {entry}"
            );
            // `revisionChildren` is "an array of commit IDs that contain more recent
            // versions of this file", empty where there are none — so it names the
            // next newer version, and nothing for the newest.
            let expected_children: Vec<String> = if position == 0 {
                Vec::new()
            } else {
                vec![commits[expected[expected.len() - position].0].clone()]
            };
            assert_eq!(
                strings_at(entry, "/revisionChildren"),
                expected_children,
                "case {label:?} position {position}: revisionChildren names the more recent versions, got {entry}"
            );

            // The blob the DAG names is the content that commit really holds.
            let read = ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": case.repository,
                    "commitSpecifier": commit_id,
                    "filePath": case.sent_path,
                }),
            )
            .await;
            assert_eq!(
                &from_base64(&string_at(&read, "/fileContent")),
                content,
                "case {label:?} position {position}: the version's content at {commit_id}"
            );
        }

        // A read does not change what the next caller sees.
        let again = ok(&router, "ListFileCommitHistory", &request).await;
        assert_eq!(
            again, listed,
            "case {label:?}: the history must be repeatable"
        );
    }
}

#[tokio::test]
async fn the_history_pages_by_max_results_and_next_token() {
    let router = emulator();
    let versions = 5usize;
    let case = Case {
        label: "paged",
        repository: "paged-history",
        branch: "main",
        writes: (0..versions)
            .map(|index| ("f.txt", format!("revision {index}").into_bytes()))
            .collect(),
        sent_path: "f.txt",
        stored_path: "f.txt",
    };
    apply(&router, &case).await;

    let unpaged = dag_of(
        &ok(
            &router,
            "ListFileCommitHistory",
            &json!({ "repositoryName": case.repository, "filePath": case.sent_path }),
        )
        .await,
    );
    assert_eq!(
        unpaged.len(),
        versions,
        "the unpaged history holds every version, got {}",
        unpaged.len()
    );

    // Rows: page sizes below, at and above the number of versions.
    for page_size in [1usize, 2, 3, 4, 5, 6, 1000] {
        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut input = json!({
                "repositoryName": case.repository,
                "filePath": case.sent_path,
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                input["nextToken"] = json!(token);
            }
            let page = ok(&router, "ListFileCommitHistory", &input).await;
            let entries = dag_of(&page);
            assert!(
                entries.len() <= page_size,
                "case page size {page_size}: a page holds at most maxResults entries, got {}",
                entries.len()
            );
            collected.extend(entries);
            pages += 1;
            assert!(
                pages <= versions + 1,
                "case page size {page_size}: paging must terminate, got {pages} pages"
            );
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, unpaged,
            "case page size {page_size}: the pages must reassemble the whole DAG in order"
        );
        assert_eq!(
            pages,
            versions.div_ceil(page_size),
            "case page size {page_size}: expected {} page(s), got {pages}",
            versions.div_ceil(page_size)
        );
    }
}

#[tokio::test]
async fn the_page_size_is_bounded_by_the_service_not_by_the_request() {
    use service_com_amazonaws_codecommit_codecommit_20150413::ops::list_file_commit_history::MAX_RESULTS_PER_PAGE;

    let router = emulator();
    // A history two versions longer than the service's own page maximum, so a
    // caller asking for more than that can be told apart from one asking for all.
    let versions = MAX_RESULTS_PER_PAGE + 2;
    let case = Case {
        label: "capped",
        repository: "capped-history",
        branch: "main",
        writes: (0..versions)
            .map(|index| ("f.txt", format!("revision {index}").into_bytes()))
            .collect(),
        sent_path: "f.txt",
        stored_path: "f.txt",
    };
    apply(&router, &case).await;

    // Rows: no page size at all, a page size at the maximum, and ones far above
    // it — up to the largest value the modelled `Limit` (an integer) can carry.
    // Every one is served, and none of them widens the page.
    for requested in [
        None,
        Some(MAX_RESULTS_PER_PAGE),
        Some(10_000usize),
        Some(2_147_483_647usize),
    ] {
        let mut input = json!({
            "repositoryName": case.repository,
            "filePath": case.sent_path,
        });
        if let Some(requested) = requested {
            input["maxResults"] = json!(requested);
        }
        let page = ok(&router, "ListFileCommitHistory", &input).await;
        let entries = dag_of(&page);
        assert_eq!(
            entries.len(),
            MAX_RESULTS_PER_PAGE,
            "case maxResults {requested:?}: a page holds at most the service's own maximum, got {}",
            entries.len()
        );
        assert!(
            page.get("nextToken").and_then(Value::as_str).is_some(),
            "case maxResults {requested:?}: versions remain, so a token must be returned, got {page}"
        );
    }
}

#[tokio::test]
async fn the_commit_specifier_bounds_the_history_it_reports() {
    let router = emulator();
    // Four commits: the file, another file, then the file twice more — so one
    // commit in the middle of the history changed something else.
    let case = Case {
        label: "bounded",
        repository: "bounded-history",
        branch: "main",
        writes: vec![
            ("f.txt", b"one".to_vec()),
            ("other.txt", b"other".to_vec()),
            ("f.txt", b"two".to_vec()),
            ("f.txt", b"three".to_vec()),
        ],
        sent_path: "f.txt",
        stored_path: "f.txt",
    };
    let commits = apply(&router, &case).await;

    // Rows: each kind of specifier — absent (the default branch), the branch name,
    // and every commit id, including the one that changed another file. The
    // expected value is the versions reachable from it, newest first.
    let rows: Vec<(&str, Option<String>, Vec<usize>)> = vec![
        ("absent: the default branch", None, vec![3, 2, 0]),
        ("the branch name", Some(case.branch.to_string()), vec![3, 2, 0]),
        ("the tip commit id", Some(commits[3].clone()), vec![3, 2, 0]),
        ("the second version's commit id", Some(commits[2].clone()), vec![2, 0]),
        (
            "a commit that changed another file",
            Some(commits[1].clone()),
            vec![0],
        ),
        ("the first commit id", Some(commits[0].clone()), vec![0]),
    ];

    for (label, specifier, expected) in rows {
        let mut input = json!({
            "repositoryName": case.repository,
            "filePath": case.sent_path,
        });
        if let Some(specifier) = &specifier {
            input["commitSpecifier"] = json!(specifier);
        }
        let listed = ok(&router, "ListFileCommitHistory", &input).await;
        let reported: Vec<String> = dag_of(&listed)
            .iter()
            .map(|entry| string_at(entry, "/commit/commitId"))
            .collect();
        let expected_ids: Vec<String> = expected
            .iter()
            .map(|index| commits[*index].clone())
            .collect();
        assert_eq!(
            reported, expected_ids,
            "case {label:?}: expected the versions {expected:?} newest first, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_unservable_history_request_is_refused_with_the_modeled_error() {
    let router = emulator();
    let case = Case {
        label: "guarded",
        repository: "guarded-history",
        branch: "main",
        writes: vec![
            ("f.txt", b"one".to_vec()),
            ("f.txt", b"two".to_vec()),
        ],
        sent_path: "f.txt",
        stored_path: "f.txt",
    };
    apply(&router, &case).await;
    // A repository with no commit at all, so no default branch to resolve.
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "empty-history" }),
    )
    .await;

    // Each row is a request that cannot be served and the error named for it.
    // They differ along which part of the request is at fault: the repository, the
    // commit specifier, the page size, the continuation token, the path.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            json!({ "repositoryName": "no-such-repository", "filePath": "f.txt" }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a commit specifier that names nothing",
            json!({
                "repositoryName": case.repository,
                "filePath": "f.txt",
                "commitSpecifier": "no-such-branch",
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a well-formed commit id the repository does not hold",
            json!({
                "repositoryName": case.repository,
                "filePath": "f.txt",
                "commitSpecifier": "0".repeat(40),
            }),
            "CommitDoesNotExistException",
        ),
        (
            "no specifier in a repository with no default branch",
            json!({ "repositoryName": "empty-history", "filePath": "f.txt" }),
            "CommitDoesNotExistException",
        ),
        (
            // "A non-zero, non-negative integer used to limit the number of
            // returned results" (the model's own `maxResults` documentation).
            "a zero page size",
            json!({ "repositoryName": case.repository, "filePath": "f.txt", "maxResults": 0 }),
            "InvalidMaxResultsException",
        ),
        (
            "a negative page size",
            json!({ "repositoryName": case.repository, "filePath": "f.txt", "maxResults": -3 }),
            "InvalidMaxResultsException",
        ),
        (
            "a continuation token that is not one",
            json!({
                "repositoryName": case.repository,
                "filePath": "f.txt",
                "nextToken": "not-a-token",
            }),
            "InvalidContinuationTokenException",
        ),
        (
            "a continuation token past the end of the history",
            json!({
                "repositoryName": case.repository,
                "filePath": "f.txt",
                "nextToken": "99",
            }),
            "InvalidContinuationTokenException",
        ),
        (
            // An offset AT the end is one no response can emit: a token is
            // returned only while versions remain.
            "a continuation token at the end of the history",
            json!({
                "repositoryName": case.repository,
                "filePath": "f.txt",
                "nextToken": "2",
            }),
            "InvalidContinuationTokenException",
        ),
        (
            // This operation models no path error, so a path that cannot name a
            // file in a tree is refused as an invalid request.
            "a path that is only separators",
            json!({ "repositoryName": case.repository, "filePath": "/" }),
            "ValidationException",
        ),
        (
            "a path with a traversal segment",
            json!({ "repositoryName": case.repository, "filePath": "a/../b" }),
            "ValidationException",
        ),
    ];

    for (label, input, expected) in rows {
        let code = error(&router, "ListFileCommitHistory", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code} for {input}"
        );
    }

    // None of the refusals changed the history: it is still the two versions that
    // were written.
    let listed = ok(
        &router,
        "ListFileCommitHistory",
        &json!({ "repositoryName": case.repository, "filePath": "f.txt" }),
    )
    .await;
    assert_eq!(
        dag_of(&listed).len(),
        2,
        "a refused read must leave the history alone, got {listed}"
    );
}
