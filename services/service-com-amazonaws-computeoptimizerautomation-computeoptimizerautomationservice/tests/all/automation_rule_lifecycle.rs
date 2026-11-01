// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the enrollment + automation-rule lifecycle.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: ComputeOptimizerAutomationService.<Operation>`, dispatched
//! through the same [`MultiServiceRouter`] the emulator binary mounts. Nothing
//! reaches into the store directly, so a response that is not computed from
//! persisted state fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router with one freshly-engined copy of this service mounted, for an
/// account that is OPTED IN to Compute Optimizer Automation.
///
/// Every operation but the two enrollment-configuration ones refuses an account
/// that is not opted in with `OptInRequiredException` (see
/// `state::…::opt_in_required`), so a suite about anything else starts from an
/// enrolled account. The gate itself, and the enrollment round trip that opens
/// it over the wire, are asserted in [`crate::enrollment_gate`] and in
/// [`enrollment_reports_the_default_until_an_update_persists_one`].
///
/// `pub(crate)` so the other suites in this test binary drive the service through
/// the same harness instead of each growing a copy of it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same harness, handing the engine back as well, for a suite that must
/// record into the store what no operation of this API creates.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = unenrolled_emulator_with_engine();
    enroll_caller(&engine);
    (router, engine)
}

/// The harness BEFORE enrollment: the account has never called
/// `UpdateEnrollmentConfiguration`, which is the state a brand-new account is
/// in.
pub(crate) fn unenrolled_emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>)
{
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the process-wide default: these cases are
    // about the service's behaviour, not about SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Put the calling account's enrollment in the store at `Active`, the one
/// status under which the service performs an action.
///
/// A test PRECONDITION, written where `UpdateEnrollmentConfiguration` writes it,
/// because a harness is not `async`; that operation's own wire path is covered
/// by the enrollment cases in this suite and in [`crate::enrollment_gate`]. It
/// stamps the update instant for the same reason that operation does: a stored
/// configuration is always one some call wrote, and the call that wrote it
/// records when.
pub(crate) fn enroll_caller(engine: &Arc<engine::InMemoryEngine>) {
    let state = state::service_state(&**engine).expect("the engine bears this service's state");
    state.lock().enrollments.insert(
        state::caller_account_id(),
        state::EnrollmentConfig {
            status: state::ENROLLED_STATUS.to_string(),
            last_updated_timestamp: Some(state::now()),
            ..state::EnrollmentConfig::default()
        },
    );
}

/// Put `event` in the store the mounted service reads from.
///
/// A test PRECONDITION for the suites about an automation event in a status this
/// emulator's own runs do not produce, or mid-run: one copy here rather than one
/// per suite.
pub(crate) fn seed_event(engine: &Arc<engine::InMemoryEngine>, event: &state::StoredEvent) {
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    service_state.lock().events.insert(event.clone());
}

/// Call one operation and return `(http status, parsed body)`.
///
/// `pub(crate)` because the other suites in this crate drive the service the same
/// way: one awsJson1_0 harness for all of them, not one each.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_as(router, AMBIENT_ACCESS_KEY_ID, operation, body).await
}

/// The access key id every call of this harness presents unless a suite names
/// another: a key no session was ever minted under, so it speaks for the ambient
/// identity this emulator publishes.
pub(crate) const AMBIENT_ACCESS_KEY_ID: &str = "AKIAEXAMPLEEXAMPLE";

/// [`call`], presenting `access_key_id` as the request's SigV4 credential — for
/// a suite about an answer that depends on WHICH credential signed the request.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://aco-automation.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("ComputeOptimizerAutomationService.{operation}"),
        )
        // The router keys on the SigV4 credential scope's service field
        // (`aco-automation`, this service's signing name). The signature itself
        // is not checked, because authentication is off above.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/aco-automation/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation, requiring success.
pub(crate) async fn call_ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to succeed with HTTP 200, got {status} and {parsed}"
    );
    parsed
}

/// The awsJson error code an error body carries in `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert a call was refused with `expected_code` and `expected_status`.
pub(crate) fn assert_refused(
    (status, body): (u16, Value),
    expected_status: u16,
    expected_code: &str,
    case: &str,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} and {body}"
    );
    assert_eq!(
        error_code(&body),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {body}"
    );
}

/// A create request with every required member and nothing else.
pub(crate) fn minimal_rule(name: &str) -> Value {
    json!({
        "name": name,
        "ruleType": "AccountRule",
        "recommendedActionTypes": ["UpgradeEbsVolumeType"],
        "schedule": {
            "scheduleExpression": "30 12 * * ? *",
            "scheduleExpressionTimezone": "UTC",
            "executionWindowInMinutes": 60
        },
        "status": "Active"
    })
}

/// Create a rule named `name` in `status` carrying `tags`, and answer
/// `(ruleArn, ruleRevision)`.
///
/// An empty `tags` sends no `tags` member at all, which is the state a plain
/// create leaves. `pub(crate)` because more than one suite needs a rule in a
/// chosen status with a chosen tag list to write against.
pub(crate) async fn create_rule(
    router: &MultiServiceRouter,
    name: &str,
    status: &str,
    tags: &Value,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule(name);
    request["status"] = json!(status);
    if !tags.as_array().is_some_and(Vec::is_empty) {
        request["tags"] = tags.clone();
    }
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision")
            .as_i64()
            .expect("the created rule carries a revision"),
    )
}

/// The rule `GetAutomationRule` reports for `rule_arn`.
pub(crate) async fn read_rule(router: &MultiServiceRouter, rule_arn: &str, case: &str) -> Value {
    call_ok(
        router,
        "GetAutomationRule",
        &json!({"ruleArn": rule_arn}),
        case,
    )
    .await
}

/// The `AutomationRule` summary `ListAutomationRules` reports for `rule_arn` —
/// the second, independent reader of a stored rule.
pub(crate) async fn listed_rule(router: &MultiServiceRouter, rule_arn: &str, case: &str) -> Value {
    let listed = call_ok(
        router,
        "ListAutomationRules",
        &json!({"maxResults": 100}),
        case,
    )
    .await;
    let found = member(&listed, "automationRules")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|rule| string_member(rule, "ruleArn") == rule_arn);
    assert!(
        found.is_some(),
        "case {case:?}: the rule {rule_arn} must be listed, got {listed}"
    );
    found.unwrap_or(Value::Null)
}

/// The member, required to be present and an epoch-seconds `Timestamp`.
pub(crate) fn timestamp(body: &Value, name: &str, case: &str) -> f64 {
    let value = member(body, name).as_f64();
    assert!(
        value.is_some_and(|seconds| seconds > 0.0),
        "case {case:?}: {name} must be present and an epoch-seconds Timestamp, got {body}"
    );
    value.unwrap_or_default()
}

pub(crate) fn member<'a>(body: &'a Value, name: &str) -> &'a Value {
    body.get(name).unwrap_or(&Value::Null)
}

pub(crate) fn string_member(body: &Value, name: &str) -> String {
    member(body, name).as_str().unwrap_or_default().to_string()
}

/// `RuleId` is modelled as `^[0-9A-Za-z]{16}$`.
pub(crate) fn is_rule_id(value: &str) -> bool {
    value.len() == 16 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `NextToken` is modelled as `^[A-Za-z0-9+/=]+$`.
pub(crate) fn is_next_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

// ---------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------

/// Before any enrollment call the service reports the unenrolled default, and
/// afterwards it reports exactly what was stored — for every value of
/// `EnrollmentStatus` a caller may set. The two the member's doc excludes
/// ("The Pending and Failed options cannot be used to update the enrollment
/// status of an account") are refused and change nothing.
#[tokio::test]
async fn enrollment_reports_the_default_until_an_update_persists_one() {
    // The unenrolled harness: this case is about the status an account that
    // never enrolled reports, so nothing may have been stored for it first.
    let router = unenrolled_emulator_with_engine().0;

    let initial = call_ok(&router, "GetEnrollmentConfiguration", &json!({}), "initial").await;
    assert_eq!(
        string_member(&initial, "status"),
        "Inactive",
        "case \"initial\": an account that never enrolled must report Inactive, got {initial}"
    );
    assert!(
        initial.get("lastUpdatedTimestamp").is_none(),
        "case \"initial\": nothing was ever stored, so there is no update time: {initial}"
    );

    // The `EnrollmentStatus` value set, from the Smithy model, with whether a
    // caller may set it (see the `status` member's doc).
    let cases = [
        ("Active", true),
        ("Inactive", true),
        ("Pending", false),
        ("Failed", false),
    ];

    let mut expected_status = "Inactive".to_string();
    let mut expected_timestamp = Value::Null;
    for (status_value, settable) in cases {
        let request = json!({ "status": status_value });
        if settable {
            let updated = call_ok(
                &router,
                "UpdateEnrollmentConfiguration",
                &request,
                status_value,
            )
            .await;
            assert_eq!(
                string_member(&updated, "status"),
                status_value,
                "case {status_value:?}: the update must report the status it stored, got {updated}"
            );
            let stamped = member(&updated, "lastUpdatedTimestamp").clone();
            assert!(
                stamped.as_f64().is_some_and(|t| t > 0.0),
                "case {status_value:?}: lastUpdatedTimestamp must be the store time, got {updated}"
            );
            assert_ne!(
                stamped, expected_timestamp,
                "case {status_value:?}: each update must stamp its own store time, got {updated}"
            );
            expected_status = status_value.to_string();
            expected_timestamp = stamped;
        } else {
            assert_refused(
                call(&router, "UpdateEnrollmentConfiguration", &request).await,
                400,
                "InvalidParameterValueException",
                status_value,
            );
        }

        let read_back =
            call_ok(&router, "GetEnrollmentConfiguration", &json!({}), status_value).await;
        assert_eq!(
            string_member(&read_back, "status"),
            expected_status,
            "case {status_value:?}: Get must report the persisted status, got {read_back}"
        );
        assert_eq!(
            member(&read_back, "lastUpdatedTimestamp").clone(),
            expected_timestamp,
            "case {status_value:?}: Get must report the update's own timestamp, got {read_back}"
        );
    }
}

/// "Inactive - Disables the Automation feature for your account and stops all of
/// your automation rules. If you opt in again later, all rules will be inactive"
/// — so opting out deactivates every rule, and opting back in leaves them so.
/// While the account is opted out the service performs none of its actions, so
/// the rules are readable again only once it is opted back in.
#[tokio::test]
async fn opting_out_stops_every_rule() {
    let router = emulator();
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Active"}),
        "enroll",
    )
    .await;

    let mut arns = Vec::new();
    for name in ["rule-one", "rule-two", "rule-three"] {
        let created = call_ok(&router, "CreateAutomationRule", &minimal_rule(name), name).await;
        arns.push(string_member(&created, "ruleArn"));
    }

    for opting in ["Inactive", "Active"] {
        call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": opting}),
            opting,
        )
        .await;
        for arn in &arns {
            let response = call(&router, "GetAutomationRule", &json!({"ruleArn": arn})).await;
            // While the account is opted OUT the service performs none of its
            // actions, so the rules are only readable again once it is opted
            // back in — and then they stand as the opt-out left them.
            if opting == "Inactive" {
                assert_refused(response, 403, "OptInRequiredException", opting);
                continue;
            }
            let (status, rule) = response;
            assert_eq!(
                status, 200,
                "case {opting:?}: an opted-in account reads its rules, got {status} and {rule}"
            );
            assert_eq!(
                string_member(&rule, "status"),
                "Inactive",
                "case {opting:?}: opting out stops every rule and opting in leaves them \
                 inactive, got {rule}"
            );
            assert_eq!(
                member(&rule, "ruleRevision").as_i64(),
                Some(1),
                "case {opting:?}: a service-driven stop is not a caller update, so the \
                 revision stands, got {rule}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Create / Get round trip
// ---------------------------------------------------------------------------

/// Every created rule is readable back field-for-field through
/// `GetAutomationRule`, across the span the shape allows: both rule types, one
/// and several recommended action types, with and without the optional
/// `description` / `priority` / `criteria` / `tags` / `organizationConfiguration`
/// members, and both `RuleStatus` values.
#[tokio::test]
async fn created_rules_round_trip_through_get() {
    let router = emulator();

    // The member account the organization-rule case scopes a rule to, in the
    // model's `AccountId` form, ASSOCIATED first: an organization rule applies
    // its recommended actions in the accounts its configuration names, which is
    // what an association permits, so an unassociated account cannot be scoped
    // (that gate is what `crate::organization_rule_scope_gate` is about). This
    // suite is about the round trip, so the precondition is established over the
    // wire here.
    const SCOPED_MEMBER: &str = "000000000001";
    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [SCOPED_MEMBER]}),
        "associate-the-scoped-member",
    )
    .await;

    // The action types and apply orders come from the model's value sets
    // (`RecommendedActionType`, `RuleApplyOrder`).
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", minimal_rule("minimal")),
        (
            "every-optional-member",
            json!({
                "name": "fully-specified_rule-1",
                "description": "Upgrades gp2 volumes in us-east-1.",
                "ruleType": "AccountRule",
                "priority": "0.25",
                "recommendedActionTypes": [
                    "SnapshotAndDeleteUnattachedEbsVolume",
                    "UpgradeEbsVolumeType"
                ],
                "criteria": {
                    "region": [{"comparison": "StringEquals", "values": ["us-east-1"]}],
                    "ebsVolumeSizeInGib": [{"comparison": "NumericLessThan", "values": [500]}],
                    "estimatedMonthlySavings": [
                        {"comparison": "NumericGreaterThan", "values": [12.5]}
                    ]
                },
                "schedule": {
                    "scheduleExpression": "0 3 * * ? *",
                    "scheduleExpressionTimezone": "America/New_York",
                    "executionWindowInMinutes": 1440
                },
                "status": "Inactive",
                "tags": [
                    {"key": "team", "value": "platform"},
                    {"key": "cost-center", "value": "4711"}
                ]
            }),
        ),
        (
            "organization-rule",
            json!({
                "name": "org-wide",
                "ruleType": "OrganizationRule",
                "organizationConfiguration": {
                    "ruleApplyOrder": "BeforeAccountRules",
                    "accountIds": [engine::EMULATOR_ACCOUNT_ID, SCOPED_MEMBER]
                },
                "recommendedActionTypes": ["SnapshotAndDeleteUnattachedEbsVolume"],
                "schedule": {"scheduleExpression": "15 1 * * ? *"},
                "status": "Active"
            }),
        ),
        (
            "priority-at-its-documented-bounds",
            json!({
                "name": "lowest-priority-value",
                "ruleType": "AccountRule",
                "priority": "0.000000000000000000000000000001",
                "recommendedActionTypes": ["UpgradeEbsVolumeType"],
                // A schedule carries the expression that says when the rule
                // runs, so this case names one; what it is about is the
                // priority bound above.
                "schedule": {
                    "scheduleExpression": "59 23 * * ? *",
                    "executionWindowInMinutes": 60
                },
                "status": "Active"
            }),
        ),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (case, request) in &cases {
        let created = call_ok(&router, "CreateAutomationRule", request, case).await;

        let rule_id = string_member(&created, "ruleId");
        assert!(
            is_rule_id(&rule_id),
            "case {case:?}: ruleId must match ^[0-9A-Za-z]{{16}}$, got {rule_id:?}"
        );
        assert!(
            !seen_ids.contains(&rule_id),
            "case {case:?}: every create mints a distinct ruleId, got a repeat of {rule_id:?}"
        );
        seen_ids.push(rule_id.clone());

        let rule_arn = string_member(&created, "ruleArn");
        let expected_arn = format!(
            "arn:aws:compute-optimizer::{}:automation-rule/{rule_id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            rule_arn, expected_arn,
            "case {case:?}: the ARN must name this account and this rule, got {created}"
        );
        assert_eq!(
            member(&created, "ruleRevision").as_i64(),
            Some(1),
            "case {case:?}: a new rule starts at revision 1, got {created}"
        );
        assert!(
            member(&created, "createdTimestamp")
                .as_f64()
                .is_some_and(|t| t > 0.0),
            "case {case:?}: createdTimestamp must be the store time, got {created}"
        );

        // Every member the request carried comes back as stored.
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        for (name, sent) in request.as_object().expect("the request is an object") {
            assert_eq!(
                member(&created, name),
                sent,
                "case {case:?}: create must echo the stored {name}, got {created}"
            );
            assert_eq!(
                member(&fetched, name),
                sent,
                "case {case:?}: Get must report the stored {name}, got {fetched}"
            );
        }
        assert_eq!(
            string_member(&fetched, "accountId"),
            engine::EMULATOR_ACCOUNT_ID,
            "case {case:?}: Get reports the owning account, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "ruleId").as_str(),
            Some(rule_id.as_str()),
            "case {case:?}: Get reports the minted ruleId, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "createdTimestamp"),
            member(&created, "createdTimestamp"),
            "case {case:?}: createdTimestamp is stable across reads, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "lastUpdatedTimestamp"),
            member(&fetched, "createdTimestamp"),
            "case {case:?}: a rule that was never updated was last changed when it was created, \
             got {fetched}"
        );
        // Tags are reported by Get whether or not the create carried any.
        if request.get("tags").is_none() {
            assert_eq!(
                member(&fetched, "tags"),
                &json!([]),
                "case {case:?}: a rule created without tags has none, got {fetched}"
            );
        }
    }

    // All four rules are in the store, in creation order.
    let listed = call_ok(&router, "ListAutomationRules", &json!({}), "list-all").await;
    let ids: Vec<String> = member(&listed, "automationRules")
        .as_array()
        .expect("automationRules is a list")
        .iter()
        .map(|rule| string_member(rule, "ruleId"))
        .collect();
    assert_eq!(
        ids, seen_ids,
        "case \"list-all\": the store holds exactly the created rules, in creation order"
    );
}

/// A create whose values break a rule the service enforces is refused, and
/// stores nothing.
#[tokio::test]
async fn an_invalid_create_is_refused_and_stores_nothing() {
    let router = emulator();

    let base = minimal_rule("valid-name");

    // (case, request) — the documented rules the model cannot express, plus
    // the modelled constraints the boundary enforces.
    let cases: Vec<(&str, Value)> = vec![
        ("empty-name", {
            let mut r = base.clone();
            r["name"] = json!("");
            r
        }),
        ("organization-rule-without-its-configuration", {
            let mut r = base.clone();
            r["ruleType"] = json!("OrganizationRule");
            r
        }),
        ("priority-above-one", {
            let mut r = base.clone();
            r["priority"] = json!("1.5");
            r
        }),
        ("priority-not-a-decimal", {
            let mut r = base.clone();
            r["priority"] = json!("high");
            r
        }),
        ("priority-with-31-fraction-digits", {
            let mut r = base.clone();
            r["priority"] = json!("0.0000000000000000000000000000001");
            r
        }),
        ("status-outside-the-enum", {
            let mut r = base.clone();
            r["status"] = json!("Paused");
            r
        }),
        ("rule-type-outside-the-enum", {
            let mut r = base.clone();
            r["ruleType"] = json!("RegionRule");
            r
        }),
        ("name-outside-its-pattern", {
            let mut r = base.clone();
            r["name"] = json!("not a legal name!");
            r
        }),
        ("execution-window-below-its-range", {
            let mut r = base.clone();
            r["schedule"] = json!({"executionWindowInMinutes": 59});
            r
        }),
        ("required-member-absent", {
            let mut r = base.clone();
            r.as_object_mut().expect("an object").remove("schedule");
            r
        }),
    ];

    for (case, request) in &cases {
        assert_refused(
            call(&router, "CreateAutomationRule", request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    let listed = call_ok(&router, "ListAutomationRules", &json!({}), "nothing-stored").await;
    assert_eq!(
        member(&listed, "automationRules"),
        &json!([]),
        "case \"nothing-stored\": no refused create may leave a rule behind, got {listed}"
    );

    // The same values, each on its own, are accepted — so the refusals above are
    // about the values and not about the request shape.
    for (case, priority) in [("zero", "0"), ("one", "1"), ("half", "0.5")] {
        let mut request = base.clone();
        request["priority"] = json!(priority);
        request["name"] = json!(format!("priority-{case}"));
        let created = call_ok(&router, "CreateAutomationRule", &request, case).await;
        assert_eq!(
            string_member(&created, "priority"),
            priority,
            "case {case:?}: a priority between 0 and 1 is stored as sent, got {created}"
        );
    }
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// An update applies only the members it carries, bumps the revision by one,
/// stamps `lastUpdatedTimestamp`, and refuses a revision that is not the current
/// one.
#[tokio::test]
async fn update_applies_only_what_it_carries_and_is_revision_checked() {
    let router = emulator();
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &json!({
            "name": "rule-under-test",
            "description": "original description",
            "ruleType": "AccountRule",
            "priority": "0.5",
            "recommendedActionTypes": ["UpgradeEbsVolumeType"],
            "criteria": {"region": [{"comparison": "StringEquals", "values": ["eu-west-1"]}]},
            "schedule": {"scheduleExpression": "30 12 * * ? *", "executionWindowInMinutes": 120},
            "status": "Active",
            "tags": [{"key": "team", "value": "platform"}]
        }),
        "create",
    )
    .await;
    let rule_arn = string_member(&created, "ruleArn");

    // (case, the single member the update carries)
    let cases: Vec<(&str, &str, Value)> = vec![
        ("status", "status", json!("Inactive")),
        ("name", "name", json!("renamed_rule-2")),
        ("description", "description", json!("a new description")),
        ("priority", "priority", json!("0.125")),
        (
            "recommendedActionTypes",
            "recommendedActionTypes",
            json!(["SnapshotAndDeleteUnattachedEbsVolume"]),
        ),
        (
            "schedule",
            "schedule",
            json!({"scheduleExpression": "0 4 * * ? *", "executionWindowInMinutes": 60}),
        ),
        (
            "criteria",
            "criteria",
            json!({"ebsVolumeType": [{"comparison": "StringEquals", "values": ["gp2"]}]}),
        ),
    ];

    let mut revision = member(&created, "ruleRevision")
        .as_i64()
        .expect("the create reported a revision");
    let mut expected = call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": &rule_arn}),
        "baseline",
    )
    .await;

    for (case, name, value) in &cases {
        // A stale revision is refused, and changes nothing.
        assert_refused(
            call(
                &router,
                "UpdateAutomationRule",
                &json!({"ruleArn": &rule_arn, "ruleRevision": revision - 1, (*name): value}),
            )
            .await,
            400,
            "InvalidParameterValueException",
            case,
        );
        let unchanged = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&unchanged, name),
            member(&expected, name),
            "case {case:?}: a refused update must not apply, got {unchanged}"
        );
        assert_eq!(
            member(&unchanged, "ruleRevision").as_i64(),
            Some(revision),
            "case {case:?}: a refused update must not bump the revision, got {unchanged}"
        );

        let updated = call_ok(
            &router,
            "UpdateAutomationRule",
            &json!({"ruleArn": &rule_arn, "ruleRevision": revision, (*name): value}),
            case,
        )
        .await;
        assert_eq!(
            member(&updated, "ruleRevision").as_i64(),
            Some(revision + 1),
            "case {case:?}: an update bumps the revision by one, got {updated}"
        );
        assert_eq!(
            member(&updated, name),
            value,
            "case {case:?}: the supplied {name} must be applied, got {updated}"
        );
        assert!(
            member(&updated, "lastUpdatedTimestamp")
                .as_f64()
                .is_some_and(|t| t > 0.0),
            "case {case:?}: an update stamps lastUpdatedTimestamp, got {updated}"
        );
        assert_eq!(
            member(&updated, "createdTimestamp"),
            member(&created, "createdTimestamp"),
            "case {case:?}: an update never moves createdTimestamp, got {updated}"
        );

        revision += 1;
        let fetched = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": &rule_arn}),
            case,
        )
        .await;
        // Only the member the update carried differs from the rule as it stood.
        for (field, previous) in expected.as_object().expect("an object") {
            if field == name || field == "ruleRevision" || field == "lastUpdatedTimestamp" {
                continue;
            }
            assert_eq!(
                member(&fetched, field),
                previous,
                "case {case:?}: updating {name} must leave {field} alone, got {fetched}"
            );
        }
        assert_eq!(
            member(&fetched, "ruleRevision").as_i64(),
            Some(revision),
            "case {case:?}: Get reports the bumped revision, got {fetched}"
        );
        assert_eq!(
            member(&fetched, "lastUpdatedTimestamp"),
            member(&updated, "lastUpdatedTimestamp"),
            "case {case:?}: Get reports the update's own timestamp, got {fetched}"
        );
        expected = fetched;
    }

    // An update of a rule that does not exist is a 404, whatever revision it
    // carries.
    assert_refused(
        call(
            &router,
            "UpdateAutomationRule",
            &json!({
                "ruleArn": format!(
                    "arn:aws:compute-optimizer::{}:automation-rule/0000000000000000",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "ruleRevision": 1,
                "status": "Inactive"
            }),
        )
        .await,
        404,
        "ResourceNotFoundException",
        "unknown-rule",
    );
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// `ListAutomationRules` reports what the store holds, pages it at `maxResults`,
/// and resumes exactly where the previous page stopped — for every page size
/// from one to one past the number of rules.
#[tokio::test]
async fn list_pages_over_the_stored_rules() {
    let empty = emulator();
    let listed = call_ok(&empty, "ListAutomationRules", &json!({}), "empty-store").await;
    assert_eq!(
        member(&listed, "automationRules"),
        &json!([]),
        "case \"empty-store\": an empty store yields an empty list, got {listed}"
    );
    assert!(
        listed.get("nextToken").is_none(),
        "case \"empty-store\": there is no further page, got {listed}"
    );

    let router = emulator();
    let mut created_arns = Vec::new();
    for name in ["rule-a", "rule-b", "rule-c", "rule-d", "rule-e"] {
        let created = call_ok(&router, "CreateAutomationRule", &minimal_rule(name), name).await;
        created_arns.push(string_member(&created, "ruleArn"));
    }

    for page_size in 1..=6usize {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = call_ok(&router, "ListAutomationRules", &request, &case).await;
            let rules = member(&page, "automationRules")
                .as_array()
                .expect("automationRules is a list")
                .clone();
            let remaining = created_arns.len() - walked.len();
            assert_eq!(
                rules.len(),
                remaining.min(page_size),
                "case {case:?}: a page holds min(maxResults, remaining) rules, got {page}"
            );
            walked.extend(rules.iter().map(|rule| string_member(rule, "ruleArn")));

            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if let Some(token) = &token {
                assert!(
                    is_next_token(token),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                );
            }
            pages += 1;
            assert!(
                pages <= created_arns.len() + 1,
                "case {case:?}: the walk must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created_arns,
            "case {case:?}: the walk must report every rule once, in creation order"
        );
        assert_eq!(
            pages,
            created_arns.len().div_ceil(page_size),
            "case {case:?}: no page beyond the last, and no nextToken on it"
        );
    }

    // A token this service did not issue, and one issued for other filters, are
    // both refused rather than answered with a page of something else.
    let first = call_ok(
        &router,
        "ListAutomationRules",
        &json!({"maxResults": 1}),
        "token-reuse",
    )
    .await;
    let token = string_member(&first, "nextToken");
    assert_refused(
        call(
            &router,
            "ListAutomationRules",
            &json!({"maxResults": 1, "nextToken": token, "filters": [
                {"name": "Status", "values": ["Active"]}
            ]}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "token-with-different-filters",
    );
    assert_refused(
        call(
            &router,
            "ListAutomationRules",
            &json!({"nextToken": "notatokenweissued"}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "foreign-token",
    );
}

/// Each `AutomationRuleFilterName` selects on the field it names.
#[tokio::test]
async fn list_filters_select_on_the_field_they_name() {
    let router = emulator();

    let mut account_rule = minimal_rule("account-scoped");
    account_rule["status"] = json!("Inactive");
    let account = call_ok(&router, "CreateAutomationRule", &account_rule, "create-account").await;

    let org = call_ok(
        &router,
        "CreateAutomationRule",
        &json!({
            "name": "org-scoped",
            "ruleType": "OrganizationRule",
            "organizationConfiguration": {"ruleApplyOrder": "AfterAccountRules"},
            "recommendedActionTypes": ["SnapshotAndDeleteUnattachedEbsVolume"],
            "schedule": {"scheduleExpression": "0 5 * * ? *"},
            "status": "Active"
        }),
        "create-org",
    )
    .await;

    let account_arn = string_member(&account, "ruleArn");
    let org_arn = string_member(&org, "ruleArn");

    // (case, filter, the ARNs the filter must select) — one row per value of the
    // model's `AutomationRuleFilterName`.
    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "Name",
            json!([{"name": "Name", "values": ["org-scoped"]}]),
            vec![&org_arn],
        ),
        (
            "RecommendedActionType",
            json!([{"name": "RecommendedActionType", "values": ["UpgradeEbsVolumeType"]}]),
            vec![&account_arn],
        ),
        (
            "Status",
            json!([{"name": "Status", "values": ["Inactive"]}]),
            vec![&account_arn],
        ),
        (
            "RuleType",
            json!([{"name": "RuleType", "values": ["OrganizationRule"]}]),
            vec![&org_arn],
        ),
        (
            "OrganizationConfigurationRuleApplyOrder",
            json!([{
                "name": "OrganizationConfigurationRuleApplyOrder",
                "values": ["AfterAccountRules"]
            }]),
            vec![&org_arn],
        ),
        (
            "AccountId",
            json!([{"name": "AccountId", "values": [engine::EMULATOR_ACCOUNT_ID]}]),
            vec![&account_arn, &org_arn],
        ),
        (
            "AccountId-no-match",
            json!([{"name": "AccountId", "values": ["999999999999"]}]),
            vec![],
        ),
        (
            "two-filters-are-anded",
            json!([
                {"name": "RuleType", "values": ["OrganizationRule"]},
                {"name": "Status", "values": ["Inactive"]}
            ]),
            vec![],
        ),
    ];

    for (case, filters, expected) in &cases {
        let listed = call_ok(
            &router,
            "ListAutomationRules",
            &json!({"filters": filters}),
            case,
        )
        .await;
        let selected: Vec<String> = member(&listed, "automationRules")
            .as_array()
            .expect("automationRules is a list")
            .iter()
            .map(|rule| string_member(rule, "ruleArn"))
            .collect();
        let expected: Vec<String> = expected.iter().map(|arn| (*arn).clone()).collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: the filter must select exactly those rules, got {listed}"
        );
    }

    // A filter name outside the modelled value set is refused, not ignored.
    assert_refused(
        call(
            &router,
            "ListAutomationRules",
            &json!({"filters": [{"name": "RuleArn", "values": [account_arn]}]}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "unknown-filter-name",
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// A delete is revision-checked, and once it lands every later reader of that
/// ARN sees a 404 — including a second delete.
#[tokio::test]
async fn delete_is_revision_checked_and_observable() {
    let router = emulator();
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("doomed-rule"),
        "create",
    )
    .await;
    let rule_arn = string_member(&created, "ruleArn");

    // Move the revision on, so the create's revision is stale.
    let updated = call_ok(
        &router,
        "UpdateAutomationRule",
        &json!({"ruleArn": &rule_arn, "ruleRevision": 1, "status": "Inactive"}),
        "update",
    )
    .await;
    let revision = member(&updated, "ruleRevision")
        .as_i64()
        .expect("a revision");

    assert_refused(
        call(
            &router,
            "DeleteAutomationRule",
            &json!({"ruleArn": &rule_arn, "ruleRevision": revision - 1}),
        )
        .await,
        400,
        "InvalidParameterValueException",
        "stale-revision",
    );
    call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": &rule_arn}),
        "survives-a-refused-delete",
    )
    .await;

    let deleted = call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({"ruleArn": &rule_arn, "ruleRevision": revision}),
        "delete",
    )
    .await;
    assert_eq!(
        deleted,
        json!({}),
        "case \"delete\": the modelled response carries no members, got {deleted}"
    );

    for case in ["get-after-delete", "second-delete", "update-after-delete"] {
        let refused = match case {
            "get-after-delete" => call(&router, "GetAutomationRule", &json!({"ruleArn": &rule_arn}))
                .await,
            "second-delete" => call(
                &router,
                "DeleteAutomationRule",
                &json!({"ruleArn": &rule_arn, "ruleRevision": revision}),
            )
            .await,
            _ => call(
                &router,
                "UpdateAutomationRule",
                &json!({"ruleArn": &rule_arn, "ruleRevision": revision, "status": "Active"}),
            )
            .await,
        };
        assert_refused(refused, 404, "ResourceNotFoundException", case);
    }

    let listed = call_ok(&router, "ListAutomationRules", &json!({}), "list-after-delete").await;
    assert_eq!(
        member(&listed, "automationRules"),
        &json!([]),
        "case \"list-after-delete\": a delete is observable by the next reader, got {listed}"
    );
}

/// A resource string that names no automation rule of this service is refused
/// by AUTHORIZATION (403 `AccessDeniedException`) — authorization is evaluated
/// against the string as supplied, before the service parses it — while a
/// well-formed ARN no rule has gets as far as the lookup and is a 404.
#[tokio::test]
async fn an_arn_is_authorized_on_its_form_then_looked_up() {
    let router = emulator();

    // (case, ruleArn, expected status)
    let cases = [
        ("not-an-arn", "my-rule".to_string(), 403u16),
        (
            "wrong-service-namespace",
            format!(
                "arn:aws:compute-optimizer-automation::{}:automation-rule/abcdefghij123456",
                engine::EMULATOR_ACCOUNT_ID
            ),
            403,
        ),
        (
            "account-not-twelve-digits",
            "arn:aws:compute-optimizer::1:automation-rule/abcdefghij123456".to_string(),
            403,
        ),
        (
            "well-formed-but-unknown",
            format!(
                "arn:aws:compute-optimizer::{}:automation-rule/abcdefghij123456",
                engine::EMULATOR_ACCOUNT_ID
            ),
            404,
        ),
    ];

    for (case, rule_arn, expected_status) in &cases {
        let expected_code = if *expected_status == 403 {
            "AccessDeniedException"
        } else {
            "ResourceNotFoundException"
        };
        assert_refused(
            call(&router, "GetAutomationRule", &json!({"ruleArn": rule_arn})).await,
            *expected_status,
            expected_code,
            case,
        );
    }
}

// ---------------------------------------------------------------------------
// Quota
// ---------------------------------------------------------------------------

/// The store is bounded: once an account holds `MAX_RULES_PER_ACCOUNT` rules a
/// further create is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402), the rules already stored are untouched, and deleting one makes
/// room for exactly one more.
#[tokio::test]
async fn creating_past_the_rule_quota_is_refused() {
    use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::MAX_RULES_PER_ACCOUNT;

    let router = emulator();
    let mut arns = Vec::with_capacity(MAX_RULES_PER_ACCOUNT);
    for index in 0..MAX_RULES_PER_ACCOUNT {
        let created = call_ok(
            &router,
            "CreateAutomationRule",
            &minimal_rule(&format!("rule-{index}")),
            "fill-to-the-quota",
        )
        .await;
        arns.push(string_member(&created, "ruleArn"));
    }

    assert_refused(
        call(
            &router,
            "CreateAutomationRule",
            &minimal_rule("one-rule-too-many"),
        )
        .await,
        402,
        "ServiceQuotaExceededException",
        "one-rule-too-many",
    );
    let listed = call_ok(
        &router,
        "ListAutomationRules",
        &json!({"maxResults": MAX_RULES_PER_ACCOUNT}),
        "still-at-the-quota",
    )
    .await;
    assert_eq!(
        member(&listed, "automationRules").as_array().map(Vec::len),
        Some(MAX_RULES_PER_ACCOUNT),
        "case \"still-at-the-quota\": a refused create leaves the store exactly as it was"
    );

    call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({"ruleArn": &arns[0], "ruleRevision": 1}),
        "make-room",
    )
    .await;
    call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("replacement-rule"),
        "make-room",
    )
    .await;
    assert_refused(
        call(
            &router,
            "CreateAutomationRule",
            &minimal_rule("still-too-many"),
        )
        .await,
        402,
        "ServiceQuotaExceededException",
        "still-too-many",
    );
}

// ---------------------------------------------------------------------------
// Client tokens
// ---------------------------------------------------------------------------

/// A repeated `clientToken` replays the first call's result; the same token with
/// different parameters is a mismatch, and the same token on another operation
/// is already in use.
#[tokio::test]
async fn a_client_token_makes_a_create_idempotent() {
    let router = emulator();

    let mut request = minimal_rule("tokened-rule");
    request["clientToken"] = json!("token-one");

    let first = call_ok(&router, "CreateAutomationRule", &request, "first").await;
    let replay = call_ok(&router, "CreateAutomationRule", &request, "replay").await;
    assert_eq!(
        first, replay,
        "case \"replay\": a repeated client token must replay the first result, not mint a \
         second rule"
    );

    let listed = call_ok(&router, "ListAutomationRules", &json!({}), "one-rule").await;
    assert_eq!(
        member(&listed, "automationRules")
            .as_array()
            .map(Vec::len),
        Some(1),
        "case \"one-rule\": the replayed create must not have stored a second rule, got {listed}"
    );

    let mut changed = request.clone();
    changed["status"] = json!("Inactive");
    assert_refused(
        call(&router, "CreateAutomationRule", &changed).await,
        409,
        "IdempotentParameterMismatchException",
        "same-token-different-parameters",
    );

    assert_refused(
        call(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": "Active", "clientToken": "token-one"}),
        )
        .await,
        409,
        "IdempotencyTokenInUseException",
        "same-token-another-operation",
    );

    // A fresh token mints a new rule.
    let mut second = minimal_rule("second-tokened-rule");
    second["clientToken"] = json!("token-two");
    let other = call_ok(&router, "CreateAutomationRule", &second, "fresh-token").await;
    assert_ne!(
        string_member(&other, "ruleId"),
        string_member(&first, "ruleId"),
        "case \"fresh-token\": a different client token mints a different rule"
    );
}
