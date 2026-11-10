// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the QUOTA a usage plan enforces at a deployed stage: the count of
//! requests "that can be made in a given time period" (`QuotaSettings$limit`) and what the
//! `execute-api` endpoint answers a request past it.
//!
//! Every case drives the real wire path — an unsigned request to the api's own invoke host
//! through the multi-service router, with the control plane driven through the same emulator
//! ([`Gateway`]) — so what is asserted is what a client of a published invoke URL sees.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a usage plan's `quota` is a BUDGET of requests per metering period, counted per
//!   `(usage plan, API key)` pair: the stage serves the requests the period's budget — the
//!   plan's `limit`, less its `offset` in the period the plan was created in — still has
//!   room for, and refuses every later request carrying that key for the rest of the period
//!   with the `QUOTA_EXCEEDED` gateway response: HTTP 429, the `message` "Limit Exceeded",
//!   and `LimitExceededException` in the `x-amzn-errortype` header a client classifies an
//!   answer by. The rows differ along the quota's `period` (every value the model declares),
//!   along its `limit`, along an `offset` that shortens the initial period, along a budget of
//!   no requests at all and along a plan carrying no quota, which bounds nothing;
//! * the accounting is per API KEY, not per request or connection: a key whose period is
//!   spent stays refused across later requests while another key of the same plan still has
//!   its own whole budget, and a key metered by two plans has to satisfy both, so the tighter
//!   quota is the one that refuses it;
//! * the refusal is an ADMISSION step — it is answered with the method's integration never
//!   performed — and it is not the plan's throttle: the `THROTTLED` refusal of a plan with no
//!   request left AT THIS INSTANT carries the other sentence and the other exception class,
//!   so a client can tell a key that is cut off for the period from one that is sending too
//!   fast.

use serde_json::{json, Value};

use crate::api_key_lifecycle::create_key;
use crate::execute_api_data_plane::{
    deploy, integration_response, invoke, json_template, method_response, put_integration,
    put_method, Invoked,
};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};
use crate::test_invoke_method::lambda_uri;
use crate::usage_plan_key_lifecycle::create_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;

/// The stage every api below is deployed to.
const STAGE: &str = "test";

/// The one key type a `UsagePlanKey` takes: "Currently, the valid key type is `API_KEY`"
/// (`smithy_query(query="doc", shape="UsagePlanKey", member="type")`).
const API_KEY_TYPE: &str = "API_KEY";

/// The path the metered method sits on.
const ROUTE: &str = "q";

/// The body the `MOCK` integration behind the metered method answers: what proves a request
/// was SERVED, so no refusal may carry it.
const SERVED: &str = r#"{"served":true}"#;

/// The `message` the `QUOTA_EXCEEDED` gateway response carries.
const LIMIT_EXCEEDED: &str = "Limit Exceeded";

/// The exception class a request past the period's budget is reported in.
const LIMIT_EXCEEDED_CLASS: &str = "LimitExceededException";

/// The `message` the `THROTTLED` gateway response carries — the OTHER 429 a metered call can
/// earn, which the quota's refusal must be distinguishable from.
const TOO_MANY_REQUESTS: &str = "Too Many Requests";

/// The exception class of that other 429.
const TOO_MANY_REQUESTS_CLASS: &str = "TooManyRequestsException";

/// The sentence an integration whose backend this emulator cannot call answers with, which is
/// what a request the quota ADMITTED earns on the proxied route: a visibly different answer
/// from a refusal, so the two cannot be confused.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// A `throttle` far above the rate these tests send at, so no row's refusal can be the
/// plan's rate limit: the quota is what every row below isolates.
fn unthrottled() -> Value {
    json!({ "burstLimit": 5_000, "rateLimit": 5_000.0 })
}

/// A distinct API key value of the 40 characters AWS mints; a value is unique per account, so
/// no two keys of one emulator may carry the same one.
fn key_value(n: usize) -> String {
    format!("{}{n:03}", "q".repeat(37))
}

/// `CreateApiKey` for a fresh value, answering `(id, value)`.
async fn api_key(gateway: &Gateway, case: &str, n: usize) -> (String, String) {
    let value = key_value(n);
    let created = create_key(
        gateway,
        case,
        json!({ "name": format!("key-{n}"), "value": value, "enabled": true }),
    )
    .await;
    (string_member(&created, "id", case), value)
}

/// A usage plan over `api`'s [`STAGE`] carrying `quota` (none when `None`) and the throttle
/// `throttle`, metering every key in `key_ids`; answers the plan id.
async fn metering_plan(
    gateway: &Gateway,
    case: &str,
    quota: Option<Value>,
    throttle: Value,
    api: &str,
    key_ids: &[&str],
) -> String {
    let mut request = json!({
        "name": format!("plan-{case}"),
        "apiStages": [{ "apiId": api, "stage": STAGE }],
        "throttle": throttle,
    });
    if let Some(quota) = quota {
        request["quota"] = quota;
    }
    let created = create_usage_plan(gateway, case, request).await;
    let plan = string_member(&created, "id", case);
    for key_id in key_ids {
        let answer = create_usage_plan_key(gateway, &plan, key_id, API_KEY_TYPE).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected CreateUsagePlanKey to answer 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
    plan
}

/// An api carrying one key-required `GET` on [`ROUTE`] behind a `MOCK` integration that
/// answers [`SERVED`], deployed to [`STAGE`]; answers the api id.
async fn metered_api(gateway: &Gateway, name: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    let resource = resource_under(gateway, &api, &root, ROUTE).await;
    put_method(
        gateway,
        &api,
        &resource,
        "GET",
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    put_integration(gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
    method_response(gateway, &api, &resource, "GET", "200", json!({})).await;
    integration_response(
        gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "responseTemplates": json_template(SERVED) }),
    )
    .await;
    deploy(gateway, &api, STAGE).await;
    api
}

/// An api carrying one key-required `GET` on [`ROUTE`] in front of a Lambda proxy whose
/// backend this emulator cannot call, deployed to [`STAGE`]: a request that REACHES the
/// integration is answered [`CONFIGURATION_ERROR`], so an admission step's refusal is
/// distinguishable from the integration's own answer.
async fn proxied_metered_api(gateway: &Gateway, name: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    let resource = resource_under(gateway, &api, &root, ROUTE).await;
    put_method(
        gateway,
        &api,
        &resource,
        "GET",
        json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
    )
    .await;
    put_integration(
        gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri() }),
    )
    .await;
    deploy(gateway, &api, STAGE).await;
    api
}

/// One `GET` of the metered route presenting `key_value`.
async fn send(gateway: &Gateway, api: &str, key_value: &str) -> Invoked {
    invoke(
        gateway,
        api,
        "GET",
        &format!("/{STAGE}/{ROUTE}"),
        &[("x-api-key", key_value)],
    )
    .await
}

/// Assert one invoke was SERVED by the integration behind the metered route.
fn assert_served(answered: &Invoked, case: &str) {
    assert_eq!(
        answered.status, 200,
        "case {case}: expected the quota to admit the request, got {} with {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.body, SERVED,
        "case {case}: expected the integration's own body, got {:?}",
        answered.body
    );
}

/// Assert one invoke was refused as a request past the period's budget: the status, the
/// sentence, the exception class, and that nothing of the integration's answer reached the
/// caller.
fn assert_past_the_quota(answered: &Invoked, case: &str) {
    assert_eq!(
        answered.status, 429,
        "case {case}: expected 429 past the quota, got {} with {:?}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.message().as_deref(),
        Some(LIMIT_EXCEEDED),
        "case {case}: expected the sentence {LIMIT_EXCEEDED:?}, got {:?}",
        answered.body
    );
    assert_eq!(
        answered.header("x-amzn-errortype"),
        Some(LIMIT_EXCEEDED_CLASS),
        "case {case}: expected the refusal to name the class {LIMIT_EXCEEDED_CLASS:?}, got {:?}",
        answered.header("x-amzn-errortype")
    );
    assert!(
        !answered.body.contains("served"),
        "case {case}: expected nothing of the integration's answer, got {:?}",
        answered.body
    );
}

/// One quota row: the plan's `quota`, how many requests the period admits, and how many
/// sequential requests the key sends.
struct Budgeted {
    case: &'static str,
    /// The plan's `quota` member, `None` for a plan created without one.
    quota: Option<Value>,
    /// The requests the metering period admits: the `limit`, less the `offset` in the period
    /// the plan was created in.
    budget: usize,
    /// How many requests the key sends, one after another.
    requests: usize,
}

/// The rows: every metering period the model declares (`QuotaPeriodType`, read with
/// `smithy_query(query="shape", shape="QuotaPeriodType")` — `DAY`, `WEEK`, `MONTH`), several
/// limits, an `offset` that shortens the initial period, a budget of no requests at all, and
/// a plan that carries no quota and so bounds nothing. A `DAY` period admits no offset
/// (`max_quota_offset`), so the offset rows name the wider periods.
fn rows() -> Vec<Budgeted> {
    vec![
        Budgeted {
            case: "a daily quota of one request",
            quota: Some(json!({ "limit": 1, "offset": 0, "period": "DAY" })),
            budget: 1,
            requests: 4,
        },
        Budgeted {
            case: "a daily quota of two requests, over eight sequential requests",
            quota: Some(json!({ "limit": 2, "offset": 0, "period": "DAY" })),
            budget: 2,
            requests: 8,
        },
        Budgeted {
            case: "a daily quota of five requests",
            quota: Some(json!({ "limit": 5, "offset": 0, "period": "DAY" })),
            budget: 5,
            requests: 7,
        },
        Budgeted {
            case: "a weekly quota shortened by its offset",
            quota: Some(json!({ "limit": 4, "offset": 1, "period": "WEEK" })),
            budget: 3,
            requests: 6,
        },
        Budgeted {
            case: "a monthly quota shortened by its offset",
            quota: Some(json!({ "limit": 3, "offset": 2, "period": "MONTH" })),
            budget: 1,
            requests: 4,
        },
        Budgeted {
            case: "a monthly quota of two requests",
            quota: Some(json!({ "limit": 2, "offset": 0, "period": "MONTH" })),
            budget: 2,
            requests: 5,
        },
        Budgeted {
            case: "a quota of no requests at all",
            quota: Some(json!({ "limit": 0, "offset": 0, "period": "DAY" })),
            budget: 0,
            requests: 3,
        },
        Budgeted {
            case: "a weekly quota whose offset is its whole limit",
            quota: Some(json!({ "limit": 2, "offset": 2, "period": "WEEK" })),
            budget: 0,
            requests: 3,
        },
        Budgeted {
            case: "a plan carrying no quota at all",
            quota: None,
            budget: 6,
            requests: 6,
        },
    ]
}

#[tokio::test]
async fn a_usage_plan_quota_refuses_every_request_past_the_periods_budget() {
    for (index, row) in rows().into_iter().enumerate() {
        let gateway = Gateway::new();
        let api = metered_api(&gateway, "usage-plan-quota").await;
        let (key_id, key) = api_key(&gateway, row.case, index).await;
        metering_plan(
            &gateway,
            row.case,
            row.quota.clone(),
            unthrottled(),
            &api,
            &[key_id.as_str()],
        )
        .await;

        let mut answers = Vec::new();
        for _ in 0..row.requests {
            answers.push(send(&gateway, &api, &key).await);
        }
        let statuses: Vec<u16> = answers.iter().map(|answered| answered.status).collect();
        let served = statuses.iter().take_while(|status| **status == 200).count();
        assert_eq!(
            served, row.budget,
            "case {}: expected the period to admit {} of {} requests, got {statuses:?}",
            row.case, row.budget, row.requests
        );
        // Every request the budget had room for is served, and every later one — not just the
        // first past it — is refused for the rest of the period.
        for (sent, answered) in answers.iter().enumerate() {
            let case = format!("{} (request {})", row.case, sent + 1);
            if sent < row.budget {
                assert_served(answered, &case);
            } else {
                assert_past_the_quota(answered, &case);
            }
        }
    }
}

#[tokio::test]
async fn the_quota_is_counted_per_api_key_so_one_spent_key_does_not_refuse_another() {
    // Two keys of ONE plan: the plan's limit is the plan's FOR EACH of its keys, so the
    // second key's period is whole however much the first has spent, and the first stays
    // refused across later requests rather than recovering after a refusal.
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-plan-quota-per-key").await;
    let case = "a daily quota of two requests, two keys";
    let (first_id, first) = api_key(&gateway, case, 10).await;
    let (second_id, second) = api_key(&gateway, case, 11).await;
    metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 2, "offset": 0, "period": "DAY" })),
        unthrottled(),
        &api,
        &[first_id.as_str(), second_id.as_str()],
    )
    .await;

    for sent in 1..=2 {
        let answered = send(&gateway, &api, &first).await;
        assert_served(&answered, &format!("{case}: the first key, request {sent}"));
    }
    let refused = send(&gateway, &api, &first).await;
    assert_past_the_quota(&refused, &format!("{case}: the first key, spent"));

    // The second key's own budget, read AFTER the first key was cut off.
    for sent in 1..=2 {
        let answered = send(&gateway, &api, &second).await;
        assert_served(&answered, &format!("{case}: the second key, request {sent}"));
    }
    let refused = send(&gateway, &api, &second).await;
    assert_past_the_quota(&refused, &format!("{case}: the second key, spent"));

    // And the first key is still refused: a refused request is not a reset, so the period
    // stays spent for that key alone.
    let refused = send(&gateway, &api, &first).await;
    assert_past_the_quota(&refused, &format!("{case}: the first key, spent again"));
}

#[tokio::test]
async fn a_key_metered_by_two_plans_is_refused_by_the_tighter_quota() {
    // One key, two plans over the same api stage: a plan meters each of its keys against its
    // OWN quota, so the key has to satisfy both and the narrower budget is what cuts it off.
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-plan-quota-two-plans").await;
    let case = "a key metered by a quota of one and a quota of five";
    let (key_id, key) = api_key(&gateway, case, 20).await;
    for (name, limit) in [("tight", 1), ("wide", 5)] {
        metering_plan(
            &gateway,
            name,
            Some(json!({ "limit": limit, "offset": 0, "period": "DAY" })),
            unthrottled(),
            &api,
            &[key_id.as_str()],
        )
        .await;
    }

    let served = send(&gateway, &api, &key).await;
    assert_served(&served, &format!("{case}: the request both plans admit"));
    for sent in 2..=4 {
        let refused = send(&gateway, &api, &key).await;
        assert_past_the_quota(&refused, &format!("{case}: request {sent}"));
    }
}

#[tokio::test]
async fn the_quota_refusal_precedes_the_integration_and_is_not_the_throttles() {
    // The quota is an ADMISSION step: a request it refuses is answered with the integration
    // never performed. The route's backend is a Lambda this emulator cannot call, so a
    // request that reached the integration answers CONFIGURATION_ERROR — visibly different
    // from either 429.
    let gateway = Gateway::new();
    let api = proxied_metered_api(&gateway, "usage-plan-quota-admission").await;
    let case = "a daily quota of one request in front of a proxy integration";
    let (key_id, key) = api_key(&gateway, case, 30).await;
    metering_plan(
        &gateway,
        case,
        Some(json!({ "limit": 1, "offset": 0, "period": "DAY" })),
        unthrottled(),
        &api,
        &[key_id.as_str()],
    )
    .await;

    let admitted = send(&gateway, &api, &key).await;
    assert_eq!(
        admitted.status, 500,
        "case {case}: expected the admitted request to reach the integration, got {} with {:?}",
        admitted.status, admitted.body
    );
    assert_eq!(
        admitted.message().as_deref(),
        Some(CONFIGURATION_ERROR),
        "case {case}: expected the integration's own answer, got {:?}",
        admitted.body
    );
    let refused = send(&gateway, &api, &key).await;
    assert_past_the_quota(&refused, case);
    assert_ne!(
        refused.message().as_deref(),
        Some(CONFIGURATION_ERROR),
        "case {case}: expected the refusal to precede the integration, got {:?}",
        refused.body
    );

    // The OTHER 429: a plan whose quota has room to spare but whose throttle admits one
    // request at a time sheds the second one as `THROTTLED`, with the other sentence and the
    // other exception class. The rate is a thousandth of a request per second, so no refill
    // can fall inside this burst however long the requests take on the machine running them.
    let throttled_api = metered_api(&gateway, "usage-plan-quota-vs-throttle").await;
    let throttle_case = "a wide quota with a burst of one";
    let (throttled_id, throttled_key) = api_key(&gateway, throttle_case, 31).await;
    metering_plan(
        &gateway,
        throttle_case,
        Some(json!({ "limit": 1_000, "offset": 0, "period": "DAY" })),
        json!({ "burstLimit": 1, "rateLimit": 0.001 }),
        &throttled_api,
        &[throttled_id.as_str()],
    )
    .await;

    let served = send(&gateway, &throttled_api, &throttled_key).await;
    assert_served(&served, throttle_case);
    let shed = send(&gateway, &throttled_api, &throttled_key).await;
    assert_eq!(
        shed.status, 429,
        "case {throttle_case}: expected the throttle to shed the second request, got {} with \
         {:?}",
        shed.status, shed.body
    );
    assert_eq!(
        shed.message().as_deref(),
        Some(TOO_MANY_REQUESTS),
        "case {throttle_case}: expected the sentence {TOO_MANY_REQUESTS:?}, got {:?}",
        shed.body
    );
    assert_eq!(
        shed.header("x-amzn-errortype"),
        Some(TOO_MANY_REQUESTS_CLASS),
        "case {throttle_case}: expected the class {TOO_MANY_REQUESTS_CLASS:?}, got {:?}",
        shed.header("x-amzn-errortype")
    );
}
