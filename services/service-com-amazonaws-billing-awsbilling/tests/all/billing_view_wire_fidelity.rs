// SPDX-License-Identifier: CC0-1.0

//! What the billing-view surface puts ON THE WIRE, end to end through the
//! router. Five rules, each over a table that spans the class of inputs it
//! generalises over rather than one request:
//!
//! 1. A billing view's `healthStatus` always carries BOTH members — the stored
//!    status code and its reason list, empty when the service recorded no
//!    reason — and this surface carries each of them under both wire spellings
//!    (`statusCode` beside `StatusCode`), so a client keyed on either reads the
//!    same value. The rows span every `BillingViewStatus` the model declares and
//!    reason lists of several sizes.
//! 2. The description of the account's default `PRIMARY` view is one the SERVICE
//!    wrote, and it names the account that owns the view: it is derived from the
//!    owner, not a constant.
//! 3. A view the service manages on the account's behalf reports none of the
//!    lifecycle metadata of a caller-created one — no `createdAt`, no
//!    `sourceViewCount`, no `derivedViewCount` — while a `CUSTOM` view reports
//!    all three. The rows span every `BillingViewType` the model declares.
//! 4. A response to a request this service ACCEPTED announces
//!    `application/x-amz-json-1.1`; a request REFUSED before the operation ran
//!    keeps the `application/x-amz-json-1.0` the model declares.
//! 5. A refusal of a member outside its modelled constraints carries the
//!    modelled `reason` and one `fieldList` entry naming that member, whatever
//!    the operation and whatever the constraint class.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::ops::types::{
    BILLING_VIEW_STATUS_REASON_VALUES, BILLING_VIEW_STATUS_VALUES, BILLING_VIEW_TYPE_VALUES,
};
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{
    call, call_with_headers, emulator_with_engine, error_code, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN,
};
use crate::rejection_fidelity::{message, ARN_PATTERN, MONTH_PATTERN, NAME_PATTERN};

/// The fixed first half of the description the service writes for the default
/// `PRIMARY` view; the owning account id completes it. Spelled out here so the
/// expectation is written independently of the service code that builds it.
const PRIMARY_DESCRIPTION_PREFIX: &str = "Primary billing view for accountId: ";

/// The content-type a response to an ACCEPTED request announces.
const JSON_1_1: &str = "application/x-amz-json-1.1";

/// The content-type a refusal rendered before the operation ran announces — the
/// version this service's model declares.
const JSON_1_0: &str = "application/x-amz-json-1.0";

/// The `ValidationExceptionReason` a refusal about a request member carries.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The shape id the wire stamps on a `ValidationExceptionField` entry's `__type`
/// discriminator: the endpoint serving this surface names its error shapes in
/// its own namespace, not this model's.
const FIELD_WIRE_TYPE: &str = "com.amazonaws.awsinsightsindexservice.v0#ValidationExceptionField";

/// What a refusal puts where it would otherwise echo the rejected value.
const REDACTED: &str = "[redacted]";

/// The `content-type` a response announced.
fn content_type(headers: &http::HeaderMap) -> String {
    headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Put the stored view's health status at `code` with `reasons`, the one way a
/// view's health moves in this emulator: no modelled operation sets it.
fn set_health(engine: &Arc<engine::InMemoryEngine>, arn: &str, code: &str, reasons: &[String]) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    let view = state
        .billing_views
        .get_mut(arn)
        .expect("the harness knows this view");
    view.health_status_code = code.to_string();
    view.health_status_reasons = reasons.to_vec();
}

/// Put the stored view's `billingViewType` at `view_type`: no modelled operation
/// mints a view of any type but `CUSTOM`, so this is how a managed view other
/// than the account's `PRIMARY` one comes to exist in a test.
fn set_view_type(engine: &Arc<engine::InMemoryEngine>, arn: &str, view_type: &str) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    state
        .billing_views
        .get_mut(arn)
        .expect("the harness created this view")
        .billing_view_type = view_type.to_string();
}

/// Create one `CUSTOM` view drawing from `sources` and return its minted ARN.
async fn create_view(router: &MultiServiceRouter, name: &str, sources: &[String]) -> String {
    let (status, created) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": sources, "clientToken": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the harness create must answer 200, got {status} {created:?}"
    );
    created["arn"].as_str().unwrap_or_default().to_string()
}

/// The `billingView` `GetBillingView` reports for `arn`.
async fn get_view(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBillingView to answer 200, got {status} {body:?}"
    );
    body["billingView"].clone()
}

/// The entry `ListBillingViews` reports for `arn`.
async fn listed_view(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(router, "ListBillingViews", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListBillingViews to answer 200, got {status} {body:?}"
    );
    body["billingViews"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListBillingViews answered no list: {body:?}"))
        .iter()
        .find(|view| view["arn"].as_str() == Some(arn))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {arn} is not in the list: {body:?}"))
}

/// Every health-status key a read must carry, as a map of key -> value, built
/// from the status the store holds. The two spellings carry one value each way.
fn expected_health(code: &str, reasons: &[String]) -> BTreeMap<String, Value> {
    let reasons = Value::Array(reasons.iter().map(|reason| json!(reason)).collect());
    [
        ("statusCode".to_string(), json!(code)),
        ("StatusCode".to_string(), json!(code)),
        ("statusReasons".to_string(), reasons.clone()),
        ("StatusReasons".to_string(), reasons),
    ]
    .into_iter()
    .collect()
}

/// The health status a response carried, as a map, so an assertion is about the
/// WHOLE object — every key it carries and no other.
fn health_of(view: &Value) -> BTreeMap<String, Value> {
    view["healthStatus"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

#[tokio::test]
async fn a_health_status_is_reported_under_both_wire_spellings() {
    let (router, engine) = emulator_with_engine();
    let custom = create_view(&router, "health-view", &[PRIMARY_ARN.to_string()]).await;

    // The rows differ along both members of the status: every `BillingViewStatus`
    // the model declares (source: `ops::types::BILLING_VIEW_STATUS_VALUES`)
    // against reason lists of none, one and several entries drawn from
    // `ops::types::BILLING_VIEW_STATUS_REASON_VALUES` — including entries the
    // recorded case never carried.
    let catalogue: Vec<String> = BILLING_VIEW_STATUS_REASON_VALUES
        .iter()
        .map(|reason| (*reason).to_string())
        .collect();
    assert!(
        BILLING_VIEW_STATUS_VALUES.len() > 1 && catalogue.len() > 4,
        "case \"model catalogues\": expected the modelled status and reason sets, got {:?} {:?}",
        BILLING_VIEW_STATUS_VALUES,
        catalogue
    );
    let reason_sets: Vec<Vec<String>> = vec![
        Vec::new(),
        vec![catalogue[0].clone()],
        vec![catalogue[1].clone(), catalogue[4].clone()],
        catalogue.clone(),
    ];

    for code in BILLING_VIEW_STATUS_VALUES {
        for reasons in &reason_sets {
            let case = format!("{code} with {} reason(s)", reasons.len());
            for arn in [PRIMARY_ARN, custom.as_str()] {
                set_health(&engine, arn, code, reasons);
                let expected = expected_health(code, reasons);
                let read = health_of(&get_view(&router, &case, arn).await);
                assert_eq!(
                    read, expected,
                    "case {case:?}: GetBillingView({arn}) must report the stored health status \
                     under both spellings, got {read:?}"
                );
                let listed = health_of(&listed_view(&router, &case, arn).await);
                assert_eq!(
                    listed, expected,
                    "case {case:?}: ListBillingViews({arn}) must report the same object, \
                     got {listed:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_default_views_description_is_built_from_the_account_that_owns_it() {
    let (router, _engine) = emulator_with_engine();

    // The description is not a constant: it is the service's fixed sentence
    // followed by the id of the account that owns the view — the very account
    // the same record reports in `ownerAccountId`.
    for (case, view) in [
        ("GetBillingView", get_view(&router, "get", PRIMARY_ARN).await),
        (
            "ListBillingViews",
            listed_view(&router, "list", PRIMARY_ARN).await,
        ),
    ] {
        let owner = view["ownerAccountId"].as_str().unwrap_or_default();
        assert_eq!(
            view["description"].as_str(),
            Some(format!("{PRIMARY_DESCRIPTION_PREFIX}{owner}").as_str()),
            "case {case:?}: the description names the owning account, got {:?}",
            view["description"]
        );
        assert_eq!(
            owner, ACCOUNT_ID,
            "case {case:?}: the emulated account owns its own default view, got {owner:?}"
        );
    }
}

#[tokio::test]
async fn only_a_caller_created_view_reports_lifecycle_metadata() {
    let (router, engine) = emulator_with_engine();

    // The members a view reports only when a CALLER created it.
    let caller_only = ["createdAt", "sourceViewCount", "derivedViewCount"];

    // The rows span every `BillingViewType` the model declares (source:
    // `ops::types::BILLING_VIEW_TYPE_VALUES`): `CUSTOM` is the only one a caller
    // can create, every other one names a view the service maintains. Each row's
    // view holds a source view, so a reported count would not have been zero.
    for (index, view_type) in BILLING_VIEW_TYPE_VALUES.iter().enumerate() {
        let arn = create_view(
            &router,
            &format!("typed-view-{index}"),
            &[PRIMARY_ARN.to_string()],
        )
        .await;
        set_view_type(&engine, &arn, view_type);
        let view = get_view(&router, view_type, &arn).await;

        if *view_type == "CUSTOM" {
            assert_eq!(
                (
                    view["sourceViewCount"].as_u64(),
                    view["derivedViewCount"].as_u64(),
                    view["createdAt"].is_number()
                ),
                (Some(1), Some(0), true),
                "case {view_type:?}: a caller-created view reports its lifecycle metadata, \
                 got {view:?}"
            );
            continue;
        }
        for member in caller_only {
            assert!(
                view.get(member).is_none(),
                "case {view_type:?}: a service-managed view reports no {member:?}, got {view:?}"
            );
        }
    }

    // The account's own default view is managed the same way, and the members it
    // DOES carry are unaffected.
    let primary = get_view(&router, "the default PRIMARY view", PRIMARY_ARN).await;
    for member in caller_only {
        assert!(
            primary.get(member).is_none(),
            "case \"the default PRIMARY view\": expected no {member:?}, got {primary:?}"
        );
    }
    assert_eq!(
        (
            primary["arn"].as_str(),
            primary["billingViewType"].as_str(),
            primary["name"].as_str().map(str::is_empty)
        ),
        (Some(PRIMARY_ARN), Some("PRIMARY"), Some(false)),
        "case \"the default PRIMARY view\": the members it does carry are unchanged, \
         got {primary:?}"
    );
}

#[tokio::test]
async fn an_accepted_request_announces_json_1_1_and_a_refusal_announces_1_0() {
    let (router, _engine) = emulator_with_engine();
    let custom = create_view(&router, "content-type-view", &[PRIMARY_ARN.to_string()]).await;

    // (case, operation, request, expected status, expected content-type). The
    // rows span the three kinds of answer: a success, an error the operation
    // itself raised, and a refusal of the request before the operation ran.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a read of the default view",
            "GetBillingView",
            json!({ "arn": PRIMARY_ARN }),
            200,
            JSON_1_1,
        ),
        (
            "a read of a created view",
            "GetBillingView",
            json!({ "arn": custom }),
            200,
            JSON_1_1,
        ),
        ("an unfiltered list", "ListBillingViews", json!({}), 200, JSON_1_1),
        (
            "a list with a filter",
            "ListBillingViews",
            json!({ "billingViewTypes": ["PRIMARY"] }),
            200,
            JSON_1_1,
        ),
        (
            "an ARN no view is stored under",
            "GetBillingView",
            json!({ "arn": ABSENT_ARN }),
            404,
            JSON_1_1,
        ),
        (
            "an ARN outside the caller's account",
            "GetBillingView",
            json!({ "arn": "arn:aws:billing::111122223333:billingview/primary" }),
            403,
            JSON_1_1,
        ),
        (
            "an ARN outside its modelled pattern",
            "GetBillingView",
            json!({ "arn": "not-an-arn" }),
            400,
            JSON_1_0,
        ),
        (
            "an absent required member",
            "GetBillingView",
            json!({}),
            400,
            JSON_1_0,
        ),
        (
            "a name outside its modelled pattern",
            "CreateBillingView",
            json!({ "name": "bad/name!", "sourceViews": [PRIMARY_ARN] }),
            400,
            JSON_1_0,
        ),
    ];

    for (case, operation, request, expected_status, expected_type) in rows {
        let (status, headers, body) = call_with_headers(&router, operation, &request).await;
        assert_eq!(
            (status, content_type(&headers).as_str()),
            (expected_status, expected_type),
            "case {case:?}: expected {expected_status} {expected_type}, got {status} {:?} {body:?}",
            content_type(&headers)
        );
    }
}

#[tokio::test]
async fn a_constraint_refusal_carries_the_reason_and_the_field_that_failed() {
    let (router, _engine) = emulator_with_engine();
    let over_long_name = "n".repeat(129);

    // (case, operation, request, member, the requirement the entry states, the
    // rejected value). The rows span the constraint classes the model puts on
    // this service — pattern, length, range, required — across four operations
    // and four members, including a `@sensitive` one.
    let rows: Vec<(&str, &str, Value, &str, String, String)> = vec![
        (
            "an arn outside its pattern",
            "GetBillingView",
            json!({ "arn": "not-an-arn" }),
            "arn",
            format!("Member must satisfy regular expression pattern: {ARN_PATTERN}"),
            "not-an-arn".to_string(),
        ),
        (
            "an arn absent altogether",
            "GetBillingView",
            json!({}),
            "arn",
            "Member must not be null".to_string(),
            String::new(),
        ),
        (
            "an arn outside its pattern on another operation",
            "DeleteBillingView",
            json!({ "arn": "billingview/primary" }),
            "arn",
            format!("Member must satisfy regular expression pattern: {ARN_PATTERN}"),
            "billingview/primary".to_string(),
        ),
        (
            "a sensitive name outside its pattern",
            "CreateBillingView",
            json!({ "name": "bad/name!", "sourceViews": [PRIMARY_ARN] }),
            "name",
            format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            "bad/name!".to_string(),
        ),
        (
            "a name over its length maximum",
            "CreateBillingView",
            json!({ "name": over_long_name.clone(), "sourceViews": [PRIMARY_ARN] }),
            "name",
            "Member must have length less than or equal to 128".to_string(),
            over_long_name.clone(),
        ),
        (
            "maxResults below its range minimum",
            "ListBillingViews",
            json!({ "maxResults": 0 }),
            "maxResults",
            "Member must have value greater than or equal to 1".to_string(),
            String::new(),
        ),
        (
            "maxResults above its range maximum",
            "ListBillingViews",
            json!({ "maxResults": 101 }),
            "maxResults",
            "Member must have value less than or equal to 100".to_string(),
            String::new(),
        ),
        (
            "a billingMonth outside its pattern",
            "GetEnterpriseSupportChargeSummary",
            json!({ "billingMonth": "2024-13" }),
            "billingMonth",
            format!("Member must satisfy regular expression pattern: {MONTH_PATTERN}"),
            "2024-13".to_string(),
        ),
    ];

    for (case, operation, request, member, requirement, rejected) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body:?}"
        );
        assert_eq!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: a member refusal carries the modelled reason, got {:?}",
            body["reason"]
        );
        let required = requirement == "Member must not be null";
        let field_subject = if required { "null" } else { REDACTED };
        let envelope_subject = if required { "Value null" } else { "Value" };
        assert_eq!(
            body["fieldList"].as_array().map(Vec::as_slice),
            Some(
                [json!({
                    "name": member,
                    "message": format!(
                        "Value {field_subject} failed to satisfy constraint: {requirement}"
                    ),
                    "__type": FIELD_WIRE_TYPE,
                })]
                .as_slice()
            ),
            "case {case:?}: expected one fieldList entry naming {member:?}, got {:?}",
            body["fieldList"]
        );
        // The message envelope names the member path and the constraint, and the
        // rejected value is nowhere in the response.
        assert_eq!(
            message(&body),
            format!(
                "1 validation error detected: {envelope_subject} at '{member}' failed to \
                 satisfy constraint: {requirement}"
            ),
            "case {case:?}: expected the AWS envelope, got {:?}",
            message(&body)
        );
        if !rejected.is_empty() {
            assert!(
                !body.to_string().contains(&rejected),
                "case {case:?}: the rejected value must not be echoed back, got {body:?}"
            );
        }
    }
}
