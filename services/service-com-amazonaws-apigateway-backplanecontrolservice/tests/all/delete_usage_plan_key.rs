// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DeleteUsagePlanKey` changes BEYOND the records its own reads
//! report: the `execute-api` gate the association exists to open, and the metering the
//! association carries.
//!
//! The CONTROL-PLANE half of this operation — the modelled 202, the pair's read and the
//! plan's collection after the delete, the second delete's 404, the refusal each
//! unresolvable pair earns — is the sibling `usage_plan_state_and_refusals` suite's, and is
//! not repeated here; its helpers ([`meter_key`], [`delete_plan_key`], [`metered_key_ids`])
//! are reused so one association is made and removed one way. What this suite adds is the
//! half no reader of the control plane can state.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service for the control plane, and an unsigned
//! request to the api's own invoke host for the gate — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi`/`PutMethod`/`CreateDeployment` helpers, the invoke reader, `CreateApiKey`
//! and `CreateUsagePlan` are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `execute_api_data_plane`, `api_key_lifecycle` and
//! `usage_plan_lifecycle`.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one recorded call:
//!
//! * deleting a usage plan key removes exactly the association the pair addresses, so that
//!   plan stops metering that API key: a key-required method of an api stage the plan covers
//!   stops admitting the key, while every OTHER association of the same key goes on
//!   admitting it, and an association made again admits it once more. The rows differ along
//!   WHICH association of the plan is removed and along whether a second plan still meters
//!   that key;
//! * the removal takes that pair's metering with it, so an association made again is
//!   metered from the plan's full `throttle` rather than inheriting the accounting of the
//!   association that was deleted, over a table of burst capacities;
//! * the association is resolved and removed under one hold of the store, so concurrent
//!   deletes of ONE association resolve to exactly one success and every other caller is
//!   refused the modelled not-found, whatever the racer count, with nothing else the plan
//!   holds removed.

use std::sync::Arc;

use serde_json::json;

use crate::api_key_lifecycle::{create_key, get_key};
use crate::execute_api_data_plane::{
    deploy, integration_response, invoke, json_template, method_response, put_integration,
    put_method,
};
use crate::rest_api_lifecycle::{
    assert_failure, create_api, resource_under, string_member, Answer, Gateway,
};
use crate::usage_plan_key_lifecycle::get_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;
use crate::usage_plan_state_and_refusals::{
    delete_plan_key, meter_key, metered_key_ids, DELETE_STATUS, NO_SUCH_USAGE_PLAN_KEY,
};

/// The error a pair that names no stored association is refused with: `NotFoundException`
/// (HTTP 404), which `DeleteUsagePlanKey` models
/// (`smithy_query(query="operation", operation="DeleteUsagePlanKey")`).
const NOT_FOUND: &str = "NotFoundException";

/// The stage the metered api is deployed to; one stage drives the gate.
const STAGE: &str = "prod";

/// The body the key-required `MOCK` method answers, so an admitted call is told apart from
/// a refused one by its body alone.
const SERVED: &str = r#"{"served":true}"#;

/// What the `execute-api` gate answers a request whose API key no record associates with
/// the invoked stage.
const FORBIDDEN: &str = "Forbidden";

/// What a usage plan answers a request it has no capacity left for at this instant.
const THROTTLED: &str = "Too Many Requests";

/// The request rate every throttled row declares: a thousandth of a request per second, as
/// the sibling `stage_request_validation` suite documents, so no refill can fall inside a
/// burst however long these requests take on the machine running them — what a row asserts
/// is the capacity the plan admits, not the clock.
const SLOW_RATE: f64 = 0.001;

/// One api deployed to [`STAGE`] carrying a GET method that requires an API key, behind a
/// `MOCK` integration answering [`SERVED`]: the gate a metered key is admitted by.
///
/// `MOCK` is the one integration type that needs no backend — "for testing without actually
/// invoking the backend" (`IntegrationType`) — so an admitted call is served in full.
async fn metered_api(gateway: &Gateway, name: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    let resource = resource_under(gateway, &api, &root, "metered").await;
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

/// A usage plan over one api stage of `api`, answering the plan id: the association that
/// makes the plan gate traffic on that stage.
async fn plan_over_stage(gateway: &Gateway, case: &str, api: &str, name: &str) -> String {
    let created = create_usage_plan(
        gateway,
        case,
        json!({
            "name": name,
            "apiStages": [{ "apiId": api, "stage": STAGE }],
        }),
    )
    .await;
    string_member(&created, "id", case)
}

/// `CreateApiKey` for an enabled key, answering `(id, value)`.
///
/// Both members, which is what a gate case needs and what the sibling suite's id-only helper
/// cannot answer: the id is what the association is addressed by, and the VALUE is what a
/// client presents in `x-api-key`.
async fn api_key(gateway: &Gateway, case: &str) -> (String, String) {
    let created = create_key(gateway, case, json!({ "name": case, "enabled": true })).await;
    (
        string_member(&created, "id", case),
        string_member(&created, "value", case),
    )
}

/// Assert a delete was accepted: the modelled status, and the empty body an operation whose
/// output has no members answers with.
fn assert_deleted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, DELETE_STATUS,
        "case {case}: expected {DELETE_STATUS} from DeleteUsagePlanKey, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.is_null(),
        "case {case}: expected an empty body, got {:?}",
        answer.body
    );
}

/// One invoke of the metered method presenting `value` as the API key, as the status and
/// the text a client reads: the gateway response's `message` for a refusal, the served body
/// otherwise.
async fn present(gateway: &Gateway, api: &str, value: &str) -> (u16, String) {
    let answered = invoke(
        gateway,
        api,
        "GET",
        &format!("/{STAGE}/metered"),
        &[("x-api-key", value)],
    )
    .await;
    let text = answered.message().unwrap_or_else(|| answered.body.clone());
    (answered.status, text)
}

/// Assert the plan admits exactly `burst` requests for this key at this instant and sheds
/// the next one.
async fn assert_burst(gateway: &Gateway, api: &str, value: &str, burst: usize, case: &str) {
    let mut answered = Vec::with_capacity(burst + 1);
    for _ in 0..=burst {
        answered.push(present(gateway, api, value).await);
    }
    let served = answered
        .iter()
        .take_while(|(status, _)| *status == 200)
        .count();
    assert_eq!(
        served, burst,
        "case {case}: expected the plan to admit {burst} request(s), got {answered:?}"
    );
    assert_eq!(
        answered[burst],
        (429, THROTTLED.to_string()),
        "case {case}: expected the request past the burst to be shed, got {answered:?}"
    );
}

/// One row of the unmetering table: which association of which plan is removed, and whether
/// another association still covers that key afterwards.
struct Row {
    case: &'static str,
    /// The index into the plans the test stands up.
    plan: usize,
    /// The index into the keys the test stands up.
    key: usize,
    /// Whether the gate still admits the key once this association is gone.
    admitted_after: bool,
}

#[tokio::test]
async fn deleting_a_usage_plan_key_stops_that_plan_metering_that_key_alone() {
    let gateway = Gateway::new();
    let api = metered_api(&gateway, "usage-plan-key-delete").await;

    // Two plans over the SAME api stage, so a key one plan stops metering is told apart
    // from a key no plan meters at all.
    let mut plans: Vec<String> = Vec::new();
    for name in ["plan-first", "plan-second"] {
        plans.push(plan_over_stage(&gateway, name, &api, name).await);
    }

    // Three keys metered by the first plan; the middle one is metered by both, which is the
    // row where the delete must leave the key admitted.
    let mut keys: Vec<(String, String)> = Vec::new();
    for n in 0..3 {
        let case = format!("unmeter-{n}");
        let (id, value) = api_key(&gateway, &case).await;
        meter_key(&gateway, &plans[0], &id).await;
        keys.push((id, value));
    }
    meter_key(&gateway, &plans[1], &keys[1].0).await;

    // Baseline: every key the records associate with this api stage is admitted, so a 403
    // below is the delete's doing.
    for (n, (_, value)) in keys.iter().enumerate() {
        let answered = present(&gateway, &api, value).await;
        assert_eq!(
            answered,
            (200, SERVED.to_string()),
            "case key {n} before any delete: expected the served body, got {answered:?}"
        );
    }

    // Rows: WHICH association of the plan is removed — the first, a middle one, the last —
    // and whether a second plan still meters that key, which is what decides whether the
    // gate still admits it once this association is gone.
    let rows = [
        Row {
            case: "the first association of the plan",
            plan: 0,
            key: 0,
            admitted_after: false,
        },
        Row {
            case: "an association of a key a second plan also meters",
            plan: 0,
            key: 1,
            admitted_after: true,
        },
        Row {
            case: "the last association of the plan",
            plan: 0,
            key: 2,
            admitted_after: false,
        },
        Row {
            case: "the second plan's association of that same key",
            plan: 1,
            key: 1,
            admitted_after: false,
        },
    ];
    for row in rows {
        let case = row.case;
        let plan = plans[row.plan].clone();
        let (key_id, key_value) = keys[row.key].clone();

        assert_deleted(&delete_plan_key(&gateway, &plan, &key_id).await, case);
        // The pair's own read stops resolving, which is the record the gate's answer below
        // follows from.
        let read = get_usage_plan_key(&gateway, &plan, &key_id).await;
        assert_failure(&read, 404, NOT_FOUND, case);
        assert_eq!(
            read.body["message"].as_str(),
            Some(NO_SUCH_USAGE_PLAN_KEY),
            "case {case}: expected the sentence {NO_SUCH_USAGE_PLAN_KEY:?}, got {:?}",
            read.body["message"]
        );

        // The gate: that plan stops metering the key, so the key is admitted only while
        // another association still covers it.
        let answered = present(&gateway, &api, &key_value).await;
        let expected = if row.admitted_after {
            (200, SERVED.to_string())
        } else {
            (403, FORBIDDEN.to_string())
        };
        assert_eq!(
            answered, expected,
            "case {case}: expected {expected:?} at the gate, got {answered:?}"
        );
    }

    // Every pair can be metered again, and the gate admits it once it is: the deletes
    // reclaimed the associations rather than blocking the keys.
    for (n, (id, value)) in keys.iter().enumerate() {
        meter_key(&gateway, &plans[0], id).await;
        let answered = present(&gateway, &api, value).await;
        assert_eq!(
            answered,
            (200, SERVED.to_string()),
            "case key {n} re-metered: expected the served body, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn an_association_made_again_is_metered_from_the_plans_full_throttle() {
    // Rows differ along the plan's burst capacity: how many requests the plan admits for
    // one of its keys before it sheds one.
    for burst in [1_usize, 3] {
        let case = format!("a plan with a burst of {burst}");
        let gateway = Gateway::new();
        let api = metered_api(&gateway, "usage-plan-key-delete-metering").await;
        let created = create_usage_plan(
            &gateway,
            &case,
            json!({
                "name": format!("plan-burst-{burst}"),
                "apiStages": [{ "apiId": api, "stage": STAGE }],
                "throttle": { "burstLimit": burst, "rateLimit": SLOW_RATE },
            }),
        )
        .await;
        let plan = string_member(&created, "id", &case);
        let (key_id, key_value) = api_key(&gateway, "metered-burst").await;
        meter_key(&gateway, &plan, &key_id).await;

        // The plan admits its burst for this pair and sheds the next request: the pair's
        // accounting is spent.
        assert_burst(
            &gateway,
            &api,
            &key_value,
            burst,
            &format!("{case}, before the delete"),
        )
        .await;

        // The delete takes that accounting with the association, and between the delete and
        // the next create the plan meters nothing for the key at all.
        assert_deleted(&delete_plan_key(&gateway, &plan, &key_id).await, &case);
        let answered = present(&gateway, &api, &key_value).await;
        assert_eq!(
            answered,
            (403, FORBIDDEN.to_string()),
            "case {case}: an unmetered key is refused at the gate, got {answered:?}"
        );

        // So the association made again is metered from the plan's full burst rather than
        // inheriting the spent accounting of the one that was deleted.
        meter_key(&gateway, &plan, &key_id).await;
        assert_burst(
            &gateway,
            &api,
            &key_value,
            burst,
            &format!("{case}, after the delete"),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_of_one_association_resolve_to_exactly_one_success() {
    for racers in [2_usize, 3, 6] {
        let case = format!("{racers} concurrent deletes of one association");
        let gateway = Arc::new(Gateway::new());
        let created = create_usage_plan(&gateway, &case, json!({ "name": "plan-raced" })).await;
        let plan = string_member(&created, "id", &case);
        let (target, _) = api_key(&gateway, "raced").await;
        // A sibling association of the same plan that no racer addresses: what the contended
        // delete removes is the pair it names and nothing else.
        let (sibling, _) = api_key(&gateway, "raced-sibling").await;
        meter_key(&gateway, &plan, &target).await;
        meter_key(&gateway, &plan, &sibling).await;

        let mut tasks = Vec::with_capacity(racers);
        for _ in 0..racers {
            let gateway = Arc::clone(&gateway);
            let plan = plan.clone();
            let target = target.clone();
            tasks.push(tokio::spawn(async move {
                let answer = delete_plan_key(&gateway, &plan, &target).await;
                (answer.status, answer.error, answer.body)
            }));
        }
        let mut answers = Vec::with_capacity(racers);
        for task in tasks {
            answers.push(task.await.expect("the racing delete answered"));
        }

        let accepted = answers
            .iter()
            .filter(|(status, _, _)| *status == DELETE_STATUS)
            .count();
        assert_eq!(
            accepted, 1,
            "case {case}: expected exactly one accepted delete, got {answers:?}"
        );
        for (status, error, body) in &answers {
            if *status == DELETE_STATUS {
                continue;
            }
            assert_eq!(
                (*status, error.as_deref(), body["message"].as_str()),
                (404, Some(NOT_FOUND), Some(NO_SUCH_USAGE_PLAN_KEY)),
                "case {case}: every loser is the modelled not-found, got {answers:?}"
            );
        }

        // The contended delete removed the pair it addressed and nothing else: the plan's
        // other association is still held, and the API key is the account's.
        let listed = metered_key_ids(&gateway, &plan, &case).await;
        assert_eq!(
            listed,
            vec![sibling.clone()],
            "case {case}: expected only the sibling association left, got {listed:?}"
        );
        let read = get_key(&gateway, &target, false).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the API key must outlive the raced association, got {} with {:?}",
            read.status, read.body
        );
    }
}
