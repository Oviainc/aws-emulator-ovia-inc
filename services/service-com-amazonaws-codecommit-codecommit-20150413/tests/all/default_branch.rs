// SPDX-License-Identifier: CC0-1.0

//! `UpdateDefaultBranch` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: `UpdateDefaultBranch` stores the named branch as the
//! repository's default branch when the repository holds a branch of that name, so
//! every later read of that repository reports it; it changes nothing else about the
//! repository (not its id, its ARN, its clone URLs or its branch tips), it succeeds
//! again when the name is already the default, and it refuses a repository or a
//! branch the store does not hold with the error the model declares for it.
//!
//! The tables below span the class the rule covers — branch-name form and length
//! (one character, ordinary, slashed, punctuated, unicode, the modeled maximum of
//! 256), the number of branches a repository holds, and the direction the default is
//! moved — rather than one recorded request. The harness (`emulator_with_engine`,
//! `ok`, `error`) is the one in [`crate::repository_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    Branch, ComAmazonawsCodecommitCodecommit20150413State,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{emulator_with_engine, error, ok};

/// The branch `PutFile` creates when it writes the first commit of an empty
/// repository; it is that repository's default branch until this operation moves it.
const BOOTSTRAP_BRANCH: &str = "bootstrap";

/// The repository's metadata as `GetRepository` reports it — the read this test
/// proves the write through.
async fn repository_metadata(router: &MultiServiceRouter, repository: &str) -> Value {
    let payload = ok(router, "GetRepository", &json!({ "repositoryName": repository })).await;
    payload
        .get("repositoryMetadata")
        .unwrap_or_else(|| panic!("repository {repository:?}: no repositoryMetadata in {payload}"))
        .clone()
}

/// A numeric member of a `repositoryMetadata` payload (an epoch-seconds timestamp).
fn as_f64(metadata: &Value, member: &str) -> f64 {
    metadata
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no {member} in {metadata}"))
}

/// The repository's stored `defaultBranch`, or `None` when it has none.
async fn default_branch(router: &MultiServiceRouter, repository: &str) -> Option<String> {
    repository_metadata(router, repository)
        .await
        .get("defaultBranch")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create a repository and write its first commit, which `PutFile` puts on
/// [`BOOTSTRAP_BRANCH`] — the repository's default branch until it is moved.
/// Answers the tip commit id.
async fn repository_with_bootstrap_branch(router: &MultiServiceRouter, repository: &str) -> String {
    ok(router, "CreateRepository", &json!({ "repositoryName": repository })).await;
    let put = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": BOOTSTRAP_BRANCH,
            "filePath": "README.md",
            "fileContent": aws_smithy_types::base64::encode(b"# init\n".as_slice()),
            "commitMessage": "init",
            "name": "Test Author",
            "email": "author@example.com",
        }),
    )
    .await;
    put.get("commitId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("repository {repository:?}: no commitId in {put}"))
        .to_string()
}

/// Add one more branch, at the same tip, to a repository the store already holds.
///
/// `CreateBranch` is still a scaffold stub in this crate (its own issue owns it), so
/// a repository with SEVERAL branches cannot be arranged over the wire yet. This
/// writes the branch into the service's own state — the same map `PutFile` writes and
/// `GetBranch` reads — so `UpdateDefaultBranch` is still exercised end-to-end against
/// stored state, over branch names of every form the model allows.
fn seed_branch(engine: &engine::InMemoryEngine, repository: &str, branch: &str, tip: &str) {
    let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
    let mut store = state.lock();
    let stored = store
        .repositories
        .get_mut(repository)
        .unwrap_or_else(|| panic!("repository {repository:?} must be in the store"));
    stored.branches.insert(
        branch.to_string(),
        Branch {
            name: branch.to_string(),
            commit_id: tip.to_string(),
        },
    );
}

/// One row of the branch-name table: a branch the repository holds, which the
/// default branch is then moved to and back from.
#[derive(Debug, Clone)]
struct BranchCase {
    label: &'static str,
    branch: String,
}

/// The table. Rows differ along branch-name form and length, which is what the rule
/// generalises over: `BranchName`'s modeled bounds are `length` 1..256 with no
/// pattern (`smithy_query(query="operation",operation="UpdateDefaultBranch")`), so
/// one character and 256 characters are the published bounds, and slashes, dots,
/// dashes and non-ASCII characters are all inside the class.
fn branch_cases() -> Vec<BranchCase> {
    vec![
        BranchCase {
            label: "one character — the modeled minimum length",
            branch: "x".to_string(),
        },
        BranchCase {
            label: "ordinary name",
            branch: "main".to_string(),
        },
        BranchCase {
            label: "slashed name",
            branch: "feature/area/topic".to_string(),
        },
        BranchCase {
            label: "dots, dashes and underscores",
            branch: "release-1.0_final.rc2".to_string(),
        },
        BranchCase {
            label: "non-ascii name",
            branch: "feature/ünïcode".to_string(),
        },
        BranchCase {
            label: "256 characters — the modeled maximum length",
            branch: "b".repeat(256),
        },
    ]
}

#[tokio::test]
async fn the_default_branch_moves_to_any_branch_the_repository_holds() {
    let (router, engine) = emulator_with_engine();

    for (index, case) in branch_cases().into_iter().enumerate() {
        let label = case.label;
        let repository = format!("default-branch-{index}");
        let tip = repository_with_bootstrap_branch(&router, &repository).await;

        // The first branch a repository gets is its default branch, so this is where
        // every row starts.
        assert_eq!(
            default_branch(&router, &repository).await.as_deref(),
            Some(BOOTSTRAP_BRANCH),
            "case {label:?}: the bootstrap branch must be the default before the update"
        );
        seed_branch(&engine, &repository, &case.branch, &tip);
        let before = repository_metadata(&router, &repository).await;

        // The update itself: an empty success payload, and the new default branch
        // read back through GetRepository.
        let updated = ok(
            &router,
            "UpdateDefaultBranch",
            &json!({ "repositoryName": repository, "defaultBranchName": case.branch }),
        )
        .await;
        assert!(
            updated.is_null() || updated.as_object().is_some_and(serde_json::Map::is_empty),
            "case {label:?}: UpdateDefaultBranch has no modeled output, got {updated}"
        );
        let after = repository_metadata(&router, &repository).await;
        assert_eq!(
            after.get("defaultBranch").and_then(Value::as_str),
            Some(case.branch.as_str()),
            "case {label:?}: expected defaultBranch {:?}, got {after}",
            case.branch
        );

        // Nothing else about the repository moved.
        for field in [
            "repositoryId",
            "repositoryName",
            "Arn",
            "cloneUrlHttp",
            "cloneUrlSsh",
            "accountId",
            "creationDate",
        ] {
            assert_eq!(
                after.get(field),
                before.get(field),
                "case {label:?}: {field} must be unchanged by UpdateDefaultBranch, got {after}"
            );
        }
        // A change to the repository is a modification of it, so the last-modified
        // instant cannot predate the update that moved the default branch.
        let modified_before = as_f64(&before, "lastModifiedDate");
        let modified_after = as_f64(&after, "lastModifiedDate");
        assert!(
            modified_after >= modified_before,
            "case {label:?}: lastModifiedDate must not go backwards, got {modified_after} after {modified_before}"
        );
        // Both branches still point at the commit they pointed at.
        for branch in [BOOTSTRAP_BRANCH, case.branch.as_str()] {
            let read = ok(
                &router,
                "GetBranch",
                &json!({ "repositoryName": repository, "branchName": branch }),
            )
            .await;
            assert_eq!(
                read.pointer("/branch/commitId").and_then(Value::as_str),
                Some(tip.as_str()),
                "case {label:?}: branch {branch:?} must still point at the tip, got {read}"
            );
        }

        // "If you use this operation to change the default branch name to the current
        // default branch name, a success message is returned even though the default
        // branch did not change."
        ok(
            &router,
            "UpdateDefaultBranch",
            &json!({ "repositoryName": repository, "defaultBranchName": case.branch }),
        )
        .await;
        let unchanged = repository_metadata(&router, &repository).await;
        assert_eq!(
            unchanged.get("defaultBranch").and_then(Value::as_str),
            Some(case.branch.as_str()),
            "case {label:?}: setting the current default again must leave it in place"
        );
        assert_eq!(
            as_f64(&unchanged, "lastModifiedDate"),
            modified_after,
            "case {label:?}: the default branch did not change, so the repository was not modified"
        );

        // And the move is not one-way: the default goes back to the branch it came
        // from.
        ok(
            &router,
            "UpdateDefaultBranch",
            &json!({ "repositoryName": repository, "defaultBranchName": BOOTSTRAP_BRANCH }),
        )
        .await;
        assert_eq!(
            default_branch(&router, &repository).await.as_deref(),
            Some(BOOTSTRAP_BRANCH),
            "case {label:?}: the default branch must move back"
        );
    }
}

#[tokio::test]
async fn the_default_branch_is_per_repository_and_survives_repeated_reads() {
    let (router, engine) = emulator_with_engine();

    // Two repositories, each holding the same two branch names: moving one's default
    // must not touch the other's.
    for repository in ["left-repo", "right-repo"] {
        let tip = repository_with_bootstrap_branch(&router, repository).await;
        seed_branch(&engine, repository, "shared-branch", &tip);
    }
    ok(
        &router,
        "UpdateDefaultBranch",
        &json!({ "repositoryName": "left-repo", "defaultBranchName": "shared-branch" }),
    )
    .await;
    assert_eq!(
        default_branch(&router, "left-repo").await.as_deref(),
        Some("shared-branch"),
        "the updated repository must report the new default branch"
    );
    assert_eq!(
        default_branch(&router, "right-repo").await.as_deref(),
        Some(BOOTSTRAP_BRANCH),
        "another repository's default branch must be untouched"
    );

    // A read reports the same default branch however often it is repeated: reading
    // never changes what the next caller sees.
    for attempt in 0..3 {
        assert_eq!(
            default_branch(&router, "left-repo").await.as_deref(),
            Some("shared-branch"),
            "read {attempt}: GetRepository must keep reporting the stored default branch"
        );
    }
}

/// One row of the refusal table: a request, the modeled error code it must answer,
/// and its HTTP status.
struct RefusalCase {
    label: &'static str,
    input: Value,
    code: &'static str,
    status: u16,
}

#[tokio::test]
async fn an_unknown_repository_or_branch_and_a_malformed_name_are_refused() {
    let (router, engine) = emulator_with_engine();
    let tip = repository_with_bootstrap_branch(&router, "known-repo").await;
    seed_branch(&engine, "known-repo", "other-branch", &tip);
    // A second repository, holding a branch name the first one does not: the default
    // branch names a branch OF THE NAMED REPOSITORY, not of any repository.
    let other_tip = repository_with_bootstrap_branch(&router, "neighbour-repo").await;
    seed_branch(&engine, "neighbour-repo", "only-in-neighbour", &other_tip);

    // The length bounds come from the modeled `repositoryName` (len 1..100, pattern
    // ^[\w\.-]+$) and `defaultBranchName` (len 1..256): each case is a value on or
    // past one published bound, or a resource the store does not hold.
    let cases = vec![
        RefusalCase {
            label: "a repository the store does not hold",
            input: json!({ "repositoryName": "absent-repo", "defaultBranchName": BOOTSTRAP_BRANCH }),
            code: "RepositoryDoesNotExistException",
            status: 400,
        },
        RefusalCase {
            label: "a branch the repository does not hold",
            input: json!({ "repositoryName": "known-repo", "defaultBranchName": "no-such-branch" }),
            code: "BranchDoesNotExistException",
            status: 400,
        },
        RefusalCase {
            label: "a branch of another repository is not a branch of this one",
            input: json!({ "repositoryName": "known-repo", "defaultBranchName": "only-in-neighbour" }),
            code: "BranchDoesNotExistException",
            status: 400,
        },
        RefusalCase {
            label: "no repository name at all",
            input: json!({ "defaultBranchName": BOOTSTRAP_BRANCH }),
            code: "RepositoryNameRequiredException",
            status: 400,
        },
        RefusalCase {
            label: "no branch name at all",
            input: json!({ "repositoryName": "known-repo" }),
            code: "BranchNameRequiredException",
            status: 400,
        },
        RefusalCase {
            label: "a repository name outside the modeled pattern",
            input: json!({ "repositoryName": "not a repo name", "defaultBranchName": BOOTSTRAP_BRANCH }),
            code: "InvalidRepositoryNameException",
            status: 400,
        },
        RefusalCase {
            label: "a repository name one character past the modeled maximum of 100",
            input: json!({ "repositoryName": "r".repeat(101), "defaultBranchName": BOOTSTRAP_BRANCH }),
            code: "InvalidRepositoryNameException",
            status: 400,
        },
        RefusalCase {
            label: "a branch name one character past the modeled maximum of 256",
            input: json!({ "repositoryName": "known-repo", "defaultBranchName": "b".repeat(257) }),
            code: "InvalidBranchNameException",
            status: 400,
        },
        RefusalCase {
            label: "a branch name below the modeled minimum length of 1",
            input: json!({ "repositoryName": "known-repo", "defaultBranchName": "" }),
            code: "InvalidBranchNameException",
            status: 400,
        },
    ];

    for case in cases {
        let label = case.label;
        let code = error(&router, "UpdateDefaultBranch", &case.input, case.status).await;
        assert_eq!(
            code, case.code,
            "case {label:?}: expected {}, got {code}",
            case.code
        );
        // A refused update persists nothing: the default branch is still the one the
        // repository had.
        assert_eq!(
            default_branch(&router, "known-repo").await.as_deref(),
            Some(BOOTSTRAP_BRANCH),
            "case {label:?}: a refused update must not move the default branch"
        );
    }

    // And the branch the repository does hold is still accepted after every refusal.
    ok(
        &router,
        "UpdateDefaultBranch",
        &json!({ "repositoryName": "known-repo", "defaultBranchName": "other-branch" }),
    )
    .await;
    assert_eq!(
        default_branch(&router, "known-repo").await.as_deref(),
        Some("other-branch"),
        "a branch the repository holds must still be accepted"
    );
}
