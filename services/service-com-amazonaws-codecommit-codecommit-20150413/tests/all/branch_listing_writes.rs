// SPDX-License-Identifier: CC0-1.0

//! What a `ListBranches` listing does as a repository's branches CHANGE, over the wire
//! through the router that serves the real emulator — the half the sibling
//! [`crate::branch_listing`] suite (which owns the paging contract and the refusals)
//! does not cover.
//!
//! The rule under test: a listing reports exactly the references the named repository
//! holds AT THAT MOMENT — every one a write created, none a write removed, and none
//! belonging to another repository — read from the same stored branch map `GetBranch`
//! reads, so every listed name reads back there at the commit it was created at.
//!
//! The rows differ along what the rule generalises over: the reference name's form and
//! length (one character, slashed, punctuated, non-ASCII, and both published bounds of
//! `BranchName`, len 1..256 per
//! `smithy_query(query="shape",shape="BranchName")`), how many references the
//! repository holds (none at all, one, several, the store's own
//! `MAX_BRANCHES_PER_REPOSITORY`), and which write last changed the set
//! (`PutFile`'s first commit, `CreateBranch`, `DeleteBranch`, `UpdateDefaultBranch`
//! followed by deleting the old default). The harness (`emulator`, `ok`, `error`) is
//! the one in [`crate::repository_lifecycle`].

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, emulator, error, ok, string_at};

/// The reference the first commit of an empty repository creates, so it is the one
/// every seeded repository holds.
const BOOTSTRAP_BRANCH: &str = "main";

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

/// The names one `ListBranches` call reports for `repository`.
async fn listed(router: &MultiServiceRouter, repository: &str, case: &str) -> Vec<String> {
    let payload = ok(
        router,
        "ListBranches",
        &json!({ "repositoryName": repository }),
    )
    .await;
    payload
        .pointer("/branches")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a branches list, got {payload}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a name string, got {payload}"))
                .to_string()
        })
        .collect()
}

/// Assert the whole listing of `repository` is exactly `expected`, as a set, and that
/// no name is reported twice.
async fn assert_listing(
    router: &MultiServiceRouter,
    repository: &str,
    expected: &[&str],
    case: &str,
) {
    let names = listed(router, repository, case).await;
    let got: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let want: BTreeSet<&str> = expected.iter().copied().collect();
    assert_eq!(
        got, want,
        "case {case:?}: expected branches {want:?}, got {names:?}"
    );
    assert_eq!(
        names.len(),
        want.len(),
        "case {case:?}: a name must be reported once, got {names:?}"
    );
}

#[tokio::test]
async fn every_listed_name_reads_back_as_a_branch_at_the_commit_it_was_created_at() {
    let router = emulator();

    // (case, repository, the references created beyond the bootstrap commit — `None`
    // for a repository that was created and never written to, which holds none).
    let rows: Vec<(&str, &str, Option<Vec<String>>)> = vec![
        (
            "a repository with no commit, so no reference at all",
            "reads-none",
            None,
        ),
        (
            "one reference, the one the first commit created",
            "reads-one",
            Some(Vec::new()),
        ),
        (
            "several references created out of alphabetical order",
            "reads-unordered",
            Some(vec![
                "zeta".to_string(),
                "alpha".to_string(),
                "middle".to_string(),
            ]),
        ),
        (
            "slashed and punctuated reference names",
            "reads-shapes",
            Some(vec![
                "feature/login".to_string(),
                "release-1.0".to_string(),
                "v1.2.3_rc".to_string(),
            ]),
        ),
        (
            // Both published bounds of `BranchName` (len 1..256).
            "the shortest and the longest name the model allows",
            "reads-bounds",
            Some(vec!["a".to_string(), "b".repeat(256)]),
        ),
        (
            "a non-ASCII reference name",
            "reads-unicode",
            Some(vec!["fonctionnalité/ÄÖÜ".to_string()]),
        ),
    ];

    for (case, repository, extra) in rows {
        let mut expected: BTreeSet<String> = BTreeSet::new();
        let tip = match &extra {
            None => {
                ok(
                    &router,
                    "CreateRepository",
                    &json!({ "repositoryName": repository }),
                )
                .await;
                None
            }
            Some(branches) => {
                let tip = seed(&router, repository).await;
                expected.insert(BOOTSTRAP_BRANCH.to_string());
                for branch in branches {
                    create_branch(&router, repository, branch, &tip).await;
                    expected.insert(branch.clone());
                }
                Some(tip)
            }
        };

        let names = listed(&router, repository, case).await;
        assert_eq!(
            names.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: expected branches {expected:?}, got {names:?}"
        );
        assert_eq!(
            names.len(),
            expected.len(),
            "case {case:?}: a name must be reported once, got {names:?}"
        );

        // The listing reads the same stored branch map `GetBranch` reads: every name
        // it reported is a reference, at the commit it was created at.
        if let Some(tip) = &tip {
            for branch in &names {
                let read = ok(
                    &router,
                    "GetBranch",
                    &json!({ "repositoryName": repository, "branchName": branch }),
                )
                .await;
                assert_eq!(
                    string_at(&read, "/branch/branchName"),
                    *branch,
                    "case {case:?}: GetBranch must report the listed name, got {read}"
                );
                assert_eq!(
                    string_at(&read, "/branch/commitId"),
                    *tip,
                    "case {case:?}: branch {branch:?} must point at the tip it was created at, got {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_listing_follows_the_writes_and_is_per_repository() {
    let router = emulator();
    let tip = seed(&router, "follows-writes").await;

    // A second repository, so the listing is shown to be per repository rather than
    // per store.
    let other_tip = seed(&router, "follows-other").await;
    create_branch(&router, "follows-other", "only-over-here", &other_tip).await;

    assert_listing(
        &router,
        "follows-writes",
        &[BOOTSTRAP_BRANCH],
        "the first commit created one reference",
    )
    .await;

    create_branch(&router, "follows-writes", "feature/added", &tip).await;
    assert_listing(
        &router,
        "follows-writes",
        &[BOOTSTRAP_BRANCH, "feature/added"],
        "CreateBranch added a reference",
    )
    .await;

    ok(
        &router,
        "DeleteBranch",
        &json!({ "repositoryName": "follows-writes", "branchName": "feature/added" }),
    )
    .await;
    assert_listing(
        &router,
        "follows-writes",
        &[BOOTSTRAP_BRANCH],
        "DeleteBranch removed that reference",
    )
    .await;

    // The default branch moved onto another reference, so the one the repository
    // started from can be removed too — and the listing follows that as well.
    create_branch(&router, "follows-writes", "promoted", &tip).await;
    ok(
        &router,
        "UpdateDefaultBranch",
        &json!({ "repositoryName": "follows-writes", "defaultBranchName": "promoted" }),
    )
    .await;
    ok(
        &router,
        "DeleteBranch",
        &json!({ "repositoryName": "follows-writes", "branchName": BOOTSTRAP_BRANCH }),
    )
    .await;
    assert_listing(
        &router,
        "follows-writes",
        &["promoted"],
        "the old default was removed once the default moved",
    )
    .await;

    // Neither repository's listing ever carried the other's reference.
    assert_listing(
        &router,
        "follows-other",
        &[BOOTSTRAP_BRANCH, "only-over-here"],
        "the other repository's listing is its own",
    )
    .await;
}

#[tokio::test]
async fn a_repository_filled_to_its_branch_bound_lists_every_reference() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_BRANCHES_PER_REPOSITORY;

    let router = emulator();
    let tip = seed(&router, "bound-full").await;
    // The bootstrap commit already created one reference, so this fills the repository
    // exactly to its per-repository bound.
    for index in 1..MAX_BRANCHES_PER_REPOSITORY {
        create_branch(&router, "bound-full", &format!("fill/{index:05}"), &tip).await;
    }

    let case = "a repository at its branch bound";
    let names = listed(&router, "bound-full", case).await;
    assert_eq!(
        names.len(),
        MAX_BRANCHES_PER_REPOSITORY,
        "case {case:?}: expected {MAX_BRANCHES_PER_REPOSITORY} names, got {}",
        names.len()
    );
    assert_eq!(
        names.iter().collect::<BTreeSet<&String>>().len(),
        MAX_BRANCHES_PER_REPOSITORY,
        "case {case:?}: every name must be reported once, got {names:?}"
    );
}

#[tokio::test]
async fn a_deleted_repository_holds_no_listing() {
    let router = emulator();
    seed(&router, "kept-repository").await;
    seed(&router, "deleted-repository").await;
    ok(
        &router,
        "DeleteRepository",
        &json!({ "repositoryName": "deleted-repository" }),
    )
    .await;

    // The refusal follows the STORE, not the shape of the name: a name that was a live
    // repository a moment ago is refused exactly like one that never existed.
    let rows: Vec<(&str, &str)> = vec![
        ("a name no repository ever had", "never-created"),
        ("the name of a deleted repository", "deleted-repository"),
    ];
    for (case, repository) in rows {
        let code = error(
            &router,
            "ListBranches",
            &json!({ "repositoryName": repository }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {case:?}: expected RepositoryDoesNotExistException, got {code}"
        );
    }

    // And the repository that was kept still lists its own reference, so the rows above
    // were refused for their repository and not because the request was unusable.
    assert_listing(
        &router,
        "kept-repository",
        &[BOOTSTRAP_BRANCH],
        "a refused listing leaves the other repository readable",
    )
    .await;
}
