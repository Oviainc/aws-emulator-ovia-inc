// SPDX-License-Identifier: CC0-1.0

//! `EvaluatePullRequestApprovalRules` over the wire, driven as awsJson1_1 requests
//! through the router that serves the real emulator (the wire harness comes from the
//! sibling `repository_lifecycle` suite, and calling as a NAMED principal from
//! `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: the evaluation of a pull request's CURRENT revision reports
//! each approval rule applied to it under `approvalRulesSatisfied` when the approvals
//! the pull request holds meet that rule's every statement — at least
//! `NumberOfApprovalsNeeded` approvers, drawn from `ApprovalPoolMembers` where the
//! statement names a pool — and under `approvalRulesNotSatisfied` otherwise;
//! `approved` is true exactly when no rule is left unmet, and `overridden` is the
//! pull request's override state, which is reported on its own member and leaves the
//! rules it sets aside reported as the unmet rules they still are. The evaluation
//! is a report, so asking for it leaves the pull request as the next caller found it.
//!
//! The table below spans what that rule generalises over: the number of rules applied
//! and the order they were applied in, how many approvals a statement asks for, how
//! many statements a document carries, a statement with no pool and one whose pool
//! names an identity exactly or through the wildcard CodeCommit's pool members allow,
//! approvers inside and outside that pool, both values of `ApprovalState`
//! (`smithy_query(query="shape",shape="ApprovalState")`) including an approval that
//! was later revoked, and a pull request carrying no rules at all. The refusals span
//! every member that can name no pull request and no revision.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::harness::{
    empty_pull_request_id, error_with_message, invalid_pull_request_id, EMPTY_REVISION_ID,
    INVALID_REVISION_ID, PULL_REQUEST_DOES_NOT_EXIST,
};
use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, fresh_client_request_token, ok_as};

/// The operation under test.
const OPERATION: &str = "EvaluatePullRequestApprovalRules";
const APPROVAL_STATE_OPERATION: &str = "UpdatePullRequestApprovalState";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// Both values of `ApprovalState`
/// (`smithy_query(query="shape",shape="ApprovalState")`).
const APPROVE: &str = "APPROVE";
const REVOKE: &str = "REVOKE";

const REPOSITORY: &str = "evaluation";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// The principals the calls below are made by. A caller is identified by the access
/// key id its request is signed with, so the approval a call records belongs to that
/// caller's ARN — and the author of a pull request cannot approve its own.
const AUTHOR: &str = "AKIDAUTHOR";
const ONE: &str = "AKIDONE";
const TWO: &str = "AKIDTWO";
const THREE: &str = "AKIDTHREE";

/// The sentences the refused calls carry, written out here rather than read from the
/// service, so each refusal is checked against the words CodeCommit answers that
/// error class with.
/// The wording for a `pullRequestId` or a `revisionId` naming nothing is
/// `crate::harness`'s, so every suite checks the same words.
const REVISION_NOT_CURRENT: &str = "The revision ID provided in the request does not match the \
                                    current revision ID. Use GetPullRequest to retrieve the \
                                    current revision ID.";

/// One `Approvers` statement: how many approvals it asks for and, where it names
/// one, the pool the approvals must come from.
struct Statement {
    approvals: u32,
    pool: Vec<String>,
}

fn statement(approvals: u32, pool: &[String]) -> Statement {
    Statement {
        approvals,
        pool: pool.to_vec(),
    }
}

/// An approval rule document carrying `statements`, as compact JSON text.
fn document(statements: &[Statement]) -> String {
    let bodies: Vec<Value> = statements
        .iter()
        .map(|statement| {
            let mut body = json!({
                "Type": STATEMENT_TYPE,
                "NumberOfApprovalsNeeded": statement.approvals,
            });
            if !statement.pool.is_empty() {
                body["ApprovalPoolMembers"] = json!(statement.pool);
            }
            body
        })
        .collect();
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": bodies,
    }))
    .expect("a JSON document")
}

/// A pool naming exactly the identity that signs with `access_key_id`.
fn exact_pool(access_key_id: &str) -> Vec<String> {
    vec![caller_arn(access_key_id)]
}

/// A pool naming that identity through the `*` wildcard a CodeCommit approval pool
/// member admits, so the account segment of the ARN is not spelled out.
fn wildcard_pool(access_key_id: &str) -> Vec<String> {
    vec![format!("arn:aws:iam::*:user/{access_key_id}")]
}

/// An emulator holding one repository with two diverged branches and one open pull
/// request between them, opened by [`AUTHOR`], plus the engine so a test can seed
/// stored state no implemented operation reaches yet.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    pull_request_id: String,
}

async fn fixture() -> Fixture {
    let (router, engine) = emulator_with_callers(&[AUTHOR, ONE, TWO, THREE]);
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
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    Fixture {
        router,
        engine,
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

    async fn create_rule(&self, name: &str, content: &str) {
        ok_as(
            &self.router,
            AUTHOR,
            "CreatePullRequestApprovalRule",
            &json!({
                "pullRequestId": self.pull_request_id,
                "approvalRuleName": name,
                "approvalRuleContent": content,
            }),
        )
        .await;
    }

    /// Record `approval_state` for the caller signing with `access_key_id`, on the
    /// pull request's current revision.
    async fn record_approval(&self, access_key_id: &str, approval_state: &str) {
        let revision_id = self.revision_id().await;
        ok_as(
            &self.router,
            access_key_id,
            APPROVAL_STATE_OPERATION,
            &json!({
                "pullRequestId": self.pull_request_id,
                "revisionId": revision_id,
                "approvalState": approval_state,
            }),
        )
        .await;
    }

    /// The `evaluation` the operation reports for the current revision.
    async fn evaluation(&self) -> Value {
        let revision_id = self.revision_id().await;
        let read = ok(
            &self.router,
            OPERATION,
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

    /// Set the stored pull request's approval rules aside, as an override would:
    /// the override is one axis the evaluation reports on, and seeding it here keeps
    /// this suite's subject the evaluation rather than the operation that records it
    /// (`OverridePullRequestApprovalRules`, driven over the wire by the sibling
    /// `pull_request_approval_rule_override` suite).
    fn override_rules(&self) {
        let state = self.engine.state::<CodeCommitState>();
        let mut store = state.lock();
        let id: u64 = self
            .pull_request_id
            .parse()
            .expect("the id the service minted");
        store
            .pull_request_mut(id)
            .expect("the pull request opened above")
            .approval_rules_overridden_by = Some(caller_arn("AKIDOVERRIDER"));
    }
}

/// The names in one of the evaluation's two lists.
fn names(evaluation: &Value, member: &str) -> Vec<String> {
    evaluation
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no {member} in {evaluation}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("a non-string name in {evaluation}"))
                .to_string()
        })
        .collect()
}

fn flag(evaluation: &Value, member: &str) -> bool {
    evaluation
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("no {member} in {evaluation}"))
}

/// One row of the evaluation table.
struct Case {
    label: &'static str,
    /// The rules applied to the pull request, in the order they are applied.
    rules: Vec<(&'static str, Vec<Statement>)>,
    /// The approval states recorded before the evaluation, in order: the caller and
    /// the state it asks for.
    approvals: Vec<(&'static str, &'static str)>,
    /// The names expected in each of the evaluation's two lists.
    satisfied: Vec<&'static str>,
    not_satisfied: Vec<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a pull request carrying no approval rules at all",
            rules: vec![],
            approvals: vec![],
            satisfied: vec![],
            not_satisfied: vec![],
        },
        Case {
            label: "one rule needing one approval, nobody has approved",
            rules: vec![("needs-one", vec![statement(1, &[])])],
            approvals: vec![],
            satisfied: vec![],
            not_satisfied: vec!["needs-one"],
        },
        Case {
            label: "one rule needing one approval, one approver",
            rules: vec![("needs-one", vec![statement(1, &[])])],
            approvals: vec![(ONE, APPROVE)],
            satisfied: vec!["needs-one"],
            not_satisfied: vec![],
        },
        Case {
            label: "one rule needing two approvals, one approver",
            rules: vec![("needs-two", vec![statement(2, &[])])],
            approvals: vec![(ONE, APPROVE)],
            satisfied: vec![],
            not_satisfied: vec!["needs-two"],
        },
        Case {
            label: "one rule needing two approvals, two approvers",
            rules: vec![("needs-two", vec![statement(2, &[])])],
            approvals: vec![(ONE, APPROVE), (TWO, APPROVE)],
            satisfied: vec!["needs-two"],
            not_satisfied: vec![],
        },
        Case {
            label: "one rule needing three approvals, three approvers",
            rules: vec![("needs-three", vec![statement(3, &[])])],
            approvals: vec![(ONE, APPROVE), (TWO, APPROVE), (THREE, APPROVE)],
            satisfied: vec!["needs-three"],
            not_satisfied: vec![],
        },
        Case {
            label: "an approval that was revoked again no longer counts",
            rules: vec![("needs-one", vec![statement(1, &[])])],
            approvals: vec![(ONE, APPROVE), (ONE, REVOKE)],
            satisfied: vec![],
            not_satisfied: vec!["needs-one"],
        },
        Case {
            label: "a revoked approval leaves the other approver's counting",
            rules: vec![("needs-one", vec![statement(1, &[])])],
            approvals: vec![(ONE, APPROVE), (TWO, APPROVE), (ONE, REVOKE)],
            satisfied: vec!["needs-one"],
            not_satisfied: vec![],
        },
        Case {
            label: "a pooled rule approved by an identity outside its pool",
            rules: vec![("pooled", vec![statement(1, &exact_pool(TWO))])],
            approvals: vec![(ONE, APPROVE)],
            satisfied: vec![],
            not_satisfied: vec!["pooled"],
        },
        Case {
            label: "a pooled rule approved by the identity its pool names",
            rules: vec![("pooled", vec![statement(1, &exact_pool(TWO))])],
            approvals: vec![(TWO, APPROVE)],
            satisfied: vec!["pooled"],
            not_satisfied: vec![],
        },
        Case {
            label: "a rule whose pool names the approver through a wildcard",
            rules: vec![("wildcard-pooled", vec![statement(1, &wildcard_pool(THREE))])],
            approvals: vec![(THREE, APPROVE)],
            satisfied: vec!["wildcard-pooled"],
            not_satisfied: vec![],
        },
        Case {
            label: "a two-statement rule with only its first statement met",
            rules: vec![(
                "two-statements",
                vec![statement(1, &[]), statement(1, &exact_pool(THREE))],
            )],
            approvals: vec![(ONE, APPROVE)],
            satisfied: vec![],
            not_satisfied: vec!["two-statements"],
        },
        Case {
            label: "a two-statement rule with both statements met",
            rules: vec![(
                "two-statements",
                vec![statement(1, &[]), statement(1, &exact_pool(THREE))],
            )],
            approvals: vec![(ONE, APPROVE), (THREE, APPROVE)],
            satisfied: vec!["two-statements"],
            not_satisfied: vec![],
        },
        Case {
            label: "three rules reported in the order they were applied, middle met",
            rules: vec![
                ("alpha-needs-two", vec![statement(2, &[])]),
                ("beta-needs-one", vec![statement(1, &[])]),
                ("gamma-pooled", vec![statement(1, &exact_pool(TWO))]),
            ],
            approvals: vec![(ONE, APPROVE)],
            satisfied: vec!["beta-needs-one"],
            not_satisfied: vec!["alpha-needs-two", "gamma-pooled"],
        },
        Case {
            label: "three rules, every one of them met",
            rules: vec![
                ("alpha-needs-two", vec![statement(2, &[])]),
                ("beta-needs-one", vec![statement(1, &[])]),
                ("gamma-pooled", vec![statement(1, &exact_pool(TWO))]),
            ],
            approvals: vec![(ONE, APPROVE), (TWO, APPROVE)],
            satisfied: vec!["alpha-needs-two", "beta-needs-one", "gamma-pooled"],
            not_satisfied: vec![],
        },
    ]
}

#[tokio::test]
async fn the_evaluation_reports_each_rule_against_the_approvals_the_pull_request_holds() {
    for case in cases() {
        let fixture = fixture().await;
        for (name, statements) in &case.rules {
            fixture.create_rule(name, &document(statements)).await;
        }
        for (access_key_id, approval_state) in &case.approvals {
            fixture.record_approval(access_key_id, approval_state).await;
        }

        let evaluation = fixture.evaluation().await;
        let satisfied = names(&evaluation, "approvalRulesSatisfied");
        let not_satisfied = names(&evaluation, "approvalRulesNotSatisfied");
        assert_eq!(
            satisfied, case.satisfied,
            "case {:?}: expected approvalRulesSatisfied {:?}, got {satisfied:?}",
            case.label, case.satisfied
        );
        assert_eq!(
            not_satisfied, case.not_satisfied,
            "case {:?}: expected approvalRulesNotSatisfied {:?}, got {not_satisfied:?}",
            case.label, case.not_satisfied
        );
        let approved = flag(&evaluation, "approved");
        assert_eq!(
            approved,
            case.not_satisfied.is_empty(),
            "case {:?}: expected approved {}, got {approved}",
            case.label,
            case.not_satisfied.is_empty()
        );
        let overridden = flag(&evaluation, "overridden");
        assert!(
            !overridden,
            "case {:?}: nothing overrode the rules, so expected overridden false, got \
             {overridden}",
            case.label
        );
    }
}

#[tokio::test]
async fn overridden_rules_are_reported_as_overridden_with_their_conditions_still_unmet() {
    let fixture = fixture().await;
    fixture
        .create_rule("needs-two", &document(&[statement(2, &[])]))
        .await;
    fixture.create_rule("needs-one", &document(&[statement(1, &[])])).await;
    fixture.record_approval(ONE, APPROVE).await;

    let before = fixture.evaluation().await;
    assert!(
        !flag(&before, "approved") && !flag(&before, "overridden"),
        "case {:?}: expected neither approved nor overridden, got {before}",
        "an unmet rule before the override"
    );

    fixture.override_rules();

    let after = fixture.evaluation().await;
    assert!(
        flag(&after, "overridden"),
        "case {:?}: expected overridden true, got {after}",
        "an unmet rule after the override"
    );
    assert!(
        !flag(&after, "approved"),
        "case {:?}: an override is reported on its own member and does not meet a rule's \
         conditions, so expected approved false, got {after}",
        "an unmet rule after the override"
    );
    let not_satisfied = names(&after, "approvalRulesNotSatisfied");
    assert_eq!(
        not_satisfied,
        vec!["needs-two".to_string()],
        "case {:?}: the rule's conditions are still unmet, so expected it reported as such, got \
         {not_satisfied:?}",
        "an unmet rule after the override"
    );
    let satisfied = names(&after, "approvalRulesSatisfied");
    assert_eq!(
        satisfied,
        vec!["needs-one".to_string()],
        "case {:?}: expected the met rule still reported as met, got {satisfied:?}",
        "an unmet rule after the override"
    );
}

#[tokio::test]
async fn evaluating_a_pull_request_changes_nothing_a_later_caller_sees() {
    let fixture = fixture().await;
    fixture
        .create_rule("needs-two", &document(&[statement(2, &[])]))
        .await;
    fixture.record_approval(ONE, APPROVE).await;

    let before = fixture.stored().await;
    let first = fixture.evaluation().await;
    let second = fixture.evaluation().await;
    let after = fixture.stored().await;

    assert_eq!(
        first, second,
        "case {:?}: expected the same evaluation twice, got {first} then {second}",
        "two evaluations of one unchanged pull request"
    );
    assert_eq!(
        before, after,
        "case {:?}: expected the pull request unchanged, got {before} then {after}",
        "the stored pull request across two evaluations"
    );
}

/// One refused call: the body it sends and the error it must answer with.
struct Refusal {
    label: &'static str,
    /// `None` names the fixture's own pull request id, so a case can refuse on the
    /// revision alone.
    pull_request_id: Option<&'static str>,
    revision_id: Option<&'static str>,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_request_naming_no_pull_request_or_no_current_revision_is_refused() {
    let fixture = fixture().await;
    let current = fixture.revision_id().await;
    // A well-formed revision id that names no revision of this pull request: the
    // digest form with its last character stepped on.
    let mut stale: Vec<char> = current.chars().collect();
    let last = stale.len() - 1;
    stale[last] = if stale[last] == '0' { '1' } else { '0' };
    let stale: String = stale.into_iter().collect();
    let unknown_pull_request = "999999";

    let refusals = vec![
        Refusal {
            label: "an empty pull request id",
            pull_request_id: Some(""),
            revision_id: None,
            code: "InvalidPullRequestIdException",
            message: empty_pull_request_id(""),
        },
        Refusal {
            label: "a pull request id of nothing but a space",
            pull_request_id: Some(" "),
            revision_id: None,
            code: "InvalidPullRequestIdException",
            message: invalid_pull_request_id(" "),
        },
        Refusal {
            label: "a pull request id that is not a pull request id",
            pull_request_id: Some("not-an-id"),
            revision_id: None,
            code: "InvalidPullRequestIdException",
            message: invalid_pull_request_id("not-an-id"),
        },
        Refusal {
            label: "a well-formed pull request id no pull request holds",
            pull_request_id: Some(unknown_pull_request),
            revision_id: None,
            code: "PullRequestDoesNotExistException",
            message: PULL_REQUEST_DOES_NOT_EXIST.to_string(),
        },
        Refusal {
            label: "an empty revision id",
            pull_request_id: None,
            revision_id: Some(""),
            code: "InvalidRevisionIdException",
            message: EMPTY_REVISION_ID.to_string(),
        },
        Refusal {
            label: "a revision id of nothing but spaces",
            pull_request_id: None,
            revision_id: Some("   "),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
        Refusal {
            label: "a revision id that is not a revision id",
            pull_request_id: None,
            revision_id: Some("deadbeef"),
            code: "InvalidRevisionIdException",
            message: INVALID_REVISION_ID.to_string(),
        },
    ];

    for refusal in refusals {
        let body = json!({
            "pullRequestId": refusal
                .pull_request_id
                .map(str::to_string)
                .unwrap_or_else(|| fixture.pull_request_id.clone()),
            "revisionId": refusal
                .revision_id
                .map(str::to_string)
                .unwrap_or_else(|| current.clone()),
        });
        let (code, message) =
            error_with_message(&fixture.router, OPERATION, &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, refusal.code,
            "case {:?}: expected {}, got {code} saying {message:?}",
            refusal.label, refusal.code
        );
        assert_eq!(
            message, refusal.message,
            "case {:?}: expected the message {:?}, got {message:?}",
            refusal.label, refusal.message
        );
    }

    // A revision the pull request has moved past is refused rather than answered
    // with the state of a revision the caller did not ask about.
    let body = json!({
        "pullRequestId": fixture.pull_request_id,
        "revisionId": stale,
    });
    let (code, message) =
        error_with_message(&fixture.router, OPERATION, &body, CLIENT_ERROR_STATUS).await;
    assert_eq!(
        code, "RevisionNotCurrentException",
        "case {:?}: expected RevisionNotCurrentException, got {code} saying {message:?}",
        "a well-formed revision id of no revision of this pull request"
    );
    assert_eq!(
        message, REVISION_NOT_CURRENT,
        "case {:?}: expected the message {REVISION_NOT_CURRENT:?}, got {message:?}",
        "a well-formed revision id of no revision of this pull request"
    );
}
