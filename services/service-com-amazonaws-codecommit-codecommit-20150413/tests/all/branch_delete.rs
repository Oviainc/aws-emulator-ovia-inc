// SPDX-License-Identifier: CC0-1.0

//! `DeleteBranch` over the wire, driven as awsJson1_1 requests through the router that
//! serves the real emulator.
//!
//! The rule under test: `DeleteBranch` removes from the named repository the one
//! reference the request names and reports that reference's own name and tip commit,
//! leaving every other reference, the commits themselves and the rest of the
//! repository's metadata where they were; the repository's default branch is the one
//! reference it refuses to remove (`DefaultBranchCannotBeDeletedException`), and
//! because the model declares no "branch does not exist" error for it, a name the
//! repository does not hold is a success that reports no deleted branch.
//!
//! The tables below span the class the rule covers — branch-name form and length (one
//! character, ordinary, slashed, punctuated, non-ASCII, the modeled maximum of 256),
//! the number of references a repository holds, which of them is the default, and the
//! published bounds of both name members — rather than one recorded request. The
//! harness (`emulator`, `ok`, `error`) is the one in [`crate::repository_lifecycle`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, emulator, error, ok, string_at};

/// The branch `PutFile` puts the first commit of an empty repository on; it is that
/// repository's default branch, so it is the row every table starts from.
const BOOTSTRAP_BRANCH: &str = "bootstrap";

/// Create `repository` with one commit on [`BOOTSTRAP_BRANCH`] and answer that
/// commit's id — the tip every reference below is created at.
async fn seed(router: &MultiServiceRouter, repository: &str) -> String {
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
            "branchName": BOOTSTRAP_BRANCH,
            "filePath": "README.md",
            "fileContent": base64(format!("# {repository}\n").as_bytes()),
            "commitMessage": format!("seed {repository}"),
        }),
    )
    .await;
    string_at(&written, "/commitId")
}

/// Add one reference at `tip` through the modelled `CreateBranch`.
async fn create_branch(router: &MultiServiceRouter, repository: &str, branch: &str, tip: &str) {
    ok(
        router,
        "CreateBranch",
        &json!({ "repositoryName": repository, "branchName": branch, "commitId": tip }),
    )
    .await;
}

/// The repository's metadata as `GetRepository` reports it.
async fn repository_metadata(router: &MultiServiceRouter, repository: &str) -> Value {
    let payload = ok(
        router,
        "GetRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    payload
        .get("repositoryMetadata")
        .unwrap_or_else(|| panic!("repository {repository:?}: no repositoryMetadata in {payload}"))
        .clone()
}

/// An epoch-seconds timestamp member of a `repositoryMetadata` payload.
fn as_f64(metadata: &Value, member: &str) -> f64 {
    metadata
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no {member} in {metadata}"))
}

/// The commit a reference points at, or `None` where the repository holds no
/// reference of that name (`GetBranch` answers `BranchDoesNotExistException`).
async fn branch_tip(
    router: &MultiServiceRouter,
    repository: &str,
    branch: &str,
) -> Option<String> {
    let (status, payload) = crate::repository_lifecycle::call(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": branch }),
    )
    .await;
    if status == 200 {
        return Some(string_at(&payload, "/branch/commitId"));
    }
    assert_eq!(
        status, 400,
        "repository {repository:?} branch {branch:?}: GetBranch answered {status}: {payload}"
    );
    let code = payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        code.ends_with("BranchDoesNotExistException"),
        "repository {repository:?} branch {branch:?}: expected BranchDoesNotExistException, got {payload}"
    );
    None
}

/// One row of the branch-name table: a reference that is created and then deleted.
///
/// Rows differ along branch-name form and length, which is what the rule generalises
/// over: `BranchName` is modeled as `length` 1..256 with no pattern
/// (`smithy_query(query="operation",operation="DeleteBranch")`), so one character and
/// 256 characters are the published bounds, and slashes, dots, dashes, underscores and
/// non-ASCII characters are all inside the class.
fn branch_cases() -> Vec<(&'static str, String)> {
    vec![
        ("one character — the modeled minimum length", "x".to_string()),
        ("ordinary name", "main".to_string()),
        ("slashed name", "feature/area/topic".to_string()),
        (
            "dots, dashes and underscores",
            "release-1.0_final.rc2".to_string(),
        ),
        ("non-ascii name", "feature/ünïcode".to_string()),
        (
            "256 characters — the modeled maximum length",
            "b".repeat(256),
        ),
    ]
}

#[tokio::test]
async fn deleting_a_branch_removes_that_reference_and_reports_it() {
    let router = emulator();

    for (index, (label, branch)) in branch_cases().into_iter().enumerate() {
        let repository = format!("delete-branch-{index}");
        let tip = seed(&router, &repository).await;
        create_branch(&router, &repository, &branch, &tip).await;
        assert_eq!(
            branch_tip(&router, &repository, &branch).await.as_deref(),
            Some(tip.as_str()),
            "case {label:?}: the reference must exist before it is deleted"
        );
        let before = repository_metadata(&router, &repository).await;

        // The delete itself reports the reference it removed: its own name, and the
        // commit that was its tip.
        let deleted = ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": repository, "branchName": branch }),
        )
        .await;
        assert_eq!(
            deleted.pointer("/deletedBranch/branchName").and_then(Value::as_str),
            Some(branch.as_str()),
            "case {label:?}: expected deletedBranch.branchName {branch:?}, got {deleted}"
        );
        assert_eq!(
            deleted.pointer("/deletedBranch/commitId").and_then(Value::as_str),
            Some(tip.as_str()),
            "case {label:?}: expected deletedBranch.commitId {tip:?}, got {deleted}"
        );

        // And the store agrees: the reference is gone, every other reference is not.
        assert_eq!(
            branch_tip(&router, &repository, &branch).await,
            None,
            "case {label:?}: the deleted reference must be gone from the repository"
        );
        assert_eq!(
            branch_tip(&router, &repository, BOOTSTRAP_BRANCH)
                .await
                .as_deref(),
            Some(tip.as_str()),
            "case {label:?}: another reference must be untouched by the delete"
        );

        // Only the reference was deleted, not the commits it led to.
        let commit = ok(
            &router,
            "GetCommit",
            &json!({ "repositoryName": repository, "commitId": tip }),
        )
        .await;
        assert_eq!(
            string_at(&commit, "/commit/commitId"),
            tip,
            "case {label:?}: the tip commit must still be readable by id, got {commit}"
        );

        // Nothing else about the repository moved, and removing a reference is a
        // modification of it.
        let after = repository_metadata(&router, &repository).await;
        for member in [
            "repositoryId",
            "repositoryName",
            "Arn",
            "cloneUrlHttp",
            "cloneUrlSsh",
            "accountId",
            "creationDate",
            "defaultBranch",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {label:?}: {member} must be unchanged by DeleteBranch, got {after}"
            );
        }
        assert!(
            as_f64(&after, "lastModifiedDate") >= as_f64(&before, "lastModifiedDate"),
            "case {label:?}: lastModifiedDate must not go backwards, got {after}"
        );

        // The model declares no "branch does not exist" error for this operation and
        // `deletedBranch` is optional, so deleting the same name again succeeds and
        // reports no deleted branch.
        let again = ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": repository, "branchName": branch }),
        )
        .await;
        assert_eq!(
            again.pointer("/deletedBranch"),
            None,
            "case {label:?}: a second delete of the same name must report no deletedBranch, got {again}"
        );
        let settled = repository_metadata(&router, &repository).await;
        assert_eq!(
            as_f64(&settled, "lastModifiedDate"),
            as_f64(&after, "lastModifiedDate"),
            "case {label:?}: a delete that removed nothing must not modify the repository"
        );
    }
}

#[tokio::test]
async fn a_delete_takes_exactly_one_reference_of_one_repository() {
    let router = emulator();

    // Two repositories holding the same four reference names, so a delete is checked
    // against the number of references a repository holds AND against the repository
    // the request names.
    let names = ["alpha", "beta/one", "beta/two", "gamma"];
    let mut tips = Vec::new();
    for repository in ["left-repo", "right-repo"] {
        let tip = seed(&router, repository).await;
        for name in names {
            create_branch(&router, repository, name, &tip).await;
        }
        tips.push(tip);
    }
    let (left_tip, right_tip) = (tips[0].clone(), tips[1].clone());

    // Delete the four references of the left repository one at a time: each delete
    // takes exactly the named one and leaves the ones not yet named.
    for (index, name) in names.iter().enumerate() {
        let deleted = ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": "left-repo", "branchName": name }),
        )
        .await;
        assert_eq!(
            deleted.pointer("/deletedBranch/branchName").and_then(Value::as_str),
            Some(*name),
            "case {name:?}: expected the named reference to be reported deleted, got {deleted}"
        );
        for (other_index, other) in names.iter().enumerate() {
            let expected = if other_index <= index {
                None
            } else {
                Some(left_tip.clone())
            };
            assert_eq!(
                branch_tip(&router, "left-repo", other).await,
                expected,
                "case {name:?}: after deleting it, left-repo/{other} must be {expected:?}"
            );
        }
        // The other repository's references of the SAME names are untouched.
        for other in names {
            assert_eq!(
                branch_tip(&router, "right-repo", other).await.as_deref(),
                Some(right_tip.as_str()),
                "case {name:?}: right-repo/{other} must be untouched by a delete in left-repo"
            );
        }
    }
    // The default branch of the left repository is the one reference still standing.
    assert_eq!(
        branch_tip(&router, "left-repo", BOOTSTRAP_BRANCH)
            .await
            .as_deref(),
        Some(left_tip.as_str()),
        "the default branch must still be there after every other reference is deleted"
    );
}

#[tokio::test]
async fn the_default_branch_is_refused_until_the_default_moves() {
    let router = emulator();

    // Whichever reference is the default is the refused one, so the default is moved
    // around a repository holding several and the refusal follows it.
    let tip = seed(&router, "default-guard").await;
    let others = ["release/1.x", "d", "feature/ünïcode"];
    for name in others {
        create_branch(&router, "default-guard", name, &tip).await;
    }

    let mut default = BOOTSTRAP_BRANCH.to_string();
    for next in others {
        let code = error(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": "default-guard", "branchName": default }),
            400,
        )
        .await;
        assert_eq!(
            code, "DefaultBranchCannotBeDeletedException",
            "case {default:?}: expected DefaultBranchCannotBeDeletedException, got {code}"
        );
        // A refused delete persists nothing: the reference is still there.
        assert_eq!(
            branch_tip(&router, "default-guard", &default)
                .await
                .as_deref(),
            Some(tip.as_str()),
            "case {default:?}: a refused delete must leave the reference in place"
        );

        // "To delete this branch, you must first set another branch as the default
        // branch": once the default moves off it, the same delete succeeds.
        ok(
            &router,
            "UpdateDefaultBranch",
            &json!({ "repositoryName": "default-guard", "defaultBranchName": next }),
        )
        .await;
        let deleted = ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": "default-guard", "branchName": default }),
        )
        .await;
        assert_eq!(
            deleted.pointer("/deletedBranch/branchName").and_then(Value::as_str),
            Some(default.as_str()),
            "case {default:?}: the formerly default reference must be deletable, got {deleted}"
        );
        default = next.to_string();
    }

    // The last standing reference is the default, and it is still refused.
    let code = error(
        &router,
        "DeleteBranch",
        &json!({ "repositoryName": "default-guard", "branchName": default }),
        400,
    )
    .await;
    assert_eq!(
        code, "DefaultBranchCannotBeDeletedException",
        "case {default:?}: the last reference is the default, so it is still refused"
    );
}

#[tokio::test]
async fn a_name_the_repository_does_not_hold_is_a_success_that_deletes_nothing() {
    let router = emulator();
    let tip = seed(&router, "absent-names").await;
    create_branch(&router, "absent-names", "kept", &tip).await;
    let before = repository_metadata(&router, "absent-names").await;

    // Every row is a well-formed `BranchName` the repository does not hold — the model
    // declares no `BranchDoesNotExistException` for this operation, so each is a 200
    // with no `deletedBranch` rather than a refusal.
    let rows = vec![
        ("a name never created", "never-created".to_string()),
        ("a name that belongs to no repository", "k".to_string()),
        (
            "the fully qualified reference form of a reference it does hold",
            "refs/heads/kept".to_string(),
        ),
        (
            "a name differing from a held one only in case",
            "KEPT".to_string(),
        ),
        (
            "256 characters — the modeled maximum length",
            "z".repeat(256),
        ),
    ];

    for (label, branch) in rows {
        let payload = ok(
            &router,
            "DeleteBranch",
            &json!({ "repositoryName": "absent-names", "branchName": branch }),
        )
        .await;
        assert_eq!(
            payload.pointer("/deletedBranch"),
            None,
            "case {label:?}: expected no deletedBranch, got {payload}"
        );
        assert_eq!(
            branch_tip(&router, "absent-names", "kept").await.as_deref(),
            Some(tip.as_str()),
            "case {label:?}: the references the repository does hold must be untouched"
        );
        assert_eq!(
            branch_tip(&router, "absent-names", BOOTSTRAP_BRANCH)
                .await
                .as_deref(),
            Some(tip.as_str()),
            "case {label:?}: the default branch must be untouched"
        );
        let after = repository_metadata(&router, "absent-names").await;
        assert_eq!(
            as_f64(&after, "lastModifiedDate"),
            as_f64(&before, "lastModifiedDate"),
            "case {label:?}: a delete that removed nothing must not modify the repository"
        );
    }
}

#[tokio::test]
async fn a_missing_or_malformed_name_and_an_unknown_repository_are_refused() {
    let router = emulator();
    let tip = seed(&router, "refusals").await;
    create_branch(&router, "refusals", "kept", &tip).await;

    // The bounds come from the modeled members: `repositoryName` is len 1..100 with
    // pattern ^[\w\.-]+$ and `branchName` is len 1..256
    // (`smithy_query(query="operation",operation="DeleteBranch")`). Each row is a value
    // on or past a published bound, a name that cannot be a git reference, or a
    // repository the store does not hold.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository the store does not hold",
            json!({ "repositoryName": "absent-repo", "branchName": "kept" }),
            "RepositoryDoesNotExistException",
        ),
        (
            "no repository name at all",
            json!({ "branchName": "kept" }),
            "RepositoryNameRequiredException",
        ),
        (
            "no branch name at all",
            json!({ "repositoryName": "refusals" }),
            "BranchNameRequiredException",
        ),
        (
            "a repository name outside the modeled pattern",
            json!({ "repositoryName": "not a repo name", "branchName": "kept" }),
            "InvalidRepositoryNameException",
        ),
        (
            "a repository name one character past the modeled maximum of 100",
            json!({ "repositoryName": "r".repeat(101), "branchName": "kept" }),
            "InvalidRepositoryNameException",
        ),
        (
            "a branch name below the modeled minimum length of 1",
            json!({ "repositoryName": "refusals", "branchName": "" }),
            "InvalidBranchNameException",
        ),
        (
            "a branch name one character past the modeled maximum of 256",
            json!({ "repositoryName": "refusals", "branchName": "b".repeat(257) }),
            "InvalidBranchNameException",
        ),
        // A branch name is a git reference name, so these are names no repository can
        // hold: git refuses a bare "@", a trailing "/", "..", "//" and a space.
        (
            "a bare @, which is not a reference name",
            json!({ "repositoryName": "refusals", "branchName": "@" }),
            "InvalidBranchNameException",
        ),
        (
            "a name ending in a slash",
            json!({ "repositoryName": "refusals", "branchName": "feature/" }),
            "InvalidBranchNameException",
        ),
        (
            "a name containing two dots",
            json!({ "repositoryName": "refusals", "branchName": "release..1" }),
            "InvalidBranchNameException",
        ),
        (
            "a name with an empty path component",
            json!({ "repositoryName": "refusals", "branchName": "feature//topic" }),
            "InvalidBranchNameException",
        ),
        (
            "a name containing a space",
            json!({ "repositoryName": "refusals", "branchName": "my branch" }),
            "InvalidBranchNameException",
        ),
    ];

    for (label, input, expected) in cases {
        let code = error(&router, "DeleteBranch", &input, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
        // A refused delete persists nothing.
        assert_eq!(
            branch_tip(&router, "refusals", "kept").await.as_deref(),
            Some(tip.as_str()),
            "case {label:?}: a refused delete must leave every reference in place"
        );
    }

    // And the reference the repository does hold is still deletable after every
    // refusal.
    let deleted = ok(
        &router,
        "DeleteBranch",
        &json!({ "repositoryName": "refusals", "branchName": "kept" }),
    )
    .await;
    assert_eq!(
        string_at(&deleted, "/deletedBranch/branchName"),
        "kept",
        "a reference the repository holds must still be deletable, got {deleted}"
    );
}
