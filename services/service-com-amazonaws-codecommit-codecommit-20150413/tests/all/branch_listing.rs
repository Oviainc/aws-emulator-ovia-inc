// SPDX-License-Identifier: CC0-1.0

//! `ListBranches` over the wire, driven through the same router the emulator serves.
//!
//! The rule under test: `ListBranches` reports exactly the branch names the named
//! repository holds — in name order, paged by `nextToken` — and reports an EMPTY list
//! for a repository that exists but holds no branch yet, which is what tells an empty
//! repository apart from a missing one (`RepositoryDoesNotExistException`). The
//! recorded case ("no branches, then `['main']` after one commit") is one row of the
//! tables below; the rows differ along what the rule generalises over: the branch
//! names' form, how many branches are stored (0, 1, a few, exactly one page, more than
//! one page), and the order they were stored in.
//!
//! Every branch a case needs is created through the API: a repository's FIRST branch
//! can only come from a commit (`PutFile`), and the rest are made at that same commit
//! with `CreateBranch`. Only the multi-PAGE rows write the branch set into the store
//! directly, because `CreateBranch` refuses past the store's own
//! `MAX_BRANCHES_PER_REPOSITORY`, which is far below one page; the call under test is
//! made over HTTP either way, and the store is otherwise only READ, to prove a listing
//! wrote nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codecommit_codecommit_20150413::ops::list_branches::PAGE_SIZE;
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    Branch, ComAmazonawsCodecommitCodecommit20150413State, MAX_BRANCHES_PER_REPOSITORY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, emulator, emulator_with_engine, error, ok, string_at};

/// One `ListBranches` page: the names it carried and the token it handed back.
struct Page {
    branches: Vec<String>,
    next_token: Option<String>,
}

/// Request one page, asserting that `branches` is PRESENT (an empty list is an answer,
/// not an absent member).
async fn list_page(
    router: &MultiServiceRouter,
    repository: &str,
    token: Option<&str>,
    case: &str,
) -> Page {
    let mut body = json!({ "repositoryName": repository });
    if let Some(token) = token {
        body["nextToken"] = json!(token);
    }
    let payload = ok(router, "ListBranches", &body).await;
    let branches = payload
        .get("branches")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListBranches must report a branches list, got {payload}")
        })
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a branch name must be a string, got {name}"))
                .to_string()
        })
        .collect();
    Page {
        branches,
        next_token: payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Every branch name of `repository`, walked page by page the way a paginator does,
/// plus the page sizes seen.
async fn list_all(router: &MultiServiceRouter, repository: &str, case: &str) -> (Vec<String>, Vec<usize>) {
    let mut names: Vec<String> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = list_page(router, repository, token.as_deref(), case).await;
        sizes.push(page.branches.len());
        names.extend(page.branches);
        token = page.next_token;
        if token.is_none() {
            break;
        }
        assert!(
            sizes.len() <= 64,
            "case {case:?}: paging must terminate; {} pages so far",
            sizes.len()
        );
    }
    (names, sizes)
}

/// Create `names` as the branches of `repository`, in the order given, all at one
/// commit: the first through the commit `PutFile` makes (a repository with no commit
/// has no branch to create one from), the rest through `CreateBranch` at that commit.
async fn create_branches(router: &MultiServiceRouter, repository: &str, names: &[String]) {
    let mut names = names.iter();
    let Some(first) = names.next() else {
        return;
    };
    let put = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": first,
            "filePath": "etl.py",
            "fileContent": base64(b"print(\"x\")\n"),
            "commitMessage": "seed",
        }),
    )
    .await;
    let commit_id = string_at(&put, "/commitId");
    for name in names {
        ok(
            router,
            "CreateBranch",
            &json!({
                "repositoryName": repository,
                "branchName": name,
                "commitId": commit_id,
            }),
        )
        .await;
    }
}

/// Write `names` into `repository`'s branch map, in the order given, all pointing at
/// one object id. Only for a set LARGER than `CreateBranch` will create (the store's
/// `MAX_BRANCHES_PER_REPOSITORY`); everything reachable through the API is created
/// through it by [`create_branches`].
fn seed_branches(engine: &Arc<engine::InMemoryEngine>, repository: &str, names: &[String]) {
    let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
    let mut store = state.lock();
    let record = store
        .repositories
        .get_mut(repository)
        .unwrap_or_else(|| panic!("the repository {repository} was created above"));
    for (index, name) in names.iter().enumerate() {
        record.branches.insert(
            name.clone(),
            Branch {
                name: name.clone(),
                commit_id: format!("{index:040x}"),
            },
        );
    }
}

/// The branch names stored for `repository`, read straight from the store.
fn stored_branches(engine: &Arc<engine::InMemoryEngine>, repository: &str) -> Vec<String> {
    let state = engine.state::<ComAmazonawsCodecommitCodecommit20150413State>();
    let store = state.lock();
    store
        .repositories
        .get(repository)
        .map(|record| record.branches.keys().cloned().collect())
        .unwrap_or_default()
}

/// A repository that exists but has had nothing written to it lists NO branches, and
/// the first commit on a branch makes that branch — and only that branch — appear.
///
/// The rows differ along the branch name's form: the recorded `main`, a name with a
/// separator, a dotted release name, a one-character name and an upper-case name.
#[tokio::test]
async fn an_empty_repository_lists_an_empty_set_and_a_commit_adds_its_branch() {
    let (router, engine) = emulator_with_engine();

    let cases: Vec<(&str, &str)> = vec![
        // The recorded reference-architecture case: a fresh ETL source repository,
        // then one commit on `main`.
        ("etl-source", "main"),
        ("with-separator", "feature/etl-rewrite"),
        ("dotted", "release-1.0"),
        ("one-char", "b"),
        ("upper", "MAIN"),
    ];

    for (repository, branch) in cases {
        let case = format!("{repository}/{branch}");
        ok(
            &router,
            "CreateRepository",
            &json!({
                "repositoryName": repository,
                "repositoryDescription": "ETL source code repository for Glue jobs",
            }),
        )
        .await;

        let page = list_page(&router, repository, None, &case).await;
        assert!(
            page.branches.is_empty(),
            "case {case:?}: an unseeded repository must list no branches, got {:?}",
            page.branches
        );
        assert_eq!(
            page.next_token, None,
            "case {case:?}: one page holds an empty set, so there is no continuation token"
        );
        // Listing is a read: it must not have created anything.
        assert!(
            stored_branches(&engine, repository).is_empty(),
            "case {case:?}: listing must not write a branch, store holds {:?}",
            stored_branches(&engine, repository)
        );
        let code = error(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": branch }),
            400,
        )
        .await;
        assert_eq!(
            code, "BranchDoesNotExistException",
            "case {case:?}: no branch exists before the first commit"
        );

        ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": branch,
                "filePath": "etl.py",
                "fileContent": base64(b"print(\"x\")\n"),
                "commitMessage": "seed",
            }),
        )
        .await;

        let page = list_page(&router, repository, None, &case).await;
        assert_eq!(
            page.branches,
            vec![branch.to_string()],
            "case {case:?}: after one commit the committed branch is the only one listed"
        );
        assert_eq!(
            page.next_token, None,
            "case {case:?}: a single branch fits one page"
        );
        // A second read sees the same thing: the first did not consume it.
        let again = list_page(&router, repository, None, &case).await;
        assert_eq!(
            again.branches, page.branches,
            "case {case:?}: ListBranches must be repeatable"
        );
    }

    // Each repository's branches are its own: no listing leaked a name into another.
    for (repository, branch) in [("etl-source", "main"), ("one-char", "b")] {
        assert_eq!(
            stored_branches(&engine, repository),
            vec![branch.to_string()],
            "repository {repository:?} must hold only its own branch"
        );
    }
}

/// Whatever branch set a repository holds, `ListBranches` reports exactly that set in
/// name order.
///
/// Every branch here is created through the API, so the counts stay within the store's
/// own [`MAX_BRANCHES_PER_REPOSITORY`] — none, one, a handful created out of name
/// order, and the ceiling itself — which is what the "reports the set it holds" half of
/// the rule generalises over. One page holds all of them, so no token is handed out;
/// the multi-page half is the test below.
#[tokio::test]
async fn every_created_branch_is_listed_in_name_order() {
    let generated = |count: usize| -> Vec<String> {
        // Numbered so the creation order (reverse) differs from the name order.
        (0..count)
            .rev()
            .map(|index| format!("branch-{index:04}"))
            .collect()
    };

    let cases: Vec<(&str, Vec<String>)> = vec![
        ("none", Vec::new()),
        ("one", vec!["main".to_string()]),
        (
            // Created in an order that is neither the name order nor its reverse, with
            // names that order differently under byte order than under a case-folded
            // one ("MAIN" before "develop" before "main").
            "a handful out of order",
            ["main", "develop", "MAIN", "feature/x", "release-1.0"]
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        ),
        (
            "the store's whole branch ceiling",
            generated(MAX_BRANCHES_PER_REPOSITORY),
        ),
    ];

    for (case, names) in cases {
        let (router, engine) = emulator_with_engine();
        let repository = "listed";
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        create_branches(&router, repository, &names).await;

        // The names in the order the rule requires, computed here independently of
        // the service: sorted, with no duplicate key.
        let expected: Vec<String> = {
            let mut sorted = names.clone();
            sorted.sort();
            sorted.dedup();
            sorted
        };

        let (listed, sizes) = list_all(&router, repository, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: every stored branch, in name order, got {listed:?}"
        );
        assert_eq!(
            sizes.len(),
            1,
            "case {case:?}: {} names fit one page of {PAGE_SIZE}, got sizes {sizes:?}",
            expected.len()
        );

        // Listing is a read: the store still holds the same set.
        assert_eq!(
            stored_branches(&engine, repository),
            expected,
            "case {case:?}: listing must not change the stored branch set"
        );
    }
}

/// A branch set larger than one page is handed out page by page: each page but the last
/// is full, the token of a page resumes exactly where it ended, and the concatenation
/// of the pages is the whole set in name order.
///
/// `CreateBranch` refuses past the store's [`MAX_BRANCHES_PER_REPOSITORY`], which is far
/// below [`PAGE_SIZE`] (a page small enough for the API to fill would truncate the
/// branch list of an ordinary repository, which the real service never does), so the
/// only way to hold more than one page is to write the branch set into the store. The
/// call under test is still made over HTTP.
#[tokio::test]
async fn a_branch_set_larger_than_one_page_is_handed_out_page_by_page() {
    let generated = |count: usize| -> Vec<String> {
        (0..count)
            .rev()
            .map(|index| format!("branch-{index:05}"))
            .collect()
    };

    // The counts differ along where the set falls relative to the page boundary.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("exactly one page", generated(PAGE_SIZE)),
        ("one past a page", generated(PAGE_SIZE + 1)),
        ("two pages and a bit", generated(2 * PAGE_SIZE + 3)),
    ];

    for (case, names) in cases {
        let (router, engine) = emulator_with_engine();
        let repository = "paged";
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        seed_branches(&engine, repository, &names);

        let expected: Vec<String> = {
            let mut sorted = names.clone();
            sorted.sort();
            sorted.dedup();
            sorted
        };

        let (listed, sizes) = list_all(&router, repository, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: every stored branch, in name order, got {} names",
            listed.len()
        );
        let expected_pages = expected.len().div_ceil(PAGE_SIZE).max(1);
        assert_eq!(
            sizes.len(),
            expected_pages,
            "case {case:?}: {} names must come back in {expected_pages} page(s), got sizes {sizes:?}",
            expected.len()
        );
        for (index, size) in sizes.iter().enumerate() {
            let last = index + 1 == sizes.len();
            if last {
                assert!(
                    *size <= PAGE_SIZE,
                    "case {case:?}: the last page holds at most {PAGE_SIZE}, got {size}"
                );
            } else {
                assert_eq!(
                    *size, PAGE_SIZE,
                    "case {case:?}: page {index} must be full before a token is handed out"
                );
            }
        }

        // The token of the first page resumes exactly where that page ended.
        if expected.len() > PAGE_SIZE {
            let first = list_page(&router, repository, None, case).await;
            let token = first
                .next_token
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: more than one page means a token"));
            let second = list_page(&router, repository, Some(&token), case).await;
            assert_eq!(
                second.branches,
                expected[PAGE_SIZE..expected.len().min(2 * PAGE_SIZE)].to_vec(),
                "case {case:?}: the second page continues after the first"
            );
        }

        // Paging is a read: the store still holds the same set.
        assert_eq!(
            stored_branches(&engine, repository),
            expected,
            "case {case:?}: listing must not change the stored branch set"
        );
    }
}

/// A request `ListBranches` cannot serve is refused with the error the model names for
/// it — never with an empty success, which a caller would read as "this repository has
/// no branches". The rows differ along which part of the request is at fault: the
/// repository's existence, its name's form, its absence, and the continuation token.
#[tokio::test]
async fn an_unservable_list_is_refused_with_the_modeled_error() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "present" }),
    )
    .await;
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "present",
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(b"one"),
        }),
    )
    .await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that was never created",
            json!({ "repositoryName": "absent" }),
            "RepositoryDoesNotExistException",
        ),
        (
            // `RepositoryName`'s modeled pattern is `^[\w\.-]+$`: a space is not in it.
            "a name the model's pattern refuses",
            json!({ "repositoryName": "not a name" }),
            "InvalidRepositoryNameException",
        ),
        (
            "no repository name at all",
            json!({}),
            "RepositoryNameRequiredException",
        ),
        (
            "a token this service never issued",
            json!({ "repositoryName": "present", "nextToken": "not-a-token" }),
            "InvalidContinuationTokenException",
        ),
        (
            "a token past the end of the list",
            json!({ "repositoryName": "present", "nextToken": "97" }),
            "InvalidContinuationTokenException",
        ),
    ];

    for (case, body, expected) in cases {
        let code = error(&router, "ListBranches", &body, 400).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code}"
        );
    }
}
