// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 single-rule operations of a custom domain
//! name: `PutRoutingRule` and `GetRoutingRule`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly. The collection operations
//! (`CreateRoutingRule`/`ListRoutingRules`) have their own suite in
//! `routing_rule_lifecycle`; the rule a test here rewrites is seeded through the create.
//!
//! The rules asserted here, each over a TABLE rather than over one request:
//!
//! * `PutRoutingRule` REPLACES the conditions, the actions and the priority of the rule
//!   its path addresses and reports the STORED rule, which is what a later
//!   `GetRoutingRule` of the same id reports; the `RoutingRuleId` and the ARN the rule
//!   was minted with survive every rewrite.
//! * An operation whose path addresses a routing rule the named domain name does not
//!   hold is refused with the modelled `NotFoundException` (404) — the recorded case that
//!   filed the issue, one row of its table — and so is one whose path names a custom
//!   domain name the account does not hold, or names it under a `DomainNameId` that is
//!   not that domain name's own.
//! * A rewrite whose body is not a rule the service accepts is refused with the modelled
//!   `BadRequestException` (400) by the same checks the create applies, and the rule
//!   already stored survives unchanged.
//! * An action's `InvokeApi` is a reference the rule carries: an `ApiId` this service's
//!   own api store does not hold is stored as the request wrote it and reported back
//!   that way.
//! * A read reports the stored rule and changes nothing a later caller sees.
//!
//! The rows vary what each rule generalises over: the number and the form of the match
//! clauses (base paths, header matches, several values in an `AnyOf`), the number of
//! conditions, the priority (each published bound of `RoutingRulePriority` and values
//! between them), the stage and the api an action invokes, whether the action strips the
//! base path, which resource of the request is the missing one, and which rule of the
//! accepted form the body breaks.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    ARN_SERVICE, EMPTY_ROUTING_RULE_ACTIONS, EMPTY_ROUTING_RULE_CONDITIONS,
    INVALID_DOMAIN_NAME_IDENTIFIER, ROUTING_RULE_CONDITION_LIMITS, ROUTING_RULE_NOT_FOUND,
};

use crate::api_lifecycle::{as_str, assert_minted_id, call_in_region, error_code, strip_nulls};

/// The region every request in this suite is signed for. Deliberately NOT the
/// emulator's `us-east-1` default, so nothing region-derived (the rule's ARN) can be
/// hardcoded.
const REGION: &str = "eu-central-1";

/// The custom domain name every test seeds unless the row is about an absent one.
const DOMAIN: &str = "routed.example.com";

/// A second seeded domain name, so a row can address a rule that exists — under the
/// other domain name.
const OTHER_DOMAIN: &str = "elsewhere.example.com";

/// The stage every seeded api carries.
const STAGE: &str = "dev";

/// A second seeded stage, so a row can move the action's target without inventing one.
const OTHER_STAGE: &str = "prod";

/// The lowest `Priority` the model admits: `RoutingRulePriority` is an integer with
/// `range { min: 1, max: 1000000 }`
/// (`smithy_query(query="shape", shape="RoutingRulePriority")`).
const PRIORITY_MIN: i64 = 1;

/// The highest `Priority` the model admits — see [`PRIORITY_MIN`] for the source.
const PRIORITY_MAX: i64 = 1_000_000;

/// Send one request signed for [`REGION`] and answer `(status, body)`.
async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

async fn create_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/domainnames/{domain}/routingrules"),
        Some(request.clone()),
    )
    .await
}

async fn put_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    routing_rule_id: &str,
    query: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PUT",
        &format!("/v2/domainnames/{domain}/routingrules/{routing_rule_id}{query}"),
        Some(request.clone()),
    )
    .await
}

async fn get_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    routing_rule_id: &str,
    query: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules/{routing_rule_id}{query}"),
        None,
    )
    .await
}

/// Create one custom domain name, asserting the 201 the model binds to
/// `CreateDomainName`.
async fn seed_domain_name(emulator: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/domainnames",
        Some(json!({
            "domainName": domain,
            // AWS Certificate Manager is the only source `CertificateArn` documents.
            "domainNameConfigurations": [{
                "certificateArn": engine::emulator_arn(
                    "acm",
                    REGION,
                    &format!("certificate/{domain}"),
                ),
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding domain name {domain:?}: expected 201 from CreateDomainName, got {status} {body}"
    );
}

/// Create one HTTP api carrying [`STAGE`] and [`OTHER_STAGE`], and answer its `ApiId`.
async fn seed_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "seeding api {name:?}: expected 201 from CreateApi, got {status} {body}"
    );
    let api_id = as_str(&body, "apiId");
    for stage_name in [STAGE, OTHER_STAGE] {
        let (status, stage) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": stage_name })),
        )
        .await;
        assert_eq!(
            status, 201,
            "seeding stage {stage_name:?} of api {name:?}: expected 201, got {status} {stage}"
        );
    }
    api_id
}

/// A fresh emulator holding [`DOMAIN`], [`OTHER_DOMAIN`] and two apis, each with
/// [`STAGE`] and [`OTHER_STAGE`]. Answers `(emulator, api_id, other_api_id)`.
async fn seeded() -> (MultiServiceRouter, String, String) {
    let emulator = crate::api_lifecycle::emulator();
    seed_domain_name(&emulator, DOMAIN).await;
    seed_domain_name(&emulator, OTHER_DOMAIN).await;
    let api_id = seed_api(&emulator, "routed-api").await;
    let other_api_id = seed_api(&emulator, "routed-api-two").await;
    (emulator, api_id, other_api_id)
}

/// The body that seeds the rule a test rewrites: one base path and an action invoking
/// `api_id`'s [`STAGE`].
fn seed_rule_body(api_id: &str) -> Value {
    json!({
        "priority": 999,
        "conditions": [{ "matchBasePaths": { "anyOf": ["seed"] } }],
        "actions": [{ "invokeApi": { "apiId": api_id, "stage": STAGE } }],
    })
}

/// Seed one rule under `domain` and answer `(routing_rule_id, routing_rule_arn)`.
async fn seed_rule(emulator: &MultiServiceRouter, domain: &str, api_id: &str) -> (String, String) {
    let (status, created) = create_routing_rule(emulator, domain, &seed_rule_body(api_id)).await;
    assert_eq!(
        status, 201,
        "seeding a rule under {domain:?}: expected 201 from CreateRoutingRule, \
         got {status} {created}"
    );
    let routing_rule_id = as_str(&created, "routingRuleId");
    assert_minted_id("seeded rule", "routingRuleId", &routing_rule_id);
    // The ARN is derived here from the account-and-region form every resource of this
    // service is named by, so the handler's own builder is not asserted against itself.
    let routing_rule_arn = engine::emulator_arn(
        ARN_SERVICE,
        REGION,
        &format!("/domainnames/{domain}/routingrules/{routing_rule_id}"),
    );
    assert_eq!(
        as_str(&created, "routingRuleArn"),
        routing_rule_arn,
        "seeded rule: expected routingRuleArn {routing_rule_arn:?}, got {created}"
    );
    (routing_rule_id, routing_rule_arn)
}

/// Assert that `body` reports the rule `request` describes, under `routing_rule_id` and
/// `routing_rule_arn`.
fn assert_reports_rule(
    case: &str,
    what: &str,
    body: &Value,
    request: &Value,
    routing_rule_id: &str,
    routing_rule_arn: &str,
) {
    assert_eq!(
        as_str(body, "routingRuleId"),
        routing_rule_id,
        "case {case:?}: expected {what} to report routingRuleId {routing_rule_id:?}, got {body}"
    );
    assert_eq!(
        as_str(body, "routingRuleArn"),
        routing_rule_arn,
        "case {case:?}: expected {what} to report routingRuleArn {routing_rule_arn:?}, got {body}"
    );
    assert_eq!(
        body.get("priority"),
        request.get("priority"),
        "case {case:?}: expected {what} to report priority {:?}, got {body}",
        request.get("priority")
    );
    // Through the crate-wide null normalizer, so a member the request omitted and one a
    // reader reports as null are not asserted as two different things.
    assert_eq!(
        strip_nulls(&body["conditions"]),
        strip_nulls(&request["conditions"]),
        "case {case:?}: expected {what} to report conditions {:?}, got {body}",
        request["conditions"]
    );
    assert_eq!(
        strip_nulls(&body["actions"]),
        strip_nulls(&request["actions"]),
        "case {case:?}: expected {what} to report actions {:?}, got {body}",
        request["actions"]
    );
}

/// One row of the rewrite table: the body `PutRoutingRule` is called with, built from
/// the two seeded api ids so no row hardcodes one.
struct RewriteCase {
    label: &'static str,
    body: fn(&str, &str) -> Value,
}

/// The rows span what a rewrite may change: the number and the form of the match clauses
/// a condition names, the number of conditions, each published bound of
/// `RoutingRulePriority` and values between them, the stage and the api the action
/// invokes, and whether the action strips the base path.
fn rewrite_cases() -> Vec<RewriteCase> {
    vec![
        RewriteCase {
            label: "one base path, the lowest priority the model admits",
            body: |api_id, _other| {
                json!({
                    "priority": PRIORITY_MIN,
                    "conditions": [{ "matchBasePaths": { "anyOf": ["orders"] } }],
                    "actions": [{ "invokeApi": { "apiId": api_id, "stage": STAGE } }],
                })
            },
        },
        RewriteCase {
            label: "several base paths, stripBasePath true, the other stage",
            body: |api_id, _other| {
                json!({
                    "priority": 500,
                    "conditions": [{
                        "matchBasePaths": { "anyOf": ["orders", "orders/v1", "v2.0"] },
                    }],
                    "actions": [{
                        "invokeApi": {
                            "apiId": api_id,
                            "stage": OTHER_STAGE,
                            "stripBasePath": true,
                        },
                    }],
                })
            },
        },
        RewriteCase {
            label: "a header match, the highest priority the model admits",
            body: |api_id, _other| {
                json!({
                    "priority": PRIORITY_MAX,
                    "conditions": [{
                        "matchHeaders": {
                            "anyOf": [{ "header": "x-tenant", "valueGlob": "acme*" }],
                        },
                    }],
                    "actions": [{
                        "invokeApi": {
                            "apiId": api_id,
                            "stage": STAGE,
                            "stripBasePath": false,
                        },
                    }],
                })
            },
        },
        RewriteCase {
            label: "a base-path condition and two header conditions",
            body: |api_id, _other| {
                json!({
                    "priority": 7,
                    "conditions": [
                        { "matchBasePaths": { "anyOf": ["orders"] } },
                        {
                            "matchHeaders": {
                                "anyOf": [
                                    { "header": "x-tenant", "valueGlob": "acme*" },
                                    { "header": "x-channel", "valueGlob": "*mobile" },
                                ],
                            },
                        },
                        {
                            "matchHeaders": {
                                "anyOf": [{ "header": "x-region", "valueGlob": "eu-*" }],
                            },
                        },
                    ],
                    "actions": [{ "invokeApi": { "apiId": api_id, "stage": STAGE } }],
                })
            },
        },
        RewriteCase {
            label: "the other api, invoked on the other stage",
            body: |_api_id, other| {
                json!({
                    "priority": 42,
                    "conditions": [{
                        "matchHeaders": {
                            "anyOf": [{ "header": "x-tenant", "valueGlob": "acme*" }],
                        },
                    }],
                    "actions": [{
                        "invokeApi": { "apiId": other, "stage": OTHER_STAGE },
                    }],
                })
            },
        },
    ]
}

#[tokio::test]
async fn put_routing_rule_rewrites_the_stored_rule_and_get_routing_rule_reports_back() {
    let (emulator, api_id, other_api_id) = seeded().await;
    let (routing_rule_id, routing_rule_arn) = seed_rule(&emulator, DOMAIN, &api_id).await;

    for case in rewrite_cases() {
        let label = case.label;
        let rewrite = (case.body)(&api_id, &other_api_id);

        let (status, written) =
            put_routing_rule(&emulator, DOMAIN, &routing_rule_id, "", &rewrite).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from PutRoutingRule, got {status} {written}"
        );
        // The rewrite reports the stored rule, and the identity the rule was minted with
        // survives it.
        assert_reports_rule(
            label,
            "PutRoutingRule",
            &written,
            &rewrite,
            &routing_rule_id,
            &routing_rule_arn,
        );

        let (status, read) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetRoutingRule, got {status} {read}"
        );
        assert_reports_rule(
            label,
            "GetRoutingRule",
            &read,
            &rewrite,
            &routing_rule_id,
            &routing_rule_arn,
        );
        // A read changes nothing a later caller sees.
        let (status, again) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id, "").await;
        assert_eq!(
            (status, &again),
            (200, &read),
            "case {label:?}: expected a second GetRoutingRule to report the same rule, \
             got {status} {again}"
        );
    }
}

/// One row of the missing-resource table.
struct MissingCase {
    label: &'static str,
    /// The domain name the request addresses; `None` is [`DOMAIN`], which is seeded.
    domain: Option<&'static str>,
    /// The `RoutingRuleId` the request addresses; `None` is the id of the rule the OTHER
    /// domain name holds, which no request to [`DOMAIN`] may reach.
    routing_rule_id: Option<&'static str>,
    /// The query string the request carries, for a row about `DomainNameId`.
    query: &'static str,
    /// The sentence the refusal must carry.
    message: &'static str,
}

/// The rows vary which resource of the request resolves to nothing — the rule, the
/// domain name, or the domain name under the id the request named — and the form of the
/// identifier that does.
fn missing_cases() -> Vec<MissingCase> {
    vec![
        MissingCase {
            label: "an id nothing minted",
            domain: None,
            routing_rule_id: Some("abcdefghij"),
            query: "",
            message: ROUTING_RULE_NOT_FOUND,
        },
        MissingCase {
            label: "an id of a shape this service never mints",
            domain: None,
            routing_rule_id: Some("NOT-A-RULE-ID"),
            query: "",
            message: ROUTING_RULE_NOT_FOUND,
        },
        MissingCase {
            label: "a rule another domain name holds",
            domain: None,
            routing_rule_id: None,
            query: "",
            message: ROUTING_RULE_NOT_FOUND,
        },
        MissingCase {
            label: "a domain name the account does not hold",
            domain: Some("unseeded.example.com"),
            routing_rule_id: Some("abcdefghij"),
            query: "",
            message: INVALID_DOMAIN_NAME_IDENTIFIER,
        },
        MissingCase {
            label: "a domainNameId that is not this domain name's own",
            domain: None,
            routing_rule_id: None,
            query: "?domainNameId=abcdefghij",
            message: INVALID_DOMAIN_NAME_IDENTIFIER,
        },
    ]
}

#[tokio::test]
async fn put_routing_rule_on_a_rule_the_domain_name_does_not_hold_is_not_found() {
    let (emulator, api_id, _other_api_id) = seeded().await;
    // The rule that exists — under the OTHER domain name, so no request addressing
    // `DOMAIN` reaches it.
    let (elsewhere_id, _elsewhere_arn) = seed_rule(&emulator, OTHER_DOMAIN, &api_id).await;
    let rewrite = json!({
        "priority": 100,
        "conditions": [{ "matchBasePaths": { "anyOf": ["orders"] } }],
        "actions": [{ "invokeApi": { "apiId": api_id, "stage": STAGE } }],
    });

    for case in missing_cases() {
        let label = case.label;
        let domain = case.domain.unwrap_or(DOMAIN);
        let routing_rule_id = case.routing_rule_id.unwrap_or(&elsewhere_id);

        let (status, body) =
            put_routing_rule(&emulator, domain, routing_rule_id, case.query, &rewrite).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from PutRoutingRule, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            case.message,
            "case {label:?}: expected the message {:?}, got {body}",
            case.message
        );

        // The reader refuses the same request the same way, so nothing the refused write
        // addressed came into being.
        let (status, read) = get_routing_rule(&emulator, domain, routing_rule_id, case.query).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from GetRoutingRule, got {status} {read}"
        );
        assert_eq!(
            as_str(&read, "message"),
            case.message,
            "case {label:?}: expected GetRoutingRule to carry the message {:?}, got {read}",
            case.message
        );
    }

    // The rule the other domain name holds is still exactly where it was, and the rewrite
    // that was refused never reached it.
    let (status, still) = get_routing_rule(&emulator, OTHER_DOMAIN, &elsewhere_id, "").await;
    assert_eq!(
        (status, still.get("priority").and_then(Value::as_i64)),
        (200, Some(999)),
        "expected the rule of {OTHER_DOMAIN:?} to survive every refused request unchanged, \
         got {status} {still}"
    );
}

/// One row of the refused-body table: a body that is not a rule the service accepts.
struct RefusedBodyCase {
    label: &'static str,
    actions: Value,
    conditions: Value,
    /// The sentence the refusal must carry, where the constraint publishes one. `None`
    /// for a row whose wording belongs to the shared exclusive-choice check, whose own
    /// suite pins it.
    message: Option<&'static str>,
}

/// The rows break one rule of the accepted form each: no action at all, no condition at
/// all, a condition naming neither alternative, a condition naming both, and more
/// conditions of one kind than the documented limit.
fn refused_body_cases() -> Vec<RefusedBodyCase> {
    let base_paths = json!({ "matchBasePaths": { "anyOf": ["orders"] } });
    let headers = json!({
        "matchHeaders": { "anyOf": [{ "header": "x-tenant", "valueGlob": "acme*" }] },
    });
    vec![
        RefusedBodyCase {
            label: "no action at all",
            actions: json!([]),
            conditions: json!([base_paths]),
            message: Some(EMPTY_ROUTING_RULE_ACTIONS),
        },
        RefusedBodyCase {
            label: "no condition at all",
            actions: Value::Null,
            conditions: json!([]),
            message: Some(EMPTY_ROUTING_RULE_CONDITIONS),
        },
        RefusedBodyCase {
            label: "a condition naming neither alternative",
            actions: Value::Null,
            conditions: json!([{}]),
            message: None,
        },
        RefusedBodyCase {
            label: "a condition naming both alternatives",
            actions: Value::Null,
            conditions: json!([{
                "matchBasePaths": { "anyOf": ["orders"] },
                "matchHeaders": { "anyOf": [{ "header": "x-tenant", "valueGlob": "acme*" }] },
            }]),
            message: None,
        },
        RefusedBodyCase {
            label: "two base-path conditions, one more than the documented limit",
            actions: Value::Null,
            conditions: json!([base_paths, { "matchBasePaths": { "anyOf": ["invoices"] } }]),
            message: Some(ROUTING_RULE_CONDITION_LIMITS),
        },
        RefusedBodyCase {
            label: "three header conditions, one more than the documented limit",
            actions: Value::Null,
            conditions: json!([
                headers,
                { "matchHeaders": { "anyOf": [{ "header": "x-a", "valueGlob": "*" }] } },
                { "matchHeaders": { "anyOf": [{ "header": "x-b", "valueGlob": "*" }] } },
            ]),
            message: Some(ROUTING_RULE_CONDITION_LIMITS),
        },
    ]
}

#[tokio::test]
async fn put_routing_rule_refuses_a_body_that_is_not_a_rule_and_keeps_the_stored_rule() {
    let (emulator, api_id, _other_api_id) = seeded().await;
    let seed = seed_rule_body(&api_id);
    let (routing_rule_id, routing_rule_arn) = seed_rule(&emulator, DOMAIN, &api_id).await;
    let usable_action = json!([{ "invokeApi": { "apiId": api_id, "stage": STAGE } }]);

    for case in refused_body_cases() {
        let label = case.label;
        let request = json!({
            "priority": 100,
            "conditions": case.conditions,
            // A row about the conditions carries the usable action, so what it breaks is
            // the one rule it is about.
            "actions": if case.actions.is_null() { usable_action.clone() } else { case.actions },
        });

        let (status, body) =
            put_routing_rule(&emulator, DOMAIN, &routing_rule_id, "", &request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from PutRoutingRule, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );
        if let Some(message) = case.message {
            assert_eq!(
                as_str(&body, "message"),
                message,
                "case {label:?}: expected the message {message:?}, got {body}"
            );
        }

        // The refused rewrite wrote nothing: the rule still reads back as it was created.
        let (status, read) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetRoutingRule, got {status} {read}"
        );
        assert_reports_rule(
            label,
            "GetRoutingRule",
            &read,
            &seed,
            &routing_rule_id,
            &routing_rule_arn,
        );
    }
}

#[tokio::test]
async fn put_routing_rule_stores_an_action_whose_api_this_service_does_not_hold() {
    let (emulator, api_id, _other_api_id) = seeded().await;
    let (routing_rule_id, routing_rule_arn) = seed_rule(&emulator, DOMAIN, &api_id).await;

    // An action's `ApiId` is a reference the rule carries rather than a resource the rule
    // is written under, so an id this service's own api store does not hold is stored as
    // the request wrote it. The rows vary the form of such an id.
    for (label, foreign_api) in [
        ("an id of the shape this service mints", "abcdefghij"),
        ("an id of another shape", "NOT-AN-API"),
        ("an id carrying digits only", "1234567890"),
    ] {
        let request = json!({
            "priority": 100,
            "conditions": [{ "matchBasePaths": { "anyOf": ["orders"] } }],
            "actions": [{ "invokeApi": { "apiId": foreign_api, "stage": STAGE } }],
        });

        let (status, written) =
            put_routing_rule(&emulator, DOMAIN, &routing_rule_id, "", &request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from PutRoutingRule, got {status} {written}"
        );
        assert_reports_rule(
            label,
            "PutRoutingRule",
            &written,
            &request,
            &routing_rule_id,
            &routing_rule_arn,
        );

        let (status, read) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetRoutingRule, got {status} {read}"
        );
        assert_reports_rule(
            label,
            "GetRoutingRule",
            &read,
            &request,
            &routing_rule_id,
            &routing_rule_arn,
        );
    }
}
