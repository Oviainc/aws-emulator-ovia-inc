// SPDX-License-Identifier: CC0-1.0

//! Reading the approvals a pull request holds, over the wire:
//! `GetPullRequestApprovalStates`, driven as awsJson1_1 requests through the router
//! that serves the real emulator (the wire harness comes from the sibling
//! `repository_lifecycle` suite, and calling as a NAMED principal from
//! `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: the call reports one `Approval` per reviewer whose LAST
//! recorded approval state on the pull request is an approval, naming that
//! reviewer's ARN and the state `APPROVE` — the model documents the member as "The
//! state of the approval, APPROVE or REVOKE. REVOKE states are not stored"
//! (`smithy_query(query="shape",shape="Approval")`) — and it reports it without
//! changing anything a later caller sees.
//!
//! The table below spans what that rule generalises over: how many reviewers
//! recorded a state, both values of `ApprovalState`
//! (`smithy_query(query="shape",shape="ApprovalState")`), repeated and alternating
//! calls by one reviewer, a revoke by a reviewer holding no approval, the order the
//! reviewers called in against the order their approvals are reported in, and the
//! author of the pull request as a caller. A second table spans every value that
//! can name no pull request and no revision.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, ok_as, refusal_as};

/// The operation under test, and the writer whose records it reads back.
const OPERATION: &str = "GetPullRequestApprovalStates";
const RECORD_OPERATION: &str = "UpdatePullRequestApprovalState";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// Both values of `ApprovalState`
/// (`smithy_query(query="shape",shape="ApprovalState")`), the second of which is
/// the one an `Approval` never carries.
const APPROVE: &str = "APPROVE";
const REVOKE: &str = "REVOKE";

/// The `PullRequestEventType` value an approval state change is logged under
/// (`smithy_query(query="shape",shape="PullRequestEventType")`), which a reporting
/// call must leave untouched.
const APPROVAL_STATE_CHANGED: &str = "PULL_REQUEST_APPROVAL_STATE_CHANGED";

/// The principals the calls below are made by. A caller is identified by the access
/// key id its request is signed with, so the approval a call records belongs to that
/// caller's ARN.
const AUTHOR: &str = "AKIDAUTHOR";
const REVIEWER: &str = "AKIDREVIEWER";
const OTHER_REVIEWER: &str = "AKIDOTHERREVIEWER";
const THIRD_REVIEWER: &str = "AKIDTHIRDREVIEWER";

const REPOSITORY: &str = "approval-states-read";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";
/// The source branch of the second pull request ([`Fixture::other_revision`]).
const SECOND_SOURCE: &str = "second-feature";

/// The sentences the refused calls carry, written out here rather than read from the
/// service, so each refusal is checked against the words CodeCommit answers that
/// error class with.
const PULL_REQUEST_ID_REQUIRED: &str = "A pull request ID is required, but none was provided.";
const INVALID_PULL_REQUEST_ID: &str = "The pull request ID is not valid. Make sure that you have \
                                       provided the full ID and that the pull request is in the \
                                       specified repository, and then try again.";
const REVISION_ID_REQUIRED: &str = "A revision ID is required, but was not provided.";
const INVALID_REVISION_ID: &str =
    "The revision ID is not valid. Use GetPullRequest to determine the value.";

/// `PullRequestDoesNotExistException`'s sentence pair, built from the id the refused
/// request named.
fn does_not_exist(pull_request_id: &str) -> String {
    format!(
        "The pull request ID {pull_request_id} could not be found. Make sure that you have \
         specified the correct repository name and pull request ID, and then try again."
    )
}

/// An emulator holding one repository with two diverged branches and one open pull
/// request between them, opened by [`AUTHOR`].
struct Fixture {
    router: MultiServiceRouter,
    pull_request_id: String,
}

async fn fixture() -> Fixture {
    let (router, _engine) =
        emulator_with_callers(&[AUTHOR, REVIEWER, OTHER_REVIEWER, THIRD_REVIEWER]);
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let destination = ok_as(
        &router,
        AUTHOR,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
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
            "repositoryName": REPOSITORY,
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
            "repositoryName": REPOSITORY,
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
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    Fixture {
        router,
        pull_request_id,
    }
}

impl Fixture {
    /// The pull request `GetPullRequest` reports.
    async fn stored(&self) -> Value {
        let read = ok(
            &self.router,
            "GetPullRequest",
            &json!({ "pullRequestId": self.pull_request_id }),
        )
        .await;
        read.get("pullRequest")
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequest in {read}"))
    }

    /// The `revisionId` of the pull request's current revision.
    async fn revision_id(&self) -> String {
        string_at(&self.stored().await, "/revisionId")
    }

    /// The approval state changes the pull request's event log holds.
    async fn approval_events(&self) -> Vec<Value> {
        let read = ok(
            &self.router,
            "DescribePullRequestEvents",
            &json!({
                "pullRequestId": self.pull_request_id,
                "pullRequestEventType": APPROVAL_STATE_CHANGED,
            }),
        )
        .await;
        read.get("pullRequestEvents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequestEvents in {read}"))
    }

    /// The `approvals` the operation under test reports for `revision_id`, as
    /// `(userArn, approvalState)` pairs in the order they are reported.
    async fn approvals(&self, revision_id: &str) -> Vec<(String, String)> {
        let read = ok(
            &self.router,
            OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id,
                "revisionId": revision_id,
            }),
        )
        .await;
        let reported = read
            .get("approvals")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no approvals member in {read}"));
        reported
            .iter()
            .map(|approval| {
                (
                    string_at(approval, "/userArn"),
                    string_at(approval, "/approvalState"),
                )
            })
            .collect()
    }

    /// Record `approval_state` for `caller` on the pull request's current revision.
    async fn record(&self, caller: &str, revision_id: &str, approval_state: &str) {
        ok_as(
            &self.router,
            caller,
            RECORD_OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id,
                "revisionId": revision_id,
                "approvalState": approval_state,
            }),
        )
        .await;
    }

    /// Open a SECOND pull request in this repository and report ITS current revision
    /// id — a well-formed revision id that is not the fixture pull request's.
    async fn other_revision(&self) -> String {
        let branch = ok(
            &self.router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": DESTINATION }),
        )
        .await;
        let tip = string_at(&branch, "/branch/commitId");
        ok_as(
            &self.router,
            AUTHOR,
            "CreateBranch",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": SECOND_SOURCE,
                "commitId": tip,
            }),
        )
        .await;
        ok_as(
            &self.router,
            AUTHOR,
            "PutFile",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": SECOND_SOURCE,
                "filePath": "second.txt",
                "fileContent": base64(b"gamma\n"),
                "parentCommitId": tip,
                "commitMessage": "another change",
            }),
        )
        .await;
        let created = ok_as(
            &self.router,
            AUTHOR,
            "CreatePullRequest",
            &json!({
                "title": "another change to review",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": SECOND_SOURCE,
                    "destinationReference": DESTINATION,
                }],
            }),
        )
        .await;
        string_at(&created, "/pullRequest/revisionId")
    }
}

/// One row of the table: the states recorded on the pull request, and the reviewers
/// whose approvals the read then reports.
struct ApprovalsRow {
    case: &'static str,
    /// `(access key id, approvalState)` in the order the calls are made.
    recorded: Vec<(&'static str, &'static str)>,
    /// The access key ids of the reviewers reported as approvers, in the order the
    /// read reports them (reviewer ARN order, whatever order they called in).
    reported: Vec<&'static str>,
}

#[tokio::test]
async fn the_approvals_a_pull_request_holds_are_reported_for_each_approving_reviewer() {
    // Rows differ along how many reviewers recorded a state, which state they
    // recorded, how one reviewer's calls repeat and alternate, and the order the
    // reviewers called in.
    let rows = vec![
        ApprovalsRow {
            case: "a pull request nobody has reviewed",
            recorded: vec![],
            reported: vec![],
        },
        ApprovalsRow {
            case: "one approval",
            recorded: vec![(REVIEWER, APPROVE)],
            reported: vec![REVIEWER],
        },
        ApprovalsRow {
            case: "the same approval recorded twice",
            recorded: vec![(REVIEWER, APPROVE), (REVIEWER, APPROVE)],
            reported: vec![REVIEWER],
        },
        ApprovalsRow {
            case: "an approval taken back again",
            recorded: vec![(REVIEWER, APPROVE), (REVIEWER, REVOKE)],
            reported: vec![],
        },
        ApprovalsRow {
            case: "approved, revoked and approved again",
            recorded: vec![
                (REVIEWER, APPROVE),
                (REVIEWER, REVOKE),
                (REVIEWER, APPROVE),
            ],
            reported: vec![REVIEWER],
        },
        ApprovalsRow {
            case: "a revoke by a reviewer holding no approval",
            recorded: vec![(REVIEWER, REVOKE)],
            reported: vec![],
        },
        ApprovalsRow {
            case: "two reviewers approving",
            recorded: vec![(REVIEWER, APPROVE), (OTHER_REVIEWER, APPROVE)],
            reported: vec![OTHER_REVIEWER, REVIEWER],
        },
        ApprovalsRow {
            case: "two reviewers, one of whom took the approval back",
            recorded: vec![
                (REVIEWER, APPROVE),
                (OTHER_REVIEWER, APPROVE),
                (REVIEWER, REVOKE),
            ],
            reported: vec![OTHER_REVIEWER],
        },
        ApprovalsRow {
            case: "three reviewers approving in the reverse of their ARN order",
            recorded: vec![
                (THIRD_REVIEWER, APPROVE),
                (REVIEWER, APPROVE),
                (OTHER_REVIEWER, APPROVE),
            ],
            reported: vec![OTHER_REVIEWER, REVIEWER, THIRD_REVIEWER],
        },
    ];

    for row in rows {
        let case = row.case;
        let fixture = fixture().await;
        let revision = fixture.revision_id().await;
        for (caller, state) in &row.recorded {
            fixture.record(caller, &revision, state).await;
        }

        let expected: Vec<(String, String)> = row
            .reported
            .iter()
            .map(|caller| (caller_arn(caller), APPROVE.to_string()))
            .collect();
        let reported = fixture.approvals(&revision).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        // "REVOKE states are not stored": a reviewer's revoke leaves no approval
        // behind, so no reported approval can carry that state.
        assert!(
            reported.iter().all(|(_, state)| state == APPROVE),
            "case {case:?}: every reported approval carries {APPROVE:?}, got {reported:?}"
        );

        // Reporting changes nothing a later caller sees: the second read answers
        // the first, the event log gained no entry and the pull request's own
        // projection is the one the read found.
        let before = fixture.stored().await;
        let logged = fixture.approval_events().await.len();
        let again = fixture.approvals(&revision).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read answers the first, got {again:?}"
        );
        assert_eq!(
            fixture.approval_events().await.len(),
            logged,
            "case {case:?}: reading the approvals appends no event"
        );
        assert_eq!(
            fixture.stored().await,
            before,
            "case {case:?}: reading the approvals leaves the pull request unchanged"
        );
    }
}

#[tokio::test]
async fn the_approvals_are_reported_for_every_well_formed_revision_id() {
    let fixture = fixture().await;
    let revision = fixture.revision_id().await;
    fixture.record(REVIEWER, &revision, APPROVE).await;
    let expected = vec![(caller_arn(REVIEWER), APPROVE.to_string())];

    // This operation models no "revision not current" failure, so every id of the
    // form a revision id takes is ANSWERED with the approvals the pull request
    // holds: a hex digest is case insensitive, the digest of another pull request's
    // revision is one of that form, and so is one no pull request ever minted.
    let other = fixture.other_revision().await;
    let cases = vec![
        ("the current revision", revision.clone()),
        (
            "the current revision in upper-case hex",
            revision.to_ascii_uppercase(),
        ),
        ("another pull request's current revision", other),
        (
            "a digest naming no revision of any pull request",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        ),
    ];

    for (case, revision_id) in cases {
        let reported = fixture.approvals(&revision_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// How a row names the pull request and the revision: `None` leaves the member out
/// of the request altogether, which is a different request from one carrying it
/// empty.
struct Refusal {
    case: &'static str,
    /// `Some("{id}")` is the id of the pull request the fixture opened.
    pull_request_id: Option<&'static str>,
    /// `Some("{revision}")` is the current revision of that pull request.
    revision_id: Option<&'static str>,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_request_naming_no_pull_request_or_revision_reports_no_approvals() {
    let fixture = fixture().await;
    let revision = fixture.revision_id().await;
    fixture.record(REVIEWER, &revision, APPROVE).await;

    // Rows differ along which member names nothing whose approvals can be reported:
    // the pull request id's form and existence, and the revision id's form.
    let refusals = vec![
        Refusal {
            case: "no pullRequestId at all",
            pull_request_id: None,
            revision_id: Some("{revision}"),
            code: "PullRequestIdRequiredException",
            message: PULL_REQUEST_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty pullRequestId",
            pull_request_id: Some(""),
            revision_id: Some("{revision}"),
            code: "PullRequestIdRequiredException",
            message: PULL_REQUEST_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "a pullRequestId that is not a decimal integer",
            pull_request_id: Some("not-an-id"),
            revision_id: Some("{revision}"),
            code: "InvalidPullRequestIdException",
            message: INVALID_PULL_REQUEST_ID.to_string(),
        },
        Refusal {
            case: "a well-formed id nothing was minted for",
            pull_request_id: Some("99999"),
            revision_id: Some("{revision}"),
            code: "PullRequestDoesNotExistException",
            message: does_not_exist("99999"),
        },
        Refusal {
            case: "no revisionId at all",
            pull_request_id: Some("{id}"),
            revision_id: None,
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty revisionId",
            pull_request_id: Some("{id}"),
            revision_id: Some(""),
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "a revisionId that is not a digest at all",
            pull_request_id: Some("{id}"),
            revision_id: Some("not-a-revision-id"),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a digest one character short",
            pull_request_id: Some("{id}"),
            revision_id: Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde"),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a digest one character too long",
            pull_request_id: Some("{id}"),
            revision_id: Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0",
            ),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a digest of the right length holding a non-hex character",
            pull_request_id: Some("{id}"),
            revision_id: Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg"),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let mut body = json!({});
        if let Some(id) = refusal.pull_request_id {
            body["pullRequestId"] = json!(match id {
                "{id}" => fixture.pull_request_id.clone(),
                literal => literal.to_string(),
            });
        }
        if let Some(id) = refusal.revision_id {
            body["revisionId"] = json!(match id {
                "{revision}" => revision.clone(),
                literal => literal.to_string(),
            });
        }

        let (code, message) =
            refusal_as(&fixture.router, REVIEWER, OPERATION, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, refusal.code,
            "case {case:?}: expected {}, got {code:?}",
            refusal.code
        );
        assert_eq!(
            message, refusal.message,
            "case {case:?}: expected {:?}, got {message:?}",
            refusal.message
        );
    }

    // None of them disturbed the approval the pull request holds.
    let reported = fixture.approvals(&revision).await;
    assert_eq!(
        reported,
        vec![(caller_arn(REVIEWER), APPROVE.to_string())],
        "a refused read leaves the recorded approval reportable, got {reported:?}"
    );
}
