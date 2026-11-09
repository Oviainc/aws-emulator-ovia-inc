// SPDX-License-Identifier: CC0-1.0

//! What CodeCommit answers a `pullRequestId` or a `revisionId` that names nothing,
//! over the wire: awsJson1_1 requests through the router that serves the real
//! emulator (the wire harness comes from the sibling `repository_lifecycle` suite and
//! the pull request fixture from `pull_request_comments`, rather than a second copy
//! of either).
//!
//! The rule under test: a value a request CARRIES for one of these identifier members
//! and that addresses no pull request — or no revision — is refused with the
//! `Invalid<member>Exception` class, whatever the value holds, and never with the
//! `<member>RequiredException` class, which is for a request that carried no such
//! member at all; the message is the one CodeCommit words that class with — for a
//! pull request id, the offending value named in the sentence and then the operation
//! that lists the valid ones; for a revision id, the sentence stating the value was
//! empty or null. Every one of these errors is `errorKind: client`, `httpError: 400`
//! (`smithy_query(query="shape",shape="InvalidPullRequestIdException,InvalidRevisionIdException")`).
//!
//! The tables span what the rule generalises over: the operation reading the member —
//! every implemented one that takes a `pullRequestId`, reporting and mutating alike —
//! and the way the value fails to be an id: empty, nothing but space, punctuation,
//! letters, a signed or a fractional spelling, surrounding space, exponent notation,
//! a hex spelling, non-ASCII digits and a digit pair split by a space. Neither
//! `PullRequestId` nor `RevisionId` carries a length or pattern constraint
//! (`smithy_query(query="shape",shape="PullRequestId,RevisionId")`), so each of these
//! values reaches the handler rather than being refused at the request boundary.
//!
//! The last table is the other half of the same question: an id that IS one and
//! simply names no pull request is a different class — `PullRequestDoesNotExist` —
//! whose message CodeCommit words as fixed text, naming no value at all.
//!
//! Sibling suites own the neighbouring rules: `aws_error_message_text.rs` pins the
//! other client codes' message text, and each operation's own suite pins what it
//! answers a request that carries no identifier member at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    empty_pull_request_id, error_with_message, invalid_pull_request_id, EMPTY_REVISION_ID,
    INVALID_REVISION_ID, PULL_REQUEST_DOES_NOT_EXIST,
};
use crate::pull_request_comments::{pull_request_on, PullRequest};
use crate::repository_lifecycle::{emulator, ok, string_at};

/// The status every error asserted here models.
const CLIENT_ERROR_STATUS: u16 = 400;

const REPOSITORY: &str = "identifier-refusals";

/// The approval rule the fixture's pull request holds, so the delete path names a
/// rule that exists and is refused for its id rather than for the rule's name.
const RULE: &str = "needs-review";

/// A valid approval rule document, in the form the CodeCommit User Guide documents
/// for approval rule content: a `Version`, and `Statements` of `Type` `Approvers`
/// with the number of approvals needed.
const DOCUMENT: &str =
    r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":1}]}"#;

/// Every implemented operation that takes a `pullRequestId`, reporting ones and
/// mutating ones alike: the rule is the service's, not one operation's.
///
/// `pub(crate)` here and on [`Fixture`], [`fixture_on`] and [`Fixture::request`]: the
/// sibling `pull_request_does_not_exist_message` suite drives the same set of
/// operations against the same fixture rather than keeping a second copy of either.
pub(crate) const PULL_REQUEST_ID_OPERATIONS: &[&str] = &[
    "GetPullRequest",
    "GetPullRequestApprovalStates",
    "GetPullRequestOverrideState",
    "GetCommentsForPullRequest",
    "PostCommentForPullRequest",
    "DescribePullRequestEvents",
    "DeletePullRequestApprovalRule",
    "CreatePullRequestApprovalRule",
    "EvaluatePullRequestApprovalRules",
    "MergePullRequestByFastForward",
    "MergePullRequestBySquash",
    "MergePullRequestByThreeWay",
    "UpdatePullRequestTitle",
    "UpdatePullRequestDescription",
    "UpdatePullRequestStatus",
    "UpdatePullRequestApprovalState",
    "OverridePullRequestApprovalRules",
];

/// Every implemented operation that takes a `revisionId`.
const REVISION_ID_OPERATIONS: &[&str] = &[
    "GetPullRequestApprovalStates",
    "GetPullRequestOverrideState",
    "EvaluatePullRequestApprovalRules",
    "UpdatePullRequestApprovalState",
    "OverridePullRequestApprovalRules",
];

/// Values that are not a pull request id: a CodeCommit pull request id is a decimal
/// integer minted per account, which is why the refusal points a caller at
/// `ListPullRequests` for the valid ones. The empty one is the row the message's
/// "empty or null" clause is for.
fn pull_request_ids() -> Vec<&'static str> {
    vec![
        "",
        "   ",
        "not-a-valid-id!!",
        "abc-not-numeric",
        "12.5",
        "-1",
        "+1",
        " 1 ",
        "1e3",
        "0x10",
        "١٢٣",
        "9 9",
    ]
}

/// One emulator holding one pull request with one approval rule, and the revision id
/// that pull request is currently at.
pub(crate) struct Fixture {
    pub(crate) router: MultiServiceRouter,
    pub(crate) pr: PullRequest,
    pub(crate) revision: String,
}

async fn fixture() -> Fixture {
    fixture_on(REPOSITORY).await
}

/// The same fixture on a repository of the caller's naming, so a suite whose rows
/// differ along the repository builds one per row.
pub(crate) async fn fixture_on(repository: &str) -> Fixture {
    let router = emulator();
    let pr = pull_request_on(&router, repository).await;
    ok(
        &router,
        "CreatePullRequestApprovalRule",
        &json!({
            "pullRequestId": pr.pull_request_id,
            "approvalRuleName": RULE,
            "approvalRuleContent": DOCUMENT,
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetPullRequest",
        &json!({ "pullRequestId": pr.pull_request_id }),
    )
    .await;
    let revision = string_at(&read, "/pullRequest/revisionId");
    Fixture {
        router,
        pr,
        revision,
    }
}

impl Fixture {
    /// A request for `operation` that names this fixture's pull request and satisfies
    /// every other member the model requires of it, so the only thing a row changes
    /// is the identifier under test.
    pub(crate) fn request(&self, operation: &str) -> Value {
        let id = &self.pr.pull_request_id;
        let repository = &self.pr.repository;
        let revision = &self.revision;
        match operation {
            "GetPullRequest" | "DescribePullRequestEvents" => json!({ "pullRequestId": id }),
            "GetPullRequestApprovalStates"
            | "GetPullRequestOverrideState"
            | "EvaluatePullRequestApprovalRules" => {
                json!({ "pullRequestId": id, "revisionId": revision })
            }
            "GetCommentsForPullRequest" => json!({
                "pullRequestId": id,
                "repositoryName": repository,
                "beforeCommitId": self.pr.before_commit,
                "afterCommitId": self.pr.after_commit,
            }),
            "PostCommentForPullRequest" => json!({
                "pullRequestId": id,
                "repositoryName": repository,
                "beforeCommitId": self.pr.before_commit,
                "afterCommitId": self.pr.after_commit,
                "content": "a comment",
            }),
            "DeletePullRequestApprovalRule" => {
                json!({ "pullRequestId": id, "approvalRuleName": RULE })
            }
            "CreatePullRequestApprovalRule" => json!({
                "pullRequestId": id,
                "approvalRuleName": "another-rule",
                "approvalRuleContent": DOCUMENT,
            }),
            "MergePullRequestByFastForward" | "MergePullRequestBySquash"
            | "MergePullRequestByThreeWay" => {
                json!({ "pullRequestId": id, "repositoryName": repository })
            }
            "UpdatePullRequestTitle" => json!({ "pullRequestId": id, "title": "a newer title" }),
            "UpdatePullRequestDescription" => {
                json!({ "pullRequestId": id, "description": "a newer description" })
            }
            "UpdatePullRequestStatus" => {
                json!({ "pullRequestId": id, "pullRequestStatus": "CLOSED" })
            }
            "UpdatePullRequestApprovalState" => json!({
                "pullRequestId": id,
                "revisionId": revision,
                "approvalState": "APPROVE",
            }),
            "OverridePullRequestApprovalRules" => json!({
                "pullRequestId": id,
                "revisionId": revision,
                "overrideStatus": "OVERRIDE",
            }),
            other => panic!("no request is written for {other}"),
        }
    }

    /// The stored pull request, as `GetPullRequest` projects it — what a refused call
    /// must leave exactly as it found it.
    async fn stored(&self) -> Value {
        ok(
            &self.router,
            "GetPullRequest",
            &json!({ "pullRequestId": self.pr.pull_request_id }),
        )
        .await
    }
}

#[tokio::test]
async fn a_pull_request_id_naming_no_pull_request_is_refused_as_an_invalid_id_naming_the_value() {
    let fixture = fixture().await;
    let before = fixture.stored().await;

    for operation in PULL_REQUEST_ID_OPERATIONS {
        for pull_request_id in pull_request_ids() {
            let case = format!("{operation} / {pull_request_id:?}");
            let mut body = fixture.request(operation);
            body["pullRequestId"] = json!(pull_request_id);

            let (code, message) =
                error_with_message(&fixture.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidPullRequestIdException",
                "case {case:?}: expected InvalidPullRequestIdException, got {code:?} saying \
                 {message:?}"
            );
            // The empty value is the one case CodeCommit words with its "empty or
            // null" clause; every other carried value is named as not valid.
            let expected = if pull_request_id.is_empty() {
                empty_pull_request_id(pull_request_id)
            } else {
                invalid_pull_request_id(pull_request_id)
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // None of those calls — the mutating ones included — changed what the next
    // caller sees.
    let after = fixture.stored().await;
    assert_eq!(
        after, before,
        "a refused identifier leaves the pull request unchanged, got {after}"
    );
}

#[tokio::test]
async fn a_revision_id_naming_no_revision_is_refused_as_an_invalid_revision_id() {
    let fixture = fixture().await;
    let before = fixture.stored().await;

    // Rows differ along the way the value fails to be the digest a revision id is:
    // empty — the case with its own sentence — and then too short, too long, not hex
    // and not a digest at all.
    let revision_ids = vec![
        ("", EMPTY_REVISION_ID),
        ("   ", INVALID_REVISION_ID),
        ("not-a-revision-id", INVALID_REVISION_ID),
        ("deadbeef", INVALID_REVISION_ID),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde",
            INVALID_REVISION_ID,
        ),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0",
            INVALID_REVISION_ID,
        ),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg",
            INVALID_REVISION_ID,
        ),
    ];

    for operation in REVISION_ID_OPERATIONS {
        for (revision_id, expected) in &revision_ids {
            let case = format!("{operation} / {revision_id:?}");
            let mut body = fixture.request(operation);
            body["revisionId"] = json!(revision_id);

            let (code, message) =
                error_with_message(&fixture.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidRevisionIdException",
                "case {case:?}: expected InvalidRevisionIdException, got {code:?} saying \
                 {message:?}"
            );
            assert_eq!(
                message, *expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    let after = fixture.stored().await;
    assert_eq!(
        after, before,
        "a refused revision id leaves the pull request unchanged, got {after}"
    );
}

#[tokio::test]
async fn a_well_formed_id_naming_no_pull_request_is_refused_with_the_fixed_message() {
    let fixture = fixture().await;
    let before = fixture.stored().await;

    // Rows differ along the id nothing was minted as: one beside the stored one, and
    // ones orders of magnitude away. The message is the same fixed text on every row
    // and for every operation, because CodeCommit's wording for this class names no
    // value — not the id asked for, and not the repository.
    for operation in PULL_REQUEST_ID_OPERATIONS {
        for pull_request_id in ["2", "99999", "987654321"] {
            let case = format!("{operation} / {pull_request_id:?}");
            let mut body = fixture.request(operation);
            body["pullRequestId"] = json!(pull_request_id);

            let (code, message) =
                error_with_message(&fixture.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "PullRequestDoesNotExistException",
                "case {case:?}: expected PullRequestDoesNotExistException, got {code:?} saying \
                 {message:?}"
            );
            assert_eq!(
                message, PULL_REQUEST_DOES_NOT_EXIST,
                "case {case:?}: expected {PULL_REQUEST_DOES_NOT_EXIST:?}, got {message:?}"
            );
        }
    }

    let after = fixture.stored().await;
    assert_eq!(
        after, before,
        "a refused id leaves the pull request unchanged, got {after}"
    );
}
