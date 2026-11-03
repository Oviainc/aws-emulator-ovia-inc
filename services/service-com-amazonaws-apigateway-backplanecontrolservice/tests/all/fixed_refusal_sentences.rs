// SPDX-License-Identifier: CC0-1.0

//! The refusals API Gateway words as a FIXED sentence, asserted on the wire.
//!
//! The RULE: where API Gateway states one sentence for a condition, that sentence is the
//! whole message — its text exactly as the service words it, with NO part of the request
//! quoted back. A refusal that names the plan, the key, the status code or the verb the
//! caller already sent is a different message from the one the service answers, so each
//! test below drives its condition over a TABLE of requests and asserts that the one
//! sentence answers all of them and that none of the row's own identifiers appears in it.
//!
//! The conditions covered here are the ones whose sentence carries no variable part at
//! all:
//!
//! * `CreateVpcLink` whose `targetArns` is sent empty;
//! * `CreateUsagePlanKey` for an API key the addressed plan already meters;
//! * `PutMethodResponse` for a status code the addressed method already answers.
//!
//! The harness is this crate's existing one (`rest_api_lifecycle`), so every row goes
//! through the multi-service router, `restJson1` (de)serialization and the mounted
//! service, and what is asserted is what a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    METHOD_RESPONSE_ALREADY_EXISTS, USAGE_PLAN_KEY_ALREADY_EXISTS, VPC_LINK_NEEDS_A_TARGET,
};

use crate::api_key_lifecycle::create_key;
use crate::method_response::put_method_response;
use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};
use crate::stage_lifecycle::api_with_a_method;
use crate::usage_plan_key_lifecycle::create_usage_plan_key;
use crate::usage_plan_lifecycle::create_usage_plan;
use crate::vpc_links::{create_vpc_link, target_arn};

// Each sentence is written out HERE as a literal, and the service's own constant is
// asserted to equal it, so a change of wording is caught rather than followed: an
// assertion made only against the constant would compare the implementation with itself.
//
// Source for all three: API Gateway's recorded refusals of these conditions.

/// What `CreateVpcLink` answers when `targetArns` names no target.
const NEEDS_A_TARGET_SENTENCE: &str = "Need at least one target arn to create a vpc link";

/// What `CreateUsagePlanKey` answers for a key the addressed plan already meters.
const KEY_ALREADY_SUBSCRIBED_SENTENCE: &str = "API Key is already subscribed to Usage Plan";

/// What `PutMethodResponse` answers for a status code the addressed method already
/// answers.
const RESPONSE_ALREADY_EXISTS_SENTENCE: &str = "Response already exists for this resource";

/// The one key type a `UsagePlanKey` takes, as `UsagePlanKey$type` documents
/// ("Currently, the valid key type is `API_KEY`").
const API_KEY_TYPE: &str = "API_KEY";

/// AWS's published minimum length for a caller-supplied API key value, as the
/// `api_key_lifecycle` suite documents it.
const MIN_KEY_VALUE_CHARS: usize = 20;

/// The `ConflictException` a duplicate answers with, as this service models it.
const CONFLICT: (u16, &str) = (409, "ConflictException");

/// The `BadRequestException` a malformed request answers with.
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// Assert `answer` is the modelled failure `(status, code)` whose message is EXACTLY
/// `sentence` and carries none of `absent`.
fn assert_sentence(
    answer: &Answer,
    expected: (u16, &str),
    sentence: &str,
    absent: &[&str],
    case: &str,
) {
    assert_failure(answer, expected.0, expected.1, case);
    let message = answer.body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, sentence,
        "case {case}: expected the message {sentence:?}, got {message:?}"
    );
    for value in absent {
        assert!(
            !message.contains(value),
            "case {case}: expected the sentence to quote back no part of the request, \
             but it names {value:?}: {message:?}"
        );
    }
}

#[tokio::test]
async fn a_vpc_link_create_naming_no_target_is_refused_with_one_sentence() {
    // The service's own constant is that sentence, so no second wording of this condition
    // can reach a caller.
    assert_eq!(
        VPC_LINK_NEEDS_A_TARGET, NEEDS_A_TARGET_SENTENCE,
        "case the service constant: expected {NEEDS_A_TARGET_SENTENCE:?}, got \
         {VPC_LINK_NEEDS_A_TARGET:?}"
    );

    // Rows differ along the rest of the request, which the sentence does not depend on: a
    // bare create, a described one, a tagged one, and one carrying both.
    let rows: Vec<(&str, Value)> = vec![
        (
            "bare",
            json!({ "name": "no-targets-bare", "targetArns": [] }),
        ),
        (
            "described",
            json!({
                "name": "no-targets-described",
                "targetArns": [],
                "description": "a link with nothing to reach",
            }),
        ),
        (
            "tagged",
            json!({
                "name": "no-targets-tagged",
                "targetArns": [],
                "tags": { "team": "api" },
            }),
        ),
        (
            "described and tagged",
            json!({
                "name": "no-targets-both",
                "targetArns": [],
                "description": "a link with nothing to reach",
                "tags": { "team": "api", "env": "test" },
            }),
        ),
    ];

    let gateway = Gateway::new();
    for (case, request) in &rows {
        let name = request["name"].as_str().unwrap_or_default();
        let answer = create_vpc_link(&gateway, request.clone()).await;
        assert_sentence(
            &answer,
            BAD_REQUEST,
            NEEDS_A_TARGET_SENTENCE,
            &[name, "targetArns"],
            case,
        );
    }

    // `targetArns` is `@required` on the request shape
    // (`smithy_query(query="operation",operation="CreateVpcLink")`), so a request that
    // leaves the member out entirely never reaches the rule above: it is the framework's
    // own required-member refusal. Asserted here so the two refusals are not confused.
    let absent = create_vpc_link(&gateway, json!({ "name": "no-targets-absent" })).await;
    assert_failure(&absent, 400, "ValidationException", "targetArns absent");

    // A request that DOES name a target is accepted, so the rows above were refused for
    // the targets they named and nothing else.
    let accepted = create_vpc_link(
        &gateway,
        json!({ "name": "one-target", "targetArns": [target_arn(0)] }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case one target: expected 202, got {} with {:?}",
        accepted.status, accepted.body
    );
}

/// A caller-supplied API key value of the published minimum length, distinct per `n`: an
/// API key's value is unique per account, so two rows cannot supply the same one.
fn supplied_value(n: usize) -> String {
    let distinct = format!("{n}");
    format!("{}{distinct}", "v".repeat(MIN_KEY_VALUE_CHARS - distinct.len()))
}

/// One row of the key table: the case name, and the `CreateApiKey` request for the row at
/// that index (see [`supplied_value`]).
type KeyRow = (&'static str, fn(usize) -> Value);

#[tokio::test]
async fn a_key_a_plan_already_meters_is_refused_with_one_sentence() {
    // The service's own constant is that sentence; see the VPC-link suite above.
    assert_eq!(
        USAGE_PLAN_KEY_ALREADY_EXISTS, KEY_ALREADY_SUBSCRIBED_SENTENCE,
        "case the service constant: expected {KEY_ALREADY_SUBSCRIBED_SENTENCE:?}, got \
         {USAGE_PLAN_KEY_ALREADY_EXISTS:?}"
    );

    let gateway = Gateway::new();

    // Rows differ along the plan and the key the pair is made of — a plan with throttling
    // and quota set and one with neither, a bare key and a named key the caller valued —
    // so the sentence cannot be a function of either identifier.
    let plans: Vec<(&str, Value)> = vec![
        ("a bare plan", json!({ "name": "dedup-bare" })),
        (
            "a throttled and quota'd plan",
            json!({
                "name": "dedup-metered",
                "throttle": { "rateLimit": 10.0, "burstLimit": 20 },
                "quota": { "limit": 100, "period": "DAY" },
            }),
        ),
    ];
    // `usize` is the row's index, which is what keeps each supplied key value distinct.
    let keys: Vec<KeyRow> = vec![
        ("a bare key", |_| json!({})),
        ("a named key with a caller-supplied value", |n| {
            json!({ "name": format!("dedup-valued-{n}"), "value": supplied_value(n) })
        }),
    ];

    let mut row = 0usize;
    for (plan_case, plan_request) in &plans {
        let plan = create_usage_plan(&gateway, plan_case, plan_request.clone()).await;
        let plan_id = string_member(&plan, "id", plan_case);
        for (key_case, key_request) in &keys {
            row += 1;
            let case = format!("{plan_case} and {key_case}");
            let key = create_key(&gateway, &case, key_request(row)).await;
            let key_id = string_member(&key, "id", &case);

            let first = create_usage_plan_key(&gateway, &plan_id, &key_id, API_KEY_TYPE).await;
            assert_eq!(
                first.status, 201,
                "case {case}: expected the first association to be created, got {} with {:?}",
                first.status, first.body
            );

            // Two further attempts, so the sentence is the one EVERY repeat answers rather
            // than only the second call.
            for attempt in 2..=3 {
                let repeated =
                    create_usage_plan_key(&gateway, &plan_id, &key_id, API_KEY_TYPE).await;
                assert_sentence(
                    &repeated,
                    CONFLICT,
                    KEY_ALREADY_SUBSCRIBED_SENTENCE,
                    &[plan_id.as_str(), key_id.as_str()],
                    &format!("{case}, attempt {attempt}"),
                );
            }
        }
    }
}

#[tokio::test]
async fn a_method_response_a_method_already_answers_is_refused_with_one_sentence() {
    // The service's own constant is that sentence; see the VPC-link suite above.
    assert_eq!(
        METHOD_RESPONSE_ALREADY_EXISTS, RESPONSE_ALREADY_EXISTS_SENTENCE,
        "case the service constant: expected {RESPONSE_ALREADY_EXISTS_SENTENCE:?}, got \
         {METHOD_RESPONSE_ALREADY_EXISTS:?}"
    );

    let gateway = Gateway::new();
    let (api, resource) = api_with_a_method(&gateway, "response-conflict").await;

    // Rows differ along the status code the response answers (the 2xx, 4xx and 5xx classes
    // a method can declare) and along the body the repeat carries, which the sentence does
    // not depend on.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("a 200 with no models", "200", json!({})),
        (
            "a 201 with a response parameter",
            "201",
            json!({ "responseParameters": { "method.response.header.Location": true } }),
        ),
        (
            "a 404 with a response model",
            "404",
            json!({ "responseModels": { "application/json": "Error" } }),
        ),
        ("a 500 with no models", "500", json!({})),
    ];

    for (case, status, request) in &rows {
        let first =
            put_method_response(&gateway, &api, &resource, "GET", status, request.clone()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected the first PutMethodResponse to be created, got {} with {:?}",
            first.status, first.body
        );

        // The repeat carries a DIFFERENT body from the create, so a conflict cannot be
        // mistaken for an idempotent re-put of the same request.
        let repeated = put_method_response(
            &gateway,
            &api,
            &resource,
            "GET",
            status,
            json!({ "responseModels": { "application/json": "Empty" } }),
        )
        .await;
        assert_sentence(
            &repeated,
            CONFLICT,
            RESPONSE_ALREADY_EXISTS_SENTENCE,
            &[status, "GET", api.as_str(), resource.as_str()],
            case,
        );
    }
}
