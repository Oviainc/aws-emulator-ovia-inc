// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Free Tier loop — plan state → upgrade → re-read →
//! activity list → activity detail → usage list — driven over the wire
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSFreeTierService.<Operation>`)
//! through the same multi-service router the emulator boots.
//!
//! Every assertion is about a RULE over a table of inputs rather than one
//! recorded call: what the store reports after a write, which rows a filter
//! selects, where a page resumes, and that the detail view of an activity
//! agrees with its list entry. Where the expectation is derivable, it is
//! computed from what the service itself reports, so a test asserts the rule
//! and not the catalog's contents.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    ComAmazonawsFreetierAwsfreetierserviceState, ACTIVITY_STATUS_COMPLETED,
    ACTIVITY_STATUS_IN_PROGRESS, PLAN_STATUS_ACTIVE, PLAN_STATUS_EXPIRED, PLAN_TYPE_FREE,
    PLAN_TYPE_PAID,
};
use service_com_amazonaws_freetier_awsfreetierservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The model's `ActivityStatus` value set (`shared_types::ACTIVITY_STATUS_VALUES`,
/// generated from the Smithy enum).
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::{
    ACTIVITY_STATUS_VALUES, CURRENCY_CODE_VALUES, DIMENSION_VALUES, LANGUAGE_CODE_VALUES,
    MATCH_OPTION_VALUES,
};

/// The largest page `ListAccountActivities` serves, read off the service's own
/// constant (`crate::state::ACTIVITIES_MAX_RESULTS_CEILING`) so a suite asking
/// for the whole catalog in one call cannot drift from the bound the listing
/// enforces. Both paginated operations accept it.
pub(crate) const ACTIVITIES_PAGE_SIZE_CEILING: i32 =
    service_com_amazonaws_freetier_awsfreetierservice::state::ACTIVITIES_MAX_RESULTS_CEILING;

/// A router with only this service mounted, over one shared engine — the same
/// assembly `core/router` performs, minus the other services. The engine comes
/// back too, so a test can age the stored plan the way the clock would.
pub(crate) fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a
    // front-door setting, not part of what these tests are about.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

fn router() -> MultiServiceRouter {
    harness().0
}

/// A router whose account carries a vetted payment instrument — the precondition
/// `UpgradeAccountPlan` checks before it moves an account onto the paid plan.
///
/// A suite that is about what the upgrade STORES rather than about that gate
/// (whose own rule is pinned in `paid_plan_vetting.rs`) establishes the
/// precondition here instead of at every call.
pub(crate) fn vetted_router() -> MultiServiceRouter {
    let (router, engine) = harness();
    engine
        .state::<ComAmazonawsFreetierAwsfreetierserviceState>()
        .lock()
        .vet_payment_instrument(engine::emulator_account_id());
    router
}

/// Call one operation and return its HTTP status and JSON body.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://freetier.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSFreeTierService.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR/20260101/us-east-1/freetier/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call one operation, requiring success.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// The wire error code (`__type`) of an error body, without its namespace.
pub(crate) fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert that one request is refused with `expected_status` and `expected_code`.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, expected_status,
        "case {operation} {body}: expected HTTP {expected_status}, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        expected_code,
        "case {operation} {body}: expected error code {expected_code}, got {payload}"
    );
}

/// The string at `member`, required to be present.
pub(crate) fn text(payload: &Value, member: &str) -> String {
    payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string at {member}, got {payload}"))
        .to_string()
}

/// Walk every page of a paginated operation at `page_size`, returning the items
/// in the order they were reported.
///
/// Asserts the `@paginated` contract along the way: every page but the last
/// holds exactly `page_size` items, the last page carries no `nextToken`, and
/// the walk terminates.
async fn walk(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
    page_size: usize,
    extra: &Value,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..64 {
        let mut body = extra.clone();
        body["maxResults"] = json!(page_size);
        if let Some(token) = token.as_deref() {
            body["nextToken"] = json!(token);
        }
        let payload = ok(router, operation, &body).await;
        let items = payload[items_member]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {operation} page {page_index}: {items_member} must be a list, got {payload}")
            })
            .clone();
        token = payload["nextToken"].as_str().map(str::to_string);
        if token.is_some() {
            assert_eq!(
                items.len(),
                page_size,
                "case {operation} size {page_size} page {page_index}: a page that carries a \
                 nextToken must be full, got {} items",
                items.len()
            );
        }
        assert!(
            items.len() <= page_size,
            "case {operation} size {page_size} page {page_index}: expected at most {page_size} \
             items, got {}",
            items.len()
        );
        collected.extend(items);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {operation} size {page_size}: the pagination walk did not terminate");
}

// ---------------------------------------------------------------------------
// GetAccountPlanState / UpgradeAccountPlan
// ---------------------------------------------------------------------------

/// The plan the store reports once the account holds one: the free plan,
/// active, with the members the model marks required and the account id every
/// emulator presents. Repeating the read reports exactly the same body — a
/// reporting operation changes nothing a later caller sees.
#[tokio::test]
async fn account_plan_state_reports_the_stored_plan_and_repeats_it() {
    let router = router();
    // An account holds account-plan data only once the service's one
    // plan-writing operation has established it; what a read answers before
    // that is pinned in `account_plan_absence.rs`.
    ok(
        &router,
        "UpgradeAccountPlan",
        &json!({"accountPlanType": PLAN_TYPE_FREE}),
    )
    .await;

    let first = ok(&router, "GetAccountPlanState", &json!({})).await;
    let account_id = text(&first, "accountId");
    assert_eq!(
        account_id,
        engine::emulator_account_id(),
        "case first read: expected the emulator's account id, got {account_id}"
    );
    assert!(
        account_id.len() == 12 && account_id.chars().all(|c| c.is_ascii_digit()),
        "case first read: accountId must match ^[0-9]{{12}}$, got {account_id}"
    );
    assert_eq!(
        text(&first, "accountPlanType"),
        PLAN_TYPE_FREE,
        "case first read: the plan the account was moved onto is reported, got {first}"
    );
    assert_eq!(
        text(&first, "accountPlanStatus"),
        PLAN_STATUS_ACTIVE,
        "case first read: the established free plan is active, got {first}"
    );
    let unit = text(&first["accountPlanRemainingCredits"], "unit");
    assert!(
        CURRENCY_CODE_VALUES.contains(&unit.as_str()),
        "case first read: the credit unit must be a modelled CurrencyCode, got {unit}"
    );
    assert!(
        first["accountPlanRemainingCredits"]["amount"]
            .as_f64()
            .is_some_and(|amount| amount >= 0.0),
        "case first read: remaining credits carry an amount, got {first}"
    );
    // `accountPlanExpirationDate` is `@timestampFormat("date-time")`, so the instant is
    // a string on the wire; `date_time_wire_form.rs` pins the form itself.
    assert!(
        first["accountPlanExpirationDate"].as_str().is_some(),
        "case first read: a free plan expires, so it reports the instant, got {first}"
    );

    // Three further reads, to pin that reading does not move the plan.
    for attempt in 1..=3 {
        let again = ok(&router, "GetAccountPlanState", &json!({})).await;
        assert_eq!(
            again, first,
            "case read {attempt}: a read must report exactly what the first read did, got {again}"
        );
    }
}

/// `UpgradeAccountPlan` stores the target plan type and the status that goes
/// with it, and the following `GetAccountPlanState` reports what was stored —
/// the response is computed from the store, not echoed from the request.
#[tokio::test]
async fn upgrade_account_plan_persists_the_target_plan() {
    // Each row is one accepted target plan type, applied to a fresh account,
    // and what the store must then report. The rows differ in the target type,
    // including the one that asks for the plan the account is already on.
    let cases: [(&str, &str, bool); 3] = [
        // (requested plan type, expected stored type, expects an expiry)
        (PLAN_TYPE_PAID, PLAN_TYPE_PAID, false),
        (PLAN_TYPE_FREE, PLAN_TYPE_FREE, true),
        (PLAN_TYPE_PAID, PLAN_TYPE_PAID, false),
    ];

    for (requested, expected_type, expects_expiry) in cases {
        let router = vetted_router();

        let upgraded = ok(
            &router,
            "UpgradeAccountPlan",
            &json!({"accountPlanType": requested}),
        )
        .await;
        assert_eq!(
            text(&upgraded, "accountId"),
            engine::emulator_account_id(),
            "case upgrade to {requested}: the account is the caller's own, got {upgraded}"
        );
        assert_eq!(
            text(&upgraded, "accountPlanType"),
            expected_type,
            "case upgrade to {requested}: expected the stored type {expected_type}, got {upgraded}"
        );
        assert_eq!(
            text(&upgraded, "accountPlanStatus"),
            PLAN_STATUS_ACTIVE,
            "case upgrade to {requested}: the plan it moved to is active, got {upgraded}"
        );

        let after = ok(&router, "GetAccountPlanState", &json!({})).await;
        assert_eq!(
            text(&after, "accountPlanType"),
            expected_type,
            "case upgrade to {requested}: a later read must report the upgraded plan, got {after}"
        );
        assert_eq!(
            text(&after, "accountPlanStatus"),
            text(&upgraded, "accountPlanStatus"),
            "case upgrade to {requested}: the read and the upgrade must agree on the status, \
             got {after}"
        );
        assert_eq!(
            after["accountPlanExpirationDate"].as_str().is_some(),
            expects_expiry,
            "case upgrade to {requested}: only a free plan expires, got {after}"
        );

        // Asking for the same plan again is the same answer: the call stores a
        // target, it does not advance anything.
        let repeated = ok(
            &router,
            "UpgradeAccountPlan",
            &json!({"accountPlanType": requested}),
        )
        .await;
        assert_eq!(
            repeated, upgraded,
            "case upgrade to {requested} twice: expected the same stored plan, got {repeated}"
        );
    }
}

/// A plan type outside the modelled enum is a refused request, not a stored
/// plan, and the refusal leaves the plan where it was.
#[tokio::test]
async fn upgrade_account_plan_refuses_a_plan_type_outside_the_enum() {
    let router = router();
    ok(
        &router,
        "UpgradeAccountPlan",
        &json!({"accountPlanType": PLAN_TYPE_FREE}),
    )
    .await;
    let before = ok(&router, "GetAccountPlanState", &json!({})).await;

    // Rows differ in how they miss the enum: casing, an unknown word, an empty
    // string, and padding.
    for requested in ["free", "paid", "GOLD", "", "PAID "] {
        refused(
            &router,
            "UpgradeAccountPlan",
            &json!({"accountPlanType": requested}),
            400,
            "ValidationException",
        )
        .await;
    }

    let after = ok(&router, "GetAccountPlanState", &json!({})).await;
    assert_eq!(
        after, before,
        "case after refusals: a refused upgrade must leave the plan untouched, got {after}"
    );
}

/// The plan moves one way: an account on the paid plan cannot be put back onto
/// the free plan, and the refusal leaves it on the paid plan.
#[tokio::test]
async fn upgrade_account_plan_refuses_a_move_back_to_the_free_plan() {
    let router = vetted_router();
    ok(
        &router,
        "UpgradeAccountPlan",
        &json!({"accountPlanType": PLAN_TYPE_PAID}),
    )
    .await;

    refused(
        &router,
        "UpgradeAccountPlan",
        &json!({"accountPlanType": PLAN_TYPE_FREE}),
        400,
        "ValidationException",
    )
    .await;

    let after = ok(&router, "GetAccountPlanState", &json!({})).await;
    assert_eq!(
        text(&after, "accountPlanType"),
        PLAN_TYPE_PAID,
        "case refused downgrade: the account stays on the paid plan, got {after}"
    );
}

/// A free plan whose expiration instant has passed reports `EXPIRED` — the
/// status follows the clock, not the call. A paid plan has no expiry and stays
/// active.
#[tokio::test]
async fn a_free_plan_past_its_expiration_instant_reports_expired() {
    // Rows differ in plan type and in where the stored expiration instant sits
    // relative to now, including both sides of the boundary.
    let cases: [(&str, i64, &str); 5] = [
        // (stored plan type, expiration offset in seconds, expected status)
        (PLAN_TYPE_FREE, -86_400, PLAN_STATUS_EXPIRED),
        (PLAN_TYPE_FREE, -1, PLAN_STATUS_EXPIRED),
        (PLAN_TYPE_FREE, 3_600, PLAN_STATUS_ACTIVE),
        (PLAN_TYPE_FREE, 86_400, PLAN_STATUS_ACTIVE),
        (PLAN_TYPE_PAID, -86_400, PLAN_STATUS_ACTIVE),
    ];

    for (plan_type, offset, expected_status) in cases {
        let (router, engine) = harness();
        {
            let state = engine.state::<ComAmazonawsFreetierAwsfreetierserviceState>();
            let mut store = state.lock();
            let plan = store.plan_mut(engine::emulator_account_id());
            plan.plan_type = plan_type.to_string();
            plan.plan_status = PLAN_STATUS_ACTIVE.to_string();
            plan.expiration_date = Some(aws_smithy_types::DateTime::from_secs(
                aws_smithy_types::DateTime::from(std::time::SystemTime::now()).secs() + offset,
            ));
        }

        let payload = ok(&router, "GetAccountPlanState", &json!({})).await;
        assert_eq!(
            text(&payload, "accountPlanStatus"),
            expected_status,
            "case {plan_type} expiring at now{offset:+}s: expected {expected_status}, got {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListAccountActivities / GetAccountActivity
// ---------------------------------------------------------------------------

/// Every activity the catalog holds, in catalog order, as (id, status) pairs.
async fn catalog(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let payload = ok(router, "ListAccountActivities", &json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING})).await;
    payload["activities"]
        .as_array()
        .expect("activities is a list")
        .iter()
        .map(|activity| (text(activity, "activityId"), text(activity, "status")))
        .collect()
}

/// Each summary carries the members the model marks required, and every
/// activity id satisfies the modelled `ActivityId` constraints (exactly 32
/// characters of `[a-zA-Z0-9]`).
#[tokio::test]
async fn listed_activities_satisfy_the_modelled_summary_shape() {
    let router = router();
    let payload = ok(
        &router,
        "ListAccountActivities",
        &json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING}),
    )
    .await;
    let activities = payload["activities"]
        .as_array()
        .expect("activities is a list");
    assert!(
        !activities.is_empty(),
        "case full list: the account has an activity catalog, got {payload}"
    );

    for activity in activities {
        let id = text(activity, "activityId");
        assert!(
            id.len() == 32 && id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {id}: activityId must be 32 characters of [a-zA-Z0-9], got {id}"
        );
        assert!(
            !text(activity, "title").is_empty(),
            "case {id}: every summary carries a title, got {activity}"
        );
        let status = text(activity, "status");
        assert!(
            ACTIVITY_STATUS_VALUES.contains(&status.as_str()),
            "case {id}: status must be a modelled ActivityStatus, got {status}"
        );
        let credit = &activity["reward"]["credit"];
        assert!(
            credit["amount"].as_f64().is_some_and(|amount| amount > 0.0),
            "case {id}: the reward union carries a credit amount, got {activity}"
        );
        assert!(
            CURRENCY_CODE_VALUES.contains(&text(credit, "unit").as_str()),
            "case {id}: the credit unit must be a modelled CurrencyCode, got {activity}"
        );
    }
}

/// `filterActivityStatuses` selects exactly the activities whose status it
/// lists, in catalog order; a request that lists no status expresses no filter.
#[tokio::test]
async fn activity_status_filter_selects_exactly_the_listed_statuses() {
    let router = router();
    let all = catalog(&router).await;

    // Rows span the model's whole `ActivityStatus` value set: each status on
    // its own, a pair, every status at once, the empty list, and a status no
    // activity holds is covered by whichever single-status row selects nothing.
    let mut cases: Vec<Vec<&str>> = ACTIVITY_STATUS_VALUES
        .iter()
        .map(|status| vec![*status])
        .collect();
    cases.push(vec![ACTIVITY_STATUS_COMPLETED, ACTIVITY_STATUS_IN_PROGRESS]);
    cases.push(ACTIVITY_STATUS_VALUES.to_vec());
    cases.push(Vec::new());

    for wanted in cases {
        let payload = ok(
            &router,
            "ListAccountActivities",
            &json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING, "filterActivityStatuses": wanted}),
        )
        .await;
        let got: Vec<String> = payload["activities"]
            .as_array()
            .expect("activities is a list")
            .iter()
            .map(|activity| text(activity, "activityId"))
            .collect();
        let expected: Vec<String> = all
            .iter()
            .filter(|(_, status)| wanted.is_empty() || wanted.contains(&status.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(
            got, expected,
            "case filter {wanted:?}: expected {expected:?} in catalog order, got {got:?}"
        );
    }

    // The filter is a filter: at least one status selects strictly fewer rows
    // than the unfiltered list, so a passing table cannot mean "ignored".
    let narrowed = ACTIVITY_STATUS_VALUES.iter().any(|status| {
        all.iter().filter(|(_, held)| held == status).count() < all.len()
    });
    assert!(
        narrowed,
        "case filter narrowing: no single status selects fewer than all {} activities",
        all.len()
    );
}

/// Paging the catalog at any page size reports the catalog exactly once, in
/// order, and the token resumes where the previous page stopped. Exercised over
/// every page size from one to past the catalog's length, which is the whole
/// space of page sizes the rule has to hold for here.
#[tokio::test]
async fn activity_pages_reassemble_the_catalog_at_every_page_size() {
    let router = router();
    let all: Vec<String> = catalog(&router).await.into_iter().map(|(id, _)| id).collect();

    for page_size in 1..=all.len() + 2 {
        let walked: Vec<String> = walk(
            &router,
            "ListAccountActivities",
            "activities",
            page_size,
            &json!({}),
        )
        .await
        .iter()
        .map(|activity| text(activity, "activityId"))
        .collect();
        assert_eq!(
            walked, all,
            "case page size {page_size}: paging must reassemble the catalog in order, got \
             {walked:?}"
        );
    }

    // Paging a FILTERED list follows the same rule: the pages reassemble the
    // filtered view, not the catalog.
    for status in ACTIVITY_STATUS_VALUES {
        let expected: Vec<String> = catalog(&router)
            .await
            .into_iter()
            .filter(|(_, held)| held == status)
            .map(|(id, _)| id)
            .collect();
        for page_size in 1..=expected.len() + 1 {
            let walked: Vec<String> = walk(
                &router,
                "ListAccountActivities",
                "activities",
                page_size,
                &json!({"filterActivityStatuses": [status]}),
            )
            .await
            .iter()
            .map(|activity| text(activity, "activityId"))
            .collect();
            assert_eq!(
                walked, expected,
                "case filter {status} at page size {page_size}: expected {expected:?}, got \
                 {walked:?}"
            );
        }
    }
}

/// A `nextToken` this service did not issue for this list is refused, not
/// silently read as page one.
#[tokio::test]
async fn an_unissued_activity_token_is_refused() {
    let router = router();

    // A token issued by the OTHER paginated operation, to pin that a token is
    // bound to its list.
    let usage_page = ok(&router, "GetFreeTierUsage", &json!({"maxResults": 1})).await;
    let foreign = text(&usage_page, "nextToken");

    // Rows differ in how the token is wrong: not hex, hex of unrelated bytes,
    // the right encoding with a non-numeric offset, and another list's token.
    for token in [
        "not-a-token".to_string(),
        ::hex::encode("garbage"),
        ::hex::encode("ftv1:activities:not-a-number"),
        ::hex::encode("ftv1:freeTierUsages:1"),
        foreign,
    ] {
        refused(
            &router,
            "ListAccountActivities",
            &json!({"nextToken": token}),
            400,
            "ValidationException",
        )
        .await;
    }
}

/// `GetAccountActivity` reads the SAME catalog the list pages over: for every
/// activity, the detail view repeats its summary members and adds the detail
/// ones, and the timestamps it reports are the ones the status allows
/// (`startedAt` only for IN_PROGRESS or COMPLETED, `completedAt` only for
/// COMPLETED).
#[tokio::test]
async fn activity_detail_agrees_with_its_summary() {
    let router = router();
    let listed = ok(&router, "ListAccountActivities", &json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING})).await;

    for summary in listed["activities"].as_array().expect("activities is a list") {
        let id = text(summary, "activityId");
        let detail = ok(&router, "GetAccountActivity", &json!({"activityId": id})).await;

        for member in ["activityId", "title", "status"] {
            assert_eq!(
                detail[member], summary[member],
                "case {id}: the detail view must repeat the summary's {member}, got {detail}"
            );
        }
        assert_eq!(
            detail["reward"], summary["reward"],
            "case {id}: the detail view must repeat the summary's reward, got {detail}"
        );
        assert!(
            !text(&detail, "description").is_empty(),
            "case {id}: the detail view carries a description, got {detail}"
        );
        assert!(
            text(&detail, "instructionsUrl").starts_with("https://"),
            "case {id}: the detail view carries an instructions URL, got {detail}"
        );

        let status = text(&detail, "status");
        let may_have_started =
            status == ACTIVITY_STATUS_IN_PROGRESS || status == ACTIVITY_STATUS_COMPLETED;
        assert!(
            may_have_started || detail["startedAt"].is_null(),
            "case {id} in {status}: startedAt appears only for IN_PROGRESS or COMPLETED, got \
             {detail}"
        );
        assert!(
            status == ACTIVITY_STATUS_COMPLETED || detail["completedAt"].is_null(),
            "case {id} in {status}: completedAt appears only for COMPLETED, got {detail}"
        );
        if status == ACTIVITY_STATUS_COMPLETED {
            assert!(
                detail["completedAt"].as_str().is_some() && detail["startedAt"].as_str().is_some(),
                "case {id}: a completed activity reports when it started and finished, got \
                 {detail}"
            );
        }
    }
}

/// An id that could name an activity but names none is refused as
/// `ResourceNotFoundException`; an id that cannot be an `ActivityId` at all is
/// refused as `ValidationException`. The two failures are different claims, and
/// this service answers both with the status it sends for that error shape.
#[tokio::test]
async fn unknown_and_malformed_activity_ids_are_told_apart() {
    let router = router();
    let known: BTreeSet<String> = catalog(&router).await.into_iter().map(|(id, _)| id).collect();

    // Well-formed ids (exactly 32 of [a-zA-Z0-9]) that name no activity.
    for id in [
        "00000000000000000000000000000000",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "ffffffffffffffffffffffffffffffff",
    ] {
        assert!(
            !known.contains(id),
            "case {id}: the probe id must not be in the catalog"
        );
        refused(
            &router,
            "GetAccountActivity",
            &json!({"activityId": id}),
            400,
            "ResourceNotFoundException",
        )
        .await;
    }

    // Rows differ in how the id misses the modelled constraint: too short, one
    // short, one long, and an illegal character at length 32.
    for id in [
        "short",
        "",
        "0000000000000000000000000000000",
        "000000000000000000000000000000000",
        "0000000000000000000000000000000-",
        "0000000000000000000000000000 000",
    ] {
        refused(
            &router,
            "GetAccountActivity",
            &json!({"activityId": id}),
            400,
            "ValidationException",
        )
        .await;
    }
}

/// A `languageCode` outside the modelled enum is refused on both activity
/// readers; one the model declares is accepted.
#[tokio::test]
async fn activity_readers_check_the_language_code() {
    let router = router();
    let id = catalog(&router).await[0].0.clone();

    for language_code in ["en-US", "ja-JP", "zh-TW"] {
        ok(
            &router,
            "GetAccountActivity",
            &json!({"activityId": id, "languageCode": language_code}),
        )
        .await;
        ok(
            &router,
            "ListAccountActivities",
            &json!({"languageCode": language_code}),
        )
        .await;
    }

    for language_code in ["en_US", "klingon", ""] {
        refused(
            &router,
            "GetAccountActivity",
            &json!({"activityId": id, "languageCode": language_code}),
            400,
            "ValidationException",
        )
        .await;
        refused(
            &router,
            "ListAccountActivities",
            &json!({"languageCode": language_code}),
            400,
            "ValidationException",
        )
        .await;
    }
}

/// The title the LIST reports for every activity in `language_code`, keyed by
/// activity id. `None` asks for no language at all.
async fn listed_titles(
    router: &MultiServiceRouter,
    language_code: Option<&str>,
) -> BTreeMap<String, String> {
    let mut body = json!({"maxResults": ACTIVITIES_PAGE_SIZE_CEILING});
    if let Some(code) = language_code {
        body["languageCode"] = json!(code);
    }
    let payload = ok(router, "ListAccountActivities", &body).await;
    payload["activities"]
        .as_array()
        .expect("activities is a list")
        .iter()
        .map(|activity| (text(activity, "activityId"), text(activity, "title")))
        .collect()
}

/// The detail view of `activity_id` in `language_code`.
async fn detail_in(
    router: &MultiServiceRouter,
    activity_id: &str,
    language_code: Option<&str>,
) -> Value {
    let mut body = json!({"activityId": activity_id});
    if let Some(code) = language_code {
        body["languageCode"] = json!(code);
    }
    ok(router, "GetAccountActivity", &body).await
}

/// `languageCode` selects the language the activity text is reported in:
/// `ListAccountActivities$languageCode` is "the language code used to return
/// translated titles" and `GetAccountActivity$languageCode` the same for the
/// title and the description, so the text a reader reports is a function of the
/// stored row AND the requested code, with a request that names no code
/// answered in the service's base language.
///
/// The table spans the model's WHOLE `LanguageCode` value set
/// (`shared_types::LANGUAGE_CODE_VALUES`, generated from the Smithy enum) plus
/// the no-code row, and every activity in the catalog — not the one code and
/// one row the gap was found on.
#[tokio::test]
async fn activity_text_is_reported_in_the_requested_language() {
    let router = router();
    let ids: Vec<String> = catalog(&router).await.into_iter().map(|(id, _)| id).collect();
    assert!(
        !ids.is_empty(),
        "case catalog: the account has an activity catalog to read in each language"
    );

    // The language a request that names none is answered in, and the baseline
    // every other row is compared against.
    let english = listed_titles(&router, Some("en-US")).await;
    let unspecified = listed_titles(&router, None).await;
    assert_eq!(
        unspecified, english,
        "case no languageCode: a request that names no code must be answered in the service's \
         base language, got {unspecified:?} against {english:?}"
    );

    let mut per_language: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    for code in LANGUAGE_CODE_VALUES {
        let titles = listed_titles(&router, Some(code)).await;

        // Every activity is reported in every modelled language, with a title
        // the modelled `GenericString` constraints allow (len 0..1024).
        let listed: BTreeSet<&String> = titles.keys().collect();
        let expected: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            listed, expected,
            "case {code}: the list must report the whole catalog whatever the language, got \
             {listed:?}"
        );
        for (id, title) in &titles {
            assert!(
                !title.is_empty() && title.chars().count() <= 1024,
                "case {code}/{id}: the title must be non-empty and at most 1024 characters, \
                 got {title:?}"
            );
        }

        // Asking twice in one language reports the same text: a read reports
        // stored text and never rewrites it.
        let again = listed_titles(&router, Some(code)).await;
        assert_eq!(
            again, titles,
            "case {code}: the text reported must depend only on the row and the code, got \
             {again:?} then {titles:?}"
        );

        for id in &ids {
            let detail = detail_in(&router, id, Some(code)).await;

            // The two readers read the SAME localized row, so the detail view
            // repeats the summary's title in the language asked for.
            assert_eq!(
                text(&detail, "title"),
                titles[id],
                "case {code}/{id}: the detail view must repeat the summary's title in the \
                 requested language, got {detail}"
            );

            let description = text(&detail, "description");
            let english_description = text(&detail_in(&router, id, Some("en-US")).await, "description");
            assert!(
                !description.is_empty() && description.chars().count() <= 1024,
                "case {code}/{id}: the description must be non-empty and at most 1024 \
                 characters, got {description:?}"
            );

            // A code for a language other than the base one reports DIFFERENT
            // text — the dropped effect this test exists to pin. `en-GB` is a
            // variant of the base language, so it resolves to the base text.
            if !code.starts_with("en-") {
                assert_ne!(
                    titles[id], english[id],
                    "case {code}/{id}: a non-English language code must report a translated \
                     title, got {:?} for both",
                    titles[id]
                );
                assert_ne!(
                    description, english_description,
                    "case {code}/{id}: a non-English language code must report a translated \
                     description, got {description:?} for both"
                );
            } else {
                assert_eq!(
                    titles[id], english[id],
                    "case {code}/{id}: a variant of the base language reports the base text, \
                     got {:?} against {:?}",
                    titles[id], english[id]
                );
            }
        }

        per_language.insert(code, titles);
    }

    // The codes do not all collapse onto one rendering: every language but the
    // `en-GB` variant of the base language reports its own text.
    let renderings: BTreeSet<Vec<(String, String)>> = per_language
        .values()
        .map(|titles| titles.iter().map(|(id, title)| (id.clone(), title.clone())).collect())
        .collect();
    assert!(
        renderings.len() >= LANGUAGE_CODE_VALUES.len() - 1,
        "case distinct renderings: expected at least {} distinct title sets over {} codes, got \
         {}",
        LANGUAGE_CODE_VALUES.len() - 1,
        LANGUAGE_CODE_VALUES.len(),
        renderings.len()
    );
}

// ---------------------------------------------------------------------------
// GetFreeTierUsage
// ---------------------------------------------------------------------------

/// A usage record's identity, for comparing sets of records.
fn usage_key(record: &Value) -> String {
    format!(
        "{}|{}|{}",
        text(record, "service"),
        text(record, "usageType"),
        text(record, "region")
    )
}

/// Every usage record the account holds, in report order.
async fn usages(router: &MultiServiceRouter) -> Vec<Value> {
    ok(router, "GetFreeTierUsage", &json!({"maxResults": 1000}))
        .await["freeTierUsages"]
        .as_array()
        .expect("freeTierUsages is a list")
        .clone()
}

/// The value one `Dimension` takes on a reported record: text for every
/// dimension but `USAGE_PERCENTAGE`, which is the share of the offer's limit
/// the account has used.
fn dimension_of(record: &Value, key: &str) -> Result<String, f64> {
    match key {
        "SERVICE" => Ok(text(record, "service")),
        "OPERATION" => Ok(text(record, "operation")),
        "USAGE_TYPE" => Ok(text(record, "usageType")),
        "REGION" => Ok(text(record, "region")),
        "FREE_TIER_TYPE" => Ok(text(record, "freeTierType")),
        "DESCRIPTION" => Ok(text(record, "description")),
        "USAGE_PERCENTAGE" => {
            let actual = record["actualUsageAmount"].as_f64().unwrap_or_default();
            let limit = record["limit"].as_f64().unwrap_or_default();
            Err(if limit == 0.0 {
                0.0
            } else {
                actual / limit * 100.0
            })
        }
        other => panic!("{other} is not a modelled Dimension"),
    }
}

/// The records a `Dimensions` filter must select, computed from the reported
/// records by the rule the model documents: a record matches when its value for
/// the dimension matches at least one of `Values` under the single match option.
fn expected_for_dimension(
    records: &[Value],
    key: &str,
    option: &str,
    values: &[&str],
) -> BTreeSet<String> {
    records
        .iter()
        .filter(|record| match dimension_of(record, key) {
            Ok(value) => values.iter().any(|wanted| match option {
                "EQUALS" => value == *wanted,
                "STARTS_WITH" => value.starts_with(wanted),
                "ENDS_WITH" => value.ends_with(wanted),
                "CONTAINS" => value.contains(wanted),
                other => panic!("{other} does not apply to a text dimension"),
            }),
            Err(number) => values.iter().any(|wanted| {
                let wanted: f64 = wanted.parse().expect("a numeric probe value");
                match option {
                    "EQUALS" => (number - wanted).abs() < f64::EPSILON,
                    "GREATER_THAN_OR_EQUAL" => number >= wanted,
                    other => panic!("{other} does not apply to a numeric dimension"),
                }
            }),
        })
        .map(usage_key)
        .collect()
}

/// Ask `GetFreeTierUsage` for every record a filter selects.
async fn filtered(router: &MultiServiceRouter, filter: &Value) -> BTreeSet<String> {
    let payload = ok(
        router,
        "GetFreeTierUsage",
        &json!({"maxResults": 1000, "filter": filter}),
    )
    .await;
    payload["freeTierUsages"]
        .as_array()
        .expect("freeTierUsages is a list")
        .iter()
        .map(usage_key)
        .collect()
}

/// A `Dimensions` filter selects exactly the records whose value for that
/// dimension matches one of `Values` under the given match option — for every
/// dimension the model declares and every match option that applies to it.
#[tokio::test]
async fn a_dimensions_filter_selects_exactly_the_matching_records() {
    let router = router();
    let records = usages(&router).await;
    assert!(
        records.len() > 1,
        "the account must hold several usage records for a filter to narrow, got {}",
        records.len()
    );

    // Rows span every `Dimension` the model declares and every `MatchOption`
    // that applies to it, with probe values of differing form (whole value,
    // prefix, suffix, substring, several values at once, a value nothing holds).
    let cases: [(&str, &str, &[&str], bool); 17] = [
        // (dimension, match option, values, must select at least one record)
        ("SERVICE", "EQUALS", &["AWSLambda"], true),
        ("SERVICE", "EQUALS", &["AmazonEC2", "AmazonS3"], true),
        ("SERVICE", "CONTAINS", &["Lambda"], true),
        ("SERVICE", "STARTS_WITH", &["Amazon"], true),
        ("SERVICE", "ENDS_WITH", &["EC2"], true),
        ("SERVICE", "EQUALS", &["NoSuchService"], false),
        ("OPERATION", "EQUALS", &["Invoke"], true),
        ("OPERATION", "CONTAINS", &["Instance"], true),
        ("USAGE_TYPE", "STARTS_WITH", &["TimedStorage"], true),
        ("USAGE_TYPE", "ENDS_WITH", &["micro"], true),
        ("USAGE_TYPE", "CONTAINS", &["Usage"], true),
        ("REGION", "EQUALS", &["us-west-2"], true),
        ("REGION", "STARTS_WITH", &["us-"], true),
        ("FREE_TIER_TYPE", "EQUALS", &["Always Free"], true),
        ("DESCRIPTION", "CONTAINS", &["free per month"], true),
        ("USAGE_PERCENTAGE", "GREATER_THAN_OR_EQUAL", &["0"], true),
        ("USAGE_PERCENTAGE", "GREATER_THAN_OR_EQUAL", &["75"], true),
    ];

    for (key, option, values, must_select) in cases {
        assert!(
            DIMENSION_VALUES.contains(&key) && MATCH_OPTION_VALUES.contains(&option),
            "case {key} {option}: both must be values the model declares"
        );
        let expected = expected_for_dimension(&records, key, option, values);
        assert_eq!(
            expected.is_empty(),
            !must_select,
            "case {key} {option} {values:?}: the probe's own expectation is {} records",
            expected.len()
        );
        let got = filtered(
            &router,
            &json!({"Dimensions": {"Key": key, "Values": values, "MatchOptions": [option]}}),
        )
        .await;
        assert_eq!(
            got, expected,
            "case {key} {option} {values:?}: expected {expected:?}, got {got:?}"
        );
        assert!(
            got.len() <= records.len(),
            "case {key} {option} {values:?}: a filter cannot invent records, got {got:?}"
        );
    }
}

/// `And`, `Or` and `Not` combine their expressions the way the model documents
/// them: all of them, any of them, and the negation.
#[tokio::test]
async fn and_or_and_not_combine_their_expressions() {
    let router = router();
    let records = usages(&router).await;
    let all: BTreeSet<String> = records.iter().map(usage_key).collect();

    let lambda = json!({"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]}});
    let east = json!({"Dimensions": {"Key": "REGION", "Values": ["us-east-1"], "MatchOptions": ["EQUALS"]}});
    let always_free = json!({"Dimensions": {"Key": "FREE_TIER_TYPE", "Values": ["Always Free"], "MatchOptions": ["EQUALS"]}});

    let lambda_rows = expected_for_dimension(&records, "SERVICE", "EQUALS", &["AWSLambda"]);
    let east_rows = expected_for_dimension(&records, "REGION", "EQUALS", &["us-east-1"]);
    let always_free_rows =
        expected_for_dimension(&records, "FREE_TIER_TYPE", "EQUALS", &["Always Free"]);

    let intersection: BTreeSet<String> = lambda_rows.intersection(&east_rows).cloned().collect();
    let union: BTreeSet<String> = east_rows.union(&always_free_rows).cloned().collect();
    let complement: BTreeSet<String> = all.difference(&lambda_rows).cloned().collect();

    // Rows differ in the combinator and in how deeply it nests.
    let cases: [(&str, Value, BTreeSet<String>); 5] = [
        ("And", json!({"And": [lambda.clone(), east.clone()]}), intersection.clone()),
        ("Or", json!({"Or": [east.clone(), always_free.clone()]}), union),
        ("Not", json!({"Not": lambda.clone()}), complement.clone()),
        (
            "Not of And",
            json!({"Not": {"And": [lambda.clone(), east.clone()]}}),
            all.difference(&intersection).cloned().collect(),
        ),
        (
            "And of Or and Not",
            json!({"And": [{"Or": [lambda.clone(), east.clone()]}, {"Not": lambda.clone()}]}),
            east_rows.difference(&lambda_rows).cloned().collect(),
        ),
    ];

    for (name, filter, expected) in cases {
        assert!(
            !expected.is_empty(),
            "case {name}: the probe's own expectation must select something"
        );
        let got = filtered(&router, &filter).await;
        assert_eq!(
            got, expected,
            "case {name}: expected {expected:?}, got {got:?}"
        );
    }
}

/// A filter the service cannot evaluate is refused, never ignored: a match
/// option that does not apply to the dimension's kind, more than one match
/// option, no values, and an expression that states no condition.
#[tokio::test]
async fn an_unevaluable_filter_is_refused() {
    let router = router();

    // Rows differ in which part of the expression is wrong.
    let cases: [(&str, Value); 8] = [
        (
            "GREATER_THAN_OR_EQUAL on a text dimension",
            json!({"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["GREATER_THAN_OR_EQUAL"]}}),
        ),
        (
            "CONTAINS on a numeric dimension",
            json!({"Dimensions": {"Key": "USAGE_PERCENTAGE", "Values": ["50"], "MatchOptions": ["CONTAINS"]}}),
        ),
        (
            "a non-numeric value on a numeric dimension",
            json!({"Dimensions": {"Key": "USAGE_PERCENTAGE", "Values": ["half"], "MatchOptions": ["EQUALS"]}}),
        ),
        (
            "two match options",
            json!({"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS", "CONTAINS"]}}),
        ),
        (
            "no match option",
            json!({"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": []}}),
        ),
        (
            "a dimension outside the enum",
            json!({"Dimensions": {"Key": "SERVICE_NAME", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]}}),
        ),
        ("an expression that states no condition", json!({})),
        (
            "a nested expression that states no condition",
            json!({"And": [{"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]}}, {}]}),
        ),
    ];

    for (name, filter) in cases {
        let (status, payload) = call(
            &router,
            "GetFreeTierUsage",
            &json!({"maxResults": 1000, "filter": filter}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {name}: expected a ValidationException, got {payload}"
        );
    }
}

/// Paging the usage records at any page size reports them exactly once, in
/// order — with and without a filter.
#[tokio::test]
async fn usage_pages_reassemble_the_records_at_every_page_size() {
    let router = router();
    let all: Vec<String> = usages(&router).await.iter().map(usage_key).collect();

    for page_size in 1..=all.len() + 2 {
        let walked: Vec<String> = walk(
            &router,
            "GetFreeTierUsage",
            "freeTierUsages",
            page_size,
            &json!({}),
        )
        .await
        .iter()
        .map(usage_key)
        .collect();
        assert_eq!(
            walked, all,
            "case page size {page_size}: paging must reassemble the records in order, got \
             {walked:?}"
        );
    }

    // The same rule under a filter that selects a strict subset.
    let filter = json!({"Dimensions": {"Key": "FREE_TIER_TYPE", "Values": ["Always Free"], "MatchOptions": ["EQUALS"]}});
    let expected: Vec<String> = usages(&router)
        .await
        .iter()
        .filter(|record| text(record, "freeTierType") == "Always Free")
        .map(usage_key)
        .collect();
    assert!(
        !expected.is_empty() && expected.len() < all.len(),
        "case filtered paging: the probe filter must select a strict subset, got {expected:?}"
    );
    for page_size in 1..=expected.len() + 1 {
        let walked: Vec<String> = walk(
            &router,
            "GetFreeTierUsage",
            "freeTierUsages",
            page_size,
            &json!({"filter": filter}),
        )
        .await
        .iter()
        .map(usage_key)
        .collect();
        assert_eq!(
            walked, expected,
            "case filtered paging at size {page_size}: expected {expected:?}, got {walked:?}"
        );
    }
}

/// An absent `maxResults` is the page size the model declares as its default
/// (10), not "every record".
#[tokio::test]
async fn an_absent_max_results_uses_the_modelled_default() {
    let router = router();
    let all = usages(&router).await;
    let default_page = ok(&router, "GetFreeTierUsage", &json!({})).await;
    let returned = default_page["freeTierUsages"]
        .as_array()
        .expect("freeTierUsages is a list")
        .len();
    assert_eq!(
        returned,
        all.len().min(10),
        "case no maxResults: the page holds the modelled default of 10 (or every record when \
         fewer are stored), got {returned}"
    );

    let activities = ok(&router, "ListAccountActivities", &json!({})).await["activities"]
        .as_array()
        .expect("activities is a list")
        .len();
    let catalog_len = catalog(&router).await.len();
    assert_eq!(
        activities,
        catalog_len.min(10),
        "case no maxResults: the activity page holds the modelled default of 10 (or the whole \
         catalog when it is shorter), got {activities}"
    );
}

/// A `nextToken` `GetFreeTierUsage` did not issue is refused, including one the
/// other paginated operation issued.
#[tokio::test]
async fn an_unissued_usage_token_is_refused() {
    let router = router();
    let activity_page = ok(&router, "ListAccountActivities", &json!({"maxResults": 1})).await;
    let foreign = text(&activity_page, "nextToken");

    for token in [
        "not-a-token".to_string(),
        ::hex::encode("ftv1:freeTierUsages:"),
        ::hex::encode("ftv1:activities:1"),
        foreign,
    ] {
        refused(
            &router,
            "GetFreeTierUsage",
            &json!({"nextToken": token}),
            400,
            "ValidationException",
        )
        .await;
    }
}

/// The `Expression` root operators, by the names the model gives the shape's
/// members (`smithy_query shape=Expression`: `Or`, `And`, `Not`, `Dimensions` —
/// "In the following Contents, you must specify exactly one of the following
/// root operators").
const ROOT_OPERATORS: [&str; 4] = ["Or", "And", "Not", "Dimensions"];

/// A well-formed one-member expression that sets exactly `operator`, each with
/// an operand the service can evaluate, so the only thing a combination of them
/// gets wrong is how many root operators it sets.
fn root_operator(operator: &str) -> Value {
    let leaf =
        json!({"Dimensions": {"Key": "REGION", "Values": ["us-east-1"], "MatchOptions": ["EQUALS"]}});
    match operator {
        "Dimensions" => leaf,
        "Not" => json!({"Not": leaf}),
        "And" => json!({"And": [leaf]}),
        "Or" => json!({"Or": [leaf]}),
        other => panic!("{other} is not an Expression root operator"),
    }
}

/// One expression object that sets every operator in `operators` at once.
fn expression_with(operators: &[&str]) -> Value {
    let mut members = serde_json::Map::new();
    for operator in operators {
        let one = root_operator(operator);
        let (name, value) = one
            .as_object()
            .and_then(|members| members.iter().next())
            .map(|(name, value)| (name.clone(), value.clone()))
            .unwrap_or_else(|| panic!("case {operator}: a root operator is one JSON member"));
        members.insert(name, value);
    }
    Value::Object(members)
}

/// Assert that `GetFreeTierUsage` refuses `filter` with the modelled
/// `ValidationException` (HTTP 400).
async fn refuse_filter(router: &MultiServiceRouter, case: &str, filter: &Value) {
    let (status, payload) = call(
        router,
        "GetFreeTierUsage",
        &json!({"maxResults": 1000, "filter": filter}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case} ({filter}): expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case} ({filter}): expected a ValidationException, got {payload}"
    );
}

/// An `Expression` sets exactly one root operator: one is evaluated as the
/// model documents that operator, and every other arity — none, any pair, any
/// triple, all four — is refused rather than combined by a rule the model does
/// not state. The rule holds at every level, so the same expression nested
/// inside another is refused too.
#[tokio::test]
async fn an_expression_sets_exactly_one_root_operator() {
    let router = router();
    let records = usages(&router).await;
    assert!(
        records.len() > 1,
        "case setup: the account must hold several usage records, got {}",
        records.len()
    );
    let all: BTreeSet<String> = records.iter().map(usage_key).collect();
    let leaf_rows = expected_for_dimension(&records, "REGION", "EQUALS", &["us-east-1"]);
    assert!(
        !leaf_rows.is_empty() && leaf_rows != all,
        "case setup: the probe condition must select some records but not all, got {leaf_rows:?}"
    );

    // Exactly one root operator: accepted, and evaluated as that operator.
    for operator in ROOT_OPERATORS {
        let expected: BTreeSet<String> = if operator == "Not" {
            all.difference(&leaf_rows).cloned().collect()
        } else {
            leaf_rows.clone()
        };
        let got = filtered(&router, &root_operator(operator)).await;
        assert_eq!(
            got, expected,
            "case only {operator}: expected {expected:?}, got {got:?}"
        );
    }

    // Every other arity: refused. The rows are each subset of the four root
    // operators whose size is not one — the empty expression, the six pairs,
    // the four triples and all four at once.
    for mask in 0u8..(1 << ROOT_OPERATORS.len()) {
        let operators: Vec<&str> = ROOT_OPERATORS
            .iter()
            .copied()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, name)| name)
            .collect();
        if operators.len() == 1 {
            continue;
        }
        let filter = expression_with(&operators);
        let case = format!("{} root operators {operators:?}", operators.len());
        refuse_filter(&router, &case, &filter).await;

        // The same expression, one level down: nested in each operator that
        // carries an expression.
        for (place, nested) in [
            (
                "inside And",
                json!({"And": [root_operator("Dimensions"), filter.clone()]}),
            ),
            ("inside Or", json!({"Or": [filter.clone()]})),
            ("inside Not", json!({"Not": filter.clone()})),
        ] {
            refuse_filter(&router, &format!("{case} {place}"), &nested).await;
        }
    }
}

/// A logical operator that lists no expression states no condition, exactly as
/// an expression that sets no root operator does, and is refused the same way.
#[tokio::test]
async fn a_logical_operator_listing_no_expression_is_refused() {
    let router = router();
    let leaf = root_operator("Dimensions");

    // Rows differ in which operator is empty and how deeply it sits.
    let cases: [(&str, Value); 5] = [
        ("And of nothing", json!({"And": []})),
        ("Or of nothing", json!({"Or": []})),
        ("And of an Or of nothing", json!({"And": [{"Or": []}]})),
        ("Not of an And of nothing", json!({"Not": {"And": []}})),
        (
            "And of a condition and an Or of nothing",
            json!({"And": [leaf, {"Or": []}]}),
        ),
    ];

    for (case, filter) in cases {
        refuse_filter(&router, case, &filter).await;
    }
}

/// Whether a filter is one the service can evaluate depends on the filter
/// alone, so an account that holds no usage records refuses exactly the same
/// expressions instead of answering an empty list — and still accepts a
/// well-formed one.
#[tokio::test]
async fn a_filter_is_checked_before_any_record_is_read() {
    let (router, engine) = harness();
    {
        let state = engine.state::<ComAmazonawsFreetierAwsfreetierserviceState>();
        state.lock().usages.clear();
    }
    let records = usages(&router).await;
    assert!(
        records.is_empty(),
        "case setup: the account must hold no usage records, got {records:?}"
    );

    // Rows span the ways an expression can be unevaluable: its arity, its
    // dimension, its match options and its values.
    let cases: [(&str, Value); 6] = [
        ("no root operator", json!({})),
        ("two root operators", expression_with(&["Not", "Dimensions"])),
        ("four root operators", expression_with(&ROOT_OPERATORS)),
        (
            "a dimension outside the enum",
            json!({"Dimensions": {"Key": "SERVICE_NAME", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]}}),
        ),
        (
            "two match options",
            json!({"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS", "CONTAINS"]}}),
        ),
        (
            "a non-numeric value on a numeric dimension",
            json!({"Dimensions": {"Key": "USAGE_PERCENTAGE", "Values": ["half"], "MatchOptions": ["EQUALS"]}}),
        ),
    ];

    for (case, filter) in cases {
        refuse_filter(&router, &format!("{case} with no records"), &filter).await;
    }

    let got = filtered(&router, &root_operator("Dimensions")).await;
    assert!(
        got.is_empty(),
        "case a well-formed filter with no records: expected no records, got {got:?}"
    );
}
