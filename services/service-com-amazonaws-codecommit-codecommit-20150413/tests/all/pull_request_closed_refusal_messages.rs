// SPDX-License-Identifier: CC0-1.0

//! The TEXT every operation puts on `PullRequestAlreadyClosedException`, over the
//! wire.
//!
//! The rule under test: one error CODE, several sentences — CodeCommit words this
//! refusal for the CHANGE the caller attempted on a pull request that is no longer
//! open, so the message belongs to the operation that refused, not to the exception
//! shape. Setting approval rule requirements aside says the pull request is already
//! merged or closed and that its approval status is not overridden; replacing the
//! pull request's text names the state the pull request is in and which text a
//! closed pull request no longer takes; every other operation carries the sentence
//! the model documents for the shape. None of the three depends on how the pull
//! request stopped being open, on the caller, or on anything else the request said.
//!
//! The table below spans what that rule generalises over: the operation refused
//! (the override path, both text paths, the approval-rule and approval-state paths,
//! and all three merge strategies), the other values the request carries (both
//! values of `OverrideStatus` — `smithy_query(query="shape",
//! shape="OverrideStatus")` — both values of `ApprovalState`, the text asked for,
//! the rule named), the identity making the call, and the two ways a pull request
//! stops being open (its author closes it with `UpdatePullRequestStatus`, or it is
//! merged). The status and code each refusal carries are asserted on every row too:
//! the model binds `PullRequestAlreadyClosedException` with `errorKind: client` and
//! `httpError: 400`.
//!
//! Sibling suites own the neighbouring behaviour: `pull_request_approval_rule_override`
//! drives the override path itself, `pull_request_text_update` the title and
//! description paths, and `merge_pull_request_repeat` the merge strategies' writes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{emulator_with_callers, ok_as, refusal_as};

/// The error every row asserts, and the status the model binds it with
/// (`errorKind: client`, `httpError: 400`).
const CODE: &str = "PullRequestAlreadyClosedException";
const CLIENT_ERROR_STATUS: u16 = 400;

/// The `PullRequestStatusEnum` value a pull request that is no longer open holds
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const STATUS_CLOSED: &str = "CLOSED";

/// Both values of `OverrideStatus` and of `ApprovalState`
/// (`smithy_query(query="shape",shape="OverrideStatus")`,
/// `smithy_query(query="shape",shape="ApprovalState")`).
const OVERRIDE: &str = "OVERRIDE";
const REVOKE: &str = "REVOKE";
const APPROVE: &str = "APPROVE";

/// The principals the calls are made by: a caller is identified by the access key id
/// its request is signed with, and the refusal's wording does not name it.
const AUTHOR: &str = "AKIDCLOSEDAUTHOR";
const OTHER: &str = "AKIDCLOSEDOTHER";

const DESTINATION: &str = "main";
const SOURCE: &str = "feature";
const RULE: &str = "needs-review";
const SECOND_RULE: &str = "needs-another-review";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The sentences the three refusals carry, written out here rather than read from
/// the service, so each is checked against the words CodeCommit answers that change
/// with.
///
/// The override sentences name nothing from the request or from the stored record;
/// the documented sentence names nothing either; the text sentence names the state
/// the pull request is IN, which is why it is a template over that status.
const OVERRIDE_REFUSAL: &str = "This pull request is already merged or closed. You cannot override \
                                the approval status of a merged or closed pull request. Modify \
                                your request, and then try again.";
const DOCUMENTED_REFUSAL: &str =
    "The pull request status cannot be updated because it is already closed.";

fn text_refusal(status: &str) -> String {
    format!(
        "The change cannot be made because the state of the pull request is {status}. You cannot \
         update the title or description of a pull request after it has been closed."
    )
}

fn override_refusal(_status: &str) -> String {
    OVERRIDE_REFUSAL.to_string()
}

fn documented_refusal(_status: &str) -> String {
    DOCUMENTED_REFUSAL.to_string()
}

/// An approval rule document asking for `approvals` approvals, as compact JSON text.
fn document(approvals: u32) -> String {
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [{
            "Type": STATEMENT_TYPE,
            "NumberOfApprovalsNeeded": approvals,
        }],
    }))
    .expect("the document serializes")
}

/// A pull request that is no longer open, and everything a call against it needs to
/// name.
struct ClosedPullRequest {
    router: MultiServiceRouter,
    repository: String,
    pull_request_id: String,
    revision_id: String,
    /// The `pullRequestStatus` the stored pull request now holds.
    status: String,
    /// How it stopped being open, for the assertion messages.
    closing: &'static str,
}

/// Open a repository with two diverged branches, one pull request between them
/// carrying one approval rule, and then close that pull request: either by its
/// author updating its status, or by merging it (which needs the unmet rule set
/// aside first).
async fn closed_pull_request(repository: &str, by_merging: bool) -> ClosedPullRequest {
    let (router, _engine) = emulator_with_callers(&[AUTHOR, OTHER]);
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let destination = ok_as(
        &router,
        AUTHOR,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "filePath": "README.md",
            "fileContent": base64(b"alpha\n"),
            "commitMessage": "the destination version",
        }),
    )
    .await;
    let base = string_at(&destination, "/commitId");
    ok_as(
        &router,
        AUTHOR,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    ok_as(
        &router,
        AUTHOR,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "filePath": "feature.txt",
            "fileContent": base64(b"beta\n"),
            "parentCommitId": base,
            "commitMessage": "the source version",
        }),
    )
    .await;
    let created = ok_as(
        &router,
        AUTHOR,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "description": "what it changes",
            "targets": [{
                "repositoryName": repository,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    let revision_id = string_at(&created, "/pullRequest/revisionId");
    ok_as(
        &router,
        AUTHOR,
        "CreatePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": RULE,
            "approvalRuleContent": document(1),
        }),
    )
    .await;

    let closed = if by_merging {
        // The rule the pull request carries is unmet, so the merge goes through only
        // once the requirements are set aside — which is allowed while it is open.
        ok_as(
            &router,
            OTHER,
            "OverridePullRequestApprovalRules",
            &json!({
                "pullRequestId": pull_request_id,
                "revisionId": revision_id,
                "overrideStatus": OVERRIDE,
            }),
        )
        .await;
        ok_as(
            &router,
            AUTHOR,
            "MergePullRequestByFastForward",
            &json!({
                "repositoryName": repository,
                "pullRequestId": pull_request_id,
            }),
        )
        .await
    } else {
        ok_as(
            &router,
            AUTHOR,
            "UpdatePullRequestStatus",
            &json!({
                "pullRequestId": pull_request_id,
                "pullRequestStatus": STATUS_CLOSED,
            }),
        )
        .await
    };
    let status = string_at(&closed, "/pullRequest/pullRequestStatus");
    assert_eq!(
        status, STATUS_CLOSED,
        "the pull request has to be closed before the table runs, got {closed}"
    );

    ClosedPullRequest {
        router,
        repository: repository.to_string(),
        pull_request_id,
        revision_id,
        status,
        closing: if by_merging {
            "closed by a merge"
        } else {
            "closed by its author"
        },
    }
}

/// One row: the call made against the closed pull request, and the sentence its
/// refusal has to carry.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The request body, built from the closed pull request the row runs against.
    body: fn(&ClosedPullRequest) -> Value,
    /// The message, over the status the stored pull request holds.
    expected: fn(&str) -> String,
    /// Whether the call is still REFUSED on a pull request closed by a merge: a
    /// merge of an already merged pull request is answered idempotently instead, so
    /// the merge strategies have nothing to assert there.
    refused_after_a_merge: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "override, setting the requirements aside",
            operation: "OverridePullRequestApprovalRules",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "revisionId": closed.revision_id,
                    "overrideStatus": OVERRIDE,
                })
            },
            expected: override_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "override, holding the requirements again",
            operation: "OverridePullRequestApprovalRules",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "revisionId": closed.revision_id,
                    "overrideStatus": REVOKE,
                })
            },
            expected: override_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "a new title",
            operation: "UpdatePullRequestTitle",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "title": "a retitled change",
                })
            },
            expected: text_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "a new description",
            operation: "UpdatePullRequestDescription",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "description": "a redescribed change",
                })
            },
            expected: text_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "another approval rule",
            operation: "CreatePullRequestApprovalRule",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "approvalRuleName": SECOND_RULE,
                    "approvalRuleContent": document(2),
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "deleting the approval rule it carries",
            operation: "DeletePullRequestApprovalRule",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "approvalRuleName": RULE,
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "approving it",
            operation: "UpdatePullRequestApprovalState",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "revisionId": closed.revision_id,
                    "approvalState": APPROVE,
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "revoking an approval of it",
            operation: "UpdatePullRequestApprovalState",
            body: |closed| {
                json!({
                    "pullRequestId": closed.pull_request_id,
                    "revisionId": closed.revision_id,
                    "approvalState": REVOKE,
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: true,
        },
        Row {
            case: "merging it by fast forward",
            operation: "MergePullRequestByFastForward",
            body: |closed| {
                json!({
                    "repositoryName": closed.repository,
                    "pullRequestId": closed.pull_request_id,
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: false,
        },
        Row {
            case: "merging it by squash",
            operation: "MergePullRequestBySquash",
            body: |closed| {
                json!({
                    "repositoryName": closed.repository,
                    "pullRequestId": closed.pull_request_id,
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: false,
        },
        Row {
            case: "merging it three way",
            operation: "MergePullRequestByThreeWay",
            body: |closed| {
                json!({
                    "repositoryName": closed.repository,
                    "pullRequestId": closed.pull_request_id,
                })
            },
            expected: documented_refusal,
            refused_after_a_merge: false,
        },
    ]
}

#[tokio::test]
async fn a_closed_pull_request_refuses_each_change_in_that_operations_own_words() {
    for (by_merging, repository) in [(false, "closed-by-its-author"), (true, "closed-by-a-merge")] {
        let closed = closed_pull_request(repository, by_merging).await;
        for row in rows() {
            if by_merging && !row.refused_after_a_merge {
                continue;
            }
            let expected = (row.expected)(&closed.status);
            // The same row is run by two principals: the words a refusal carries are
            // the operation's, so the identity asking does not change them.
            for caller in [AUTHOR, OTHER] {
                let (code, message) = refusal_as(
                    &closed.router,
                    caller,
                    row.operation,
                    &(row.body)(&closed),
                    CLIENT_ERROR_STATUS,
                )
                .await;
                let case = row.case;
                let closing = closed.closing;
                assert_eq!(
                    code, CODE,
                    "case {case:?} ({closing}, as {caller}): expected {CODE}, got {code:?} saying \
                     {message:?}"
                );
                assert_eq!(
                    message, expected,
                    "case {case:?} ({closing}, as {caller}): expected {expected:?}, got \
                     {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_three_closed_pull_request_refusals_are_worded_apart() {
    // The point of the rule: the override refusal, the text refusal and the
    // documented sentence are three DIFFERENT strings, so a client reading the
    // message of one code can still tell which change was refused.
    let closed = closed_pull_request("closed-refusals-differ", false).await;
    let mut seen: Vec<String> = Vec::new();
    for row in rows() {
        let (_, message) = refusal_as(
            &closed.router,
            AUTHOR,
            row.operation,
            &(row.body)(&closed),
            CLIENT_ERROR_STATUS,
        )
        .await;
        if !seen.contains(&message) {
            seen.push(message);
        }
    }
    let expected = vec![
        OVERRIDE_REFUSAL.to_string(),
        text_refusal(STATUS_CLOSED),
        DOCUMENTED_REFUSAL.to_string(),
    ];
    assert_eq!(
        seen, expected,
        "case \"distinct wordings\": expected {expected:?}, got {seen:?}"
    );
}
