// SPDX-License-Identifier: CC0-1.0

//! The bound `CreateCommit` holds one commit's file entries to, and the TEXT it
//! refuses a commit past it with, over the wire.
//!
//! The rule under test: the `putFiles`, `deleteFiles` and `setFileModes` entries of
//! one request count TOGETHER against the published limit of 100 files changed in a
//! commit — each entry is one file the commit changes, however it changes it — so a
//! request holding exactly 100 of them is served and one holding 101 is refused with
//! HTTP 400 `MaximumFileEntriesExceededException`
//! (`smithy_query(query="shape",shape="MaximumFileEntriesExceededException")`:
//! `errorKind` client, `httpError` 400) and leaves the branch where it was. The
//! refusal's sentence is FIXED: it states the limit, where that limit applies and
//! the two ways out, and interpolates nothing from the request — in particular not
//! how many entries the refused request held.
//!
//! The table below spans what the rule generalises over rather than one recorded
//! request: how the entries are SPREAD over the three members (puts alone, deletes
//! alone, mode changes alone, and each pair and the triple mixed), and how far past
//! the bound the count is (one entry past it, and twice it). The accepted rows are
//! the bound itself, reached two different ways, which is what makes the refusals
//! about the count rather than about the entries.

use serde_json::{json, Value};
use service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_FILE_ENTRIES_PER_COMMIT;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, ok, string_at};

/// The status the model gives `MaximumFileEntriesExceededException`.
const CLIENT_ERROR_STATUS: u16 = 400;

const CODE: &str = "MaximumFileEntriesExceededException";

/// The FIXED sentence group the service answers that code with.
const EXPECTED_MESSAGE: &str =
    "The commit cannot be created because it exceeds the maximum number of changed files allowed \
     in a commit. The maximum limit for the number of files is 100 in the AWS CodeCommit console \
     and in the CreateCommit API. Either create multiple commits to push these file changes, or \
     use a Git client.";

const REPOSITORY: &str = "commit-entry-bound";
const BRANCH: &str = "main";

/// How many files the fixture seeds, so a row can delete or re-mode paths that are
/// really there. Half the bound, so a row can reach exactly 100 entries by mixing
/// deletes of these with puts of new paths.
const SEEDED: usize = MAX_FILE_ENTRIES_PER_COMMIT / 2;

fn seeded_path(index: usize) -> String {
    format!("seeded/file-{index:03}.txt")
}

/// `putFiles` entries writing `count` paths no row shares with another.
fn puts(label: &str, count: usize) -> Value {
    (0..count)
        .map(|index| {
            json!({
                "filePath": format!("{label}/new-{index:03}.txt"),
                "fileContent": base64(format!("{label} {index}\n").as_bytes()),
            })
        })
        .collect()
}

/// `deleteFiles` entries naming `count` of the seeded paths.
fn deletes(count: usize) -> Value {
    (0..count)
        .map(|index| json!({ "filePath": seeded_path(index) }))
        .collect()
}

/// `setFileModes` entries naming `count` of the seeded paths.
fn mode_changes(count: usize) -> Value {
    (0..count)
        .map(|index| json!({ "filePath": seeded_path(index), "fileMode": "EXECUTABLE" }))
        .collect()
}

/// A repository with [`SEEDED`] files on one branch, and the commit its branch is at.
async fn seeded() -> (MultiServiceRouter, String) {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let seeded = ok(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": BRANCH,
            "commitMessage": "the seeded files",
            "putFiles": puts("seeded-root", 1),
        }),
    )
    .await;
    let mut tip = string_at(&seeded, "/commitId");
    // Written in batches inside the bound, so the fixture itself never leans on the
    // rule under test.
    let entries: Vec<Value> = (0..SEEDED)
        .map(|index| {
            json!({
                "filePath": seeded_path(index),
                "fileContent": base64(format!("seeded {index}\n").as_bytes()),
            })
        })
        .collect();
    for batch in entries.chunks(MAX_FILE_ENTRIES_PER_COMMIT / 2) {
        let written = ok(
            &router,
            "CreateCommit",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": BRANCH,
                "parentCommitId": tip,
                "commitMessage": "one batch of the seeded files",
                "putFiles": Value::Array(batch.to_vec()),
            }),
        )
        .await;
        tip = string_at(&written, "/commitId");
    }
    (router, tip)
}

/// One row: how many entries of each kind the request carries.
struct Row {
    case: &'static str,
    put_count: usize,
    delete_count: usize,
    mode_count: usize,
}

impl Row {
    fn total(&self) -> usize {
        self.put_count + self.delete_count + self.mode_count
    }

    fn body(&self, label: &str, parent: &str) -> Value {
        let mut body = json!({
            "repositoryName": REPOSITORY,
            "branchName": BRANCH,
            "parentCommitId": parent,
            "commitMessage": format!("a commit of {} entries", self.total()),
        });
        if self.put_count > 0 {
            body["putFiles"] = puts(label, self.put_count);
        }
        if self.delete_count > 0 {
            body["deleteFiles"] = deletes(self.delete_count);
        }
        if self.mode_count > 0 {
            body["setFileModes"] = mode_changes(self.mode_count);
        }
        body
    }
}

/// The rows past the bound. Each spreads one entry more than the limit — or twice the
/// limit — over a different combination of the three members.
fn refused_rows() -> Vec<Row> {
    let bound = MAX_FILE_ENTRIES_PER_COMMIT;
    vec![
        Row {
            case: "one put past the bound",
            put_count: bound + 1,
            delete_count: 0,
            mode_count: 0,
        },
        Row {
            case: "one delete past the bound",
            put_count: 0,
            delete_count: bound + 1,
            mode_count: 0,
        },
        Row {
            case: "one mode change past the bound",
            put_count: 0,
            delete_count: 0,
            mode_count: bound + 1,
        },
        Row {
            case: "puts and deletes together, one past the bound",
            put_count: 60,
            delete_count: 41,
            mode_count: 0,
        },
        Row {
            case: "puts and mode changes together, one past the bound",
            put_count: 50,
            delete_count: 0,
            mode_count: 51,
        },
        Row {
            case: "all three kinds together, one past the bound",
            put_count: 34,
            delete_count: 34,
            mode_count: 33,
        },
        Row {
            case: "twice the bound in puts",
            put_count: 2 * bound,
            delete_count: 0,
            mode_count: 0,
        },
    ]
}

#[tokio::test]
async fn a_commit_past_the_entry_bound_is_refused_in_fixed_words_and_writes_nothing() {
    let (router, tip) = seeded().await;

    for row in refused_rows() {
        let case = row.case;
        let total = row.total();
        let (code, message) = error_with_message(
            &router,
            "CreateCommit",
            &row.body(case, &tip),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, CODE,
            "case {case:?}: expected the modelled {CODE}, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, EXPECTED_MESSAGE,
            "case {case:?}: expected the exact sentence AWS words this class with, got {message:?}"
        );
        // The count the request carried is this request's, not part of the
        // requirement, so it cannot be in the text.
        assert!(
            !message.contains(&total.to_string()),
            "case {case:?}: the message must interpolate nothing, but {message:?} carries the \
             submitted count {total}"
        );

        // Nothing was committed: the branch is still at the commit the request named
        // as its parent.
        let read = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": BRANCH }),
        )
        .await;
        assert_eq!(
            string_at(&read, "/branch/commitId"),
            tip,
            "case {case:?}: a refused commit must leave the branch where it was, got {read}"
        );
    }
}

#[tokio::test]
async fn a_commit_at_the_entry_bound_is_served() {
    let (router, tip) = seeded().await;
    let bound = MAX_FILE_ENTRIES_PER_COMMIT;

    // The bound reached two ways: all puts, and deletes of every seeded path mixed
    // with puts of new ones. Each row is applied to the branch in turn, so the second
    // also proves the first was really committed.
    let rows = [
        Row {
            case: "the bound in puts alone",
            put_count: bound,
            delete_count: 0,
            mode_count: 0,
        },
        Row {
            case: "the bound in puts and deletes together",
            put_count: bound - SEEDED,
            delete_count: SEEDED,
            mode_count: 0,
        },
    ];

    let mut parent = tip;
    for row in rows {
        let case = row.case;
        let written = ok(&router, "CreateCommit", &row.body(case, &parent)).await;
        let commit_id = string_at(&written, "/commitId");
        let read = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": BRANCH }),
        )
        .await;
        assert_eq!(
            string_at(&read, "/branch/commitId"),
            commit_id,
            "case {case:?}: a commit at the bound must advance the branch, got {read}"
        );
        parent = commit_id;
    }
}
