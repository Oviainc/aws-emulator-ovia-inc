// SPDX-License-Identifier: CC0-1.0

//! `CreateBranch` over the wire, driven as awsJson1_1 requests through the router that
//! serves the real emulator.
//!
//! The rule under test: `CreateBranch` adds the reference `refs/heads/<branchName>` to the
//! named repository pointing at the commit it is given, answers 200 with an empty body,
//! leaves every other reference — including the repository's default branch — where it
//! was, and otherwise refuses with the modelled error (the repository or the commit is not
//! one the store holds, the name is already taken, or the name or id is malformed). A
//! branch tip is never pruned, so the number of branches one repository holds is an axis a
//! caller drives: the store bounds it, which is also what keeps the commit-retention bound
//! reachable.
//!
//! The tables below span the class that rule covers — branch names differing in form,
//! depth, alphabet and length (1 character up to `BranchName`'s modelled maximum of 256),
//! and commit targets differing in which commit of a repository's history they name,
//! including two branches on one commit — rather than one recorded request. The round trip
//! is read back through `GetBranch` and `GetFile`, so what is asserted is stored state.

use serde_json::{json, Value};

use crate::repository_lifecycle::{base64, call, emulator, error, from_base64, ok, string_at};

/// One row of the creation table: the branch to create and which commit of the
/// seeded history it must point at.
#[derive(Debug, Clone)]
struct Creation {
    branch: String,
    /// Index into the seeded commit list: 0 is the first commit, 1 the second.
    commit: usize,
}

/// The table. Rows differ along what the rule generalises over: name form (plain,
/// slash-separated, dotted, digits, unicode), name length (1 character and
/// `BranchName`'s modelled maximum of 256, from
/// `smithy_query(query="operation",operation="CreateBranch")`), and which commit in
/// the history the new reference names — including two names on the same commit.
fn creations() -> Vec<Creation> {
    vec![
        Creation {
            branch: "unlisted".to_string(),
            commit: 1,
        },
        Creation {
            branch: "feature/nested/work".to_string(),
            commit: 1,
        },
        Creation {
            branch: "release-1.0.x".to_string(),
            commit: 0,
        },
        Creation {
            branch: "b".to_string(),
            commit: 0,
        },
        Creation {
            branch: "ünïcode-brünch".to_string(),
            commit: 1,
        },
        Creation {
            // Two references on one commit: the second must not disturb the first.
            branch: "also-at-the-first-commit".to_string(),
            commit: 0,
        },
        Creation {
            // The modelled maximum length of `BranchName`.
            branch: "x".repeat(256),
            commit: 1,
        },
    ]
}

/// Seed a repository with two commits on `main`, returning their ids oldest first
/// together with the content each commit holds at `f.txt`.
async fn seed_history(
    router: &smithy_server_runtime::MultiServiceRouter,
    repository: &str,
) -> Vec<(String, Vec<u8>)> {
    ok(router, "CreateRepository", &json!({ "repositoryName": repository })).await;
    let first = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(b"one"),
            "commitMessage": "seed",
        }),
    )
    .await;
    let first_id = string_at(&first, "/commitId");
    let second = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": "f.txt",
            "fileContent": base64(b"two"),
            "commitMessage": "second",
            "parentCommitId": first_id,
        }),
    )
    .await;
    vec![
        (first_id, b"one".to_vec()),
        (string_at(&second, "/commitId"), b"two".to_vec()),
    ]
}

#[tokio::test]
async fn a_created_branch_points_at_the_commit_it_was_given() {
    let router = emulator();
    let history = seed_history(&router, "branching").await;
    let tip = history[1].0.clone();

    for case in creations() {
        let label = case.branch.clone();
        let (commit_id, content) = history[case.commit].clone();

        let (status, payload) = call(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": "branching",
                "branchName": case.branch,
                "commitId": commit_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {status} with {payload}"
        );
        // `CreateBranchInput`'s output shape is `Unit`: an empty body, which the
        // harness reports as JSON null, or an empty object.
        assert!(
            payload.is_null() || payload == json!({}),
            "case {label:?}: CreateBranch answers an empty body, got {payload}"
        );

        // The reference is in state: GetBranch reads it back.
        let read = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": "branching", "branchName": case.branch }),
        )
        .await;
        assert_eq!(
            string_at(&read, "/branch/branchName"),
            case.branch,
            "case {label:?}: GetBranch must report the created branch's name, got {read}"
        );
        assert_eq!(
            string_at(&read, "/branch/commitId"),
            commit_id,
            "case {label:?}: GetBranch must report the commit the branch was created at, got {read}"
        );

        // And the reference resolves as a commit specifier: a read at the branch
        // sees that commit's tree, not the tip's.
        let file = ok(
            &router,
            "GetFile",
            &json!({
                "repositoryName": "branching",
                "filePath": "f.txt",
                "commitSpecifier": case.branch,
            }),
        )
        .await;
        assert_eq!(
            string_at(&file, "/commitId"),
            commit_id,
            "case {label:?}: a read at the new branch must resolve to its commit, got {file}"
        );
        assert_eq!(
            from_base64(&string_at(&file, "/fileContent")),
            content,
            "case {label:?}: a read at the new branch must return that commit's content, got {}",
            string_at(&file, "/commitId")
        );

        // Creating a branch moves no other reference, and does not set the
        // repository's default branch ("Calling the create branch operation does
        // not set a repository's default branch.").
        let main = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": "branching", "branchName": "main" }),
        )
        .await;
        assert_eq!(
            string_at(&main, "/branch/commitId"),
            tip,
            "case {label:?}: main's tip must be untouched, got {main}"
        );
        let repository = ok(
            &router,
            "GetRepository",
            &json!({ "repositoryName": "branching" }),
        )
        .await;
        assert_eq!(
            string_at(&repository, "/repositoryMetadata/defaultBranch"),
            "main",
            "case {label:?}: the default branch must be unchanged, got {repository}"
        );
    }

    // Every created branch is still readable at the end: one creation does not
    // replace another.
    for case in creations() {
        let read = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": "branching", "branchName": case.branch }),
        )
        .await;
        assert_eq!(
            string_at(&read, "/branch/commitId"),
            history[case.commit].0,
            "case {:?}: every created branch keeps its own commit, got {read}",
            case.branch
        );
    }
}

/// Branch names a git reference cannot be formed from, each with the git rule that
/// refuses it (`git check-ref-format`: no component starting with `.`, no `..`, no
/// ASCII control character, space, `~ ^ : ? * [` or backslash, no component ending
/// in `.lock`, no trailing `/` or `.`, no `@{`, and `@` is not a name).
fn invalid_branch_names() -> Vec<&'static str> {
    vec![
        "bad name",        // space
        "feature..old",    // ".."
        ".hidden",         // component starting with "."
        "nested/.hidden",  // ditto, deeper
        "trailing/",       // trailing "/"
        "/leading",        // empty first component
        "double//slash",   // empty inner component
        "ends.",           // trailing "."
        "feature@{0}",     // "@{"
        "@",               // the bare "@"
        "tilde~1",         // "~"
        "caret^1",         // "^"
        "colon:name",      // ":"
        "question?",       // "?"
        "star*",           // "*"
        "bracket[0]",      // "["
        "back\\slash",     // backslash
        "work.lock",       // component ending in ".lock"
        "nested/work.lock", // ditto, deeper
        "control\u{1}char", // ASCII control character
    ]
}

#[tokio::test]
async fn create_branch_is_refused_with_the_modeled_error() {
    let router = emulator();
    let history = seed_history(&router, "guarded-branches").await;
    let tip = history[1].0.clone();
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "guarded-branches",
            "branchName": "taken",
            "commitId": tip,
        }),
    )
    .await;

    let mut cases: Vec<(String, Value, &str)> = vec![
        (
            "an unknown repository".to_string(),
            json!({
                "repositoryName": "no-such-repository",
                "branchName": "whatever",
                "commitId": tip,
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a well-formed commit id the repository does not hold".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "whatever",
                "commitId": "0".repeat(40),
            }),
            "CommitDoesNotExistException",
        ),
        (
            "a branch name already in use".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "taken",
                "commitId": tip,
            }),
            "BranchNameExistsException",
        ),
        (
            "the default branch's name".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "main",
                "commitId": history[0].0,
            }),
            "BranchNameExistsException",
        ),
        (
            "a commit id that is not an object name".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "whatever",
                "commitId": "not-a-commit-id",
            }),
            "InvalidCommitIdException",
        ),
        (
            "a commit id one character short".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "whatever",
                "commitId": "a".repeat(39),
            }),
            "InvalidCommitIdException",
        ),
        (
            "a commit id one character long".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "whatever",
                "commitId": "a".repeat(41),
            }),
            "InvalidCommitIdException",
        ),
        (
            "a branch name one character past the modelled maximum".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "x".repeat(257),
                "commitId": tip,
            }),
            "InvalidBranchNameException",
        ),
        (
            "an empty branch name".to_string(),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": "",
                "commitId": tip,
            }),
            "InvalidBranchNameException",
        ),
        (
            "no branch name at all".to_string(),
            json!({ "repositoryName": "guarded-branches", "commitId": tip }),
            "BranchNameRequiredException",
        ),
        (
            "no commit id at all".to_string(),
            json!({ "repositoryName": "guarded-branches", "branchName": "whatever" }),
            "CommitIdRequiredException",
        ),
        (
            "no repository name at all".to_string(),
            json!({ "branchName": "whatever", "commitId": tip }),
            "RepositoryNameRequiredException",
        ),
    ];
    for name in invalid_branch_names() {
        cases.push((
            format!("the invalid reference name {name:?}"),
            json!({
                "repositoryName": "guarded-branches",
                "branchName": name,
                "commitId": tip,
            }),
            "InvalidBranchNameException",
        ));
    }

    for (label, body, expected) in cases {
        let code = error(&router, "CreateBranch", &body, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code} for {body}"
        );
    }

    // A refused create persists nothing, and nothing it refused moved a reference.
    let code = error(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "guarded-branches", "branchName": "whatever" }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "a refused CreateBranch must create no branch, got {code}"
    );
    let main = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "guarded-branches", "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&main, "/branch/commitId"),
        tip,
        "a refused CreateBranch must not move an existing branch, got {main}"
    );
    let taken = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "guarded-branches", "branchName": "taken" }),
    )
    .await;
    assert_eq!(
        string_at(&taken, "/branch/commitId"),
        tip,
        "a refused re-create must leave the existing branch at its commit, got {taken}"
    );
}

#[tokio::test]
async fn the_branch_count_one_repository_holds_is_bounded() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::{
        ComAmazonawsCodecommitCodecommit20150413State, MAX_BRANCHES_PER_REPOSITORY,
        MAX_COMMITS_RETAINED,
    };

    // A branch tip is never pruned, so the number of references one repository
    // holds is an axis a caller drives and the store bounds it: CreateBranch
    // refuses once the repository is at the maximum, and the commit retention
    // bound still holds afterwards.
    let (router, engine) = crate::repository_lifecycle::emulator_with_engine();
    let history = seed_history(&router, "bounded-branches").await;
    let tip = history[1].0.clone();

    // `main`, written by the seed, is already one of the branches.
    let existing = 1;
    for index in existing..MAX_BRANCHES_PER_REPOSITORY {
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": "bounded-branches",
                "branchName": format!("branch-{index:05}"),
                "commitId": tip,
            }),
        )
        .await;
    }
    let code = error(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "bounded-branches",
            "branchName": "one-too-many",
            "commitId": tip,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "ValidationException",
        "the store must refuse a branch beyond its bound of {MAX_BRANCHES_PER_REPOSITORY}, got {code}"
    );
    let stored = engine
        .state::<ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .get("bounded-branches")
        .map(|repository| repository.branches.len())
        .unwrap_or_default();
    assert_eq!(
        stored, MAX_BRANCHES_PER_REPOSITORY,
        "the refused create must not have been stored, got {stored} branches"
    );
    let code = error(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "bounded-branches", "branchName": "one-too-many" }),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchDoesNotExistException",
        "a refused CreateBranch must create no branch, got {code}"
    );

    // The branch bound is what keeps the commit bound reachable: pruning never
    // drops a tip, so with every branch pinned the retained commits must still
    // stay within MAX_COMMITS_RETAINED as writes continue.
    let mut tip = tip;
    for index in 0..(MAX_COMMITS_RETAINED + 8) {
        let written = ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": "bounded-branches",
                "branchName": "main",
                "filePath": "f.txt",
                "fileContent": base64(format!("revision {index}").as_bytes()),
                "commitMessage": "write",
                "parentCommitId": tip,
            }),
        )
        .await;
        tip = string_at(&written, "/commitId");
    }
    let commits = engine
        .state::<ComAmazonawsCodecommitCodecommit20150413State>()
        .lock()
        .repositories
        .get("bounded-branches")
        .map(|repository| repository.commits.len())
        .unwrap_or_default();
    assert!(
        commits <= MAX_COMMITS_RETAINED,
        "with every branch pinning a tip the store must still retain at most {MAX_COMMITS_RETAINED} commits, got {commits}"
    );
}

#[tokio::test]
async fn a_new_branch_is_a_writable_reference_of_its_own() {
    let router = emulator();
    let history = seed_history(&router, "diverging").await;
    let (first_commit, _) = history[0].clone();
    let tip = history[1].0.clone();

    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": "diverging",
            "branchName": "side",
            "commitId": first_commit,
        }),
    )
    .await;

    // A commit on the new branch advances only that branch.
    let written = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": "diverging",
            "branchName": "side",
            "filePath": "f.txt",
            "fileContent": base64(b"side"),
            "commitMessage": "on the side branch",
            "parentCommitId": first_commit,
        }),
    )
    .await;
    let side_tip = string_at(&written, "/commitId");
    assert_ne!(
        side_tip, tip,
        "a commit on the new branch is its own commit, got {side_tip}"
    );

    let side = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "diverging", "branchName": "side" }),
    )
    .await;
    assert_eq!(
        string_at(&side, "/branch/commitId"),
        side_tip,
        "the new branch must advance to the commit written on it, got {side}"
    );
    let main = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": "diverging", "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&main, "/branch/commitId"),
        tip,
        "writing on another branch must not move main, got {main}"
    );
    assert_eq!(
        from_base64(&string_at(
            &ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": "diverging",
                    "filePath": "f.txt",
                    "commitSpecifier": "main",
                }),
            )
            .await,
            "/fileContent"
        )),
        b"two".to_vec(),
        "main's content must be untouched by a write on another branch"
    );
}
