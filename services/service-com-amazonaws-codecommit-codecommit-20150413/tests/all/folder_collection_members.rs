// SPDX-License-Identifier: CC0-1.0

//! The collection members `GetFolder` answers with, over the wire.
//!
//! The rule under test: every collection a read ENUMERATES is present on its
//! answer, empty where the folder holds no entry of that kind — `subFolders`,
//! `files`, `symbolicLinks` and `subModules` are part of the response shape, so a
//! caller may index each one unconditionally instead of having to tell an empty
//! folder from a missing member.
//!
//! The table below spans what that generalises over: which kinds a folder holds
//! and which it lacks (files only, sub-folders only, symbolic links only, a mix,
//! and nothing at all), the depth of the folder (root, one level, three levels),
//! the way the folder is spelled (empty, `/`, bare, with separators), and the
//! commit the folder is read at (the tip, an earlier commit, and the tree a
//! deletion emptied). Expectations are computed in this file from the tree that
//! was written, so the answer is checked against the rule rather than against
//! itself.
//!
//! `folder_listing.rs` owns the CONTENT of those lists — which entries a folder
//! reports, with which paths, ids and modes; this suite owns their PRESENCE.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::repository_lifecycle::{base64, emulator, ok, string_at};

/// The four collection members `GetFolderOutput` declares
/// (`smithy_query(query="operation",operation="GetFolder")`: `subFolders`,
/// `files`, `symbolicLinks`, `subModules`).
const COLLECTION_MEMBERS: [&str; 4] = ["subFolders", "files", "symbolicLinks", "subModules"];

/// One file of the tree every test here writes: the path, the `fileMode` sent (and
/// so the list the entry belongs in).
#[derive(Debug, Clone, Copy)]
struct Entry {
    path: &'static str,
    mode: &'static str,
    content: &'static [u8],
}

/// A tree whose folders cover each combination of "holds this kind" the rule
/// generalises over: a root with files and sub-folders, a folder with files only,
/// a folder with symbolic links only, a folder with a sub-folder only, and a
/// three-level chain.
fn tree() -> Vec<Entry> {
    vec![
        Entry {
            path: "README.md",
            mode: "NORMAL",
            content: b"# demo\n",
        },
        Entry {
            path: "plain/one.txt",
            mode: "NORMAL",
            content: b"one\n",
        },
        Entry {
            path: "plain/two.sh",
            mode: "EXECUTABLE",
            content: b"#!/bin/sh\n",
        },
        Entry {
            path: "links/to-readme",
            mode: "SYMLINK",
            content: b"../README.md",
        },
        Entry {
            path: "links/to-one",
            mode: "SYMLINK",
            content: b"../plain/one.txt",
        },
        Entry {
            path: "chain/level/deep/leaf.txt",
            mode: "NORMAL",
            content: b"leaf\n",
        },
    ]
}

/// What the rule says one folder of the written tree holds, as the counts of its
/// immediate entries of each kind — this file's own reading of the tree, computed
/// independently of the service.
fn expected_counts(entries: &[Entry], folder: &str) -> [usize; 4] {
    let mut folders: BTreeSet<&str> = BTreeSet::new();
    let mut files = 0usize;
    let mut links = 0usize;
    for entry in entries {
        let relative = if folder.is_empty() {
            Some(entry.path)
        } else {
            entry.path.strip_prefix(&format!("{folder}/"))
        };
        let Some(relative) = relative else { continue };
        match relative.split_once('/') {
            Some((head, _)) => {
                folders.insert(head);
            }
            None if entry.mode == "SYMLINK" => links += 1,
            None => files += 1,
        }
    }
    // A submodule is a gitlink entry, and nothing this service can be asked to
    // write creates one, so that count is always zero — which is exactly why the
    // member has to be PRESENT and empty rather than absent.
    [folders.len(), files, links, 0]
}

/// The folder a `folderPath` addresses, named the way a tree names it: the root by
/// no segment at all.
fn canonical(folder_path: &str) -> String {
    folder_path.trim_matches('/').to_string()
}

/// Assert the four collections are present on `payload`, and that each holds the
/// number of entries the rule says that folder has.
fn assert_collections(payload: &Value, case: &str, expected: [usize; 4]) {
    for (member, count) in COLLECTION_MEMBERS.iter().zip(expected) {
        let value = payload.get(*member).unwrap_or_else(|| {
            panic!("case {case}: the {member} member must be present, got {payload}")
        });
        let list = value.as_array().unwrap_or_else(|| {
            panic!("case {case}: the {member} member must be a list, got {payload}")
        });
        assert_eq!(
            list.len(),
            count,
            "case {case}: expected {count} entries in {member}, got {payload}"
        );
    }
}

/// Write the tree and read every folder of it: each answer carries all four
/// collections, empty where the folder holds no entry of that kind.
#[tokio::test]
async fn get_folder_always_carries_its_four_collections() {
    let router = emulator();
    let repository = "folder-collections";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;

    let entries = tree();
    let mut parent: Option<String> = None;
    for entry in &entries {
        let mut body = json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": entry.path,
            "fileContent": base64(entry.content),
            "fileMode": entry.mode,
        });
        if let (Some(parent), Some(object)) = (parent.as_ref(), body.as_object_mut()) {
            object.insert("parentCommitId".to_string(), json!(parent));
        }
        let written = ok(&router, "PutFile", &body).await;
        parent = Some(string_at(&written, "/commitId"));
    }

    // Rows: every folder of the tree, in each spelling the service accepts — the
    // root (which holds files AND sub-folders), a files-only folder, a
    // symlinks-only folder, a folder holding nothing but a sub-folder, and the
    // deepest folder, which holds one file and no folder.
    let folders = [
        "", "/", "plain", "/plain", "plain/", "links", "chain", "chain/level",
        "chain/level/deep",
    ];
    for folder_path in folders {
        let request = json!({ "repositoryName": repository, "folderPath": folder_path });
        let payload = ok(&router, "GetFolder", &request).await;
        let folder = canonical(folder_path);
        let case = format!("folderPath {folder_path:?}");
        assert_collections(&payload, &case, expected_counts(&entries, &folder));

        // The folder really does lack a kind in at least one row, and the member
        // is still there: a client indexing it reads an empty list.
        if folder == "links" {
            assert_eq!(
                payload.get("files"),
                Some(&json!([])),
                "case {case}: a folder holding only symbolic links reports an empty files list, \
                 got {payload}"
            );
        }
        if folder == "plain" {
            assert_eq!(
                payload.get("symbolicLinks"),
                Some(&json!([])),
                "case {case}: a folder holding no symbolic link reports an empty symbolicLinks \
                 list, got {payload}"
            );
            assert_eq!(
                payload.get("subFolders"),
                Some(&json!([])),
                "case {case}: a folder holding no sub-folder reports an empty subFolders list, \
                 got {payload}"
            );
        }

        // A read must not change what the next caller sees.
        let again = ok(&router, "GetFolder", &request).await;
        assert_eq!(again, payload, "case {case}: GetFolder must be repeatable");
    }
}

/// A root that holds nothing but a sub-folder, and a tree a deletion emptied
/// completely: the extreme rows of the rule, where several collections — and in
/// the empty tree all four — are empty.
#[tokio::test]
async fn an_empty_or_folder_only_tree_still_carries_every_collection() {
    let router = emulator();

    // A repository whose only file lives in a sub-folder, so its root holds no
    // file at all.
    let folder_only = "root-holds-a-folder";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": folder_only }),
    )
    .await;
    let written = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": folder_only,
            "branchName": "main",
            "filePath": "src/app.sh",
            "fileContent": base64(b"#!/bin/sh\n"),
            "fileMode": "EXECUTABLE",
        }),
    )
    .await;
    let tip = string_at(&written, "/commitId");

    let root = ok(
        &router,
        "GetFolder",
        &json!({ "repositoryName": folder_only, "folderPath": "/" }),
    )
    .await;
    assert_collections(&root, "a root holding only a sub-folder", [1, 0, 0, 0]);
    assert_eq!(
        root.get("files"),
        Some(&json!([])),
        "a root holding only a sub-folder reports an empty files list, got {root}"
    );

    // The same repository after the only file is deleted: the tree is empty, so
    // every collection is empty and every one is still present.
    let deleted = ok(
        &router,
        "DeleteFile",
        &json!({
            "repositoryName": folder_only,
            "branchName": "main",
            "filePath": "src/app.sh",
            "parentCommitId": tip,
        }),
    )
    .await;
    assert!(
        string_at(&deleted, "/commitId") != tip,
        "the deletion must commit, got {deleted}"
    );
    let empty = ok(
        &router,
        "GetFolder",
        &json!({ "repositoryName": folder_only, "folderPath": "/" }),
    )
    .await;
    assert_collections(&empty, "a tree a deletion emptied", [0, 0, 0, 0]);
    for member in COLLECTION_MEMBERS {
        assert_eq!(
            empty.get(member),
            Some(&json!([])),
            "an empty tree reports {member} as an empty list, got {empty}"
        );
    }

    // The earlier commit is unchanged: reading the emptied tree did not rewrite
    // what the sub-folder's commit holds.
    let historic = ok(
        &router,
        "GetFolder",
        &json!({
            "repositoryName": folder_only,
            "folderPath": "/",
            "commitSpecifier": tip,
        }),
    )
    .await;
    assert_collections(&historic, "the root at the earlier commit", [1, 0, 0, 0]);
}
