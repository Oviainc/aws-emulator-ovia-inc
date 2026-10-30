// SPDX-License-Identifier: CC0-1.0

//! A reviewer's approval state on a pull request, over the wire:
//! `UpdatePullRequestApprovalState`, driven as awsJson1_1 requests through the
//! router that serves the real emulator (the wire harness comes from the sibling
//! `repository_lifecycle` suite, and calling as a NAMED principal from
//! `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: the call records the state of the CALLING identity's
//! approval on the pull request's current revision — the approvals its approval
//! rules are then counted against — and a change appends one
//! `PULL_REQUEST_APPROVAL_STATE_CHANGED` entry to the pull request's event log
//! naming that identity, the revision and the new state, while a call asking for the
//! state the identity already holds changes nothing. An approval is not a revision
//! of the pull request, so the revision id does not move.
//!
//! The tables below span what that rule generalises over: both values of
//! `ApprovalState` (`smithy_query(query="shape",shape="ApprovalState")`), the number
//! of reviewers and of approvals a rule asks for, repeated and alternating calls by
//! one reviewer, a reviewer inside and outside a rule's approval pool, the author of
//! the pull request as the caller, every member that can name no pull request, no
//! revision and no state, and a pull request holding as many approvals as this
//! emulator stores for one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_APPROVALS_PER_PULL_REQUEST;

use crate::harness::{
    empty_pull_request_id, invalid_pull_request_id, EMPTY_REVISION_ID, INVALID_REVISION_ID,
    PULL_REQUEST_DOES_NOT_EXIST,
};
use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, hold_long_term_key, ok_as, refusal_as};

/// The operation under test, and the readers a recorded approval is read back
/// through.
const OPERATION: &str = "UpdatePullRequestApprovalState";
const SQUASH_OPERATION: &str = "MergePullRequestBySquash";
const APPROVAL_STATES_OPERATION: &str = "GetPullRequestApprovalStates";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// Both values of `ApprovalState`
/// (`smithy_query(query="shape",shape="ApprovalState")`).
const APPROVE: &str = "APPROVE";
const REVOKE: &str = "REVOKE";

/// The `PullRequestEventType` value an approval state change is logged under, and
/// the `PullRequestStatusEnum` values a pull request holds
/// (`smithy_query(query="shape",shape="PullRequestEventType")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const APPROVAL_STATE_CHANGED: &str = "PULL_REQUEST_APPROVAL_STATE_CHANGED";
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The principals the calls below are made by. A caller is identified by the access
/// key id its request is signed with, so the approval a call records belongs to that
/// caller's ARN.
const AUTHOR: &str = "AKIDAUTHOR";
const REVIEWER: &str = "AKIDREVIEWER";
const OTHER_REVIEWER: &str = "AKIDOTHERREVIEWER";

const REPOSITORY: &str = "approval-state";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";
/// The source branch of the second pull request
/// ([`Fixture::second_pull_request_revision`]).
const SECOND_SOURCE: &str = "second-feature";
const RULE: &str = "needs-review";

/// The sentences the refused calls carry, written out here rather than read from the
/// service, so each refusal is checked against the words CodeCommit answers that
/// error class with.
const PULL_REQUEST_ID_REQUIRED: &str = "A pull request ID is required, but none was provided.";
const REVISION_ID_REQUIRED: &str = "A revision ID is required, but was not provided.";
const REVISION_NOT_CURRENT: &str = "The revision ID provided in the request does not match the \
                                    current revision ID. Use GetPullRequest to retrieve the \
                                    current revision ID.";
const APPROVAL_STATE_REQUIRED: &str = "An approval state is required, but was not specified.";
const INVALID_APPROVAL_STATE: &str =
    "An approval state is required, but the supplied value is not valid. Valid values for \
     approval states are: [APPROVE, REVOKE].";
const CANNOT_BE_APPROVED_BY_AUTHOR: &str =
    "The approval cannot be applied because the user approving the pull request matches the user \
     who created the pull request. You cannot approve a pull request that you created.";
const ALREADY_CLOSED: &str = "The pull request status cannot be updated because it is already \
                              closed.";
const RULES_NOT_SATISFIED: &str = "You cannot merge the pull request because the requirements for \
                                   one or more approval rules have not been met. For details \
                                   about the approval rules that still have requirements, use \
                                   EvaluatePullRequestApprovalRules.";

/// The sentence `MaximumNumberOfApprovalsExceededException` is documented with
/// (`smithy_query(query="doc",shape="MaximumNumberOfApprovalsExceededException")`),
/// which is what the refusal states before naming the bound.
const APPROVALS_EXCEEDED_SENTENCE: &str = "The number of approvals required for the approval rule \
                                           exceeds the maximum number allowed.";

/// An approval rule document asking for `approvals` approvals, from `pool` where it
/// names one, as compact JSON text.
fn document(approvals: u32, pool: &[String]) -> String {
    let mut statement = json!({
        "Type": STATEMENT_TYPE,
        "NumberOfApprovalsNeeded": approvals,
    });
    if !pool.is_empty() {
        statement["ApprovalPoolMembers"] = json!(pool);
    }
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [statement],
    }))
    .expect("a JSON document")
}

/// An emulator holding one repository with two diverged branches and one open pull
/// request between them, opened by [`AUTHOR`] and carrying the approval rule the
/// case asked for.
struct Fixture {
    router: MultiServiceRouter,
    /// The shared engine, so a case needing MORE principals than the fixture names
    /// can give one its own long-term access key.
    engine: std::sync::Arc<engine::InMemoryEngine>,
    pull_request_id: String,
}

async fn fixture(rule: Option<&str>) -> Fixture {
    let (router, engine) = emulator_with_callers(&[AUTHOR, REVIEWER, OTHER_REVIEWER]);
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
    if let Some(content) = rule {
        ok_as(
            &router,
            AUTHOR,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": pull_request_id,
                "approvalRuleName": RULE,
                "approvalRuleContent": content,
            }),
        )
        .await;
    }
    Fixture {
        router,
        engine,
        pull_request_id,
    }
}

impl Fixture {
    /// Give `access_key_id` a long-term key of its own, so a call signed with it
    /// speaks for a principal of its own.
    fn hold(&self, access_key_id: &str) -> String {
        hold_long_term_key(&self.engine, access_key_id)
    }
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

    /// The `pullRequestStatus` the pull request holds.
    async fn status(&self) -> String {
        string_at(&self.stored().await, "/pullRequestStatus")
    }

    /// The approval state changes the pull request's event log holds, oldest first,
    /// across every page the reader serves (one page carries at most 100 entries).
    async fn approval_events(&self) -> Vec<Value> {
        let mut events = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = json!({
                "pullRequestId": self.pull_request_id,
                "pullRequestEventType": APPROVAL_STATE_CHANGED,
            });
            if let Some(token) = &next_token {
                body["nextToken"] = json!(token);
            }
            let read = ok(&self.router, "DescribePullRequestEvents", &body).await;
            let page = read
                .get("pullRequestEvents")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("no pullRequestEvents in {read}"));
            events.extend(page);
            next_token = read
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                return events;
            }
        }
    }

    fn body(&self, revision_id: &str, approval_state: &str) -> Value {
        json!({
            "pullRequestId": self.pull_request_id,
            "revisionId": revision_id,
            "approvalState": approval_state,
        })
    }

    /// Open a SECOND pull request in this repository, between another source branch
    /// and the same destination, and report ITS current revision id — a well-formed
    /// revision id that names no revision of the fixture's pull request.
    async fn second_pull_request_revision(&self) -> String {
        self.pull_request_opened_by(AUTHOR, SECOND_SOURCE).await.1
    }

    /// Open another pull request in this repository AS `caller`, between a new
    /// branch `source` off the destination tip and the same destination, and report
    /// its `pullRequestId` and the `revisionId` of its current revision.
    ///
    /// Who opened a pull request is what makes them its author, so this is how a
    /// case gets a pull request whose author is a chosen identity.
    async fn pull_request_opened_by(&self, caller: &str, source: &str) -> (String, String) {
        let branch = ok(
            &self.router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": DESTINATION }),
        )
        .await;
        let tip = string_at(&branch, "/branch/commitId");
        ok_as(
            &self.router,
            caller,
            "CreateBranch",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": source,
                "commitId": tip,
            }),
        )
        .await;
        ok_as(
            &self.router,
            caller,
            "PutFile",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": source,
                "filePath": format!("{source}.txt"),
                "fileContent": base64(b"gamma\n"),
                "parentCommitId": tip,
                "commitMessage": "another change",
            }),
        )
        .await;
        let created = ok_as(
            &self.router,
            caller,
            "CreatePullRequest",
            &json!({
                "title": "another change to review",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": source,
                    "destinationReference": DESTINATION,
                }],
            }),
        )
        .await;
        (
            string_at(&created, "/pullRequest/pullRequestId"),
            string_at(&created, "/pullRequest/revisionId"),
        )
    }
}

/// The ARNs of the reviewers `GetPullRequestApprovalStates` reports an approval of
/// this revision for, in the order it reports them.
async fn approver_arns(
    router: &MultiServiceRouter,
    pull_request_id: &str,
    revision_id: &str,
) -> Vec<String> {
    let read = ok(
        router,
        APPROVAL_STATES_OPERATION,
        &json!({
            "pullRequestId": pull_request_id,
            "revisionId": revision_id,
        }),
    )
    .await;
    read.get("approvals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("no approvals member in {read}"))
        .iter()
        .map(|approval| string_at(approval, "/userArn"))
        .collect()
}

/// One row of the recorded-state table: the rule the pull request carries, the calls
/// made against it, the log those calls must leave behind, and whether the pull
/// request can then be merged.
struct ApprovalRow {
    case: &'static str,
    rule: Option<String>,
    /// `(access key id, approvalState)` in the order the calls are made.
    calls: Vec<(&'static str, &'static str)>,
    /// `(access key id, approvalStatus)` of each logged change, oldest first.
    logged: Vec<(&'static str, &'static str)>,
    merges: bool,
}

#[tokio::test]
async fn an_approval_state_is_recorded_for_the_calling_identity_and_logged() {
    let reviewer_pool = vec![caller_arn(REVIEWER)];
    let foreign_pool = vec![caller_arn("AKIDNOBODYHOLDSTHIS")];

    // Rows differ along the state asked for, how many approvals the rule needs and
    // from which pool, how many reviewers call, and how one reviewer's calls repeat
    // and alternate.
    let rows = vec![
        ApprovalRow {
            case: "one approval on a rule needing one",
            rule: Some(document(1, &[])),
            calls: vec![(REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE)],
            merges: true,
        },
        ApprovalRow {
            case: "the approval taken back again",
            rule: Some(document(1, &[])),
            calls: vec![(REVIEWER, APPROVE), (REVIEWER, REVOKE)],
            logged: vec![(REVIEWER, APPROVE), (REVIEWER, REVOKE)],
            merges: false,
        },
        ApprovalRow {
            case: "the same approval asked for twice",
            rule: Some(document(1, &[])),
            calls: vec![(REVIEWER, APPROVE), (REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE)],
            merges: true,
        },
        ApprovalRow {
            case: "a revoke by a reviewer holding no approval",
            rule: Some(document(1, &[])),
            calls: vec![(REVIEWER, REVOKE)],
            logged: vec![],
            merges: false,
        },
        ApprovalRow {
            case: "approved, revoked and approved again",
            rule: Some(document(1, &[])),
            calls: vec![
                (REVIEWER, APPROVE),
                (REVIEWER, REVOKE),
                (REVIEWER, APPROVE),
            ],
            logged: vec![
                (REVIEWER, APPROVE),
                (REVIEWER, REVOKE),
                (REVIEWER, APPROVE),
            ],
            merges: true,
        },
        ApprovalRow {
            case: "one of the two approvals a rule needs",
            rule: Some(document(2, &[])),
            calls: vec![(REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE)],
            merges: false,
        },
        ApprovalRow {
            case: "both of the two approvals a rule needs",
            rule: Some(document(2, &[])),
            calls: vec![(REVIEWER, APPROVE), (OTHER_REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE), (OTHER_REVIEWER, APPROVE)],
            merges: true,
        },
        ApprovalRow {
            case: "two reviewers, one of whom took the approval back",
            rule: Some(document(2, &[])),
            calls: vec![
                (REVIEWER, APPROVE),
                (OTHER_REVIEWER, APPROVE),
                (REVIEWER, REVOKE),
            ],
            logged: vec![
                (REVIEWER, APPROVE),
                (OTHER_REVIEWER, APPROVE),
                (REVIEWER, REVOKE),
            ],
            merges: false,
        },
        ApprovalRow {
            case: "a pool that names the approving reviewer",
            rule: Some(document(1, &reviewer_pool)),
            calls: vec![(REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE)],
            merges: true,
        },
        ApprovalRow {
            case: "a pool the approving reviewer is not in",
            rule: Some(document(1, &foreign_pool)),
            calls: vec![(REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE)],
            merges: false,
        },
        ApprovalRow {
            case: "an approval on a pull request carrying no rule at all",
            rule: None,
            calls: vec![(REVIEWER, APPROVE)],
            logged: vec![(REVIEWER, APPROVE)],
            merges: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let fixture = fixture(row.rule.as_deref()).await;
        let revision = fixture.revision_id().await;

        for (caller, state) in &row.calls {
            let answered = ok_as(
                &fixture.router,
                caller,
                OPERATION,
                &fixture.body(&revision, state),
            )
            .await;
            let members = answered.as_object().map_or(0, serde_json::Map::len);
            assert_eq!(
                members, 0,
                "case {case:?}: {OPERATION} answers no output members, got {answered}"
            );
        }

        // An approval is not a revision of the pull request, so every call above
        // named the revision the pull request still holds.
        assert_eq!(
            fixture.revision_id().await,
            revision,
            "case {case:?}: recording an approval must not move the revision id"
        );

        let logged = fixture.approval_events().await;
        assert_eq!(
            logged.len(),
            row.logged.len(),
            "case {case:?}: expected {} logged change(s), got {logged:?}",
            row.logged.len()
        );
        for (event, (caller, status)) in logged.iter().zip(&row.logged) {
            assert_eq!(
                event.get("pullRequestEventType").and_then(Value::as_str),
                Some(APPROVAL_STATE_CHANGED),
                "case {case:?}: the log entry's type, got {event}"
            );
            assert_eq!(
                event.get("actorArn").and_then(Value::as_str),
                Some(caller_arn(caller).as_str()),
                "case {case:?}: the change is attributed to the calling identity, got {event}"
            );
            assert_eq!(
                event
                    .pointer("/approvalStateChangedEventMetadata/approvalStatus")
                    .and_then(Value::as_str),
                Some(*status),
                "case {case:?}: expected the state {status:?} logged, got {event}"
            );
            assert_eq!(
                event
                    .pointer("/approvalStateChangedEventMetadata/revisionId")
                    .and_then(Value::as_str),
                Some(revision.as_str()),
                "case {case:?}: the change names the revision it was given on, got {event}"
            );
        }

        // The approvals the pull request now holds are what its rules are counted
        // against: the merge goes through exactly when they meet every rule.
        let merge = json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": fixture.pull_request_id,
        });
        if row.merges {
            let merged = ok(&fixture.router, SQUASH_OPERATION, &merge).await;
            assert_eq!(
                string_at(&merged, "/pullRequest/pullRequestStatus"),
                STATUS_CLOSED,
                "case {case:?}: a merged pull request is closed, got {merged}"
            );
        } else {
            let (code, message) = refusal_as(
                &fixture.router,
                REVIEWER,
                SQUASH_OPERATION,
                &merge,
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "PullRequestApprovalRulesNotSatisfiedException",
                "case {case:?}: expected PullRequestApprovalRulesNotSatisfiedException, \
                 got {code:?}"
            );
            assert_eq!(
                message, RULES_NOT_SATISFIED,
                "case {case:?}: expected {RULES_NOT_SATISFIED:?}, got {message:?}"
            );
            assert_eq!(
                fixture.status().await,
                STATUS_OPEN,
                "case {case:?}: a refused merge leaves the pull request open"
            );
        }
    }
}

/// How a row names the pull request, the revision and the state: `None` leaves the
/// member out of the request altogether, which is a different request from one
/// carrying it empty.
struct Refusal {
    case: &'static str,
    caller: &'static str,
    /// `Some("{id}")` is the id of the pull request the fixture opened.
    pull_request_id: Option<&'static str>,
    /// `Some("{revision}")` is the current revision of that pull request.
    revision_id: Option<&'static str>,
    approval_state: Option<&'static str>,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_request_naming_no_pull_request_revision_or_state_records_nothing() {
    let fixture = fixture(Some(&document(1, &[]))).await;
    let revision = fixture.revision_id().await;
    // A SECOND pull request in the same repository, so the revision id of another
    // pull request is a row of the table rather than a value written out by hand.
    let other_revision = fixture.second_pull_request_revision().await;

    // Rows differ along which member names nothing an approval can be recorded
    // against: the pull request id's form and existence, the revision id's form and
    // which revision it names, the state's spelling, and the calling identity.
    let refusals = vec![
        Refusal {
            case: "no pullRequestId at all",
            caller: REVIEWER,
            pull_request_id: None,
            revision_id: Some("{revision}"),
            approval_state: Some(APPROVE),
            code: "PullRequestIdRequiredException",
            message: PULL_REQUEST_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty pullRequestId",
            caller: REVIEWER,
            pull_request_id: Some(""),
            revision_id: Some("{revision}"),
            approval_state: Some(APPROVE),
            code: "InvalidPullRequestIdException",
            message: empty_pull_request_id(""),
        },
        Refusal {
            case: "a pullRequestId that is not a decimal integer",
            caller: REVIEWER,
            pull_request_id: Some("not-an-id"),
            revision_id: Some("{revision}"),
            approval_state: Some(APPROVE),
            code: "InvalidPullRequestIdException",
            message: invalid_pull_request_id("not-an-id"),
        },
        Refusal {
            case: "a well-formed id nothing was minted for",
            caller: REVIEWER,
            pull_request_id: Some("99999"),
            revision_id: Some("{revision}"),
            approval_state: Some(APPROVE),
            code: "PullRequestDoesNotExistException",
            message: PULL_REQUEST_DOES_NOT_EXIST.to_string(),
        },
        Refusal {
            case: "no revisionId at all",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: None,
            approval_state: Some(APPROVE),
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty revisionId",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some(""),
            approval_state: Some(APPROVE),
            code: "InvalidRevisionIdException",
            message: EMPTY_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a revisionId that is not a digest at all",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some("not-a-revision-id"),
            approval_state: Some(APPROVE),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a digest one character short",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde",
            ),
            approval_state: Some(APPROVE),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a well-formed digest naming no revision of this pull request",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            approval_state: Some(APPROVE),
            code: "RevisionNotCurrentException",
            message: REVISION_NOT_CURRENT.to_string(),
        },
        Refusal {
            case: "the current revision of ANOTHER pull request",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some("{other}"),
            approval_state: Some(APPROVE),
            code: "RevisionNotCurrentException",
            message: REVISION_NOT_CURRENT.to_string(),
        },
        Refusal {
            case: "no approvalState at all",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            approval_state: None,
            code: "ApprovalStateRequiredException",
            message: APPROVAL_STATE_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty approvalState",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            approval_state: Some(""),
            code: "InvalidApprovalStateException",
            message: INVALID_APPROVAL_STATE.to_string(),
        },
        Refusal {
            case: "an approvalState in the wrong case",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            approval_state: Some("approve"),
            code: "InvalidApprovalStateException",
            message: INVALID_APPROVAL_STATE.to_string(),
        },
        Refusal {
            case: "an approvalState outside the catalog",
            caller: REVIEWER,
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            approval_state: Some("APPROVED"),
            code: "InvalidApprovalStateException",
            message: INVALID_APPROVAL_STATE.to_string(),
        },
        Refusal {
            case: "the author of the pull request approving it",
            caller: AUTHOR,
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            approval_state: Some(APPROVE),
            code: "PullRequestCannotBeApprovedByAuthorException",
            message: CANNOT_BE_APPROVED_BY_AUTHOR.to_string(),
        },
        Refusal {
            case: "the author of the pull request revoking on it",
            caller: AUTHOR,
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            approval_state: Some(REVOKE),
            code: "PullRequestCannotBeApprovedByAuthorException",
            message: CANNOT_BE_APPROVED_BY_AUTHOR.to_string(),
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
                "{other}" => other_revision.clone(),
                literal => literal.to_string(),
            });
        }
        if let Some(state) = refusal.approval_state {
            body["approvalState"] = json!(state);
        }

        let (code, message) = refusal_as(
            &fixture.router,
            refusal.caller,
            OPERATION,
            &body,
            CLIENT_ERROR_STATUS,
        )
        .await;
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

    // Nothing was recorded by any of them: the log holds no approval state change,
    // and the rule the pull request carries is still unmet.
    let logged = fixture.approval_events().await;
    assert!(
        logged.is_empty(),
        "a refused request records no approval state change, got {logged:?}"
    );
    let (code, _) = refusal_as(
        &fixture.router,
        REVIEWER,
        SQUASH_OPERATION,
        &json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": fixture.pull_request_id,
        }),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "PullRequestApprovalRulesNotSatisfiedException",
        "a refused request leaves the pull request's rules unmet, got {code:?}"
    );
}

#[tokio::test]
async fn a_pull_request_holds_no_more_approvals_than_the_published_maximum() {
    let fixture = fixture(Some(&document(1, &[]))).await;
    let revision = fixture.revision_id().await;

    // Fill the pull request to exactly the bound it enforces, read from the service
    // rather than written out here, each state recorded by a distinct identity.
    for index in 0..MAX_APPROVALS_PER_PULL_REQUEST {
        let reviewer = format!("AKIDREVIEWER{index}");
        fixture.hold(&reviewer);
        ok_as(
            &fixture.router,
            &reviewer,
            OPERATION,
            &fixture.body(&revision, APPROVE),
        )
        .await;
    }
    assert_eq!(
        fixture.approval_events().await.len(),
        MAX_APPROVALS_PER_PULL_REQUEST,
        "the pull request had to fill up to the bound"
    );

    // One further identity: the approval it asks for is the write the bound refuses,
    // while the revoke it asks for records nothing in the first place, so the pull
    // request answers it without storing anything.
    let one_too_many = "AKIDONETOOMANY";
    fixture.hold(one_too_many);
    let (code, message) = refusal_as(
        &fixture.router,
        one_too_many,
        OPERATION,
        &fixture.body(&revision, APPROVE),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "MaximumNumberOfApprovalsExceededException",
        "expected MaximumNumberOfApprovalsExceededException, got {code:?}"
    );
    assert!(
        message.starts_with(APPROVALS_EXCEEDED_SENTENCE),
        "expected a message opening {APPROVALS_EXCEEDED_SENTENCE:?}, got {message:?}"
    );
    assert!(
        message.contains(&MAX_APPROVALS_PER_PULL_REQUEST.to_string()),
        "the refusal must state the bound it enforces, got {message:?}"
    );
    ok_as(
        &fixture.router,
        one_too_many,
        OPERATION,
        &fixture.body(&revision, REVOKE),
    )
    .await;
    assert_eq!(
        fixture.approval_events().await.len(),
        MAX_APPROVALS_PER_PULL_REQUEST,
        "neither call records anything once the pull request is at the bound"
    );

    // A reviewer the pull request ALREADY holds a state for changes it at the bound:
    // the bound is on what the pull request stores, not on reviewing.
    ok_as(
        &fixture.router,
        "AKIDREVIEWER0",
        OPERATION,
        &fixture.body(&revision, REVOKE),
    )
    .await;
    assert_eq!(
        fixture.approval_events().await.len(),
        MAX_APPROVALS_PER_PULL_REQUEST + 1,
        "a reviewer already holding a state must be able to change it at the bound"
    );
}

#[tokio::test]
async fn a_pull_request_that_is_no_longer_open_records_no_approval_state() {
    let fixture = fixture(Some(&document(1, &[]))).await;
    let revision = fixture.revision_id().await;
    let closed = ok_as(
        &fixture.router,
        AUTHOR,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": fixture.pull_request_id,
            "pullRequestStatus": STATUS_CLOSED,
        }),
    )
    .await;
    assert_eq!(
        string_at(&closed, "/pullRequest/pullRequestStatus"),
        STATUS_CLOSED,
        "the pull request had to be closed first, got {closed}"
    );

    // Rows differ along the state asked for and the caller asking for it: neither is
    // recorded on a pull request that is no longer open.
    for (caller, state) in [
        (REVIEWER, APPROVE),
        (REVIEWER, REVOKE),
        (OTHER_REVIEWER, APPROVE),
    ] {
        let (code, message) = refusal_as(
            &fixture.router,
            caller,
            OPERATION,
            &fixture.body(&revision, state),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestAlreadyClosedException",
            "case {caller:?}/{state:?}: expected PullRequestAlreadyClosedException, got {code:?}"
        );
        assert_eq!(
            message, ALREADY_CLOSED,
            "case {caller:?}/{state:?}: expected {ALREADY_CLOSED:?}, got {message:?}"
        );
    }

    let logged = fixture.approval_events().await;
    assert!(
        logged.is_empty(),
        "a closed pull request records no approval state change, got {logged:?}"
    );
}

/// One row of the author table: who calls and with which state, and what the pull
/// request holds afterwards.
struct AuthorRow {
    case: &'static str,
    caller: &'static str,
    approval_state: &'static str,
    /// Whether the call is refused as one made by the pull request's own author.
    refused: bool,
    /// The access key ids of the reviewers the pull request holds an approval for
    /// once the call has been answered.
    approvers: Vec<&'static str>,
    /// How many approval state changes its event log holds by then.
    logged: usize,
}

#[tokio::test]
async fn the_author_of_a_pull_request_may_set_no_approval_state_on_it() {
    // Rows differ along the state asked for and whether the caller is the author of
    // the pull request: the author is refused for EITHER state, and either state
    // from a reviewer is answered.
    let rows = vec![
        AuthorRow {
            case: "the author approving",
            caller: AUTHOR,
            approval_state: APPROVE,
            refused: true,
            approvers: vec![OTHER_REVIEWER],
            logged: 1,
        },
        AuthorRow {
            case: "the author revoking",
            caller: AUTHOR,
            approval_state: REVOKE,
            refused: true,
            approvers: vec![OTHER_REVIEWER],
            logged: 1,
        },
        AuthorRow {
            case: "a reviewer approving",
            caller: REVIEWER,
            approval_state: APPROVE,
            refused: false,
            approvers: vec![OTHER_REVIEWER, REVIEWER],
            logged: 2,
        },
        AuthorRow {
            case: "a reviewer revoking an approval they never held",
            caller: REVIEWER,
            approval_state: REVOKE,
            refused: false,
            approvers: vec![OTHER_REVIEWER],
            logged: 1,
        },
        AuthorRow {
            case: "the reviewer whose approval is on record revoking it",
            caller: OTHER_REVIEWER,
            approval_state: REVOKE,
            refused: false,
            approvers: vec![],
            logged: 2,
        },
    ];

    for row in rows {
        let case = row.case;
        // The rule asks for more approvals than any row records, so no row can be
        // told apart by a merge going through instead.
        let fixture = fixture(Some(&document(3, &[]))).await;
        let revision = fixture.revision_id().await;
        // One approval already on record, so a refused call is seen to leave the
        // approvals the pull request holds exactly as they were.
        ok_as(
            &fixture.router,
            OTHER_REVIEWER,
            OPERATION,
            &fixture.body(&revision, APPROVE),
        )
        .await;

        let body = fixture.body(&revision, row.approval_state);
        if row.refused {
            let (code, message) = refusal_as(
                &fixture.router,
                row.caller,
                OPERATION,
                &body,
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "PullRequestCannotBeApprovedByAuthorException",
                "case {case:?}: expected PullRequestCannotBeApprovedByAuthorException, \
                 got {code:?}"
            );
            assert_eq!(
                message, CANNOT_BE_APPROVED_BY_AUTHOR,
                "case {case:?}: expected {CANNOT_BE_APPROVED_BY_AUTHOR:?}, got {message:?}"
            );
        } else {
            let answered = ok_as(&fixture.router, row.caller, OPERATION, &body).await;
            let members = answered.as_object().map_or(0, serde_json::Map::len);
            assert_eq!(
                members, 0,
                "case {case:?}: {OPERATION} answers no output members, got {answered}"
            );
        }

        // What the pull request holds afterwards: a refused call records nothing, so
        // neither the approvals nor the event log moved for it.
        let mut expected: Vec<String> = row.approvers.iter().copied().map(caller_arn).collect();
        expected.sort();
        let reported = approver_arns(&fixture.router, &fixture.pull_request_id, &revision).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the approvals {expected:?}, got {reported:?}"
        );
        let logged = fixture.approval_events().await;
        assert_eq!(
            logged.len(),
            row.logged,
            "case {case:?}: expected {} logged change(s), got {logged:?}",
            row.logged
        );
    }
}

#[tokio::test]
async fn an_identity_may_set_an_approval_state_on_a_pull_request_it_did_not_open() {
    let fixture = fixture(Some(&document(1, &[]))).await;
    // A SECOND pull request in the same repository, opened by the REVIEWER: whose
    // approval is refused is decided per pull request, against the identity that
    // opened THAT one, so the author of the first is a reviewer of the second and
    // the other way round.
    let (other_id, other_revision) = fixture
        .pull_request_opened_by(REVIEWER, SECOND_SOURCE)
        .await;

    for state in [APPROVE, REVOKE] {
        let body = json!({
            "pullRequestId": other_id,
            "revisionId": other_revision,
            "approvalState": state,
        });
        let answered = ok_as(&fixture.router, AUTHOR, OPERATION, &body).await;
        let members = answered.as_object().map_or(0, serde_json::Map::len);
        assert_eq!(
            members, 0,
            "case {state:?}: {OPERATION} answers no output members, got {answered}"
        );
        // The state the author of the OTHER pull request just set is what this one
        // holds: an approval while it stands, none once taken back.
        let expected: Vec<String> = if state == APPROVE {
            vec![caller_arn(AUTHOR)]
        } else {
            vec![]
        };
        let reported = approver_arns(&fixture.router, &other_id, &other_revision).await;
        assert_eq!(
            reported, expected,
            "case {state:?}: expected the approvals {expected:?}, got {reported:?}"
        );

        // The SAME call by the identity that opened it is refused, whichever state
        // it asks for.
        let (code, message) =
            refusal_as(&fixture.router, REVIEWER, OPERATION, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "PullRequestCannotBeApprovedByAuthorException",
            "case {state:?}: expected PullRequestCannotBeApprovedByAuthorException, got {code:?}"
        );
        assert_eq!(
            message, CANNOT_BE_APPROVED_BY_AUTHOR,
            "case {state:?}: expected {CANNOT_BE_APPROVED_BY_AUTHOR:?}, got {message:?}"
        );
    }
}
