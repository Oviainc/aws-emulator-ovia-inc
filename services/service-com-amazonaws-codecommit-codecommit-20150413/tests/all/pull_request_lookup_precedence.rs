// SPDX-License-Identifier: CC0-1.0

//! Which of two lookups decides the refusal, over the wire: an operation that
//! takes BOTH a `pullRequestId` and a `repositoryName` resolves the pull request
//! FIRST, so a request whose id names no pull request is told that — with
//! `PullRequestDoesNotExistException` and its fixed sentence pair — even when the
//! repository it named does not exist either.
//!
//! The rule under test is about precedence rather than about one request: a pull
//! request id is unique per account, so whether it names a pull request at all is a
//! question about the whole store and is answered before the repository the request
//! names is resolved. The table below therefore varies what the rule generalises
//! over — the operation that takes both members, and the shape of the id that names
//! no pull request (the smallest id, an id past every id minted, a long id) — and
//! asserts the same code and the same sentence on every row.
//!
//! The complementary half of the precedence — an id that DOES name a pull request,
//! with a repository name that names no repository, answered with
//! `RepositoryDoesNotExistException` — is pinned by
//! `crate::pull_request_comments` (which mints a pull request to have an id that
//! exists) and by `crate::client_error_messages`, which pins that code's template.

use serde_json::{json, Value};

use crate::harness::{error_with_message, PULL_REQUEST_DOES_NOT_EXIST};
use crate::repository_lifecycle::emulator;

/// The HTTP status `PullRequestDoesNotExistException` models (`errorKind: client`,
/// `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// A syntactically valid git object id (40 hex characters), so an operation taking
/// a commit id gets past its own `InvalidCommitIdException` check and reaches the
/// lookups under test.
const AFTER_COMMIT_ID: &str = "0123456789abcdef0123456789abcdef01234567";
const BEFORE_COMMIT_ID: &str = "76543210fedcba9876543210fedcba9876543210";

/// A repository name that is a valid `RepositoryName` and names no repository, so
/// the repository lookup would fail if it ran first.
const ABSENT_REPOSITORY: &str = "no-such-repository";

/// Pull request ids that are ids (`PullRequestId` is a decimal string: see
/// `crate::state::normalize_pull_request_id`) and name no pull request on an
/// emulator that has minted none.
fn absent_pull_request_ids() -> Vec<&'static str> {
    vec!["1", "7", "999999", "18446744073709551615"]
}

/// Every operation that takes both a `pullRequestId` and a `repositoryName`, with
/// the rest of its required members filled in so each reaches the two lookups.
fn requests_naming(pull_request_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "PostCommentForPullRequest",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": ABSENT_REPOSITORY,
                "beforeCommitId": BEFORE_COMMIT_ID,
                "afterCommitId": AFTER_COMMIT_ID,
                "content": "a comment",
            }),
        ),
        (
            "GetCommentsForPullRequest",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": ABSENT_REPOSITORY,
                // Naming the repository makes both sides of the comparison
                // required on this operation, so both are filled in.
                "beforeCommitId": BEFORE_COMMIT_ID,
                "afterCommitId": AFTER_COMMIT_ID,
            }),
        ),
        (
            "MergePullRequestByFastForward",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": ABSENT_REPOSITORY,
            }),
        ),
        (
            "MergePullRequestBySquash",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": ABSENT_REPOSITORY,
            }),
        ),
        (
            "MergePullRequestByThreeWay",
            json!({
                "pullRequestId": pull_request_id,
                "repositoryName": ABSENT_REPOSITORY,
            }),
        ),
    ]
}

#[tokio::test]
async fn the_pull_request_lookup_comes_before_the_repository_lookup() {
    let router = emulator();

    for pull_request_id in absent_pull_request_ids() {
        for (operation, body) in requests_naming(pull_request_id) {
            let (code, message) =
                error_with_message(&router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "PullRequestDoesNotExistException",
                "case ({operation}, id {pull_request_id}): the pull request is resolved first, \
                 so expected code PullRequestDoesNotExistException, got {code:?}"
            );
            assert_eq!(
                message, PULL_REQUEST_DOES_NOT_EXIST,
                "case ({operation}, id {pull_request_id}): expected message \
                 {PULL_REQUEST_DOES_NOT_EXIST:?}, got {message:?}"
            );
        }
    }
}
