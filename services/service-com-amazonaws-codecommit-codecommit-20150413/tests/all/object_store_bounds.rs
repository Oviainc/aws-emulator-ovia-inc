// SPDX-License-Identifier: CC0-1.0

//! What one repository's git object store may hold, over the wire through the
//! router that serves the real emulator.
//!
//! The rule under test: a write that stores git objects is refused when it would
//! take the repository's object store PAST the emulator's own bound on it — in
//! objects (`MAX_OBJECTS_PER_REPOSITORY`) or in object bytes
//! (`MAX_OBJECT_BYTES_PER_REPOSITORY`) — and the refused request stores nothing,
//! whichever operation makes the write. Every face charges the bound with that
//! same comparison: `PutFile`, `CreateCommit` and a pushed packfile. The bound is on the STORE rather than on
//! one request, because an object is retained for as long as a ref reaches it: a
//! branch tip pins its whole parent chain, so nothing evicts what successive
//! commits wrote and the number of requests is the axis a caller drives.
//!
//! The rows span what the rule generalises over: which cap is reached (the object
//! count, the object bytes, and the bytes the incoming write itself would add),
//! and which face makes the write (`PutFile`, one blob per call, and
//! `CreateCommit`, many) — plus the control row every bound needs, a store below
//! the cap, where the same calls succeed and round-trip through `GetFile`.

use serde_json::json;

use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    ComAmazonawsCodecommitCodecommit20150413State as Store, MAX_OBJECTS_PER_REPOSITORY,
    MAX_OBJECT_BYTES_PER_REPOSITORY,
};

use crate::repository_lifecycle::{base64, emulator_with_engine, error, from_base64, ok, string_at};

/// How full the store is before the write under test.
#[derive(Debug, Clone, Copy)]
enum Fill {
    /// Nothing stored: the control.
    Empty,
    /// Exactly as many objects as the store holds.
    AtObjectCap,
    /// All but `headroom` bytes of what the store holds, so a write larger than
    /// the headroom is what would take it past the cap.
    BytesWithHeadroom(usize),
}

/// Put `fill` into the named repository's object store, directly in state — the
/// bound is the store's, so the test sets the store up rather than spending
/// thousands of requests to reach it, and then drives the WRITE over the wire.
fn fill_store(engine: &std::sync::Arc<engine::InMemoryEngine>, name: &str, fill: Fill) {
    let bound = engine.state::<Store>();
    let mut store = bound.lock();
    let repository = store
        .repositories
        .get_mut(name)
        .unwrap_or_else(|| panic!("repository {name:?} must have been created"));
    match fill {
        Fill::Empty => {}
        Fill::AtObjectCap => {
            // Distinct content, so each is its own object (a blob's name is the
            // SHA-1 of its bytes, so equal content is ONE object).
            for index in 0..MAX_OBJECTS_PER_REPOSITORY {
                repository.insert_blob(format!("filler {index}\n").into_bytes());
            }
        }
        Fill::BytesWithHeadroom(headroom) => {
            repository.insert_blob(vec![b'x'; MAX_OBJECT_BYTES_PER_REPOSITORY - headroom]);
        }
    }
}

/// The named repository's object count and total object bytes.
fn store_size(engine: &std::sync::Arc<engine::InMemoryEngine>, name: &str) -> (usize, usize) {
    let bound = engine.state::<Store>();
    let store = bound.lock();
    let repository = store
        .repositories
        .get(name)
        .unwrap_or_else(|| panic!("repository {name:?} must have been created"));
    (repository.objects.len(), repository.object_bytes())
}

/// The branch names the named repository holds.
fn branches(engine: &std::sync::Arc<engine::InMemoryEngine>, name: &str) -> Vec<String> {
    let bound = engine.state::<Store>();
    let store = bound.lock();
    store
        .repositories
        .get(name)
        .map(|repository| repository.branches.keys().cloned().collect())
        .unwrap_or_default()
}

/// Which operation makes the write, and the request body that writes `content` at
/// `path` on `branch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    PutFile,
    CreateCommit,
}

impl Writer {
    fn operation(self) -> &'static str {
        match self {
            Writer::PutFile => "PutFile",
            Writer::CreateCommit => "CreateCommit",
        }
    }

    fn body(
        self,
        repository: &str,
        branch: &str,
        path: &str,
        content: &[u8],
    ) -> serde_json::Value {
        match self {
            Writer::PutFile => json!({
                "repositoryName": repository,
                "branchName": branch,
                "filePath": path,
                "fileContent": base64(content),
            }),
            // Two entries, so this face writes MORE than one blob per call: what
            // the bound counts is what the request would add, not the call.
            Writer::CreateCommit => json!({
                "repositoryName": repository,
                "branchName": branch,
                "putFiles": [
                    { "filePath": path, "fileContent": base64(content) },
                    {
                        "filePath": format!("{path}.second"),
                        "fileContent": base64(&[content, b" second"].concat()),
                    },
                ],
            }),
        }
    }
}

#[tokio::test]
async fn a_write_is_refused_once_the_object_store_would_reach_its_bound() {
    // Rows: the label, how full the store is, and how large the attempted write
    // is. Every row is tried through BOTH writing operations.
    let cases: Vec<(&str, Fill, usize)> = vec![
        ("the store is at its object count", Fill::AtObjectCap, 16),
        ("the store is at its byte cap", Fill::BytesWithHeadroom(0), 16),
        // The write itself is what crosses the cap: the store is below it, and
        // the bound is charged on what this request would ADD.
        (
            "the write itself crosses the byte cap",
            Fill::BytesWithHeadroom(1_024),
            4_096,
        ),
    ];

    for (label, fill, write_bytes) in cases {
        for writer in [Writer::PutFile, Writer::CreateCommit] {
            let case = format!("{label} / {}", writer.operation());
            let (router, engine) = emulator_with_engine();
            let repository = "bounded-objects";
            ok(
                &router,
                "CreateRepository",
                &json!({ "repositoryName": repository }),
            )
            .await;
            fill_store(&engine, repository, fill);
            let before = store_size(&engine, repository);

            let code = error(
                &router,
                writer.operation(),
                &writer.body(
                    repository,
                    "main",
                    "docs/readme.md",
                    &vec![b'w'; write_bytes],
                ),
                400,
            )
            .await;
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: the store's bound must refuse the write, got {code}"
            );

            let after = store_size(&engine, repository);
            assert_eq!(
                after, before,
                "case {case:?}: the refused write must store nothing, store went from \
                 {before:?} to {after:?}"
            );
            let stored_branches = branches(&engine, repository);
            assert!(
                stored_branches.is_empty(),
                "case {case:?}: the refused write must create no branch, got {stored_branches:?}"
            );
            let code = error(
                &router,
                "GetFile",
                &json!({ "repositoryName": repository, "filePath": "docs/readme.md" }),
                400,
            )
            .await;
            assert_eq!(
                code, "CommitDoesNotExistException",
                "case {case:?}: the refused write must leave nothing to read, got {code}"
            );
        }
    }
}

#[tokio::test]
async fn a_write_below_the_bound_is_stored_and_reads_back() {
    // The control every bound needs: below the cap — including a store that is
    // already most of the way to the byte cap — the same calls succeed and what
    // they wrote round-trips through `GetFile`.
    let fills: Vec<(&str, Fill)> = vec![
        ("an empty store", Fill::Empty),
        (
            "a store just below the byte cap",
            Fill::BytesWithHeadroom(1_024 * 1_024),
        ),
    ];

    for (label, fill) in fills {
        for writer in [Writer::PutFile, Writer::CreateCommit] {
            let case = format!("{label} / {}", writer.operation());
            let (router, engine) = emulator_with_engine();
            let repository = "within-bounds";
            ok(
                &router,
                "CreateRepository",
                &json!({ "repositoryName": repository }),
            )
            .await;
            fill_store(&engine, repository, fill);

            let content = b"the content a write below the bound stores\n";
            let written = ok(
                &router,
                writer.operation(),
                &writer.body(repository, "main", "docs/readme.md", content),
            )
            .await;
            let commit_id = string_at(&written, "/commitId");
            assert_eq!(
                commit_id.len(),
                40,
                "case {case:?}: the write must report a full commit id, got {commit_id:?}"
            );
            let read = ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "filePath": "docs/readme.md",
                    "commitSpecifier": "main",
                }),
            )
            .await;
            assert_eq!(
                from_base64(&string_at(&read, "/fileContent")),
                content.to_vec(),
                "case {case:?}: the stored file must read back byte for byte"
            );
            assert_eq!(
                branches(&engine, repository),
                vec!["main".to_string()],
                "case {case:?}: the write must create the branch it names"
            );
        }
    }
}
