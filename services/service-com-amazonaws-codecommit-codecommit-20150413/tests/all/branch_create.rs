// SPDX-License-Identifier: CC0-1.0

//! `CreateBranch` cases the `branch_creation` suite does not cover; it owns the general
//! rule (a created reference points at the commit it was given, and a malformed name or
//! id is refused), checked against `crate::state::is_valid_branch_name`, and this module
//! only adds the rows that suite has no case for:
//!
//! * an EMPTY `commitId`. The model gives `CommitId` no length
//!   (`smithy_query(query="operation",operation="CreateBranch")`), so an empty value is
//!   not refused at the boundary and reaches the handler, where it names no commit at
//!   all: that is the modelled `CommitIdRequiredException` ("A commit ID was not
//!   specified"), not `InvalidCommitIdException`, which is for an id whose shape is
//!   wrong.
//! * a commit id ANOTHER repository minted: commits are held per repository, so an id
//!   that is a real object name elsewhere is still not one this repository holds.
//! * a repository that was deleted, so the refusal follows the store rather than the
//!   shape of the name.
//! * the per-repository branch budget: one repository at the bound does not stop
//!   another from creating a branch.

use serde_json::json;

use crate::repository_lifecycle::{base64, emulator, error, ok, string_at};

/// Create `repository` with one commit on `main`, and return that commit's id. The
/// content names the repository, because a commit id is the object name of what was
/// written: two repositories seeded with identical content in the same second would
/// otherwise mint the identical id and the cross-repository row below would prove
/// nothing.
async fn seed(router: &smithy_server_runtime::MultiServiceRouter, repository: &str) -> String {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let written = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(format!("content of {repository}\n").as_bytes()),
            "commitMessage": format!("seed {repository}"),
        }),
    )
    .await;
    string_at(&written, "/commitId")
}

#[tokio::test]
async fn a_commit_id_that_names_no_commit_here_is_refused() {
    let router = emulator();
    let here = seed(&router, "targets-here").await;
    let elsewhere = seed(&router, "targets-elsewhere").await;
    assert_ne!(
        here, elsewhere,
        "the two repositories must hold different commits, got {here} twice"
    );

    // (case, commitId as sent, the modelled error it must be refused with)
    let rows: Vec<(&str, String, &str)> = vec![
        // Empty: an id was not specified at all.
        ("an empty commit id", String::new(), "CommitIdRequiredException"),
        // Blank in other forms is still no id.
        ("a blank commit id", "   ".to_string(), "InvalidCommitIdException"),
        // Right length, wrong alphabet: an object name is hex.
        (
            "a commit id of the right length that is not hex",
            "g".repeat(40),
            "InvalidCommitIdException",
        ),
        (
            // `is_object_id` accepts hex in either case, as a git object name is
            // written either way, while the commit map is keyed by the lower-case id
            // the store minted: an upper-case id is therefore well formed and simply
            // names no commit here.
            "an object name in upper-case hex the repository does not hold",
            "F".repeat(40),
            "CommitDoesNotExistException",
        ),
        (
            "a commit id another repository minted",
            elsewhere.clone(),
            "CommitDoesNotExistException",
        ),
    ];

    for (case, commit_id, expected) in rows {
        let code = error(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": "targets-here",
                "branchName": "from-a-bad-target",
                "commitId": commit_id,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected} for {commit_id:?}, got {code}"
        );
    }

    // None of those refusals created the reference they named.
    let code = error(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "targets-here", "branchName": "from-a-bad-target" }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "a refused CreateBranch must create no branch, got {code}"
    );

    // And the id this repository DOES hold still creates one, so the rows above were
    // refused for their target and not because the request was unusable.
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "targets-here",
            "branchName": "from-a-good-target",
            "commitId": here,
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "targets-here", "branchName": "from-a-good-target" }),
    )
    .await;
    assert_eq!(
        string_at(&read, "/branch/commitId"),
        here,
        "the created branch must point at the commit this repository holds, got {read}"
    );
}

#[tokio::test]
async fn a_repository_the_store_does_not_hold_is_refused() {
    let router = emulator();
    let commit = seed(&router, "present-repository").await;
    let gone = seed(&router, "gone-repository").await;
    ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "gone-repository" }),
    )
    .await;

    // (case, repositoryName, commitId): a name no repository ever had, and the name of
    // one that was deleted — the refusal follows the store, not the shape of the name.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a name no repository ever had",
            "never-created",
            commit.as_str(),
        ),
        (
            "the name of a deleted repository",
            "gone-repository",
            gone.as_str(),
        ),
    ];
    for (case, repository, commit_id) in rows {
        let code = error(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": "feature",
                "commitId": commit_id,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {case:?}: expected RepositoryDoesNotExistException, got {code}"
        );
    }
}

#[tokio::test]
async fn the_branch_budget_is_per_repository() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_BRANCHES_PER_REPOSITORY;

    let router = emulator();
    let full = seed(&router, "budget-full").await;
    let other = seed(&router, "budget-other").await;

    // `main`, written by the seed, is already one of the branches, so this fills the
    // repository exactly to the bound.
    for index in 1..MAX_BRANCHES_PER_REPOSITORY {
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": "budget-full",
                "branchName": format!("fill/{index:05}"),
                "commitId": full,
            }),
        )
        .await;
    }
    let code = error(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "budget-full",
            "branchName": "one-too-many",
            "commitId": full,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "ValidationException",
        "a branch past the bound of {MAX_BRANCHES_PER_REPOSITORY} must be refused, got {code}"
    );

    // The bound is what ONE repository holds: another repository has its own budget.
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "budget-other",
            "branchName": "one-too-many",
            "commitId": other,
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "budget-other", "branchName": "one-too-many" }),
    )
    .await;
    assert_eq!(
        string_at(&read, "/branch/commitId"),
        other,
        "the branch budget is per repository, not per store, got {read}"
    );
}
