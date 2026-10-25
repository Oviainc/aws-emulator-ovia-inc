// SPDX-License-Identifier: CC0-1.0

//! `CreateUnreferencedMergeCommit` over the wire, driven as awsJson1_1 requests
//! through the router that serves the real emulator.
//!
//! The rule under test: the operation merges the source commit into the destination
//! commit — three-way, against the tree of the two commits' merge base, so a path
//! only one side changed takes that side's version, a path both sides changed the
//! same way takes the agreed version, and a path they changed differently is a
//! conflict settled by `conflictDetailLevel`, `conflictResolutionStrategy` and
//! `conflictResolution` or else refused — and writes the result as a commit NO
//! reference names, reporting its `commitId` and `treeId`. The merge option decides
//! that commit's parents and not its tree: `THREE_WAY_MERGE` records both tips (the
//! destination first) while `SQUASH_MERGE` records the destination tip alone, so the
//! two options over one pair of commits agree about `treeId` and differ about
//! `commitId`.
//!
//! The table below spans what the rule generalises over rather than one recorded
//! request: which side changed a path (one, both identically, both differently,
//! one deleting it), the kind of change (content, file mode, deletion, a new file,
//! a nested path), the conflict detail level (default, `FILE_LEVEL`, `LINE_LEVEL`),
//! every `conflictResolutionStrategy` the model declares, every
//! `replacementType` a `conflictResolution` entry can carry, `keepEmptyFolders`,
//! and a path one side holds as a file while the other holds it as a folder. Each
//! row's expectation is the merged tree this file computes from the rule, read back
//! through `GetFolder` as `(blobId, fileMode)` per path — content-addressed, so the
//! service's answer is checked against the content the rule says the merge produces.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_codecommit_codecommit_20150413::state::merge::{
    MAX_MERGE_FILE_BYTES, MAX_MERGE_FILE_LINES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::repository_lifecycle::{
    base64, emulator, error, from_base64, git_blob_id, is_object_id, ok, string_at,
};

/// The `fileMode` values used below, from the model's `FileModeTypeEnum`
/// (`smithy_query(query="shape",shape="FileModeTypeEnum")`).
const NORMAL: &str = "NORMAL";
const EXECUTABLE: &str = "EXECUTABLE";

/// The placeholder `keepEmptyFolders` writes into a folder a commit emptied
/// ("a .gitkeep file is created for empty folders").
const GITKEEP: &str = ".gitkeep";

/// One change a branch makes to the base tree.
#[derive(Debug, Clone)]
enum Change {
    /// Write a file — adding it, or replacing its content — in `NORMAL` mode.
    Put(&'static str, &'static str),
    /// Write a file with an explicit mode, which is how a mode-only change is made.
    PutMode(&'static str, &'static str, &'static str),
    /// Delete a file the branch holds.
    Delete(&'static str),
}

/// What the merge must answer for one row.
#[derive(Debug, Clone)]
enum Expected {
    /// The merged tree: every path the result holds, with its content and mode.
    Tree(Vec<(&'static str, &'static str, &'static str)>),
    /// The modelled error code the request is refused with.
    Refused(&'static str),
}

/// One row: a base commit, the change each side makes to it, the merge request's
/// conflict members, and what the merge must produce.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    base: Vec<(&'static str, &'static str)>,
    destination: Vec<Change>,
    source: Vec<Change>,
    /// `conflictDetailLevel` as sent; `None` leaves the member out, which is the
    /// documented `FILE_LEVEL` default.
    detail_level: Option<&'static str>,
    /// `conflictResolutionStrategy` as sent; `None` leaves it out, which is the
    /// documented `NONE` default.
    strategy: Option<&'static str>,
    /// `conflictResolution` as sent.
    resolution: Option<Value>,
    keep_empty_folders: bool,
    expected: Expected,
}

impl Row {
    /// A row with the defaults: no conflict members, no kept folders.
    fn new(
        case: &'static str,
        base: Vec<(&'static str, &'static str)>,
        destination: Vec<Change>,
        source: Vec<Change>,
        expected: Expected,
    ) -> Self {
        Self {
            case,
            base,
            destination,
            source,
            detail_level: None,
            strategy: None,
            resolution: None,
            keep_empty_folders: false,
            expected,
        }
    }

    fn with_detail_level(mut self, level: &'static str) -> Self {
        self.detail_level = Some(level);
        self
    }

    fn with_strategy(mut self, strategy: &'static str) -> Self {
        self.strategy = Some(strategy);
        self
    }

    fn with_resolution(mut self, resolution: Value) -> Self {
        self.resolution = Some(resolution);
        self
    }

    fn keeping_empty_folders(mut self) -> Self {
        self.keep_empty_folders = true;
        self
    }
}

/// A five-line file, the base of the line-level rows below.
const FIVE_LINES: &str = "one\ntwo\nthree\nfour\nfive\n";

/// The table.
fn rows() -> Vec<Row> {
    vec![
        // Each side adds a file the other does not touch: both land.
        Row::new(
            "disjoint-additions",
            vec![("README.md", "# base\n")],
            vec![Change::Put("dest/only.txt", "destination\n")],
            vec![Change::Put("src/only.txt", "source\n")],
            Expected::Tree(vec![
                ("README.md", "# base\n", NORMAL),
                ("dest/only.txt", "destination\n", NORMAL),
                ("src/only.txt", "source\n", NORMAL),
            ]),
        ),
        // Only the destination changed a path: the result takes the destination's
        // version even though the source still holds the base version.
        Row::new(
            "one-sided-change",
            vec![("a.txt", "one\n"), ("keep.txt", "keep\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![],
            Expected::Tree(vec![
                ("a.txt", "destination\n", NORMAL),
                ("keep.txt", "keep\n", NORMAL),
            ]),
        ),
        // Only the source changed a path, and it changed a NESTED one.
        Row::new(
            "one-sided-nested-change",
            vec![("deep/a/b/c.txt", "one\n")],
            vec![],
            vec![Change::Put("deep/a/b/c.txt", "source\n")],
            Expected::Tree(vec![("deep/a/b/c.txt", "source\n", NORMAL)]),
        ),
        // Both sides made the SAME change: an agreement, not a conflict.
        Row::new(
            "both-sides-agree",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "two\n")],
            vec![Change::Put("a.txt", "two\n")],
            Expected::Tree(vec![("a.txt", "two\n", NORMAL)]),
        ),
        // One side deleted a path the other left alone: the deletion is the change,
        // so it lands.
        Row::new(
            "one-sided-deletion",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![Change::Delete("b/c.txt")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "source\n", NORMAL)]),
        ),
        // The same deletion, with `keepEmptyFolders`: the folder the deletion
        // emptied is kept with the .gitkeep placeholder.
        Row::new(
            "one-sided-deletion-keeping-folders",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![Change::Delete("b/c.txt")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![
                ("a.txt", "source\n", NORMAL),
                ("b/.gitkeep", "", NORMAL),
            ]),
        )
        .keeping_empty_folders(),
        // A mode-only change on one side: the mode is part of the entry, so it
        // merges like content does.
        Row::new(
            "one-sided-mode-change",
            vec![("run.sh", "#!/bin/sh\n")],
            vec![Change::PutMode("run.sh", "#!/bin/sh\n", EXECUTABLE)],
            vec![Change::Put("other.txt", "source\n")],
            Expected::Tree(vec![
                ("other.txt", "source\n", NORMAL),
                ("run.sh", "#!/bin/sh\n", EXECUTABLE),
            ]),
        ),
        // Both sides changed one file differently, at the default FILE_LEVEL
        // detail: "a not-mergeable result if the same file has differences in both
        // branches".
        Row::new(
            "file-level-conflict",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        ),
        // The same pair of changes with FILE_LEVEL named explicitly.
        Row::new(
            "file-level-conflict-named",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_detail_level("FILE_LEVEL"),
        // One side deleted the file the other changed: neither version can be
        // merged into the other.
        Row::new(
            "delete-against-change-conflict",
            vec![("a.txt", "one\n"), ("b.txt", "b\n")],
            vec![Change::Delete("a.txt")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        ),
        // ACCEPT_SOURCE on that conflict: "which branch to use when resolving
        // conflicts".
        Row::new(
            "accept-source",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "source\n", NORMAL)]),
        )
        .with_strategy("ACCEPT_SOURCE"),
        // ACCEPT_DESTINATION on the same conflict.
        Row::new(
            "accept-destination",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "destination\n", NORMAL)]),
        )
        .with_strategy("ACCEPT_DESTINATION"),
        // ACCEPT_SOURCE where the source DELETED the conflicting file: the version
        // it keeps is "no file", so the merge result does not hold the path.
        Row::new(
            "accept-source-deletion",
            vec![("a.txt", "one\n"), ("b.txt", "b\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Delete("a.txt")],
            Expected::Tree(vec![("b.txt", "b\n", NORMAL)]),
        )
        .with_strategy("ACCEPT_SOURCE"),
        // NONE is the default, named explicitly: conflicts are still refused.
        Row::new(
            "strategy-none-named",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_strategy("NONE"),
        // LINE_LEVEL detail with the two sides changing DIFFERENT lines: not a
        // conflict, and the merged file carries both changes.
        Row::new(
            "line-level-disjoint-lines",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "ONE\ntwo\nthree\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nthree\nfour\nFIVE\n")],
            Expected::Tree(vec![("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n", NORMAL)]),
        )
        .with_detail_level("LINE_LEVEL"),
        // LINE_LEVEL detail with both sides changing the SAME line: "a conflict is
        // considered not mergeable if the same file in both branches has
        // differences on the same line".
        Row::new(
            "line-level-same-line",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "one\ntwo\nTHREE-DEST\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nTHREE-SRC\nfour\nfive\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_detail_level("LINE_LEVEL"),
        // AUTOMERGE at the default FILE_LEVEL detail: the strategy itself is
        // "whether to attempt automatically merging two versions of a file", so
        // disjoint line changes merge.
        Row::new(
            "automerge-disjoint-lines",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "ONE\ntwo\nthree\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nthree\nfour\nFIVE\n")],
            Expected::Tree(vec![("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE"),
        // AUTOMERGE with nothing to resolve a same-line conflict with: still a
        // manual merge.
        Row::new(
            "automerge-unresolved",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "one\nDEST\nthree\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\nSRC\nthree\nfour\nfive\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_strategy("AUTOMERGE"),
        // AUTOMERGE with USE_NEW_CONTENT: the caller's own bytes settle the path.
        Row::new(
            "automerge-use-new-content",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "resolved by hand\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{
                "filePath": "a.txt",
                "replacementType": "USE_NEW_CONTENT",
                "content": base64(b"resolved by hand\n"),
            }],
        })),
        // AUTOMERGE with USE_NEW_CONTENT and a file mode: "the file mode to apply
        // during conflict resolution".
        Row::new(
            "automerge-use-new-content-with-mode",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "#!/bin/sh\n", EXECUTABLE)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{
                "filePath": "a.txt",
                "replacementType": "USE_NEW_CONTENT",
                "content": base64(b"#!/bin/sh\n"),
                "fileMode": EXECUTABLE,
            }],
        })),
        // AUTOMERGE with KEEP_SOURCE.
        Row::new(
            "automerge-keep-source",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "source\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_SOURCE" }],
        })),
        // AUTOMERGE with KEEP_DESTINATION.
        Row::new(
            "automerge-keep-destination",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "destination\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_DESTINATION" }],
        })),
        // AUTOMERGE with KEEP_BASE: the version the merge base held.
        Row::new(
            "automerge-keep-base",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "one\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_BASE" }],
        })),
        // AUTOMERGE with a deletion: "files to be deleted as part of the merge
        // conflict resolution".
        Row::new(
            "automerge-delete-file",
            vec![("a.txt", "one\n"), ("b.txt", "b\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("b.txt", "b\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({ "deleteFiles": [{ "filePath": "a.txt" }] })),
        // AUTOMERGE with a file mode set on a path the merge settled on its own:
        // "file modes that are set as part of the merge conflict resolution".
        Row::new(
            "automerge-set-file-mode",
            vec![("a.txt", "one\n"), ("run.sh", "#!/bin/sh\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("other.txt", "source\n")],
            Expected::Tree(vec![
                ("a.txt", "destination\n", NORMAL),
                ("other.txt", "source\n", NORMAL),
                ("run.sh", "#!/bin/sh\n", EXECUTABLE),
            ]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "setFileModes": [{ "filePath": "run.sh", "fileMode": EXECUTABLE }],
        })),
        // A conflict resolution list under any other strategy names a branch and a
        // resolution at once, which the merge cannot honour.
        Row::new(
            "resolution-without-automerge",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("InvalidConflictResolutionException"),
        )
        .with_strategy("ACCEPT_SOURCE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_SOURCE" }],
        })),
        // One path, held as a FILE by one side and as a FOLDER by the other: a git
        // tree holds one or the other under a name, so no strategy settles it.
        Row::new(
            "file-against-folder",
            vec![("z.txt", "z\n")],
            vec![Change::Put("x", "a file\n")],
            vec![Change::Put("x/y.txt", "a folder\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_strategy("AUTOMERGE"),
        // The same collision, settled by a resolution that deletes the FILE side:
        // the folder side is representable, so the merge completes.
        Row::new(
            "file-against-folder-resolved-to-the-folder",
            vec![("z.txt", "z\n")],
            vec![Change::Put("x", "a file\n")],
            vec![Change::Put("x/y.txt", "a folder\n")],
            Expected::Tree(vec![
                ("x/y.txt", "a folder\n", NORMAL),
                ("z.txt", "z\n", NORMAL),
            ]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({ "deleteFiles": [{ "filePath": "x" }] })),
        // And settled the other way, by deleting the FOLDER side's entry.
        Row::new(
            "file-against-folder-resolved-to-the-file",
            vec![("z.txt", "z\n")],
            vec![Change::Put("x", "a file\n")],
            vec![Change::Put("x/y.txt", "a folder\n")],
            Expected::Tree(vec![("x", "a file\n", NORMAL), ("z.txt", "z\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({ "deleteFiles": [{ "filePath": "x/y.txt" }] })),
        // A resolution entry that writes a FILE at a path the merged tree holds as
        // a FOLDER is the same file-against-folder collision, arrived at through
        // the resolution: a git tree cannot carry both, so the merge is refused
        // rather than committed. The two sides conflict about nothing here, so
        // this refusal can only come from the resolved result.
        Row::new(
            "resolution-file-against-folder",
            vec![("x/y.txt", "nested\n"), ("a.txt", "one\n")],
            vec![Change::Put("d.txt", "destination\n")],
            vec![Change::Put("s.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{
                "filePath": "x",
                "replacementType": "USE_NEW_CONTENT",
                "content": base64(b"a file where a folder is\n"),
            }],
        })),
        // Two entries for one path: "a conflict can have only one conflict
        // resolution entry".
        Row::new(
            "duplicate-resolution-entries",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("MultipleConflictResolutionEntriesException"),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [
                { "filePath": "a.txt", "replacementType": "KEEP_SOURCE" },
                { "filePath": "a.txt", "replacementType": "KEEP_DESTINATION" },
            ],
        })),
        // USE_NEW_CONTENT with no content at all.
        Row::new(
            "replacement-content-missing",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ReplacementContentRequiredException"),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "USE_NEW_CONTENT" }],
        })),
        // A resolution entry naming a path that is not a path at all.
        Row::new(
            "resolution-path-invalid",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("InvalidPathException"),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a/../b.txt", "replacementType": "KEEP_SOURCE" }],
        })),
    ]
}

/// Write one commit on `branch` from `changes`, reporting its commit id. `parent`
/// is the commit the branch is at, which `CreateCommit` requires for a branch that
/// already has a tip.
async fn commit(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    parent: Option<&str>,
    changes: &[Change],
    message: &str,
) -> String {
    let mut puts: Vec<Value> = Vec::new();
    let mut deletes: Vec<Value> = Vec::new();
    for change in changes {
        match change {
            Change::Put(path, content) => puts.push(json!({
                "filePath": path,
                "fileContent": base64(content.as_bytes()),
            })),
            Change::PutMode(path, content, mode) => puts.push(json!({
                "filePath": path,
                "fileContent": base64(content.as_bytes()),
                "fileMode": mode,
            })),
            Change::Delete(path) => deletes.push(json!({ "filePath": path })),
        }
    }
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "commitMessage": message,
        "authorName": "Merge Tester",
        "email": "merge.tester@example.com",
    });
    if !puts.is_empty() {
        input["putFiles"] = Value::Array(puts);
    }
    if !deletes.is_empty() {
        input["deleteFiles"] = Value::Array(deletes);
    }
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    string_at(&ok(router, "CreateCommit", &input).await, "/commitId")
}

/// The whole tree of a commit, as `path -> (blobId, fileMode)`, walked through
/// `GetFolder` from the root. Reading the tree back this way makes the assertion
/// content-addressed: a `blobId` is the git object name of the file's bytes, which
/// this file computes independently with `git_blob_id`.
async fn tree_of(
    router: &MultiServiceRouter,
    repository: &str,
    specifier: &str,
) -> BTreeMap<String, (String, String)> {
    let mut files: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut folders: Vec<String> = vec![String::new()];
    while let Some(folder) = folders.pop() {
        let payload = ok(
            router,
            "GetFolder",
            &json!({
                "repositoryName": repository,
                "commitSpecifier": specifier,
                "folderPath": folder,
            }),
        )
        .await;
        for member in ["files", "symbolicLinks"] {
            for entry in payload
                .get(member)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                files.insert(
                    string_at(&entry, "/absolutePath"),
                    (
                        string_at(&entry, "/blobId"),
                        string_at(&entry, "/fileMode"),
                    ),
                );
            }
        }
        for entry in payload
            .get("subFolders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            folders.push(string_at(&entry, "/absolutePath"));
        }
    }
    files
}

/// The merge request for one row, under `merge_option`.
fn merge_request(row: &Row, repository: &str, merge_option: &str) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "sourceCommitSpecifier": "feature",
        "destinationCommitSpecifier": "main",
        "mergeOption": merge_option,
        "commitMessage": format!("merge for {}", row.case),
        "authorName": "Merge Tester",
        "email": "merge.tester@example.com",
    });
    if let Some(level) = row.detail_level {
        input["conflictDetailLevel"] = json!(level);
    }
    if let Some(strategy) = row.strategy {
        input["conflictResolutionStrategy"] = json!(strategy);
    }
    if let Some(resolution) = &row.resolution {
        input["conflictResolution"] = resolution.clone();
    }
    if row.keep_empty_folders {
        input["keepEmptyFolders"] = json!(true);
    }
    input
}

/// The commit a `GetCommit` of `commit_id` reports.
async fn commit_of(router: &MultiServiceRouter, repository: &str, commit_id: &str) -> Value {
    let payload = ok(
        router,
        "GetCommit",
        &json!({ "repositoryName": repository, "commitId": commit_id }),
    )
    .await;
    payload
        .get("commit")
        .cloned()
        .unwrap_or_else(|| panic!("no commit in {payload}"))
}

/// The `parents` list of a reported commit.
fn parents_of(commit: &Value) -> Vec<String> {
    commit
        .get("parents")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn an_unreferenced_merge_commit_is_the_three_way_merge_of_the_two_trees() {
    let router = emulator();

    for row in rows() {
        let repository = format!("merge-{}", row.case);
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;

        // The base commit, then a branch at it, then one commit on each side: the
        // two tips whose merge base is that base commit.
        let base_changes: Vec<Change> = row
            .base
            .iter()
            .map(|(path, content)| Change::Put(path, content))
            .collect();
        let base_commit = commit(&router, &repository, "main", None, &base_changes, "base").await;
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": "feature",
                "commitId": base_commit,
            }),
        )
        .await;
        let destination_tip = if row.destination.is_empty() {
            base_commit.clone()
        } else {
            commit(
                &router,
                &repository,
                "main",
                Some(&base_commit),
                &row.destination,
                "destination",
            )
            .await
        };
        let source_tip = if row.source.is_empty() {
            base_commit.clone()
        } else {
            commit(
                &router,
                &repository,
                "feature",
                Some(&base_commit),
                &row.source,
                "source",
            )
            .await
        };

        let case = row.case;
        match &row.expected {
            Expected::Refused(expected) => {
                for merge_option in ["THREE_WAY_MERGE", "SQUASH_MERGE"] {
                    let code = error(
                        &router,
                        "CreateUnreferencedMergeCommit",
                        &merge_request(&row, &repository, merge_option),
                        400,
                    )
                    .await;
                    assert_eq!(
                        code, *expected,
                        "case {case:?} under {merge_option}: expected {expected}, got {code}"
                    );
                }
            }
            Expected::Tree(expected_tree) => {
                // The expectation, as the rule says the merged tree looks: the
                // git object name of each file's bytes, under its mode.
                let expected: BTreeMap<String, (String, String)> = expected_tree
                    .iter()
                    .map(|(path, content, mode)| {
                        (
                            (*path).to_string(),
                            (git_blob_id(content.as_bytes()), (*mode).to_string()),
                        )
                    })
                    .collect();
                if row.keep_empty_folders {
                    assert!(
                        expected.keys().any(|path| path.ends_with(GITKEEP)),
                        "case {case:?}: a kept-folder row must expect a {GITKEEP} placeholder"
                    );
                }

                let three_way = ok(
                    &router,
                    "CreateUnreferencedMergeCommit",
                    &merge_request(&row, &repository, "THREE_WAY_MERGE"),
                )
                .await;
                let squash = ok(
                    &router,
                    "CreateUnreferencedMergeCommit",
                    &merge_request(&row, &repository, "SQUASH_MERGE"),
                )
                .await;

                let three_way_id = string_at(&three_way, "/commitId");
                let squash_id = string_at(&squash, "/commitId");
                let three_way_tree = string_at(&three_way, "/treeId");
                let squash_tree = string_at(&squash, "/treeId");
                for (option, id, tree) in [
                    ("THREE_WAY_MERGE", &three_way_id, &three_way_tree),
                    ("SQUASH_MERGE", &squash_id, &squash_tree),
                ] {
                    assert!(
                        is_object_id(id),
                        "case {case:?} under {option}: expected a 40-character commitId, got {id:?}"
                    );
                    assert!(
                        is_object_id(tree),
                        "case {case:?} under {option}: expected a 40-character treeId, got {tree:?}"
                    );
                }
                // The merge option decides the parents, not the tree.
                assert_eq!(
                    three_way_tree, squash_tree,
                    "case {case:?}: a squash merge must keep the three-way tree, got \
                     {squash_tree} against {three_way_tree}"
                );
                assert_ne!(
                    three_way_id, squash_id,
                    "case {case:?}: a squash merge collapses the history, so its commit id must \
                     differ from the three-way one, got {squash_id}"
                );

                let three_way_commit = commit_of(&router, &repository, &three_way_id).await;
                let squash_commit = commit_of(&router, &repository, &squash_id).await;
                assert_eq!(
                    parents_of(&three_way_commit),
                    vec![destination_tip.clone(), source_tip.clone()],
                    "case {case:?}: a three-way merge commit records the destination and then \
                     the source tip, got {three_way_commit}"
                );
                assert_eq!(
                    parents_of(&squash_commit),
                    vec![destination_tip.clone()],
                    "case {case:?}: a squash merge commit records the destination tip alone, got \
                     {squash_commit}"
                );
                assert_eq!(
                    string_at(&three_way_commit, "/treeId"),
                    three_way_tree,
                    "case {case:?}: GetCommit must report the tree the merge reported"
                );

                // The merged tree itself, read back at each of the two commits.
                for (option, id) in [
                    ("THREE_WAY_MERGE", &three_way_id),
                    ("SQUASH_MERGE", &squash_id),
                ] {
                    let actual = tree_of(&router, &repository, id).await;
                    assert_eq!(
                        actual, expected,
                        "case {case:?} under {option}: expected the merged tree {expected:?}, \
                         got {actual:?}"
                    );
                }

                // Nothing moved: both branches are still at the tips that were
                // written, which is what UNREFERENCED means.
                for (branch, tip) in [("main", &destination_tip), ("feature", &source_tip)] {
                    let payload = ok(
                        &router,
                        "GetBranch",
                        &json!({ "repositoryName": repository, "branchName": branch }),
                    )
                    .await;
                    assert_eq!(
                        string_at(&payload, "/branch/commitId"),
                        *tip,
                        "case {case:?}: the branch {branch} must still be at {tip}"
                    );
                }

                // The merge is a function of the two trees: asking again yields the
                // same tree (the commit id also carries the commit's own date, so
                // only the tree is a function of the inputs alone).
                let again = ok(
                    &router,
                    "CreateUnreferencedMergeCommit",
                    &merge_request(&row, &repository, "THREE_WAY_MERGE"),
                )
                .await;
                assert_eq!(
                    string_at(&again, "/treeId"),
                    three_way_tree,
                    "case {case:?}: the same merge must produce the same tree"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unreferenced_merge_commit_is_refused_with_the_modeled_error() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "guarded" }),
    )
    .await;
    let base = commit(
        &router,
        "guarded",
        "main",
        None,
        &[Change::Put("a.txt", "one\n")],
        "base",
    )
    .await;
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "guarded",
            "branchName": "feature",
            "commitId": base,
        }),
    )
    .await;
    let source = commit(
        &router,
        "guarded",
        "feature",
        Some(&base),
        &[Change::Put("b.txt", "two\n")],
        "source",
    )
    .await;

    // Every row is a request the operation cannot answer, and the error the model
    // declares for it.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            // "This API cannot be used with the fast-forward merge strategy
            // because that strategy does not create a merge commit."
            "the fast-forward merge option",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "FAST_FORWARD_MERGE",
            }),
            "InvalidMergeOptionException",
        ),
        (
            "a merge option outside the modeled enum",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "REBASE",
            }),
            "InvalidMergeOptionException",
        ),
        (
            "no merge option at all",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
            }),
            "MergeOptionRequiredException",
        ),
        (
            "an empty source commit specifier",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
            }),
            "CommitRequiredException",
        ),
        (
            "a source specifier that names no commit",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "no-such-branch",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a destination object id this repository does not hold",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "0".repeat(40),
                "mergeOption": "THREE_WAY_MERGE",
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a repository that does not exist",
            json!({
                "repositoryName": "missing-repo",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            // `RepositoryName`'s modeled pattern is `^[\w\.-]+$`.
            "a repository name outside the modeled pattern",
            json!({
                "repositoryName": "not a name",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
            }),
            "InvalidRepositoryNameException",
        ),
        (
            "no repository name at all",
            json!({
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
            }),
            "RepositoryNameRequiredException",
        ),
        (
            "a conflict detail level outside the modeled enum",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
                "conflictDetailLevel": "CHARACTER_LEVEL",
            }),
            "InvalidConflictDetailLevelException",
        ),
        (
            "a conflict resolution strategy outside the modeled enum",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
                "conflictResolutionStrategy": "ACCEPT_BOTH",
            }),
            "InvalidConflictResolutionStrategyException",
        ),
        (
            "a replacement type outside the modeled enum",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
                "conflictResolutionStrategy": "AUTOMERGE",
                "conflictResolution": {
                    "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_EITHER" }],
                },
            }),
            "InvalidReplacementTypeException",
        ),
        (
            "a file mode outside the modeled enum",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
                "conflictResolutionStrategy": "AUTOMERGE",
                "conflictResolution": {
                    "setFileModes": [{ "filePath": "a.txt", "fileMode": "SETUID" }],
                },
            }),
            "InvalidFileModeException",
        ),
        (
            // The emulator's own bound on one request's resolution entries.
            "more conflict resolution entries than the emulator accepts",
            json!({
                "repositoryName": "guarded",
                "sourceCommitSpecifier": "feature",
                "destinationCommitSpecifier": "main",
                "mergeOption": "THREE_WAY_MERGE",
                "conflictResolutionStrategy": "AUTOMERGE",
                "conflictResolution": {
                    "deleteFiles": (0..101)
                        .map(|index| json!({ "filePath": format!("file-{index}.txt") }))
                        .collect::<Vec<Value>>(),
                },
            }),
            "MaximumConflictResolutionEntriesExceededException",
        ),
    ];

    for (case, input, expected) in cases {
        let code = error(&router, "CreateUnreferencedMergeCommit", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }

    // None of the refusals wrote anything: both branches are where they were, and
    // the repository still holds exactly the two commits that were written plus
    // the base.
    for (branch, tip) in [("main", &base), ("feature", &source)] {
        let payload = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": "guarded", "branchName": branch }),
        )
        .await;
        assert_eq!(
            string_at(&payload, "/branch/commitId"),
            **tip,
            "the branch {branch} must survive every refusal"
        );
    }
}

/// A text file of `lines` lines, each exactly `width` bytes including its
/// terminator (`"line 000001\n"` at the narrowest twelve), so a case's content
/// size is its line count times its width and the two bounds below can be
/// passed one at a time.
fn padded_lines(lines: usize, width: usize) -> String {
    (0..lines)
        .map(|index| {
            let numbered = format!("line {index:06}");
            let padding = width.saturating_sub(numbered.len() + 1);
            format!("{numbered}{}\n", "x".repeat(padding))
        })
        .collect()
}

/// `content` with the line at `index` replaced. `replacement` carries its own
/// terminator, so the file's line count — and, for a 12-byte replacement, its
/// size — is unchanged.
fn with_line(content: &str, index: usize, replacement: &str) -> String {
    let mut all: Vec<&str> = content.split_inclusive('\n').collect();
    all[index] = replacement;
    all.concat()
}

/// Write one file as its own commit on `branch`, reporting the commit id.
async fn commit_one_file(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    parent: Option<&str>,
    path: &str,
    content: &str,
) -> String {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "commitMessage": format!("write {path}"),
        "putFiles": [{ "filePath": path, "fileContent": base64(content.as_bytes()) }],
    });
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    string_at(&ok(router, "CreateCommit", &input).await, "/commitId")
}

/// The line-by-line merge is attempted only within the bounds this emulator
/// merges one version of a file inside.
///
/// The rule: a file both sides changed is merged line by line while every
/// version of it is within `crate::state::merge::MAX_MERGE_FILE_BYTES` and
/// `MAX_MERGE_FILE_LINES` — the emulator's own bounds on the one step of a merge
/// its deadline cannot interrupt, since a line merge costs on the order of the
/// product of the two sides' differing lines; a version past either bound is not
/// loaded at all and the request is refused with the modeled
/// `MaximumFileContentToLoadExceededException`, which is the same answer
/// `GetMergeOptions` gives about the same pair of commits. Each bound is
/// exercised from both sides: a file well within them, a file one line short of
/// the line bound, a file AT it, and a file of few but long lines past the byte
/// bound alone.
#[tokio::test]
async fn the_line_merge_is_attempted_only_within_the_content_bounds() {
    let router = emulator();
    // (case, lines, bytes per line including its terminator, mergeable?)
    let cases: [(&str, usize, usize, bool); 4] = [
        ("well-within-both-bounds", 5_000, 12, true),
        ("one-line-short-of-the-line-bound", MAX_MERGE_FILE_LINES - 1, 12, true),
        ("at-the-line-bound", MAX_MERGE_FILE_LINES, 12, false),
        // Few lines, so this row passes the BYTE bound and nothing else.
        (
            "past-the-byte-bound",
            1_024,
            MAX_MERGE_FILE_BYTES / 1_024 + 8,
            false,
        ),
    ];

    for (case, lines, width, mergeable) in cases {
        let repository = format!("automerge-{case}");
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;

        let base_content = padded_lines(lines, width);
        let base = commit_one_file(
            &router,
            &repository,
            "main",
            None,
            "big.txt",
            &base_content,
        )
        .await;
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": "feature",
                "commitId": base,
            }),
        )
        .await;
        // The two sides change DIFFERENT lines, which is what makes the file
        // mergeable at all under LINE_LEVEL detail.
        let destination_content = with_line(&base_content, 0, "DESTINATION\n");
        let source_content = with_line(&base_content, lines - 1, "SOURCE\n");
        commit_one_file(
            &router,
            &repository,
            "main",
            Some(&base),
            "big.txt",
            &destination_content,
        )
        .await;
        commit_one_file(
            &router,
            &repository,
            "feature",
            Some(&base),
            "big.txt",
            &source_content,
        )
        .await;

        let input = json!({
            "repositoryName": repository,
            "sourceCommitSpecifier": "feature",
            "destinationCommitSpecifier": "main",
            "mergeOption": "THREE_WAY_MERGE",
            "conflictDetailLevel": "LINE_LEVEL",
            "commitMessage": format!("merge for {case}"),
        });

        if mergeable {
            let payload = ok(&router, "CreateUnreferencedMergeCommit", &input).await;
            let commit_id = string_at(&payload, "/commitId");
            let expected = with_line(&destination_content, lines - 1, "SOURCE\n");
            let file = ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "commitSpecifier": commit_id,
                    "filePath": "big.txt",
                }),
            )
            .await;
            let actual = from_base64(&string_at(&file, "/fileContent"));
            assert_eq!(
                actual,
                expected.as_bytes(),
                "case {case:?}: expected the merged file to carry both line changes, got \
                 {} bytes differing from the {} expected",
                actual.len(),
                expected.len()
            );
        } else {
            let code = error(&router, "CreateUnreferencedMergeCommit", &input, 400).await;
            assert_eq!(
                code, "MaximumFileContentToLoadExceededException",
                "case {case:?}: expected a version past the content bounds to be refused, got \
                 {code}"
            );
        }
    }
}
