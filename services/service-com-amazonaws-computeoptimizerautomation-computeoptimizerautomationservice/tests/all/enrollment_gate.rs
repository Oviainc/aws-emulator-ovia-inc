// SPDX-License-Identifier: CC0-1.0

//! The service-wide ACCOUNT ENROLLMENT GATE.
//!
//! The rule under test: every operation that performs one of this service's
//! actions refuses a caller whose account is not opted in (whose stored
//! `EnrollmentStatus` is not `Active`) with the service-common
//! `OptInRequiredException` and HTTP 403, and it does so BEFORE it resolves the
//! resource the request names or evaluates the filters it carries — so an
//! unenrolled caller is never answered "no such resource" and never handed a
//! listing. The two enrollment-configuration operations are the exception:
//! `GetEnrollmentConfiguration` reports the status and
//! `UpdateEnrollmentConfiguration` is the documented remedy, so gating them
//! would make the refusal unreadable and unrecoverable.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`] the emulator binary mounts — using the
//! harness `automation_rule_lifecycle` owns, in its UNENROLLED form, so the
//! enrollment this suite asserts about is the one the wire calls established.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, error_code, member, minimal_rule, string_member,
    unenrolled_emulator_with_engine,
};

/// A `RuleArn` no rule has, in the form the model's `RuleArn` pattern requires
/// (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`),
/// built where the service builds one.
fn absent_rule_arn() -> String {
    state::rule_arn("gatemissingrule1")
}

/// An `EventId`, modelled `^[0-9A-Za-z]{16}$`, that no automation event carries.
const ABSENT_EVENT_ID: &str = "gateevent0000001";

/// A `RecommendedActionId`, modelled `^[0-9A-Za-z]{16}$`, that no recommended
/// action carries.
const ABSENT_ACTION_ID: &str = "gateaction000001";

/// A member account id, modelled `^[0-9]{12}$`, for the organization-association
/// call this suite drives through the gate.
const GATE_MEMBER_ACCOUNT_ID: &str = "123456789012";

/// Every operation the gate covers, with a well-formed request for it and the
/// status it answers once the account IS opted in and the store is empty — the
/// operation's own outcome, which the gate is what stands in front of.
///
/// This is every implemented operation of the service except
/// `GetEnrollmentConfiguration` and `UpdateEnrollmentConfiguration`; the
/// still-scaffolded ones answer 501 whether or not the account is enrolled, so
/// they are not listed (see the `SCAFFOLDED, NOT IMPLEMENTED` headers in
/// `src/ops/`).
fn gated_calls() -> Vec<(&'static str, Value, u16)> {
    let arn = absent_rule_arn();
    vec![
        (
            "GetAutomationEvent",
            json!({"eventId": ABSENT_EVENT_ID}),
            404,
        ),
        ("GetAutomationRule", json!({"ruleArn": arn}), 404),
        ("ListAutomationEvents", json!({}), 200),
        ("ListAutomationRules", json!({}), 200),
        ("ListRecommendedActions", json!({}), 200),
        (
            "ListAutomationRulePreview",
            json!({"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"]}),
            200,
        ),
        (
            "ListAutomationRulePreviewSummaries",
            json!({"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"]}),
            200,
        ),
        ("ListTagsForResource", json!({"resourceArn": arn}), 404),
        ("CreateAutomationRule", minimal_rule("gate-rule"), 200),
        (
            "UpdateAutomationRule",
            json!({"ruleArn": arn, "ruleRevision": 1, "description": "gated"}),
            404,
        ),
        (
            "DeleteAutomationRule",
            json!({"ruleArn": arn, "ruleRevision": 1}),
            404,
        ),
        (
            "StartAutomationEvent",
            json!({"recommendedActionId": ABSENT_ACTION_ID}),
            404,
        ),
        (
            "UntagResource",
            json!({"resourceArn": arn, "ruleRevision": 1, "tagKeys": ["team"]}),
            404,
        ),
        (
            "AssociateAccounts",
            json!({"accountIds": [GATE_MEMBER_ACCOUNT_ID]}),
            200,
        ),
    ]
}

/// The `message` member of an error body, under either casing the awsJson family
/// renders it with.
fn error_message(body: &Value) -> String {
    ["message", "Message"]
        .iter()
        .find_map(|key| body.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// Opt the caller in over the wire, the way a client recovers from the refusal.
async fn opt_in(router: &MultiServiceRouter, case: &str) {
    let updated = call_ok(
        router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        case,
    )
    .await;
    assert_eq!(
        string_member(&updated, "status"),
        "Active",
        "case {case:?}: the opt-in must report the status it stored, got {updated}"
    );
}

/// Opt the caller out over the wire.
async fn opt_out(router: &MultiServiceRouter, case: &str) {
    call_ok(
        router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Inactive"}),
        case,
    )
    .await;
}

/// Put the stored enrollment at `status` where the service stores it.
///
/// Only for the two statuses a caller may NOT send (`Pending` and `Failed` are
/// "returned in the response of a request to update the enrollment status", not
/// settable), so there is no wire call that establishes them.
fn seed_enrollment(engine: &Arc<engine::InMemoryEngine>, status: &str) {
    let service_state =
        state::service_state(&**engine).expect("the engine bears this service's state");
    service_state.lock().enrollments.insert(
        state::caller_account_id(),
        state::EnrollmentConfig {
            status: status.to_string(),
            ..state::EnrollmentConfig::default()
        },
    );
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// An account that has never opted in is refused EVERY action with
/// `OptInRequiredException` (403), whatever the operation and whatever the
/// request names. The refusal carries one fixed message that echoes nothing the
/// request carried.
#[tokio::test]
async fn an_unenrolled_account_is_refused_every_action() {
    let (router, _engine) = unenrolled_emulator_with_engine();

    let mut messages: Vec<String> = Vec::new();
    for (operation, request, _) in gated_calls() {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 403,
            "case {operation:?}: an unenrolled account must be refused with HTTP 403, got \
             {status} and {body}"
        );
        assert_eq!(
            error_code(&body),
            "OptInRequiredException",
            "case {operation:?}: expected the service-common OptInRequiredException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            !message.is_empty(),
            "case {operation:?}: the refusal must say what to do, got {body}"
        );
        for echoed in [
            absent_rule_arn(),
            ABSENT_EVENT_ID.to_string(),
            ABSENT_ACTION_ID.to_string(),
            GATE_MEMBER_ACCOUNT_ID.to_string(),
            operation.to_string(),
        ] {
            assert!(
                !message.contains(&echoed),
                "case {operation:?}: the refusal is request-independent and must not echo \
                 {echoed:?}, got {message:?}"
            );
        }
        messages.push(message);
    }

    let first = messages.first().cloned().unwrap_or_default();
    for (index, (operation, _, _)) in gated_calls().into_iter().enumerate() {
        assert_eq!(
            messages.get(index).cloned().unwrap_or_default(),
            first,
            "case {operation:?}: every operation states the one enrollment requirement, got \
             {:?} against {first:?}",
            messages.get(index)
        );
    }
}

/// Opting in opens the gate for every operation — each then answers its OWN
/// outcome — and opting back out closes it again for all of them.
#[tokio::test]
async fn opting_in_opens_the_gate_and_opting_out_closes_it() {
    let (router, _engine) = unenrolled_emulator_with_engine();
    opt_in(&router, "opt-in").await;

    for (operation, request, expected) in gated_calls() {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, expected,
            "case {operation:?}: an opted-in account reaches the operation, which answers \
             HTTP {expected}, got {status} and {body}"
        );
        assert_ne!(
            error_code(&body),
            "OptInRequiredException",
            "case {operation:?}: the gate is open, so nothing may be refused on enrollment \
             grounds, got {body}"
        );
    }

    opt_out(&router, "opt-out").await;
    for (operation, request, _) in gated_calls() {
        assert_refused(
            call(&router, operation, &request).await,
            403,
            "OptInRequiredException",
            operation,
        );
    }
}

/// `Active` is the only `EnrollmentStatus` that means opted in: the gate is open
/// under it and closed under every other value of the model's enum, including
/// the two a caller cannot set.
#[tokio::test]
async fn only_active_enrollment_opens_the_gate() {
    // The `EnrollmentStatus` value set, from the Smithy model
    // (`smithy_query(query="shape",shape="EnrollmentStatus")`), with whether a
    // caller may set it (the `status` member's doc excludes Pending and Failed)
    // and whether the account counts as opted in under it.
    let cases = [
        ("Active", true, true),
        ("Inactive", true, false),
        ("Pending", false, false),
        ("Failed", false, false),
    ];

    for (status_value, settable, enrolled) in cases {
        let (router, engine) = unenrolled_emulator_with_engine();
        if settable {
            call_ok(
                &router,
                "UpdateEnrollmentConfiguration",
                &json!({"status": status_value}),
                status_value,
            )
            .await;
        } else {
            seed_enrollment(&engine, status_value);
        }

        let read_back =
            call_ok(&router, "GetEnrollmentConfiguration", &json!({}), status_value).await;
        assert_eq!(
            string_member(&read_back, "status"),
            status_value,
            "case {status_value:?}: the stored status must be the one under test, got {read_back}"
        );

        let (status, body) = call(&router, "ListAutomationRules", &json!({})).await;
        if enrolled {
            assert_eq!(
                status, 200,
                "case {status_value:?}: an opted-in account may list its rules, got {status} \
                 and {body}"
            );
        } else {
            assert_refused((status, body), 403, "OptInRequiredException", status_value);
        }
    }
}

/// The gate runs AHEAD of resource resolution: while the account is opted out,
/// an operation refuses with 403 both for a resource that exists and for one
/// that does not — never with the 404 that would say the resource is the
/// problem.
#[tokio::test]
async fn the_gate_precedes_resource_resolution() {
    let (router, _engine) = unenrolled_emulator_with_engine();
    opt_in(&router, "seed").await;

    // A rule that really is in the store, created while the gate was open.
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("gate-precedence"),
        "seed",
    )
    .await;
    let present_arn = string_member(&created, "ruleArn");
    let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();
    assert_eq!(
        call(&router, "GetAutomationRule", &json!({"ruleArn": present_arn}))
            .await
            .0,
        200,
        "case \"seed\": the rule must be readable while the account is opted in"
    );

    opt_out(&router, "opt-out").await;

    // (case, operation, request) — the same operations against a resource that
    // exists and one that does not. Both are 403: the gate answered first.
    let absent_arn = absent_rule_arn();
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "get-rule-that-exists",
            "GetAutomationRule",
            json!({"ruleArn": present_arn}),
        ),
        (
            "get-rule-that-does-not",
            "GetAutomationRule",
            json!({"ruleArn": absent_arn}),
        ),
        (
            "list-tags-of-rule-that-exists",
            "ListTagsForResource",
            json!({"resourceArn": present_arn}),
        ),
        (
            "list-tags-of-rule-that-does-not",
            "ListTagsForResource",
            json!({"resourceArn": absent_arn}),
        ),
        (
            "delete-rule-that-exists",
            "DeleteAutomationRule",
            json!({"ruleArn": present_arn, "ruleRevision": revision}),
        ),
        (
            "delete-rule-that-does-not",
            "DeleteAutomationRule",
            json!({"ruleArn": absent_arn, "ruleRevision": revision}),
        ),
        (
            "get-event-that-does-not-exist",
            "GetAutomationEvent",
            json!({"eventId": ABSENT_EVENT_ID}),
        ),
    ];

    for (case, operation, request) in cases {
        assert_refused(
            call(&router, operation, &request).await,
            403,
            "OptInRequiredException",
            case,
        );
    }

    // And the refused delete left the rule where it was: opting back in reads it.
    opt_in(&router, "opt-in-again").await;
    let read_back = call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": present_arn}),
        "opt-in-again",
    )
    .await;
    assert_eq!(
        string_member(&read_back, "ruleArn"),
        present_arn,
        "case \"opt-in-again\": a refused call must change nothing, got {read_back}"
    );
}

/// A call the gate refuses writes nothing — neither the resource it would have
/// created nor an entry in the client-token ledger — so the very same request,
/// with the very same `clientToken`, succeeds once the account opts in.
#[tokio::test]
async fn a_refused_call_writes_nothing() {
    // (case, client token) — a refused create with and without an idempotency
    // token, since the ledger is what a consumed token would show up in.
    let cases: [(&str, Option<&str>); 2] =
        [("with-token", Some("gate-token-1")), ("no-token", None)];

    for (case, token) in cases {
        // One fresh, unenrolled store per row, so each row starts from nothing.
        let (router, _engine) = unenrolled_emulator_with_engine();
        let mut request = minimal_rule(&format!("gate-write-{}", case.replace('-', "")));
        if let Some(token) = token {
            request["clientToken"] = json!(token);
        }

        assert_refused(
            call(&router, "CreateAutomationRule", &request).await,
            403,
            "OptInRequiredException",
            case,
        );

        opt_in(&router, case).await;
        // Nothing was stored while the gate was shut.
        let listed = call_ok(&router, "ListAutomationRules", &json!({}), case).await;
        assert_eq!(
            member(&listed, "automationRules")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {case:?}: the refused create must have stored no rule, got {listed}"
        );

        // The same request, same token: the refusal consumed neither.
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        let arn = string_member(&created, "ruleArn");
        assert!(
            !arn.is_empty(),
            "case {case:?}: the retried create must mint a rule, got {created}"
        );
        let listed = call_ok(&router, "ListAutomationRules", &json!({}), case).await;
        assert_eq!(
            member(&listed, "automationRules")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {case:?}: the rule the retry created must be listed, got {listed}"
        );
    }
}

/// The two enrollment-configuration operations are NOT gated: an account that
/// has never opted in can read its status and set it, which is the documented
/// way out of the refusal.
#[tokio::test]
async fn the_enrollment_configuration_operations_are_not_gated() {
    let (router, _engine) = unenrolled_emulator_with_engine();

    let initial = call_ok(&router, "GetEnrollmentConfiguration", &json!({}), "initial").await;
    assert_eq!(
        string_member(&initial, "status"),
        "Inactive",
        "case \"initial\": an unenrolled account reads its own status, got {initial}"
    );

    opt_in(&router, "remedy").await;

    let read_back = call_ok(&router, "GetEnrollmentConfiguration", &json!({}), "remedy").await;
    assert_eq!(
        string_member(&read_back, "status"),
        "Active",
        "case \"remedy\": the remedy must be readable back, got {read_back}"
    );
}
