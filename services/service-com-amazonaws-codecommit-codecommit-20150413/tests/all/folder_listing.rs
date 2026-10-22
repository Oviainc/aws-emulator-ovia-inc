// SPDX-License-Identifier: CC0-1.0

//! `GetFolder` over the wire, driven as awsJson1_1 requests through the router that
//! serves the real emulator.
//!
//! The rule under test: `GetFolder` reports, for the folder its `folderPath` names in
//! the tree snapshot of the commit `commitSpecifier` resolves to (the default branch
//! when none is given), that folder's own tree id and only its IMMEDIATE entries —
//! each blob as a file, or as a symbolic link when its mode says so, and each
//! sub-folder once, with the entry's path from the root of the tree and relative to
//! the folder queried — and refuses a folder no entry lives under.
//!
//! The tables below span what the rule generalises over rather than one recorded
//! request: tree depth (root, one level, four levels), how the folder is spelled
//! (empty, `/`, bare, leading and trailing separator), entry kind and count (NORMAL,
//! EXECUTABLE, SYMLINK, sub-folder), path alphabet (ASCII and non-ASCII) and the
//! commit read (each commit of the history, a branch name, no specifier at all).
//! Expectations are computed in this file from the table that was written, so the
//! service's answer is checked against the rule rather than against itself.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::repository_lifecycle::{
    base64, emulator, error, git_blob_id, is_object_id, ok, string_at,
};

/// One file written into the test repository: the path as SENT (a leading separator
/// addresses the same entry as none), the `fileMode` as sent (`None` exercises the
/// NORMAL default) and the mode the service must then report.
#[derive(Debug, Clone)]
struct Row {
    sent_path: &'static str,
    sent_mode: Option<&'static str>,
    mode: &'static str,
    content: &'static [u8],
}

impl Row {
    /// The path the service addresses this file by: leading separators removed, as
    /// every path in a git tree is relative to the tree's root.
    fn stored_path(&self) -> &'static str {
        self.sent_path.trim_start_matches('/')
    }
}

/// The tree every test here writes. It holds a file at the root, a folder with
/// files of each `FileModeTypeEnum` mode (from
/// `smithy_query(query="shape",shape="FileModeTypeEnum")`), a nested folder, a
/// non-ASCII path and a four-level chain, so every branch of the rule — immediate
/// blob, symbolic link, sub-folder, deeper entry — is covered at more than one
/// depth.
fn tree_rows() -> Vec<Row> {
    vec![
        Row {
            sent_path: "README.md",
            sent_mode: None,
            mode: "NORMAL",
            content: b"# demo\n",
        },
        Row {
            sent_path: "src/main.rs",
            sent_mode: Some("NORMAL"),
            mode: "NORMAL",
            content: b"fn main() {}\n",
        },
        Row {
            // A leading separator names the same entry as none at all.
            sent_path: "/src/run.sh",
            sent_mode: Some("EXECUTABLE"),
            mode: "EXECUTABLE",
            content: b"#!/bin/sh\necho hi\n",
        },
        Row {
            sent_path: "src/link",
            sent_mode: Some("SYMLINK"),
            mode: "SYMLINK",
            content: b"main.rs",
        },
        Row {
            sent_path: "src/util/helper.rs",
            sent_mode: None,
            mode: "NORMAL",
            content: b"pub fn help() {}\n",
        },
        Row {
            sent_path: "docs/gu\u{ed}a.md",
            sent_mode: None,
            mode: "NORMAL",
            content: "\u{2699} configuraci\u{f3}n\n".as_bytes(),
        },
        Row {
            sent_path: "a/b/c/d/deep.txt",
            sent_mode: None,
            mode: "NORMAL",
            content: b"deep",
        },
    ]
}

/// The `folderPath` values the listing test asks for: the root in both spellings
/// the service accepts, a folder with and without separators around it, and a
/// folder at each depth of the tree.
fn folder_queries() -> Vec<&'static str> {
    vec![
        "",
        "/",
        "src",
        "/src",
        "src/",
        "/src/",
        "src/util",
        "docs",
        "a",
        "a/b",
        "a/b/c",
        "a/b/c/d",
    ]
}

/// The canonical name of the folder a `folderPath` addresses: the root is named by
/// no segment at all.
fn canonical(folder_path: &str) -> String {
    folder_path.trim_matches('/').to_string()
}

/// The fully qualified path of an entry of `folder`.
fn join(folder: &str, entry: &str) -> String {
    if folder.is_empty() {
        entry.to_string()
    } else {
        format!("{folder}/{entry}")
    }
}

/// What the rule says one folder of a written tree holds: its immediate blobs as
/// `(absolutePath, relativePath, blobId, fileMode)`, split into files and symbolic
/// links by mode, and the names of its immediate sub-folders.
#[derive(Debug, Default, PartialEq, Eq)]
struct Expected {
    files: BTreeSet<(String, String, String, String)>,
    links: BTreeSet<(String, String, String, String)>,
    folders: BTreeSet<String>,
}

/// The expectation for `folder` over the rows written so far — this file's own
/// reading of the rule, independent of the service.
fn expected_for(rows: &[Row], folder: &str) -> Expected {
    let mut expected = Expected::default();
    for row in rows {
        let stored = row.stored_path();
        let relative = if folder.is_empty() {
            Some(stored)
        } else {
            stored.strip_prefix(&format!("{folder}/"))
        };
        let Some(relative) = relative else { continue };
        match relative.split_once('/') {
            Some((head, _)) => {
                expected.folders.insert(head.to_string());
            }
            None => {
                let entry = (
                    stored.to_string(),
                    relative.to_string(),
                    git_blob_id(row.content),
                    row.mode.to_string(),
                );
                if row.mode == "SYMLINK" {
                    expected.links.insert(entry);
                } else {
                    expected.files.insert(entry);
                }
            }
        }
    }
    expected
}

/// One of the response's blob lists (`files` or `symbolicLinks`) as the set of
/// `(absolutePath, relativePath, blobId, fileMode)` tuples it reports. A list the
/// response omits is the empty set, and a duplicated entry is caught by comparing
/// the set's size with the list's length.
fn entry_set(
    payload: &Value,
    member: &str,
    case: &str,
) -> BTreeSet<(String, String, String, String)> {
    let entries = payload
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let set: BTreeSet<(String, String, String, String)> = entries
        .iter()
        .map(|entry| {
            (
                string_at(entry, "/absolutePath"),
                string_at(entry, "/relativePath"),
                string_at(entry, "/blobId"),
                string_at(entry, "/fileMode"),
            )
        })
        .collect();
    assert_eq!(
        set.len(),
        entries.len(),
        "case {case}: {member} must not report an entry twice, got {payload}"
    );
    set
}

/// Write one file, as its own commit on `branch`.
async fn put_file(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    row: &Row,
    parent: Option<&str>,
) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "branchName": branch,
        "filePath": row.sent_path,
        "fileContent": base64(row.content),
        "commitMessage": format!("add {}", row.stored_path()),
    });
    if let Some(mode) = row.sent_mode {
        input["fileMode"] = json!(mode);
    }
    if let Some(parent) = parent {
        input["parentCommitId"] = json!(parent);
    }
    ok(router, "PutFile", &input).await
}

/// Create `repository` and write every row into it, one commit per row. Returns
/// the `(commitId, treeId)` each `PutFile` reported, oldest first, so a test can
/// read the tree as it stood at any commit of the history.
async fn write_tree(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
    rows: &[Row],
) -> Vec<(String, String)> {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let mut history: Vec<(String, String)> = Vec::new();
    for row in rows {
        let parent = history.last().map(|(commit, _)| commit.clone());
        let put = put_file(router, repository, branch, row, parent.as_deref()).await;
        history.push((string_at(&put, "/commitId"), string_at(&put, "/treeId")));
    }
    history
}

/// A `GetFolder` request for one folder of a repository.
fn get_folder(repository: &str, folder_path: &str) -> Value {
    json!({ "repositoryName": repository, "folderPath": folder_path })
}

#[tokio::test]
async fn get_folder_reports_the_immediate_entries_of_the_stored_folder() {
    let router = emulator();
    let rows = tree_rows();
    let history = write_tree(&router, "folders", "main", &rows).await;
    let (tip, tip_tree) = history
        .last()
        .cloned()
        .expect("the table writes at least one file");

    // Every spelling of one folder must name one tree.
    let mut tree_ids: BTreeMap<String, String> = BTreeMap::new();

    for folder_path in folder_queries() {
        let case = format!("folderPath {folder_path:?}");
        let request = get_folder("folders", folder_path);
        let payload = ok(&router, "GetFolder", &request).await;

        assert_eq!(
            string_at(&payload, "/folderPath"),
            folder_path,
            "case {case}: folderPath must be the folder the request named"
        );
        assert_eq!(
            string_at(&payload, "/commitId"),
            tip,
            "case {case}: with no commitSpecifier the folder is read at the default \
             branch's tip, got {payload}"
        );
        let tree_id = string_at(&payload, "/treeId");
        assert!(
            is_object_id(&tree_id),
            "case {case}: treeId must be a 40-character hex object id, got {tree_id:?}"
        );
        let folder = canonical(folder_path);
        if let Some(previous) = tree_ids.insert(folder.clone(), tree_id.clone()) {
            assert_eq!(
                previous, tree_id,
                "case {case}: every spelling of the folder {folder:?} names one tree"
            );
        }

        let expected = expected_for(&rows, &folder);
        assert_eq!(
            entry_set(&payload, "files", &case),
            expected.files,
            "case {case}: files must be this folder's immediate blobs, got {payload}"
        );
        assert_eq!(
            entry_set(&payload, "symbolicLinks", &case),
            expected.links,
            "case {case}: symbolicLinks must be this folder's immediate SYMLINK blobs, \
             got {payload}"
        );

        let sub_folders = payload
            .get("subFolders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let reported: BTreeSet<String> = sub_folders
            .iter()
            .map(|entry| string_at(entry, "/relativePath"))
            .collect();
        assert_eq!(
            reported, expected.folders,
            "case {case}: subFolders must be this folder's immediate sub-folders, each \
             once, got {payload}"
        );
        assert_eq!(
            sub_folders.len(),
            expected.folders.len(),
            "case {case}: subFolders must not report a sub-folder twice, got {payload}"
        );
        for entry in &sub_folders {
            let name = string_at(entry, "/relativePath");
            let absolute = string_at(entry, "/absolutePath");
            assert_eq!(
                absolute,
                join(&folder, &name),
                "case {case}: the absolutePath of sub-folder {name:?}"
            );
            // A sub-folder's treeId is that folder's OWN tree — the same id the
            // service reports when that folder is read directly.
            let child = ok(&router, "GetFolder", &get_folder("folders", &absolute)).await;
            assert_eq!(
                string_at(entry, "/treeId"),
                string_at(&child, "/treeId"),
                "case {case}: the treeId of sub-folder {name:?} must be that folder's own tree"
            );
        }

        assert_eq!(
            payload.get("subModules"),
            None,
            "case {case}: nothing this service can be asked to write creates a submodule, \
             got {payload}"
        );

        // A read must not change what the next caller sees.
        let again = ok(&router, "GetFolder", &request).await;
        assert_eq!(again, payload, "case {case}: GetFolder must be repeatable");
    }

    // The root folder IS the commit's tree, so its id is the one PutFile reported
    // for the tip.
    assert_eq!(
        tree_ids.get(""),
        Some(&tip_tree),
        "the root folder's treeId must be the tip commit's treeId"
    );
    // A folder that holds different content than another is a different tree.
    let distinct: BTreeSet<&String> = tree_ids.values().collect();
    assert_eq!(
        distinct.len(),
        tree_ids.len(),
        "folders holding different content must have different tree ids, got {tree_ids:?}"
    );
}

#[tokio::test]
async fn a_folder_is_read_at_the_commit_the_specifier_names() {
    let router = emulator();
    let rows = tree_rows();
    let history = write_tree(&router, "history", "trunk", &rows).await;

    // After the k-th write the tree holds exactly the first k+1 rows, and reading
    // the root at that commit must report exactly that tree.
    for (index, (commit, tree)) in history.iter().enumerate() {
        let case = format!("commit {} of the history", index + 1);
        let mut request = get_folder("history", "");
        request["commitSpecifier"] = json!(commit);
        let payload = ok(&router, "GetFolder", &request).await;
        assert_eq!(
            string_at(&payload, "/commitId"),
            *commit,
            "case {case}: commitId must be the commit asked for"
        );
        assert_eq!(
            string_at(&payload, "/treeId"),
            *tree,
            "case {case}: the root treeId must be the tree that commit wrote"
        );
        let expected = expected_for(&rows[..=index], "");
        assert_eq!(
            entry_set(&payload, "files", &case),
            expected.files,
            "case {case}: files at that commit, got {payload}"
        );
        assert_eq!(
            entry_set(&payload, "symbolicLinks", &case),
            expected.links,
            "case {case}: symbolicLinks at that commit, got {payload}"
        );
        let reported: BTreeSet<String> = payload
            .get("subFolders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| string_at(entry, "/relativePath"))
            .collect();
        assert_eq!(
            reported, expected.folders,
            "case {case}: subFolders at that commit, got {payload}"
        );
    }

    // A branch name resolves to its tip, which is what no specifier at all reads.
    let mut by_branch = get_folder("history", "src");
    by_branch["commitSpecifier"] = json!("trunk");
    let branch_payload = ok(&router, "GetFolder", &by_branch).await;
    let default_payload = ok(&router, "GetFolder", &get_folder("history", "src")).await;
    assert_eq!(
        branch_payload, default_payload,
        "the branch name and no specifier at all must read the same folder"
    );

    // The ids are content-addressed: another repository, on another branch, holding
    // the same tree reports the same folder tree ids.
    write_tree(&router, "twin", "release-1.0", &rows).await;
    for folder_path in ["", "src", "a/b/c/d"] {
        let here = ok(&router, "GetFolder", &get_folder("history", folder_path)).await;
        let there = ok(&router, "GetFolder", &get_folder("twin", folder_path)).await;
        assert_eq!(
            string_at(&here, "/treeId"),
            string_at(&there, "/treeId"),
            "case folderPath {folder_path:?}: identical content must have an identical treeId, \
             got {here} and {there}"
        );
    }
}

#[tokio::test]
async fn a_folder_that_cannot_be_read_is_refused_with_the_modeled_error() {
    let router = emulator();
    let rows = tree_rows();
    let history = write_tree(&router, "folders", "main", &rows).await;
    let tip = history
        .last()
        .map(|(commit, _)| commit.clone())
        .expect("the table writes at least one file");

    // A repository with no commit at all has no default branch to read a folder at.
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "no-commits" }),
    )
    .await;

    // Each row is a request that cannot be served, and the error the model names for
    // it. They differ along which part of the request is at fault: the repository,
    // the commit, the folder, the path's form.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            get_folder("absent-repo", ""),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name outside the modeled pattern",
            get_folder("not a name!", ""),
            "InvalidRepositoryNameException",
        ),
        (
            "no folderPath at all",
            json!({ "repositoryName": "folders" }),
            "PathRequiredException",
        ),
        (
            "a repository that holds no commit",
            get_folder("no-commits", ""),
            "CommitDoesNotExistException",
        ),
        (
            "a folder no entry lives under",
            get_folder("folders", "missing"),
            "FolderDoesNotExistException",
        ),
        (
            "a sub-folder of an existing folder that holds no entry",
            get_folder("folders", "src/missing"),
            "FolderDoesNotExistException",
        ),
        (
            "a path that names a file at the root, not a folder",
            get_folder("folders", "README.md"),
            "FolderDoesNotExistException",
        ),
        (
            "a path that names a file inside a folder",
            get_folder("folders", "src/main.rs"),
            "FolderDoesNotExistException",
        ),
        (
            "a path with an empty segment",
            get_folder("folders", "src//util"),
            "InvalidPathException",
        ),
        (
            "a path with a current-directory segment",
            get_folder("folders", "src/./util"),
            "InvalidPathException",
        ),
        (
            "a path that climbs out of the folder",
            get_folder("folders", "src/../docs"),
            "InvalidPathException",
        ),
        (
            "a commit id the repository does not hold",
            json!({
                "repositoryName": "folders",
                "folderPath": "",
                "commitSpecifier": "0".repeat(40),
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a branch that does not exist",
            json!({
                "repositoryName": "folders",
                "folderPath": "",
                "commitSpecifier": "no-such-branch",
            }),
            "CommitDoesNotExistException",
        ),
    ];

    for (case, request, expected) in cases {
        let code = error(&router, "GetFolder", &request, 400).await;
        assert_eq!(
            code, expected,
            "case {case:?} ({request}): expected {expected}, got {code}"
        );
    }

    // None of those reads changed anything: the tip and the root listing stand.
    let payload = ok(&router, "GetFolder", &get_folder("folders", "")).await;
    assert_eq!(
        string_at(&payload, "/commitId"),
        tip,
        "a refused read must not move the branch tip, got {payload}"
    );
    assert_eq!(
        entry_set(&payload, "files", "after the refusals"),
        expected_for(&rows, "").files,
        "a refused read must not change the stored tree, got {payload}"
    );
}
