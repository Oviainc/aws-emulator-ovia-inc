// SPDX-License-Identifier: CC0-1.0

//! The BODY of a `nextToken` refusal, over EVERY paginated operation of this
//! service.
//!
//! The modelled `NextToken` pattern (`^[A-Za-z0-9+/=]+$`) admits any string of
//! those characters, so a token this service never minted reaches the handler,
//! which refuses it itself. The rule under test is that such a refusal is the
//! crate's single validation report for one member — "`<n>` validation error(s)
//! detected: Value at '`<member>`' failed to satisfy constraint: `<requirement>`"
//! — with the rejected token echoed nowhere in it, whichever paginated
//! operation was called and whichever way the token fails.
//!
//! The cases span the two ways a token can fail: one this service could not have
//! minted at all (it does not decode), and one that decodes but was taken for a
//! different query than the call resuming with it. The first is driven over a
//! TABLE of token forms, the second both synthetically (a cursor carrying a
//! fingerprint no query of this service produces) and end-to-end from a token a
//! previous page really issued.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RecommendedAction;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    call, call_ok, emulator_with_engine, error_code, member, minimal_rule, string_member,
};
use super::error_payloads::error_message;

/// The requirement clause of a refusal of a token this service never issued, for
/// the operation that was called.
fn unissued_requirement(operation: &str) -> String {
    format!("Member must be a token returned by a previous {operation} call")
}

/// The whole message a `nextToken` refusal must carry: the crate's validation
/// report for one member, which names the member and the requirement and echoes
/// no value.
fn expected_message(requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: \
         {requirement}"
    )
}

/// Call `operation` with `token` spliced into `body` as its `nextToken`, assert
/// the refusal — HTTP 400, `InvalidParameterValueException`, and exactly
/// `requirement` reported on `nextToken` — and answer the message it carried, so
/// a caller can assert about the body as well.
async fn assert_token_refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    token: &str,
    requirement: &str,
    case: &str,
) -> String {
    let mut request = body.clone();
    request["nextToken"] = json!(token);
    let (status, parsed) = call(router, operation, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} and {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "InvalidParameterValueException",
        "case {case:?}: expected error code InvalidParameterValueException, got {parsed}"
    );
    let message = error_message(&parsed);
    assert_eq!(
        message,
        expected_message(requirement),
        "case {case:?}: expected message {:?}, got {message:?}",
        expected_message(requirement)
    );
    message
}

/// One recommended action Compute Optimizer's analysis would have produced, of
/// the type whose automation performs more than one step, so a page of its steps
/// can leave another behind it.
fn recommended_action(id: &str, volume: &str) -> RecommendedAction {
    RecommendedAction {
        recommended_action_id: Some(id.to_string()),
        resource_id: Some(volume.to_string()),
        account_id: Some(engine::EMULATOR_ACCOUNT_ID.to_string()),
        region: Some("us-east-1".to_string()),
        // The model's only `ResourceType` value, and the `RecommendedActionType`
        // whose event performs two steps
        // (`smithy_query(query="shape",shape="ResourceType,RecommendedActionType")`).
        resource_type: Some("EbsVolume".to_string()),
        recommended_action_type: Some("SnapshotAndDeleteUnattachedEbsVolume".to_string()),
        ..Default::default()
    }
}

/// Seed `action` where Compute Optimizer's analysis would have left it, then
/// start its automation over the wire and answer the event's id.
async fn start_event(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    action_id: &str,
    volume: &str,
    case: &str,
) -> String {
    let action = recommended_action(action_id, volume);
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    service_state
        .lock()
        .recommended_actions
        .insert(action_id.to_string(), action);
    let started = call_ok(
        router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": action_id}),
        case,
    )
    .await;
    string_member(&started, "eventId")
}

/// Every paginated operation of this service, as `(operation, a request with
/// every required member and nothing else, the requirement reported for a token
/// taken for a DIFFERENT query)`.
///
/// The operations are the ones the model marks `@paginated`
/// (`smithy_query(query="digest")`); `ListAccounts` takes no query at all, so a
/// token not taken for its own walk is the same refusal as one it never issued.
fn paginated_operations(event_id: &str) -> Vec<(&'static str, Value, &'static str)> {
    let preview_selection = json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": ["UpgradeEbsVolumeType"],
    });
    vec![
        (
            "ListAccounts",
            json!({}),
            "Member must be a token returned by a previous ListAccounts call",
        ),
        (
            "ListAutomationEvents",
            json!({}),
            "Member must be a token returned for the same filters and time range",
        ),
        (
            "ListAutomationEventSteps",
            json!({"eventId": event_id}),
            "Member must be a token returned for the same eventId",
        ),
        (
            "ListAutomationEventSummaries",
            json!({}),
            "Member must be a token returned for the same filters and dates",
        ),
        (
            "ListAutomationRules",
            json!({}),
            "Member must be a token returned for the same filters",
        ),
        (
            "ListAutomationRulePreview",
            preview_selection.clone(),
            "Member must be a token returned for the same rule type, action types and criteria",
        ),
        (
            "ListAutomationRulePreviewSummaries",
            preview_selection,
            "Member must be a token returned for the same rule type, action types and criteria",
        ),
        (
            "ListRecommendedActions",
            json!({}),
            "Member must be a token returned for the same filters",
        ),
        (
            "ListRecommendedActionSummaries",
            json!({}),
            "Member must be a token returned for the same filters",
        ),
    ]
}

/// Token forms this service cannot have minted, as `(case, token, distinctive)`.
///
/// Every one satisfies the modelled `NextToken` pattern, so it passes the
/// protocol boundary and reaches the handler, and none of them decodes to a
/// cursor. `distinctive` says the token is a string the refusal could only
/// contain by echoing it — a single hex digit can occur in the wording by
/// chance, so an echo check on it would pass for the wrong reason.
const UNISSUED_TOKENS: &[(&str, &str, bool)] = &[
    ("plain-words", "notatokenweissued", true),
    ("one-character", "A", false),
    ("odd-length", "abc", true),
    // Hex of bytes that are not text at all.
    ("not-text", "deadbeef", true),
    // Hex of "0000": text, but no cursor separator.
    ("no-separator", "30303030", true),
    // Hex of "abc:def": a separator, but no offset in front of it.
    ("no-offset", "6162633a646566", true),
    ("padding-only", "==", true),
    (
        "far-longer-than-any-token-we-mint",
        "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
        true,
    ),
];

/// A `nextToken` this service never issued is refused, on every paginated
/// operation, with the crate's validation report for `nextToken` — and the
/// rejected token appears nowhere in the body.
#[tokio::test]
async fn an_unissued_page_token_is_refused_without_being_echoed() {
    let (router, engine) = emulator_with_engine();
    let event_id = start_event(&router, &engine, "ActioN0000000001", "vol-0a1b2c3d4", "seed").await;

    for (operation, body, _) in paginated_operations(&event_id) {
        for (token_case, token, distinctive) in UNISSUED_TOKENS {
            let case = format!("{operation}-{token_case}");
            let message = assert_token_refused(
                &router,
                operation,
                &body,
                token,
                &unissued_requirement(operation),
                &case,
            )
            .await;
            if *distinctive {
                assert!(
                    !message.contains(token),
                    "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
                );
            }
        }
    }
}

/// A token that DECODES but was taken for another query is refused with the
/// requirement that names what it has to match, on every paginated operation,
/// again without echoing it.
///
/// The cursor is encoded here with a fingerprint no query of this service
/// produces, which is what every operation's token check compares against.
#[tokio::test]
async fn a_page_token_taken_for_another_query_is_refused() {
    let (router, engine) = emulator_with_engine();
    let event_id = start_event(&router, &engine, "ActioN0000000002", "vol-0a1b2c3d5", "seed").await;
    let foreign = state::encode_page_token(&state::PageCursor {
        offset: 0,
        filter_fingerprint: "nofingerprintofanyqueryofthisservice".to_string(),
    });

    for (operation, body, requirement) in paginated_operations(&event_id) {
        let case = format!("{operation}-foreign-fingerprint");
        let message =
            assert_token_refused(&router, operation, &body, &foreign, requirement, &case).await;
        assert!(
            !message.contains(&foreign),
            "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
        );
    }
}

/// A token a previous page really issued carries the query it was taken with:
/// presented to a different query it is refused with the same report, and
/// presented to the query that issued it, it resumes the walk.
///
/// One row per kind of query a page is a window onto — the steps of ONE event,
/// a filtered event listing, and a filtered rule listing — so the rule is
/// asserted on tokens this service minted, not only on ones the test built.
#[tokio::test]
async fn a_minted_token_resumes_its_own_query_and_is_refused_by_any_other() {
    let (router, engine) = emulator_with_engine();
    let first_event =
        start_event(&router, &engine, "ActioN0000000003", "vol-0a1b2c3d6", "first").await;
    let second_event =
        start_event(&router, &engine, "ActioN0000000004", "vol-0a1b2c3d7", "second").await;
    for name in ["token-rule-one", "token-rule-two"] {
        call_ok(&router, "CreateAutomationRule", &minimal_rule(name), name).await;
    }

    // (case, operation, the query the token is taken with, the query it is then
    // presented to, the requirement that refusal reports)
    let cases: Vec<(&str, &str, Value, Value, &str)> = vec![
        (
            "steps-of-another-event",
            "ListAutomationEventSteps",
            json!({"eventId": first_event.clone(), "maxResults": 1}),
            json!({"eventId": second_event.clone(), "maxResults": 1}),
            "Member must be a token returned for the same eventId",
        ),
        (
            "events-under-a-filter-the-token-was-not-taken-with",
            "ListAutomationEvents",
            json!({"maxResults": 1}),
            json!({"maxResults": 1, "filters": [{"name": "EventType", "values": ["UpgradeEbsVolumeType"]}]}),
            "Member must be a token returned for the same filters and time range",
        ),
        (
            "rules-under-a-filter-the-token-was-not-taken-with",
            "ListAutomationRules",
            json!({"maxResults": 1}),
            json!({"maxResults": 1, "filters": [{"name": "Name", "values": ["token-rule-two"]}]}),
            "Member must be a token returned for the same filters",
        ),
    ];

    for (case, operation, issuing_query, other_query, requirement) in cases {
        let first_page = call_ok(&router, operation, &issuing_query, case).await;
        let token = member(&first_page, "nextToken")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the first page must carry a token, got {first_page}")
            })
            .to_string();

        let message =
            assert_token_refused(&router, operation, &other_query, &token, requirement, case).await;
        assert!(
            !message.contains(&token),
            "case {case:?}: the refusal must not echo the rejected token, got {message:?}"
        );

        // The same token on the query that issued it is served, so the refusal
        // above is the fingerprint check and not a blanket rejection.
        let mut resumed = issuing_query.clone();
        resumed["nextToken"] = json!(token);
        let (status, parsed) = call(&router, operation, &resumed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the issuing query must resume with its own token, got {status} and \
             {parsed}"
        );
    }
}
