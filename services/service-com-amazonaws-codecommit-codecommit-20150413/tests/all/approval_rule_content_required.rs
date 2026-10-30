// SPDX-License-Identifier: CC0-1.0

//! The refusal `CreatePullRequestApprovalRule` answers content that carries no
//! approval rule document, over the wire.
//!
//! The rule under test: `approvalRuleContent` carries a document, so content holding
//! none of one — the member left out, or present and holding nothing but whitespace —
//! is refused with the modelled `InvalidApprovalRuleContentException`
//! (`smithy_query(query="shape",shape="InvalidApprovalRuleContentException")`:
//! `errorKind` client, `httpError` 400) and ONE fixed sentence that covers both forms
//! the absence takes, interpolating nothing from the request; no rule is attached to
//! the pull request. A client catching that one modelled exception therefore sees
//! both this refusal and the one for a document that is malformed — which carries the
//! OTHER sentence, so the two reasons stay distinguishable.
//!
//! The table below spans what the rule generalises over rather than one recorded
//! request: whether the member is sent at all, and which whitespace a present value
//! holds — the empty string (the value the member's `@length min 1` refuses at the
//! request boundary), a single space, several spaces, tabs and newlines, and the
//! non-breaking and ideographic spaces, which are whitespace a document is no more
//! made of than of a blank.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, ok, string_at};
use crate::wire::fresh_client_request_token;

/// The status the model gives both content refusals.
const CLIENT_ERROR_STATUS: u16 = 400;

const CODE: &str = "InvalidApprovalRuleContentException";

/// The sentence the service answers for content that carries no document at all.
const NO_CONTENT_MESSAGE: &str =
    "Content for an approval rule is required, but the content of the approval rule was either \
     empty or null.";

/// The sentence the same code carries for content that IS a document and is not a
/// valid one — the other half of the class, here to keep the two apart.
const MALFORMED_MESSAGE: &str =
    "The JSON in the approval rule is not valid. Review the JSON and make sure it is valid, and \
     then try again.";

const REPOSITORY: &str = "approval-rule-content";

/// A valid approval rule document, as the CodeCommit User Guide documents the
/// structure for approval rule content.
fn document(approvals: u32) -> String {
    serde_json::to_string(&json!({
        "Version": "2018-11-08",
        "Statements": [{
            "Type": "Approvers",
            "NumberOfApprovalsNeeded": approvals,
        }],
    }))
    .expect("the document serializes")
}

/// One repository with two diverged branches and one open pull request between them.
async fn open_pull_request() -> (MultiServiceRouter, String) {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let destination = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\n"),
            "commitMessage": "the destination version",
        }),
    )
    .await;
    let base = string_at(&destination, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "commitId": base,
        }),
    )
    .await;
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\nbeta\n"),
            "parentCommitId": base,
            "commitMessage": "the source version",
        }),
    )
    .await;
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": "feature",
                "destinationReference": "main",
            }],
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    (router, pull_request_id)
}

/// The approval rules the stored pull request holds.
async fn rules_of(router: &MultiServiceRouter, pull_request_id: &str) -> Vec<Value> {
    let read = ok(
        router,
        "GetPullRequest",
        &json!({ "pullRequestId": pull_request_id }),
    )
    .await;
    read.pointer("/pullRequest/approvalRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The rows: what the request carries as content (`None` leaves the member out).
fn no_content_rows() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("the member left out", None),
        ("the empty string", Some("")),
        ("a single space", Some(" ")),
        ("several spaces", Some("     ")),
        ("tabs and newlines", Some("\t\n\r\n\t")),
        ("a non-breaking space", Some("\u{00a0}")),
        ("an ideographic space", Some("\u{3000}")),
    ]
}

#[tokio::test]
async fn content_carrying_no_document_is_refused_in_one_fixed_sentence() {
    let (router, pull_request_id) = open_pull_request().await;

    for (index, (case, content)) in no_content_rows().into_iter().enumerate() {
        let mut body = json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": format!("rule-{index}"),
        });
        if let Some(content) = content {
            body["approvalRuleContent"] = json!(content);
        }
        let (code, message) = error_with_message(
            &router,
            "CreatePullRequestApprovalRule",
            &body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, CODE,
            "case {case:?}: expected the modelled {CODE}, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, NO_CONTENT_MESSAGE,
            "case {case:?}: expected the exact sentence AWS words this refusal with, got \
             {message:?}"
        );
        assert!(
            !message.contains(&format!("rule-{index}")),
            "case {case:?}: the message must interpolate nothing, got {message:?}"
        );
    }

    // No refused request attached a rule to the pull request.
    let rules = rules_of(&router, &pull_request_id).await;
    assert!(
        rules.is_empty(),
        "a refused rule must not be attached, got {rules:?}"
    );

    // Content that IS a document and is not a valid one takes the same code and the
    // OTHER sentence, so the two reasons remain distinguishable.
    for (case, malformed) in [
        ("prose, not JSON", "two approvals please"),
        ("a JSON array", "[]"),
        ("a document with no statements", r#"{"Version":"2018-11-08","Statements":[]}"#),
    ] {
        let (code, message) = error_with_message(
            &router,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": pull_request_id,
                "approvalRuleName": "malformed",
                "approvalRuleContent": malformed,
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, CODE,
            "case {case:?}: expected the modelled {CODE}, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, MALFORMED_MESSAGE,
            "case {case:?}: a malformed document keeps its own sentence, got {message:?}"
        );
    }

    // The positive control: a valid document is attached, so the rows above are
    // refusals about the content and not about the pull request.
    let created = ok(
        &router,
        "CreatePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": "needs-one-approval",
            "approvalRuleContent": document(1),
        }),
    )
    .await;
    assert_eq!(
        string_at(&created, "/approvalRule/approvalRuleName"),
        "needs-one-approval",
        "a valid document must be accepted, got {created}"
    );
    let rules = rules_of(&router, &pull_request_id).await;
    assert_eq!(
        rules.len(),
        1,
        "exactly the accepted rule must be attached, got {rules:?}"
    );
}
