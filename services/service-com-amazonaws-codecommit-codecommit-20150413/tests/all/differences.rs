// SPDX-License-Identifier: CC0-1.0

//! `GetDifferences` over the wire, driven as awsJson1_1 requests through the router
//! that serves the real emulator.
//!
//! The rule under test: `GetDifferences` compares the two tree snapshots the commit
//! specifiers resolve to in stored state and reports one `Difference` per path whose
//! blob or mode differs — an addition (A) for a path only the after snapshot holds, a
//! deletion (D) for one only the before snapshot holds, a modification (M) for one
//! both hold with different content, mode or (under a path filter) name — with no
//! before specifier comparing against an empty tree, each `BlobMetadata` carrying that
//! side's blob id, path and numeric git mode, the paths limited to the before/after
//! path when one is given, and the ordered result paged by `MaxResults`/`NextToken`.
//!
//! The tables below span the class that rule covers — which side holds a path, what
//! changed (content, mode, name), the specifier form (branch, fully qualified
//! reference, HEAD, commit id), the path filter's shape (file, folder, renamed folder,
//! leading separator) and the page size — rather than one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsJson1_1 wire harness, and the independent git blob-object name, live once
// for this test crate in the sibling suite that first needed them.
use crate::repository_lifecycle::{base64, emulator, error, git_blob_id, ok, string_at};

/// The numeric git file mode codes `BlobMetadata.mode` documents
/// (`smithy_query(query="doc",shape="BlobMetadata",member="mode")`): `100644`
/// read/write, `100755` read/write/execute, `120000` a symlink.
const MODE_NORMAL: &str = "100644";
const MODE_EXECUTABLE: &str = "100755";

/// One side of a difference as `path@blobId@mode`, or `-` when the difference does
/// not carry that side at all.
fn side(blob: Option<&Value>) -> String {
    match blob {
        None | Some(Value::Null) => "-".to_string(),
        Some(blob) => format!(
            "{}@{}@{}",
            blob.get("path").and_then(Value::as_str).unwrap_or("<none>"),
            blob.get("blobId").and_then(Value::as_str).unwrap_or("<none>"),
            blob.get("mode").and_then(Value::as_str).unwrap_or("<none>"),
        ),
    }
}

/// Every difference in one response, as `changeType|before|after`, in the order the
/// service returned them.
fn summaries(payload: &Value) -> Vec<String> {
    let differences = payload
        .get("differences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a differences list in {payload}"));
    differences
        .iter()
        .map(|difference| {
            format!(
                "{}|{}|{}",
                difference
                    .get("changeType")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>"),
                side(difference.get("beforeBlob")),
                side(difference.get("afterBlob")),
            )
        })
        .collect()
}

fn added(path: &str, content: &[u8], mode: &str) -> String {
    format!("A|-|{path}@{}@{mode}", git_blob_id(content))
}

fn deleted(path: &str, content: &[u8], mode: &str) -> String {
    format!("D|{path}@{}@{mode}|-", git_blob_id(content))
}

fn modified(
    before_path: &str,
    before_content: &[u8],
    before_mode: &str,
    after_path: &str,
    after_content: &[u8],
    after_mode: &str,
) -> String {
    format!(
        "M|{before_path}@{}@{before_mode}|{after_path}@{}@{after_mode}",
        git_blob_id(before_content),
        git_blob_id(after_content)
    )
}

/// A `GetDifferences` request body, with only the members a case names.
fn request(
    repository: &str,
    before: Option<&str>,
    after: &str,
    before_path: Option<&str>,
    after_path: Option<&str>,
) -> Value {
    let mut input = json!({ "repositoryName": repository, "afterCommitSpecifier": after });
    if let Some(before) = before {
        input["beforeCommitSpecifier"] = json!(before);
    }
    if let Some(before_path) = before_path {
        input["beforePath"] = json!(before_path);
    }
    if let Some(after_path) = after_path {
        input["afterPath"] = json!(after_path);
    }
    input
}

const REPOSITORY: &str = "diffs";
const ALPHA: &[u8] = b"alpha\n";
const ALPHA_TWO: &[u8] = b"alpha two\n";
const BEE: &[u8] = b"bee\n";
const SCRIPT: &[u8] = b"#!/bin/sh\necho hi\n";

/// The commit ids of a history built one `PutFile` at a time, oldest first.
struct History {
    router: MultiServiceRouter,
    commits: Vec<String>,
}

impl History {
    /// Six commits on `main`, each a single write, so that every change kind the
    /// rule covers is reachable by picking two of them (in either direction):
    ///
    /// 1. `a.txt` added, 2. `dir/b.txt` added, 3. `run.sh` added (EXECUTABLE),
    /// 4. `a.txt` content changed, 5. `dir/b.txt` mode changed only,
    /// 6. the same content added under a second folder, `moved/b.txt`.
    async fn build() -> Self {
        let router = emulator();
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": REPOSITORY }),
        )
        .await;

        let writes: Vec<(&str, &[u8], Option<&str>)> = vec![
            ("a.txt", ALPHA, None),
            ("dir/b.txt", BEE, None),
            ("run.sh", SCRIPT, Some("EXECUTABLE")),
            ("a.txt", ALPHA_TWO, None),
            ("dir/b.txt", BEE, Some("EXECUTABLE")),
            ("moved/b.txt", BEE, None),
        ];
        let mut commits: Vec<String> = Vec::new();
        for (path, content, mode) in writes {
            let mut input = json!({
                "repositoryName": REPOSITORY,
                "branchName": "main",
                "filePath": path,
                "fileContent": base64(content),
                "commitMessage": format!("write {path}"),
            });
            if let Some(mode) = mode {
                input["fileMode"] = json!(mode);
            }
            if let Some(parent) = commits.last() {
                input["parentCommitId"] = json!(parent);
            }
            let put = ok(&router, "PutFile", &input).await;
            commits.push(string_at(&put, "/commitId"));
        }
        Self { router, commits }
    }

    /// The id of the n-th commit, counting from 1 as the history is described above.
    fn commit(&self, ordinal: usize) -> &str {
        &self.commits[ordinal - 1]
    }
}

#[tokio::test]
async fn differences_are_the_changes_between_two_stored_snapshots() {
    let history = History::build().await;
    let router = &history.router;

    // Each row is a pair of commits and the differences their stored snapshots
    // disagree about. The rows differ along which side holds the path (added,
    // deleted), what changed (content, mode), and whether anything changed at all.
    let cases: Vec<(&str, Option<&str>, &str, Vec<String>)> = vec![
        (
            "no before specifier: the whole snapshot is an addition",
            None,
            history.commit(1),
            vec![added("a.txt", ALPHA, MODE_NORMAL)],
        ),
        (
            "no before specifier, three files",
            None,
            history.commit(3),
            vec![
                added("a.txt", ALPHA, MODE_NORMAL),
                added("dir/b.txt", BEE, MODE_NORMAL),
                added("run.sh", SCRIPT, MODE_EXECUTABLE),
            ],
        ),
        (
            "a file only the after snapshot holds is an addition",
            Some(history.commit(1)),
            history.commit(2),
            vec![added("dir/b.txt", BEE, MODE_NORMAL)],
        ),
        (
            "the same pair reversed: a file only the before snapshot holds is a deletion",
            Some(history.commit(2)),
            history.commit(1),
            vec![deleted("dir/b.txt", BEE, MODE_NORMAL)],
        ),
        (
            "changed content is a modification",
            Some(history.commit(3)),
            history.commit(4),
            vec![modified(
                "a.txt",
                ALPHA,
                MODE_NORMAL,
                "a.txt",
                ALPHA_TWO,
                MODE_NORMAL,
            )],
        ),
        (
            "a changed mode alone is a modification",
            Some(history.commit(4)),
            history.commit(5),
            vec![modified(
                "dir/b.txt",
                BEE,
                MODE_NORMAL,
                "dir/b.txt",
                BEE,
                MODE_EXECUTABLE,
            )],
        ),
        (
            "a snapshot compared with itself differs in nothing",
            Some(history.commit(5)),
            history.commit(5),
            vec![],
        ),
        (
            "several changes at once, ordered by path",
            Some(history.commit(1)),
            history.commit(5),
            vec![
                modified("a.txt", ALPHA, MODE_NORMAL, "a.txt", ALPHA_TWO, MODE_NORMAL),
                added("dir/b.txt", BEE, MODE_EXECUTABLE),
                added("run.sh", SCRIPT, MODE_EXECUTABLE),
            ],
        ),
        (
            "content that already exists elsewhere is still an addition at its own path",
            Some(history.commit(5)),
            history.commit(6),
            vec![added("moved/b.txt", BEE, MODE_NORMAL)],
        ),
    ];

    for (label, before, after, expected) in cases {
        let input = request(REPOSITORY, before, after, None, None);
        let payload = ok(router, "GetDifferences", &input).await;
        let got = summaries(&payload);
        assert_eq!(
            got, expected,
            "case {label:?}: expected {expected:?}, got {got:?}"
        );
        assert!(
            payload.get("NextToken").is_none_or(Value::is_null),
            "case {label:?}: a single unpaged page must not carry a continuation token, got {payload}"
        );
        // A read never changes what the next caller sees.
        let again = ok(router, "GetDifferences", &input).await;
        assert_eq!(
            again, payload,
            "case {label:?}: GetDifferences must be repeatable"
        );
    }
}

#[tokio::test]
async fn every_commit_specifier_form_resolves_to_the_same_snapshot() {
    let history = History::build().await;
    let router = &history.router;
    let tip = history.commit(6).to_string();

    // `CommitName` is "the branch, tag, HEAD, or other fully qualified reference
    // used to identify a commit": each form below names the tip of `main`, which is
    // also this repository's default branch, so each must produce the same
    // comparison as the tip's own commit id.
    let expected = vec![added("moved/b.txt", BEE, MODE_NORMAL)];
    for specifier in ["main", "refs/heads/main", "HEAD", tip.as_str()] {
        let payload = ok(
            router,
            "GetDifferences",
            &request(REPOSITORY, Some(history.commit(5)), specifier, None, None),
        )
        .await;
        let got = summaries(&payload);
        assert_eq!(
            got, expected,
            "case {specifier:?}: expected {expected:?}, got {got:?}"
        );
    }

    // The before specifier resolves through the same forms.
    let payload = ok(
        router,
        "GetDifferences",
        &request(REPOSITORY, Some("refs/heads/main"), "HEAD", None, None),
    )
    .await;
    let got = summaries(&payload);
    assert!(
        got.is_empty(),
        "case \"both sides name the tip\": expected no differences, got {got:?}"
    );
}

/// One path-filter row: a label, the before and after specifiers, the `beforePath`
/// and `afterPath` the request carries, and the differences expected back.
struct PathCase<'a>(
    &'a str,
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Vec<String>,
);

#[tokio::test]
async fn a_path_limits_the_results_to_that_file_or_folder() {
    let history = History::build().await;
    let router = &history.router;

    // Rows differ along the filter's shape: one side given or both, a file or a
    // folder, a leading separator, and a folder compared under a previous name.
    let cases: Vec<PathCase> = vec![
        PathCase(
            "afterPath names one file",
            Some(history.commit(1)),
            history.commit(5),
            None,
            Some("a.txt"),
            vec![modified(
                "a.txt",
                ALPHA,
                MODE_NORMAL,
                "a.txt",
                ALPHA_TWO,
                MODE_NORMAL,
            )],
        ),
        PathCase(
            "a leading separator addresses the same file",
            Some(history.commit(1)),
            history.commit(5),
            None,
            Some("/a.txt"),
            vec![modified(
                "a.txt",
                ALPHA,
                MODE_NORMAL,
                "a.txt",
                ALPHA_TWO,
                MODE_NORMAL,
            )],
        ),
        PathCase(
            "beforePath alone limits both sides to that folder",
            Some(history.commit(1)),
            history.commit(5),
            Some("dir"),
            None,
            vec![added("dir/b.txt", BEE, MODE_EXECUTABLE)],
        ),
        PathCase(
            "the same folder on both sides",
            Some(history.commit(2)),
            history.commit(5),
            Some("dir"),
            Some("dir"),
            vec![modified(
                "dir/b.txt",
                BEE,
                MODE_NORMAL,
                "dir/b.txt",
                BEE,
                MODE_EXECUTABLE,
            )],
        ),
        PathCase(
            "a folder written with a trailing separator addresses the same folder",
            Some(history.commit(2)),
            history.commit(5),
            Some("dir/"),
            None,
            vec![modified(
                "dir/b.txt",
                BEE,
                MODE_NORMAL,
                "dir/b.txt",
                BEE,
                MODE_EXECUTABLE,
            )],
        ),
        PathCase(
            "a folder compared under its previous name pairs entry by entry",
            Some(history.commit(5)),
            history.commit(6),
            Some("dir"),
            Some("moved"),
            vec![modified(
                "dir/b.txt",
                BEE,
                MODE_EXECUTABLE,
                "moved/b.txt",
                BEE,
                MODE_NORMAL,
            )],
        ),
        PathCase(
            "a path with no before specifier",
            None,
            history.commit(6),
            None,
            Some("run.sh"),
            vec![added("run.sh", SCRIPT, MODE_EXECUTABLE)],
        ),
    ];

    for PathCase(label, before, after, before_path, after_path, expected) in cases {
        let payload = ok(
            router,
            "GetDifferences",
            &request(REPOSITORY, before, after, before_path, after_path),
        )
        .await;
        let got = summaries(&payload);
        assert_eq!(
            got, expected,
            "case {label:?}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn max_results_pages_the_ordered_differences() {
    let history = History::build().await;
    let router = &history.router;

    // Every file at the tip, as additions against an empty before tree: four
    // differences, ordered by path.
    let whole: Vec<String> = vec![
        added("a.txt", ALPHA_TWO, MODE_NORMAL),
        added("dir/b.txt", BEE, MODE_EXECUTABLE),
        added("moved/b.txt", BEE, MODE_NORMAL),
        added("run.sh", SCRIPT, MODE_EXECUTABLE),
    ];
    let unpaged = ok(
        router,
        "GetDifferences",
        &request(REPOSITORY, None, history.commit(6), None, None),
    )
    .await;
    let got = summaries(&unpaged);
    assert_eq!(
        got, whole,
        "case \"unpaged\": expected {whole:?}, got {got:?}"
    );

    // Page sizes below, at and above the result count: every page is at most
    // `MaxResults` long, the pages concatenate to exactly the unpaged result, and
    // only a page with more to come carries a continuation token.
    for max_results in [1usize, 2, 3, 4, 10] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut input = request(REPOSITORY, None, history.commit(6), None, None);
            input["MaxResults"] = json!(max_results);
            if let Some(token) = &token {
                input["NextToken"] = json!(token);
            }
            let payload = ok(router, "GetDifferences", &input).await;
            let page = summaries(&payload);
            assert!(
                page.len() <= max_results,
                "case maxResults={max_results}: a page must hold at most {max_results} differences, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case maxResults={max_results}: paging must terminate, got {collected:?}"
            );
            token = payload
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let more = collected.len() < whole.len();
            assert_eq!(
                token.is_some(),
                more,
                "case maxResults={max_results}: a continuation token must be present exactly while differences remain, got {payload}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, whole,
            "case maxResults={max_results}: the pages must concatenate to the whole result, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_names_the_modeled_error() {
    let history = History::build().await;
    let router = &history.router;
    let tip = history.commit(6).to_string();

    // Each row is a request that cannot be served and the error the model names for
    // it, differing along which part of the request is at fault: the repository, the
    // commit specifier, the path, the page size, the continuation token.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository the store does not hold",
            request(
                "no-such-repository",
                None,
                tip.as_str(),
                None,
                None,
            ),
            "RepositoryDoesNotExistException",
        ),
        (
            "an after commit id the repository does not hold",
            request(REPOSITORY, None, &"0".repeat(40), None, None),
            "CommitDoesNotExistException",
        ),
        (
            "an after specifier that names no branch",
            request(REPOSITORY, None, "refs/heads/nowhere", None, None),
            "CommitDoesNotExistException",
        ),
        (
            "a blank after specifier names no commit",
            request(REPOSITORY, None, "   ", None, None),
            "CommitRequiredException",
        ),
        (
            "a before commit id the repository does not hold",
            request(
                REPOSITORY,
                Some(&"f".repeat(40)),
                tip.as_str(),
                None,
                None,
            ),
            "CommitDoesNotExistException",
        ),
        (
            "a path that exists in neither snapshot",
            request(REPOSITORY, None, tip.as_str(), None, Some("nowhere.txt")),
            "PathDoesNotExistException",
        ),
        (
            "a path that cannot name an entry in a tree",
            request(REPOSITORY, None, tip.as_str(), Some("a/../b"), None),
            "InvalidPathException",
        ),
        (
            "zero is not a page size",
            {
                let mut input = request(REPOSITORY, None, tip.as_str(), None, None);
                input["MaxResults"] = json!(0);
                input
            },
            "InvalidMaxResultsException",
        ),
        (
            "a negative page size",
            {
                let mut input = request(REPOSITORY, None, tip.as_str(), None, None);
                input["MaxResults"] = json!(-3);
                input
            },
            "InvalidMaxResultsException",
        ),
        (
            "a continuation token that is not one this operation issues",
            {
                let mut input = request(REPOSITORY, None, tip.as_str(), None, None);
                input["NextToken"] = json!("not-a-token");
                input
            },
            "InvalidContinuationTokenException",
        ),
        (
            "a continuation token past the end of the result",
            {
                let mut input = request(REPOSITORY, None, tip.as_str(), None, None);
                input["NextToken"] = json!("4096");
                input
            },
            "InvalidContinuationTokenException",
        ),
    ];

    for (label, input, expected) in cases {
        let code = error(router, "GetDifferences", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }

    // None of the refusals, and none of the reads above, changed the repository: the
    // tip is still the commit the last write produced.
    let branch = ok(
        router,
        "GetBranch",
        &json!({ "repositoryName": REPOSITORY, "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "GetDifferences must not change the branch tip"
    );
}
