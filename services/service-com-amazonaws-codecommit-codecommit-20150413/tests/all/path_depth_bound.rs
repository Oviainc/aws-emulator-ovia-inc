// SPDX-License-Identifier: CC0-1.0

//! How deeply a path may nest, over the wire and in the tree builder the writes go
//! through.
//!
//! The rule under test: a path may carry at most `MAX_PATH_SEGMENTS` separator-
//! delimited segments; a request naming a deeper one is refused with the modelled
//! `InvalidPathException` and stores nothing, and the tree builder every write goes
//! through refuses a snapshot nesting deeper than that rather than recursing into
//! it. The bound belongs on BOTH: `crate::state::normalize_path` holds a request's
//! `filePath` to it, and `crate::state::tree_objects` — which is public and also
//! reached for a snapshot assembled from a pushed packfile rather than from a
//! `filePath` — walks one level of recursion per segment, so a path with tens of
//! thousands of separators would otherwise drive the recursion until the thread's
//! stack is exhausted, which in Rust aborts the whole process rather than failing
//! one request.
//!
//! The rows span what the rule generalises over: the depth (the bound itself, one
//! past it, and far past it), and the operation that carries the path (`PutFile`,
//! `CreateCommit` and `GetFile`).

use std::collections::BTreeMap;

use serde_json::json;

use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    tree_id, FileEntry, MAX_PATH_SEGMENTS,
};

use crate::repository_lifecycle::{base64, emulator, error, ok, string_at};

/// A path of `segments` one-character segments: the deepest path that many
/// segments can name.
fn nested_path(segments: usize) -> String {
    (0..segments)
        .map(|_| "d")
        .collect::<Vec<_>>()
        .join("/")
}

/// A path at the bound round-trips; a deeper one is refused, by every operation
/// that takes a `filePath`, and nothing is stored for it.
#[tokio::test]
async fn a_path_deeper_than_the_bound_is_refused() {
    let router = emulator();
    let repository = "repo-for-path-depth";
    ok(
        &router,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;

    // The deepest path the bound allows: accepted, and read back through GetFile.
    let at_bound = nested_path(MAX_PATH_SEGMENTS);
    let written = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": at_bound,
            "fileContent": base64(b"deep"),
        }),
    )
    .await;
    assert!(
        written["commitId"].is_string(),
        "case {MAX_PATH_SEGMENTS} segments: the deepest allowed path must be written, \
         got {written}"
    );
    let read_back = ok(
        &router,
        "GetFile",
        &json!({"repositoryName": repository, "filePath": at_bound}),
    )
    .await;
    assert_eq!(
        string_at(&read_back, "/filePath"),
        at_bound,
        "case {MAX_PATH_SEGMENTS} segments: the stored path must read back, got {read_back}"
    );

    // One past the bound, and far past it — the depth that would exhaust the stack
    // if the walk were unbounded.
    for segments in [MAX_PATH_SEGMENTS + 1, 10_000] {
        let deeper = nested_path(segments);
        let requests: Vec<(&str, serde_json::Value)> = vec![
            (
                "PutFile",
                json!({
                    "repositoryName": repository,
                    "branchName": "main",
                    "filePath": deeper,
                    "fileContent": base64(b"too deep"),
                }),
            ),
            (
                "CreateCommit",
                json!({
                    "repositoryName": repository,
                    "branchName": "main",
                    "putFiles": [{"filePath": deeper, "fileContent": base64(b"too deep")}],
                }),
            ),
            (
                "GetFile",
                json!({"repositoryName": repository, "filePath": deeper}),
            ),
        ];
        for (operation, body) in requests {
            let code = error(&router, operation, &body, 400).await;
            assert_eq!(
                code, "InvalidPathException",
                "case ({operation}, {segments} segments): expected code \
                 InvalidPathException, got {code:?}"
            );
        }
    }

    // The refusals stored nothing: the repository still holds only the file written
    // at the bound.
    let listed = ok(
        &router,
        "GetFolder",
        &json!({"repositoryName": repository, "folderPath": "/"}),
    )
    .await;
    assert_eq!(
        listed["files"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "a refused deep path must store no file at the root, got {listed}"
    );
    assert_eq!(
        listed["subFolders"].as_array().map(Vec::len).unwrap_or(0),
        1,
        "the root must hold exactly the one folder the accepted path created, got {listed}"
    );
}

/// The tree builder itself refuses a snapshot nesting deeper than the bound,
/// answering `None` instead of recursing: this is the bound that protects the
/// process for a snapshot that did not come from a `filePath` (a pushed packfile).
#[test]
fn the_tree_builder_refuses_a_snapshot_deeper_than_the_bound() {
    let entry = || FileEntry {
        // A blob id the builder accepts: 40 hex characters, as `insert_blob` reports.
        blob_id: "0123456789abcdef0123456789abcdef01234567".to_string(),
        file_mode: "NORMAL".to_string(),
        size: 4,
    };

    // Rows: the bound itself, one past it, and far past it — the depth an unbounded
    // walk would exhaust the stack on.
    for (segments, expected_some) in [
        (1usize, true),
        (MAX_PATH_SEGMENTS, true),
        (MAX_PATH_SEGMENTS + 1, false),
        (100_000, false),
    ] {
        let mut files: BTreeMap<String, FileEntry> = BTreeMap::new();
        files.insert(nested_path(segments), entry());
        let built = tree_id(&files);
        assert_eq!(
            built.is_some(),
            expected_some,
            "case {segments} segments: expected tree_id to be {}, got {built:?}",
            if expected_some { "built" } else { "refused" }
        );
    }
}
