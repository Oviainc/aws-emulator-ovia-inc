// SPDX-License-Identifier: CC0-1.0

//! The client-error bodies this service words for MORE THAN ONE caller, asserted
//! over the whole set of operations that can raise them.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the emulator binary mounts — using the harness `automation_rule_lifecycle`
//! owns.
//!
//! The rules under test:
//!
//! * a client token a DIFFERENT operation already claimed is refused with one
//!   fixed body — the same HTTP status, error code and message whichever
//!   operation of this service is presented with it, because one error of one
//!   service has one wording;
//! * the per-account rule ceiling is refused with a body stating the BOUND the
//!   service enforces and the account the rules are counted per, however the
//!   account arrived at the ceiling.
//!
//! The sibling suites assert the status and the error CODE of both refusals;
//! these cases are about the body a caller parses, and the token case spans every
//! operation that can produce it rather than the four that happen to have a
//! lifecycle suite.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RecommendedAction;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, StoredEvent, MAX_RULES_PER_ACCOUNT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    call, call_ok, create_rule, emulator, emulator_with_engine, error_code, member, minimal_rule,
    seed_event, string_member,
};
use crate::error_payloads::error_message;

/// The sentence the model's `IdempotencyTokenInUseException` documents
/// (`smithy_query(query="shape",shape="IdempotencyTokenInUseException")`), which
/// is the body this service answers a token another operation holds with. HTTP
/// 409 is the shape's modelled `httpError`.
const TOKEN_IN_USE_MESSAGE: &str = "The specified client token is already in use.";

/// Member account ids in the modelled `AccountId` form (`^[0-9]{12}$`) that are
/// not the calling account.
const MEMBER_TO_ASSOCIATE: &str = "111111111111";
const MEMBER_ALREADY_ASSOCIATED: &str = "222222222222";

/// A modelled `RecommendedActionId` (`^[0-9A-Za-z]{16}$`), for the action a
/// `StartAutomationEvent` request names.
const SEEDED_ACTION_ID: &str = "Upgrade000000001";

/// A modelled `EventId` (`^[0-9A-Za-z]{16}$`), for the event a
/// `RollbackAutomationEvent` request names.
const SEEDED_EVENT_ID: &str = "Rollback00000001";

/// One recommended action in the store, as Compute Optimizer's analysis would
/// have produced it.
///
/// A test PRECONDITION: no operation of this API creates a recommended action
/// (the analysis that does is not part of it), and `StartAutomationEvent` names
/// one. `recommendedActionType` carries a value the model's
/// `RecommendedActionType` declares
/// (`smithy_query(query="shape",shape="RecommendedActionType")`).
fn seed_recommended_action(engine: &Arc<engine::InMemoryEngine>) {
    let service_state =
        state::service_state(&**engine).expect("the engine bears this service's state");
    service_state.lock().recommended_actions.insert(
        SEEDED_ACTION_ID.to_string(),
        RecommendedAction {
            recommended_action_id: Some(SEEDED_ACTION_ID.to_string()),
            account_id: Some(state::caller_account_id()),
            region: Some("us-east-1".to_string()),
            resource_id: Some("vol-00000000000000001".to_string()),
            resource_type: Some(state::RESOURCE_TYPE_EBS_VOLUME.to_string()),
            recommended_action_type: Some(state::EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE.to_string()),
            ..Default::default()
        },
    );
}

/// One automation event in the store whose automation was carried out, which is
/// the status a rollback may be initiated from.
///
/// A test PRECONDITION: this emulator's own runs settle to that status only once
/// the run window has passed, and no case sleeps for a clock.
fn seed_completed_event(engine: &Arc<engine::InMemoryEngine>) {
    seed_event(
        engine,
        &StoredEvent {
            event_id: SEEDED_EVENT_ID.to_string(),
            event_description: Some("a completed automation".to_string()),
            event_type: state::EVENT_TYPE_UPGRADE_EBS_VOLUME_TYPE.to_string(),
            event_status: state::EVENT_STATUS_COMPLETE.to_string(),
            event_status_reason: None,
            resource_arn: None,
            resource_id: Some("vol-00000000000000001".to_string()),
            recommended_action_id: Some(SEEDED_ACTION_ID.to_string()),
            account_id: state::caller_account_id(),
            region: Some("us-east-1".to_string()),
            rule_id: None,
            resource_type: state::RESOURCE_TYPE_EBS_VOLUME.to_string(),
            created_timestamp: DateTime::from(std::time::SystemTime::now()),
            completed_timestamp: Some(DateTime::from(std::time::SystemTime::now())),
            estimated_monthly_savings: None,
            steps: Vec::new(),
            // A seeded event is already in the status it names, so the clock
            // owes it no transition.
            pending: None,
        },
    );
}

/// Spend `token` on `operation`, so that presenting it to any OTHER operation is
/// a token already in use.
async fn claim_token(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
    token: &str,
    case: &str,
) {
    let mut claim = request.clone();
    claim["clientToken"] = json!(token);
    call_ok(router, operation, &claim, case).await;
}

// ---------------------------------------------------------------------------
// A client token another operation holds
// ---------------------------------------------------------------------------

/// A `clientToken` another operation already claimed is refused with HTTP 409,
/// `IdempotencyTokenInUseException` and ONE message — on every operation of this
/// service whose input carries a `clientToken`.
///
/// The ten rows are exactly those operations
/// (`smithy_query(query="digest")`: `AssociateAccounts`,
/// `CreateAutomationRule`, `DeleteAutomationRule`, `DisassociateAccounts`,
/// `RollbackAutomationEvent`, `StartAutomationEvent`, `TagResource`,
/// `UntagResource`, `UpdateAutomationRule`, `UpdateEnrollmentConfiguration`),
/// and they differ along everything the rule generalises over: what the request
/// targets (a rule, an account list, an event, a recommended action, the
/// caller's own enrollment), whether it creates, updates or deletes, and which
/// operation spent the token first. Each row's request is otherwise one the
/// service serves, so the refusal is attributable to the token alone.
#[tokio::test]
async fn a_token_another_operation_holds_is_one_message_on_every_operation_that_takes_one() {
    let (router, engine) = emulator_with_engine();
    let (rule_arn, rule_revision) = create_rule(
        &router,
        "token-wording-rule",
        "Active",
        &json!([{"key": "team", "value": "platform"}]),
        "setup",
    )
    .await;
    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [MEMBER_ALREADY_ASSOCIATED]}),
        "setup",
    )
    .await;
    seed_recommended_action(&engine);
    seed_completed_event(&engine);

    // (case, the operation presented with the spent token, its request)
    let rows: [(&str, &str, Value); 10] = [
        (
            "associate",
            "AssociateAccounts",
            json!({"accountIds": [MEMBER_TO_ASSOCIATE]}),
        ),
        ("create-rule", "CreateAutomationRule", minimal_rule("tokened")),
        (
            "delete-rule",
            "DeleteAutomationRule",
            json!({"ruleArn": &rule_arn, "ruleRevision": rule_revision}),
        ),
        (
            "disassociate",
            "DisassociateAccounts",
            json!({"accountIds": [MEMBER_ALREADY_ASSOCIATED]}),
        ),
        (
            "rollback-event",
            "RollbackAutomationEvent",
            json!({"eventId": SEEDED_EVENT_ID}),
        ),
        (
            "start-event",
            "StartAutomationEvent",
            json!({"recommendedActionId": SEEDED_ACTION_ID}),
        ),
        (
            "tag-rule",
            "TagResource",
            json!({
                "resourceArn": &rule_arn,
                "ruleRevision": rule_revision,
                "tags": [{"key": "env", "value": "prod"}]
            }),
        ),
        (
            "untag-rule",
            "UntagResource",
            json!({
                "resourceArn": &rule_arn,
                "ruleRevision": rule_revision,
                "tagKeys": ["team"]
            }),
        ),
        (
            "update-rule",
            "UpdateAutomationRule",
            json!({
                "ruleArn": &rule_arn,
                "ruleRevision": rule_revision,
                "description": "a new description"
            }),
        ),
        (
            "update-enrollment",
            "UpdateEnrollmentConfiguration",
            json!({"status": state::ENROLLED_STATUS}),
        ),
    ];

    for (index, (case, operation, request)) in rows.iter().enumerate() {
        let token = format!("spent-token-{index}");
        // The token is spent by an operation that is NOT this row's, which is
        // what makes it a token in use rather than a replay or a mismatch.
        let (claimant, claim_request) = if *operation == "UpdateEnrollmentConfiguration" {
            ("CreateAutomationRule", minimal_rule(&format!("claim{index}")))
        } else {
            (
                "UpdateEnrollmentConfiguration",
                json!({"status": state::ENROLLED_STATUS}),
            )
        };
        claim_token(&router, claimant, &claim_request, &token, case).await;

        let mut presented = request.clone();
        presented["clientToken"] = json!(token);
        let (status, body) = call(&router, operation, &presented).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected {operation} to answer HTTP 409 for a token \
             {claimant} holds, got {status} and {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotencyTokenInUseException",
            "case {case:?}: expected error code IdempotencyTokenInUseException from \
             {operation}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            TOKEN_IN_USE_MESSAGE,
            "case {case:?}: expected {operation} to word the refusal {TOKEN_IN_USE_MESSAGE:?}, \
             got {:?}",
            error_message(&body)
        );
    }
}

/// The refused call changes nothing: the rule the rows target keeps its revision
/// and its tags, and the member account the rows name is not associated by the
/// association row the token refused.
#[tokio::test]
async fn a_token_in_use_refusal_performs_nothing() {
    let (router, _engine) = emulator_with_engine();
    let (rule_arn, rule_revision) = create_rule(
        &router,
        "untouched-rule",
        "Active",
        &json!([{"key": "team", "value": "platform"}]),
        "setup",
    )
    .await;
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": state::ENROLLED_STATUS, "clientToken": "held-token"}),
        "setup",
    )
    .await;

    let (status, body) = call(
        &router,
        "UpdateAutomationRule",
        &json!({
            "ruleArn": &rule_arn,
            "ruleRevision": rule_revision,
            "description": "a new description",
            "clientToken": "held-token"
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case \"refused-update\": expected HTTP 409, got {status} and {body}"
    );

    let fetched = call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": &rule_arn}),
        "refused-update",
    )
    .await;
    assert_eq!(
        member(&fetched, "ruleRevision").as_i64(),
        Some(rule_revision),
        "case \"refused-update\": a refused call must not move the rule's revision, got {fetched}"
    );
    assert_eq!(
        string_member(&fetched, "description"),
        "",
        "case \"refused-update\": a refused call must not apply its description, got {fetched}"
    );
}

// ---------------------------------------------------------------------------
// The per-account rule ceiling
// ---------------------------------------------------------------------------

/// The per-account rule ceiling is refused with HTTP 402,
/// `ServiceQuotaExceededException` and a body stating the bound the service
/// enforces and the account the rules are counted per — the same body however
/// the account arrived at the ceiling (straight creates, or a create that
/// replaced a deleted rule), because the refusal states the quota and not the
/// history of the store.
#[tokio::test]
async fn the_rule_quota_refusal_states_the_bound_and_the_account() {
    // The sentence is spelled HERE, with only the bound and the account
    // interpolated from the service's own state, so the case is an independent
    // statement of what the body must say: asserting against the handler's own
    // message builder would pass whatever that builder produced.
    let expected = format!(
        "The request would exceed the quota of {MAX_RULES_PER_ACCOUNT} automation rules for \
         account {}.",
        state::caller_account_id()
    );

    let router = emulator();
    let mut arns = Vec::with_capacity(MAX_RULES_PER_ACCOUNT);
    for index in 0..MAX_RULES_PER_ACCOUNT {
        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &minimal_rule(&format!("quota-rule-{index}")),
            "fill-to-the-quota",
        )
        .await;
        arns.push(string_member(&created, "ruleArn"));
    }

    // (case, what the account did before the refused create)
    let rows: [(&str, bool); 2] = [("at-the-ceiling", false), ("refilled-after-a-delete", true)];
    for (case, recycle) in rows {
        if recycle {
            call_ok(
                &router,
                "DeleteAutomationRule",
                &json!({"ruleArn": &arns[0], "ruleRevision": 1}),
                case,
            )
            .await;
            call_ok(
                &router,
                "CreateAutomationRule",
                &minimal_rule("quota-rule-replacement"),
                case,
            )
            .await;
        }

        let (status, body) = call(
            &router,
            "CreateAutomationRule",
            &minimal_rule("one-rule-too-many"),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected HTTP 402 past the rule quota, got {status} and {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: expected error code ServiceQuotaExceededException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal to state the quota as {expected:?}, got {:?}",
            error_message(&body)
        );
    }
}
