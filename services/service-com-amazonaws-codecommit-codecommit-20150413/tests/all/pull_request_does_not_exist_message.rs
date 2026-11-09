// SPDX-License-Identifier: CC0-1.0

//! How CodeCommit words the refusal of a pull request id that IS an id and simply
//! names no pull request, over the wire: awsJson1_1 requests through the router that
//! serves the real emulator (the wire harness comes from the `repository_lifecycle`
//! suite, the pull request fixture and the operation list from the sibling
//! `pull_request_identifier_refusals` suite, rather than a second copy of either).
//!
//! The rule under test: an operation that takes a `pullRequestId` holding an id no
//! pull request was minted as is refused with `PullRequestDoesNotExistException`
//! (`errorKind: client`, `httpError: 400` defaulted, a single `message` member —
//! `smithy_query(query="shape",shape="PullRequestDoesNotExistException")`), and that
//! error carries the FIXED text CodeCommit words the class with. The text
//! interpolates nothing: it names neither the id the request carried nor the
//! repository the call was scoped to, so a caller learns which ids are addressable
//! only from the operation the sentence points at.
//!
//! The tables span what that rule generalises over rather than one recorded call:
//! the operation reading the member — every implemented one, reporting and mutating
//! alike — the magnitude and spelling of the id nothing was minted as (the id just
//! past the minted one, a single digit, a nine-digit id, one past the 32-bit range,
//! the largest id that fits in 64 bits, and a leading-zero spelling), and the
//! repository the call is scoped to (two differently shaped names), with the message
//! compared byte for byte on every row.
//!
//! Neighbouring suites own the neighbouring rules: `pull_request_identifier_refusals`
//! pins what a carried value that is not an id at all is refused with, and
//! `pull_request_lookup_precedence` pins which refusal a request that fails in more
//! than one way gets.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_with_message, PULL_REQUEST_DOES_NOT_EXIST};
use crate::pull_request_identifier_refusals::{fixture_on, Fixture, PULL_REQUEST_ID_OPERATIONS};
use crate::repository_lifecycle::ok;

/// The status the modelled error carries.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The repository the first table's pull request lives in.
const REPOSITORY: &str = "pull-request-does-not-exist";

/// Two repositories whose names differ in shape and length — a CodeCommit
/// repository name is 1..100 characters of `[\w\.-]+`
/// (`smithy_query(query="shape",shape="RepositoryName")`) — so the second table can
/// show the refusal's wording owes nothing to the repository the call named. Neither
/// name is a run of letters the fixed sentence itself contains, so the row can also
/// ask whether the name was spliced into the message.
const REPOSITORIES: &[&str] = &["q7", "pull.request_does-not.exist-in-this-longer-repository"];

/// The operations whose request also carries a `repositoryName`, together with the
/// pull-request-scoped reads the message was reported wrong on: the rows of the
/// repository table.
const REPOSITORY_SCOPED_OPERATIONS: &[&str] = &[
    "GetPullRequest",
    "GetPullRequestOverrideState",
    "EvaluatePullRequestApprovalRules",
    "UpdatePullRequestApprovalState",
    "OverridePullRequestApprovalRules",
    "GetCommentsForPullRequest",
    "PostCommentForPullRequest",
    "MergePullRequestByFastForward",
    "MergePullRequestBySquash",
    "MergePullRequestByThreeWay",
];

/// Ids nothing was minted as, for a fixture whose own pull request is `minted`.
///
/// Rows differ along magnitude and spelling: the id immediately past the minted one,
/// a single digit, a nine-digit id, one past the 32-bit range, the largest id that
/// fits in 64 bits, and a leading-zero spelling of an id that does not exist either.
fn unminted_ids(minted: &str) -> Vec<String> {
    let next = minted
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("the minted pull request id {minted:?} is not a decimal id"))
        + 1;
    let ids = vec![
        next.to_string(),
        "9".to_string(),
        "999999999".to_string(),
        u64::from(u32::MAX).to_string(),
        (u64::from(u32::MAX) + 1).to_string(),
        u64::MAX.to_string(),
        "0000000009".to_string(),
    ];
    for id in &ids {
        assert_ne!(
            id, minted,
            "case {id:?}: a row must name an id that was not minted, and this one is the \
             fixture's own"
        );
    }
    ids
}

/// The stored pull request, as `GetPullRequest` projects it — what a refused call
/// must leave exactly as it found it.
async fn stored(router: &MultiServiceRouter, pull_request_id: &str) -> Value {
    ok(
        router,
        "GetPullRequest",
        &json!({ "pullRequestId": pull_request_id }),
    )
    .await
}

/// The refusal of one call that names `pull_request_id`, asserted down to the
/// message bytes, and answered back so a caller can compare rows with each other.
async fn refusal(fixture: &Fixture, operation: &str, pull_request_id: &str) -> String {
    let case = format!("{operation} / {pull_request_id:?} / {}", fixture.pr.repository);
    let mut body = fixture.request(operation);
    body["pullRequestId"] = json!(pull_request_id);

    let (code, message) =
        error_with_message(&fixture.router, operation, &body, CLIENT_ERROR_STATUS).await;
    assert_eq!(
        code, "PullRequestDoesNotExistException",
        "case {case:?}: expected PullRequestDoesNotExistException, got {code:?} saying {message:?}"
    );
    assert_eq!(
        message, PULL_REQUEST_DOES_NOT_EXIST,
        "case {case:?}: expected {PULL_REQUEST_DOES_NOT_EXIST:?}, got {message:?}"
    );
    // The same assertion stated as the property the text has to hold: it interpolates
    // nothing, so neither the id asked for nor the repository asked about appears in
    // it.
    assert!(
        !message.contains(pull_request_id),
        "case {case:?}: the message names the requested id, got {message:?}"
    );
    assert!(
        !message.contains(&fixture.pr.repository),
        "case {case:?}: the message names the repository, got {message:?}"
    );
    message
}

#[tokio::test]
async fn every_operation_words_an_unminted_pull_request_id_with_the_same_fixed_text() {
    let fixture = fixture_on(REPOSITORY).await;
    let before = stored(&fixture.router, &fixture.pr.pull_request_id).await;

    for operation in PULL_REQUEST_ID_OPERATIONS {
        for pull_request_id in unminted_ids(&fixture.pr.pull_request_id) {
            refusal(&fixture, operation, &pull_request_id).await;
        }
    }

    // None of those calls — the mutating ones included — changed what the next
    // caller sees.
    let after = stored(&fixture.router, &fixture.pr.pull_request_id).await;
    assert_eq!(
        after, before,
        "a refused id leaves the pull request unchanged, got {after}"
    );
}

#[tokio::test]
async fn the_wording_owes_nothing_to_the_repository_the_call_was_scoped_to() {
    let mut messages: BTreeSet<String> = BTreeSet::new();

    for repository in REPOSITORIES {
        let fixture = fixture_on(repository).await;
        let before = stored(&fixture.router, &fixture.pr.pull_request_id).await;

        for operation in REPOSITORY_SCOPED_OPERATIONS {
            for pull_request_id in unminted_ids(&fixture.pr.pull_request_id) {
                messages.insert(refusal(&fixture, operation, &pull_request_id).await);
            }
        }

        let after = stored(&fixture.router, &fixture.pr.pull_request_id).await;
        assert_eq!(
            after, before,
            "case {repository:?}: a refused id leaves the pull request unchanged, got {after}"
        );
    }

    let expected: BTreeSet<String> = [PULL_REQUEST_DOES_NOT_EXIST.to_string()].into();
    assert_eq!(
        messages, expected,
        "every repository and operation words the refusal identically, got {messages:?}"
    );
}
