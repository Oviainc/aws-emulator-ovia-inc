// SPDX-License-Identifier: CC0-1.0

//! Setting a pull request's approval rule requirements aside, over the wire:
//! `OverridePullRequestApprovalRules`, driven as awsJson1_1 requests through the
//! router that serves the real emulator (the wire harness comes from the sibling
//! `repository_lifecycle` suite, and calling as a NAMED principal from
//! `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: the call records on the pull request's current revision
//! whether its approval rule requirements are set aside — `OVERRIDE` sets them
//! aside, `REVOKE` holds them again and stores no override — so a merge of it goes
//! through while a rule is unmet, a read of its override state reports that the
//! rules are set aside and which identity set them aside, and an evaluation of its
//! rules reports it on `overridden` while still reporting those rules as the unmet
//! rules they are. Each change appends one
//! `PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN` entry to the pull request's event
//! log naming the calling identity, the revision and the status asked for, while a
//! call asking for the state the pull request already holds either changes nothing
//! (`REVOKE` with no override in place) or is refused as already set (`OVERRIDE` on
//! rules already set aside). An override is not a revision of the pull request, so
//! the revision id does not move.
//!
//! The tables below span what that rule generalises over: both values of
//! `OverrideStatus` (`smithy_query(query="shape",shape="OverrideStatus")`),
//! repeated and alternating calls, the identity making them (a reviewer, a second
//! reviewer, the author of the pull request), the rules the pull request carries
//! (none, one, two, met and unmet, with and without an approval pool), every member
//! that can name no pull request, no revision and no status, and a pull request
//! that is no longer open.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, ok_as, refusal_as};

/// The operation under test, and the readers an override is read back through.
const OPERATION: &str = "OverridePullRequestApprovalRules";
const OVERRIDE_STATE_OPERATION: &str = "GetPullRequestOverrideState";
const EVALUATE_OPERATION: &str = "EvaluatePullRequestApprovalRules";
const APPROVAL_STATE_OPERATION: &str = "UpdatePullRequestApprovalState";
const SQUASH_OPERATION: &str = "MergePullRequestBySquash";
const FAST_FORWARD_OPERATION: &str = "MergePullRequestByFastForward";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// Both values of `OverrideStatus`
/// (`smithy_query(query="shape",shape="OverrideStatus")`).
const OVERRIDE: &str = "OVERRIDE";
const REVOKE: &str = "REVOKE";

/// The `ApprovalState` value a recorded approval holds
/// (`smithy_query(query="shape",shape="ApprovalState")`).
const APPROVE: &str = "APPROVE";

/// The `PullRequestEventType` value an override is logged under, and the
/// `PullRequestStatusEnum` values a pull request holds
/// (`smithy_query(query="shape",shape="PullRequestEventType")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const APPROVAL_RULE_OVERRIDDEN: &str = "PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN";
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The principals the calls below are made by. A caller is identified by the access
/// key id its request is signed with, so an override a call records is attributed to
/// that caller's ARN.
const AUTHOR: &str = "AKIDAUTHOR";
const OVERRIDER: &str = "AKIDOVERRIDER";
const OTHER_OVERRIDER: &str = "AKIDOTHEROVERRIDER";
const REVIEWER: &str = "AKIDREVIEWER";

const REPOSITORY: &str = "approval-rule-override";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";
/// The source branch of the second pull request
/// ([`Fixture::second_pull_request_revision`]).
const SECOND_SOURCE: &str = "second-feature";
const RULE: &str = "needs-review";
const SECOND_RULE: &str = "needs-another-review";

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
const REVISION_NOT_CURRENT: &str = "The revision ID provided in the request does not match the \
                                    current revision ID. Use GetPullRequest to retrieve the \
                                    current revision ID.";
const OVERRIDE_STATUS_REQUIRED: &str = "An override status is required, but no value was \
                                        provided. Valid values include OVERRIDE and REVOKE.";
const INVALID_OVERRIDE_STATUS: &str =
    "The override status is not valid. Valid statuses are OVERRIDE and REVOKE.";
const OVERRIDE_ALREADY_SET: &str =
    "The pull request has already had its approval rules set to override.";
const ALREADY_CLOSED: &str = "The pull request status cannot be updated because it is already \
                              closed.";
const RULES_NOT_SATISFIED: &str = "You cannot merge the pull request because the requirements for \
                                   one or more approval rules have not been met. For details \
                                   about the approval rules that still have requirements, use \
                                   EvaluatePullRequestApprovalRules.";

/// `PullRequestDoesNotExistException`'s sentence pair, built from the id the refused
/// request named.
fn does_not_exist(pull_request_id: &str) -> String {
    format!(
        "The pull request ID {pull_request_id} could not be found. Make sure that you have \
         specified the correct repository name and pull request ID, and then try again."
    )
}

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
/// request between them, opened by [`AUTHOR`] and carrying the approval rules the
/// case asked for.
struct Fixture {
    router: MultiServiceRouter,
    pull_request_id: String,
}

async fn fixture(rules: &[(&str, String)]) -> Fixture {
    let (router, _engine) =
        emulator_with_callers(&[AUTHOR, OVERRIDER, OTHER_OVERRIDER, REVIEWER]);
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
    for (name, content) in rules {
        ok_as(
            &router,
            AUTHOR,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": pull_request_id,
                "approvalRuleName": name,
                "approvalRuleContent": content,
            }),
        )
        .await;
    }
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

    /// The `pullRequestStatus` the pull request holds.
    async fn status(&self) -> String {
        string_at(&self.stored().await, "/pullRequestStatus")
    }

    /// The `evaluation` of the pull request's approval rules at its current
    /// revision: the reader an override is read back through.
    async fn evaluation(&self) -> Value {
        let revision_id = self.revision_id().await;
        let read = ok(
            &self.router,
            EVALUATE_OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id,
                "revisionId": revision_id,
            }),
        )
        .await;
        read.get("evaluation")
            .cloned()
            .unwrap_or_else(|| panic!("no evaluation in {read}"))
    }

    /// The override state `GetPullRequestOverrideState` reports for the pull
    /// request's current revision: the dedicated reader of the fact this operation
    /// records.
    async fn override_state(&self) -> Value {
        let revision_id = self.revision_id().await;
        ok(
            &self.router,
            OVERRIDE_STATE_OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id,
                "revisionId": revision_id,
            }),
        )
        .await
    }

    /// The override changes the pull request's event log holds, oldest first.
    async fn override_events(&self) -> Vec<Value> {
        let read = ok(
            &self.router,
            "DescribePullRequestEvents",
            &json!({
                "pullRequestId": self.pull_request_id,
                "pullRequestEventType": APPROVAL_RULE_OVERRIDDEN,
            }),
        )
        .await;
        read.get("pullRequestEvents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("no pullRequestEvents in {read}"))
    }

    /// Record an approval of the current revision by `access_key_id`.
    async fn record_approval(&self, access_key_id: &str) {
        let revision_id = self.revision_id().await;
        ok_as(
            &self.router,
            access_key_id,
            APPROVAL_STATE_OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id,
                "revisionId": revision_id,
                "approvalState": APPROVE,
            }),
        )
        .await;
    }

    fn body(&self, revision_id: &str, override_status: &str) -> Value {
        json!({
            "pullRequestId": self.pull_request_id,
            "revisionId": revision_id,
            "overrideStatus": override_status,
        })
    }

    /// Open a SECOND pull request in this repository, between another source branch
    /// and the same destination, and report ITS current revision id — a well-formed
    /// revision id that names no revision of the fixture's pull request.
    async fn second_pull_request_revision(&self) -> String {
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

fn flag(evaluation: &Value, member: &str) -> bool {
    evaluation
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("no {member} in {evaluation}"))
}

/// One row of the recorded-override table: the rules the pull request carries, the
/// approvals recorded on it, the calls made against it, the log those calls must
/// leave behind, and what the pull request then reports and allows.
struct OverrideRow {
    case: &'static str,
    /// The approval rules applied to the pull request, in the order they are applied.
    rules: Vec<(&'static str, String)>,
    /// The access key ids whose approvals are recorded before the calls are made.
    approvals: Vec<&'static str>,
    /// `(access key id, overrideStatus)` in the order the calls are made.
    calls: Vec<(&'static str, &'static str)>,
    /// `(access key id, overrideStatus)` of each logged change, oldest first.
    logged: Vec<(&'static str, &'static str)>,
    /// Whether the rules are set aside once every call has been answered.
    overridden: bool,
    /// Whether the approvals the pull request holds meet every rule applied to it:
    /// what the evaluation reports as `approved`, which an override does not change.
    approved: bool,
    /// Whether the pull request can then be merged.
    merges: bool,
}

#[tokio::test]
async fn an_override_is_recorded_for_the_calling_identity_and_logged() {
    let reviewer_pool = vec![caller_arn(REVIEWER)];
    let foreign_pool = vec![caller_arn("AKIDNOBODYHOLDSTHIS")];
    let one = || (RULE, document(1, &[]));

    // Rows differ along the status asked for, how the calls repeat and alternate,
    // which identity makes them, and the rules and approvals the pull request
    // carries while they are made.
    let rows = vec![
        OverrideRow {
            case: "an unmet rule set aside",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![(OVERRIDER, OVERRIDE)],
            logged: vec![(OVERRIDER, OVERRIDE)],
            overridden: true,
            approved: false,
            merges: true,
        },
        OverrideRow {
            case: "the override taken back again",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![(OVERRIDER, OVERRIDE), (OVERRIDER, REVOKE)],
            logged: vec![(OVERRIDER, OVERRIDE), (OVERRIDER, REVOKE)],
            overridden: false,
            approved: false,
            merges: false,
        },
        OverrideRow {
            case: "a revoke with no override in place",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![(OVERRIDER, REVOKE)],
            logged: vec![],
            overridden: false,
            approved: false,
            merges: false,
        },
        OverrideRow {
            case: "the same revoke asked for twice with no override in place",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![(OVERRIDER, REVOKE), (OVERRIDER, REVOKE)],
            logged: vec![],
            overridden: false,
            approved: false,
            merges: false,
        },
        OverrideRow {
            case: "set aside, held again and set aside once more",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![
                (OVERRIDER, OVERRIDE),
                (OVERRIDER, REVOKE),
                (OVERRIDER, OVERRIDE),
            ],
            logged: vec![
                (OVERRIDER, OVERRIDE),
                (OVERRIDER, REVOKE),
                (OVERRIDER, OVERRIDE),
            ],
            overridden: true,
            approved: false,
            merges: true,
        },
        OverrideRow {
            case: "one identity sets the rules aside and another holds them again",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![(OVERRIDER, OVERRIDE), (OTHER_OVERRIDER, REVOKE)],
            logged: vec![(OVERRIDER, OVERRIDE), (OTHER_OVERRIDER, REVOKE)],
            overridden: false,
            approved: false,
            merges: false,
        },
        OverrideRow {
            case: "the author of the pull request setting its rules aside",
            rules: vec![one()],
            approvals: vec![],
            calls: vec![(AUTHOR, OVERRIDE)],
            logged: vec![(AUTHOR, OVERRIDE)],
            overridden: true,
            approved: false,
            merges: true,
        },
        OverrideRow {
            case: "two rules set aside at once",
            rules: vec![one(), (SECOND_RULE, document(2, &[]))],
            approvals: vec![],
            calls: vec![(OVERRIDER, OVERRIDE)],
            logged: vec![(OVERRIDER, OVERRIDE)],
            overridden: true,
            approved: false,
            merges: true,
        },
        OverrideRow {
            case: "a rule whose pool the only approver is not in",
            rules: vec![(RULE, document(1, &foreign_pool))],
            approvals: vec![REVIEWER],
            calls: vec![(OVERRIDER, OVERRIDE)],
            logged: vec![(OVERRIDER, OVERRIDE)],
            overridden: true,
            approved: false,
            merges: true,
        },
        OverrideRow {
            case: "a rule the recorded approval already meets",
            rules: vec![(RULE, document(1, &reviewer_pool))],
            approvals: vec![REVIEWER],
            calls: vec![(OVERRIDER, OVERRIDE)],
            logged: vec![(OVERRIDER, OVERRIDE)],
            overridden: true,
            approved: true,
            merges: true,
        },
        OverrideRow {
            case: "a pull request carrying no rule at all",
            rules: vec![],
            approvals: vec![],
            calls: vec![(OVERRIDER, OVERRIDE)],
            logged: vec![(OVERRIDER, OVERRIDE)],
            overridden: true,
            approved: true,
            merges: true,
        },
        OverrideRow {
            case: "a pull request carrying no rule, overridden and revoked",
            rules: vec![],
            approvals: vec![],
            calls: vec![(OVERRIDER, OVERRIDE), (OVERRIDER, REVOKE)],
            logged: vec![(OVERRIDER, OVERRIDE), (OVERRIDER, REVOKE)],
            overridden: false,
            approved: true,
            merges: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let fixture = fixture(&row.rules).await;
        for reviewer in &row.approvals {
            fixture.record_approval(reviewer).await;
        }
        let revision = fixture.revision_id().await;

        for (caller, status) in &row.calls {
            let answered = ok_as(
                &fixture.router,
                caller,
                OPERATION,
                &fixture.body(&revision, status),
            )
            .await;
            let members = answered.as_object().map_or(0, serde_json::Map::len);
            assert_eq!(
                members, 0,
                "case {case:?}: {OPERATION} answers no output members, got {answered}"
            );
        }

        // An override is not a revision of the pull request, so every call above
        // named the revision the pull request still holds.
        assert_eq!(
            fixture.revision_id().await,
            revision,
            "case {case:?}: an override must not move the revision id"
        );

        // The one stored fact, read back through the dedicated reader of the
        // override state and through the evaluation of the pull request's rules:
        // both report the override the pull request HOLDS, and the reader names the
        // identity that set the rules aside.
        let read = fixture.override_state().await;
        assert_eq!(
            read.get("overridden").and_then(Value::as_bool),
            Some(row.overridden),
            "case {case:?}: expected overridden {}, got {read}",
            row.overridden
        );
        let expected_overrider = row.overridden.then(|| {
            let (caller, _) = row
                .logged
                .iter()
                .rfind(|(_, status)| *status == OVERRIDE)
                .expect("an overridden pull request logged the call that set it aside");
            caller_arn(caller)
        });
        assert_eq!(
            read.get("overrider").and_then(Value::as_str),
            expected_overrider.as_deref(),
            "case {case:?}: expected the overrider {expected_overrider:?}, got {read}"
        );

        let evaluation = fixture.evaluation().await;
        assert_eq!(
            flag(&evaluation, "overridden"),
            row.overridden,
            "case {case:?}: expected overridden {}, got {evaluation}",
            row.overridden
        );
        assert_eq!(
            flag(&evaluation, "approved"),
            row.approved,
            "case {case:?}: expected approved {}, got {evaluation}",
            row.approved
        );

        let logged = fixture.override_events().await;
        assert_eq!(
            logged.len(),
            row.logged.len(),
            "case {case:?}: expected {} logged change(s), got {logged:?}",
            row.logged.len()
        );
        for (event, (caller, status)) in logged.iter().zip(&row.logged) {
            assert_eq!(
                event.get("pullRequestEventType").and_then(Value::as_str),
                Some(APPROVAL_RULE_OVERRIDDEN),
                "case {case:?}: the log entry's type, got {event}"
            );
            assert_eq!(
                event.get("actorArn").and_then(Value::as_str),
                Some(caller_arn(caller).as_str()),
                "case {case:?}: the change is attributed to the calling identity, got {event}"
            );
            assert_eq!(
                event
                    .pointer("/approvalRuleOverriddenEventMetadata/overrideStatus")
                    .and_then(Value::as_str),
                Some(*status),
                "case {case:?}: expected the status {status:?} logged, got {event}"
            );
            assert_eq!(
                event
                    .pointer("/approvalRuleOverriddenEventMetadata/revisionId")
                    .and_then(Value::as_str),
                Some(revision.as_str()),
                "case {case:?}: the change names the revision it was given on, got {event}"
            );
        }

        // What the pull request now holds is what a merge of it reads: it goes
        // through exactly when every rule is met or set aside.
        let merge = json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": fixture.pull_request_id,
        });
        if row.merges {
            let merged = ok_as(&fixture.router, OVERRIDER, SQUASH_OPERATION, &merge).await;
            assert_eq!(
                string_at(&merged, "/pullRequest/pullRequestStatus"),
                STATUS_CLOSED,
                "case {case:?}: a merged pull request is closed, got {merged}"
            );
        } else {
            let (code, message) = refusal_as(
                &fixture.router,
                OVERRIDER,
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

/// How a row names the pull request, the revision and the status: `None` leaves the
/// member out of the request altogether, which is a different request from one
/// carrying it empty.
struct Refusal {
    case: &'static str,
    /// `Some("{id}")` is the id of the pull request the fixture opened.
    pull_request_id: Option<&'static str>,
    /// `Some("{revision}")` is the current revision of that pull request, and
    /// `Some("{other}")` the current revision of another one.
    revision_id: Option<&'static str>,
    override_status: Option<&'static str>,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_request_naming_no_pull_request_revision_or_status_overrides_nothing() {
    let fixture = fixture(&[(RULE, document(1, &[]))]).await;
    let revision = fixture.revision_id().await;
    // A SECOND pull request in the same repository, so the revision id of another
    // pull request is a row of the table rather than a value written out by hand.
    let other_revision = fixture.second_pull_request_revision().await;

    // Rows differ along which member names nothing an override can be recorded
    // against: the pull request id's form and existence, the revision id's form and
    // which revision it names, and the status's spelling.
    let refusals = vec![
        Refusal {
            case: "no pullRequestId at all",
            pull_request_id: None,
            revision_id: Some("{revision}"),
            override_status: Some(OVERRIDE),
            code: "PullRequestIdRequiredException",
            message: PULL_REQUEST_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty pullRequestId",
            pull_request_id: Some(""),
            revision_id: Some("{revision}"),
            override_status: Some(OVERRIDE),
            code: "PullRequestIdRequiredException",
            message: PULL_REQUEST_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "a pullRequestId that is not a decimal integer",
            pull_request_id: Some("not-an-id"),
            revision_id: Some("{revision}"),
            override_status: Some(OVERRIDE),
            code: "InvalidPullRequestIdException",
            message: INVALID_PULL_REQUEST_ID.to_string(),
        },
        Refusal {
            case: "a well-formed id nothing was minted for",
            pull_request_id: Some("99999"),
            revision_id: Some("{revision}"),
            override_status: Some(OVERRIDE),
            code: "PullRequestDoesNotExistException",
            message: does_not_exist("99999"),
        },
        Refusal {
            case: "no revisionId at all",
            pull_request_id: Some("{id}"),
            revision_id: None,
            override_status: Some(OVERRIDE),
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty revisionId",
            pull_request_id: Some("{id}"),
            revision_id: Some(""),
            override_status: Some(OVERRIDE),
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        Refusal {
            case: "a revisionId that is not a digest at all",
            pull_request_id: Some("{id}"),
            revision_id: Some("not-a-revision-id"),
            override_status: Some(OVERRIDE),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a digest one character short",
            pull_request_id: Some("{id}"),
            revision_id: Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde"),
            override_status: Some(OVERRIDE),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            case: "a well-formed digest naming no revision of this pull request",
            pull_request_id: Some("{id}"),
            revision_id: Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            override_status: Some(OVERRIDE),
            code: "RevisionNotCurrentException",
            message: REVISION_NOT_CURRENT.to_string(),
        },
        Refusal {
            case: "the current revision of ANOTHER pull request",
            pull_request_id: Some("{id}"),
            revision_id: Some("{other}"),
            override_status: Some(OVERRIDE),
            code: "RevisionNotCurrentException",
            message: REVISION_NOT_CURRENT.to_string(),
        },
        Refusal {
            case: "no overrideStatus at all",
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            override_status: None,
            code: "OverrideStatusRequiredException",
            message: OVERRIDE_STATUS_REQUIRED.to_string(),
        },
        Refusal {
            case: "an empty overrideStatus",
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            override_status: Some(""),
            code: "InvalidOverrideStatusException",
            message: INVALID_OVERRIDE_STATUS.to_string(),
        },
        Refusal {
            case: "an overrideStatus in the wrong case",
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            override_status: Some("override"),
            code: "InvalidOverrideStatusException",
            message: INVALID_OVERRIDE_STATUS.to_string(),
        },
        Refusal {
            case: "an overrideStatus outside the catalog",
            pull_request_id: Some("{id}"),
            revision_id: Some("{revision}"),
            override_status: Some("OVERRIDDEN"),
            code: "InvalidOverrideStatusException",
            message: INVALID_OVERRIDE_STATUS.to_string(),
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
        if let Some(status) = refusal.override_status {
            body["overrideStatus"] = json!(status);
        }

        let (code, message) = refusal_as(
            &fixture.router,
            OVERRIDER,
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

    // Nothing was recorded by any of them: the log holds no override, the evaluation
    // reports none, and the rule the pull request carries is still unmet.
    let logged = fixture.override_events().await;
    assert!(
        logged.is_empty(),
        "a refused request records no override, got {logged:?}"
    );
    let evaluation = fixture.evaluation().await;
    assert!(
        !flag(&evaluation, "overridden") && !flag(&evaluation, "approved"),
        "a refused request leaves the rules in force, got {evaluation}"
    );
    let (code, _) = refusal_as(
        &fixture.router,
        OVERRIDER,
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
async fn rules_already_set_aside_are_not_set_aside_a_second_time() {
    let fixture = fixture(&[(RULE, document(1, &[]))]).await;
    let revision = fixture.revision_id().await;
    ok_as(
        &fixture.router,
        OVERRIDER,
        OPERATION,
        &fixture.body(&revision, OVERRIDE),
    )
    .await;

    // Rows differ along the identity asking for the override already in place: the
    // one that set the rules aside, another, and the author of the pull request.
    for caller in [OVERRIDER, OTHER_OVERRIDER, AUTHOR] {
        let (code, message) = refusal_as(
            &fixture.router,
            caller,
            OPERATION,
            &fixture.body(&revision, OVERRIDE),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "OverrideAlreadySetException",
            "case {caller:?}: expected OverrideAlreadySetException, got {code:?}"
        );
        assert_eq!(
            message, OVERRIDE_ALREADY_SET,
            "case {caller:?}: expected {OVERRIDE_ALREADY_SET:?}, got {message:?}"
        );
    }

    // The refused calls recorded nothing: the one override that was asked for is
    // still the only entry in the log, and the rules are still set aside.
    let logged = fixture.override_events().await;
    assert_eq!(
        logged.len(),
        1,
        "a refused override records nothing, got {logged:?}"
    );
    let read = fixture.override_state().await;
    assert_eq!(
        read.get("overrider").and_then(Value::as_str),
        Some(caller_arn(OVERRIDER).as_str()),
        "the identity that set the rules aside still holds the override, got {read}"
    );
    let evaluation = fixture.evaluation().await;
    assert!(
        flag(&evaluation, "overridden") && !flag(&evaluation, "approved"),
        "the override asked for is still in place over a rule still unmet, got {evaluation}"
    );

    // Holding the requirements again, then asking for that same state once more: the
    // second call is answered and records nothing, because it asks for the state the
    // pull request already holds.
    ok_as(
        &fixture.router,
        OVERRIDER,
        OPERATION,
        &fixture.body(&revision, REVOKE),
    )
    .await;
    ok_as(
        &fixture.router,
        OVERRIDER,
        OPERATION,
        &fixture.body(&revision, REVOKE),
    )
    .await;
    let logged = fixture.override_events().await;
    assert_eq!(
        logged.len(),
        2,
        "only the change that happened is logged, got {logged:?}"
    );
    let evaluation = fixture.evaluation().await;
    assert!(
        !flag(&evaluation, "overridden") && !flag(&evaluation, "approved"),
        "the revoked override leaves the rules in force, got {evaluation}"
    );

    // Set the rules aside once more and merge by FAST FORWARD: every merge of a pull
    // request reads the same stored override, so the strategy the caller picks does
    // not change which pull requests it lets through.
    ok_as(
        &fixture.router,
        OTHER_OVERRIDER,
        OPERATION,
        &fixture.body(&revision, OVERRIDE),
    )
    .await;
    let merged = ok_as(
        &fixture.router,
        OTHER_OVERRIDER,
        FAST_FORWARD_OPERATION,
        &json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": fixture.pull_request_id,
        }),
    )
    .await;
    assert_eq!(
        string_at(&merged, "/pullRequest/pullRequestStatus"),
        STATUS_CLOSED,
        "a merged pull request is closed, got {merged}"
    );
}

#[tokio::test]
async fn a_pull_request_that_is_no_longer_open_records_no_override() {
    let fixture = fixture(&[(RULE, document(1, &[]))]).await;
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

    // Rows differ along the status asked for and the caller asking for it: neither
    // is recorded on a pull request that is no longer open.
    for (caller, status) in [
        (OVERRIDER, OVERRIDE),
        (OVERRIDER, REVOKE),
        (AUTHOR, OVERRIDE),
        (OTHER_OVERRIDER, REVOKE),
    ] {
        let (code, message) = refusal_as(
            &fixture.router,
            caller,
            OPERATION,
            &fixture.body(&revision, status),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestAlreadyClosedException",
            "case {caller:?}/{status:?}: expected PullRequestAlreadyClosedException, got {code:?}"
        );
        assert_eq!(
            message, ALREADY_CLOSED,
            "case {caller:?}/{status:?}: expected {ALREADY_CLOSED:?}, got {message:?}"
        );
    }

    let logged = fixture.override_events().await;
    assert!(
        logged.is_empty(),
        "a closed pull request records no override, got {logged:?}"
    );
}

/// One row of the override-state reader's table: how it names the pull request and
/// the revision, and the refusal that naming is answered with.
struct ReadRefusal {
    case: &'static str,
    pull_request_id: Option<&'static str>,
    revision_id: Option<&'static str>,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_read_of_the_override_state_names_a_pull_request_and_a_revision() {
    let fixture = fixture(&[(RULE, document(1, &[]))]).await;
    let revision = fixture.revision_id().await;

    // Nothing has set the rules aside yet, so the reader reports requirements in
    // force and names no overrider.
    let read = fixture.override_state().await;
    assert_eq!(
        read.get("overridden").and_then(Value::as_bool),
        Some(false),
        "expected overridden false, got {read}"
    );
    assert_eq!(
        read.get("overrider").and_then(Value::as_str),
        None,
        "a pull request whose requirements apply names no overrider, got {read}"
    );

    // Rows differ along which member names nothing an override state can be read
    // for: the pull request id's form and existence, and the revision id's form.
    let refusals = vec![
        ReadRefusal {
            case: "no pullRequestId at all",
            pull_request_id: None,
            revision_id: Some("{revision}"),
            code: "PullRequestIdRequiredException",
            message: PULL_REQUEST_ID_REQUIRED.to_string(),
        },
        ReadRefusal {
            case: "a pullRequestId that is not a decimal integer",
            pull_request_id: Some("not-an-id"),
            revision_id: Some("{revision}"),
            code: "InvalidPullRequestIdException",
            message: INVALID_PULL_REQUEST_ID.to_string(),
        },
        ReadRefusal {
            case: "a well-formed id nothing was minted for",
            pull_request_id: Some("99999"),
            revision_id: Some("{revision}"),
            code: "PullRequestDoesNotExistException",
            message: does_not_exist("99999"),
        },
        ReadRefusal {
            case: "no revisionId at all",
            pull_request_id: Some("{id}"),
            revision_id: None,
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        ReadRefusal {
            case: "an empty revisionId",
            pull_request_id: Some("{id}"),
            revision_id: Some(""),
            code: "RevisionIdRequiredException",
            message: REVISION_ID_REQUIRED.to_string(),
        },
        ReadRefusal {
            case: "a revisionId that is not a digest at all",
            pull_request_id: Some("{id}"),
            revision_id: Some("not-a-revision-id"),
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
        let (code, message) = refusal_as(
            &fixture.router,
            OVERRIDER,
            OVERRIDE_STATE_OPERATION,
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

    // Once the rules are set aside, the reader names the identity that set them
    // aside, for the revision the override was given on.
    ok_as(
        &fixture.router,
        OVERRIDER,
        OPERATION,
        &fixture.body(&revision, OVERRIDE),
    )
    .await;
    let read = fixture.override_state().await;
    assert_eq!(
        read.get("overridden").and_then(Value::as_bool),
        Some(true),
        "expected overridden true, got {read}"
    );
    assert_eq!(
        read.get("overrider").and_then(Value::as_str),
        Some(caller_arn(OVERRIDER).as_str()),
        "expected the identity that set the rules aside, got {read}"
    );
}
