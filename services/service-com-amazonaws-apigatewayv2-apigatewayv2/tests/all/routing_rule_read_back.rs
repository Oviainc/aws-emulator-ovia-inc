// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `CreateRoutingRule` PERSISTS, read back through the
//! single-resource reader `GetRoutingRule`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The suites beside this one read a created rule back through the COLLECTION reader
//! (`routing_rule_lifecycle`) and through the reader that follows a rewrite
//! (`routing_rule_update`). What is pinned here is the create itself against the
//! single-resource reader, each rule over a TABLE of rule forms rather than one request:
//!
//! * What `CreateRoutingRule` reports — the caller's actions, conditions and priority,
//!   the minted `RoutingRuleId` and the rule's ARN — is exactly what `GetRoutingRule`
//!   reports for that id afterwards, and reading it twice reports it twice: a read
//!   changes nothing a later caller sees.
//! * Each create mints an id of its own and leaves the rules already stored under the
//!   domain name readable, so no two rules of one domain name share an identifier or
//!   overwrite each other.
//! * A rule is a child of the ONE domain name its path named: a rule created under one
//!   domain name is reported with an ARN naming that domain name, a read addressed at
//!   another domain name the account holds answers with THAT domain name's own rules or
//!   with nothing, and a request naming a domain name the account does not hold is the
//!   modelled `NotFoundException` (404) that neither reads nor stores a rule.
//!
//! The rows vary what the rules generalise over: the kind and the count of the
//! conditions, how many values one `AnyOf` holds, the number of actions, whether the
//! action strips the base path, which stage is invoked, and where in the published
//! priority range the rule sits (both bounds included).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    ARN_SERVICE, INVALID_DOMAIN_NAME_IDENTIFIER, ROUTING_RULE_NOT_FOUND,
};

use crate::api_lifecycle::{
    as_str, assert_minted_id, call_in_region, emulator, error_code, strip_nulls,
};

/// The region every request in this suite is signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so nothing region-derived (the rule's ARN) can be hardcoded.
const REGION: &str = "ap-southeast-2";

/// The custom domain name the rules are created under.
const DOMAIN: &str = "readback.example.com";

/// A second seeded domain name, so a row can prove a rule belongs to one domain name and
/// is addressable under no other.
const NEIGHBOUR_DOMAIN: &str = "neighbour-readback.example.com";

/// The stages every seeded api carries, so an action's target can vary without inventing
/// a stage.
const STAGE: &str = "$default";
const OTHER_STAGE: &str = "staging";

/// The published bounds of `RoutingRulePriority` (`range { min: 1, max: 1000000 }`), from
/// `smithy_query(query="shape", shape="RoutingRulePriority")`.
const PRIORITY_MIN: i64 = 1;
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

async fn get_routing_rule(
    emulator: &MultiServiceRouter,
    domain: &str,
    routing_rule_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules/{routing_rule_id}"),
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

/// A fresh emulator holding [`DOMAIN`], [`NEIGHBOUR_DOMAIN`] and one api carrying both
/// stages. Answers `(emulator, api_id)`.
async fn seeded() -> (MultiServiceRouter, String) {
    let emulator = emulator();
    seed_domain_name(&emulator, DOMAIN).await;
    seed_domain_name(&emulator, NEIGHBOUR_DOMAIN).await;
    let api_id = seed_api(&emulator, "readback-api").await;
    (emulator, api_id)
}

/// One row of the table: a whole rule body, with `{api}` standing in for the `ApiId` of
/// the seeded api.
struct RuleCase {
    label: &'static str,
    request: Value,
}

/// The rows span what a rule is made of: the condition kind and count, how many values
/// one `AnyOf` holds, the number of actions, the `StripBasePath` setting, the stage
/// invoked, and the priority (each published bound and values between them).
fn rule_cases() -> Vec<RuleCase> {
    vec![
        RuleCase {
            label: "one base path at the lowest published priority",
            request: json!({
                "priority": PRIORITY_MIN,
                "actions": [{ "invokeApi": { "apiId": "{api}", "stage": STAGE } }],
                "conditions": [{ "matchBasePaths": { "anyOf": ["invoices"] } }]
            }),
        },
        RuleCase {
            label: "several base paths, stripped, at the highest published priority",
            request: json!({
                "priority": PRIORITY_MAX,
                "actions": [{
                    "invokeApi": { "apiId": "{api}", "stage": OTHER_STAGE, "stripBasePath": true }
                }],
                "conditions": [{
                    "matchBasePaths": { "anyOf": ["invoices", "invoices/v2", "ledger"] }
                }]
            }),
        },
        RuleCase {
            label: "one header match, kept unstripped",
            request: json!({
                "priority": 17,
                "actions": [{
                    "invokeApi": { "apiId": "{api}", "stage": STAGE, "stripBasePath": false }
                }],
                "conditions": [{
                    "matchHeaders": { "anyOf": [{ "header": "x-region", "valueGlob": "eu-*" }] }
                }]
            }),
        },
        RuleCase {
            label: "a base path and two header matches",
            request: json!({
                "priority": 512,
                "actions": [{ "invokeApi": { "apiId": "{api}", "stage": OTHER_STAGE } }],
                "conditions": [
                    { "matchBasePaths": { "anyOf": ["reports"] } },
                    {
                        "matchHeaders": {
                            "anyOf": [
                                { "header": "x-tier", "valueGlob": "gold" },
                                { "header": "x-tier", "valueGlob": "silver" }
                            ]
                        }
                    },
                    { "matchHeaders": { "anyOf": [{ "header": "x-trace", "valueGlob": "*" }] } }
                ]
            }),
        },
        RuleCase {
            label: "two actions invoking two stages",
            request: json!({
                "priority": 4_096,
                "actions": [
                    { "invokeApi": { "apiId": "{api}", "stage": STAGE } },
                    { "invokeApi": { "apiId": "{api}", "stage": OTHER_STAGE } }
                ],
                "conditions": [{ "matchBasePaths": { "anyOf": ["fanout"] } }]
            }),
        },
    ]
}

/// The request of `case` with `{api}` resolved to `api_id`.
fn resolved(case: &RuleCase, api_id: &str) -> Value {
    let rendered = case.request.to_string().replace("{api}", api_id);
    serde_json::from_str(&rendered).expect("the row is valid JSON")
}

/// The ARN a rule of `routing_rule_id` under `domain` is named by, built here from the
/// account-and-region form every resource of this service is named by, so the handler's
/// own builder is not asserted against itself.
fn expected_arn(domain: &str, routing_rule_id: &str) -> String {
    engine::emulator_arn(
        ARN_SERVICE,
        REGION,
        &format!("/domainnames/{domain}/routingrules/{routing_rule_id}"),
    )
}

/// Assert `body` is the modelled refusal of a request naming a resource the account does
/// not hold, carrying `message`.
fn assert_not_found(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 for a resource the account does not hold, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body}"
    );
    assert_eq!(
        as_str(body, "message"),
        message,
        "case {case:?}: expected the message {message:?}, got {body}"
    );
}

#[tokio::test]
async fn create_routing_rule_persists_the_rule_get_routing_rule_reports_it_back() {
    let (emulator, api_id) = seeded().await;
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created_rules: Vec<(&'static str, Value)> = Vec::new();

    for case in rule_cases() {
        let label = case.label;
        let request = resolved(&case, &api_id);
        let (status, created) = create_routing_rule(&emulator, DOMAIN, &request).await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreateRoutingRule, got {status} {created}"
        );

        let routing_rule_id = as_str(&created, "routingRuleId");
        assert_minted_id(label, "routingRuleId", &routing_rule_id);
        assert!(
            minted.insert(routing_rule_id.clone()),
            "case {label:?}: every create mints an id of its own, got {routing_rule_id:?} twice"
        );
        assert_eq!(
            as_str(&created, "routingRuleArn"),
            expected_arn(DOMAIN, &routing_rule_id),
            "case {label:?}: expected the rule's own ARN, got {created}"
        );

        // The single-resource reader reports the rule the create reported: through the
        // crate-wide null normalizer, so a member the request omitted and one a reader
        // reports as null are not asserted as two different things.
        let (status, read) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetRoutingRule, got {status} {read}"
        );
        assert_eq!(
            strip_nulls(&read),
            strip_nulls(&created),
            "case {label:?}: expected GetRoutingRule to report {created}, got {read}"
        );
        // Every member the caller sent is part of that, so a reader that dropped or
        // rewrote one fails here rather than agreeing with a create that did the same.
        assert_eq!(
            read.get("priority"),
            request.get("priority"),
            "case {label:?}: expected the Priority that was sent, got {read}"
        );
        assert_eq!(
            strip_nulls(&read["actions"]),
            strip_nulls(&request["actions"]),
            "case {label:?}: expected the Actions that were sent, got {read}"
        );
        assert_eq!(
            strip_nulls(&read["conditions"]),
            strip_nulls(&request["conditions"]),
            "case {label:?}: expected the Conditions that were sent, got {read}"
        );

        // Reading it again reports it again: the reader changed nothing a later caller
        // sees.
        let (status, again) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id).await;
        assert_eq!(
            (status, strip_nulls(&again)),
            (200, strip_nulls(&read)),
            "case {label:?}: expected a repeated read to report the same rule, \
             got {status} {again}"
        );
        created_rules.push((label, created));
    }

    // Every rule created under the domain name is still readable under its own id, so no
    // later create overwrote an earlier one.
    for (label, created) in &created_rules {
        let routing_rule_id = as_str(created, "routingRuleId");
        let (status, read) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id).await;
        assert_eq!(
            (status, strip_nulls(&read)),
            (200, strip_nulls(created)),
            "case {label:?}: expected {routing_rule_id:?} to survive every later create, \
             got {status} {read}"
        );
    }
}

#[tokio::test]
async fn a_created_rule_belongs_to_the_domain_name_its_path_named() {
    let (emulator, api_id) = seeded().await;

    // The one rule the neighbouring domain name holds, so a read addressed at it can
    // never answer with a rule of [`DOMAIN`].
    let neighbour_request = json!({
        "priority": 3,
        "actions": [{ "invokeApi": { "apiId": api_id, "stage": STAGE } }],
        "conditions": [{ "matchBasePaths": { "anyOf": ["neighbour"] } }]
    });
    let (status, neighbour_rule) =
        create_routing_rule(&emulator, NEIGHBOUR_DOMAIN, &neighbour_request).await;
    assert_eq!(
        status, 201,
        "seeding the neighbour's rule: expected 201 from CreateRoutingRule, \
         got {status} {neighbour_rule}"
    );
    assert_eq!(
        as_str(&neighbour_rule, "routingRuleArn"),
        expected_arn(
            NEIGHBOUR_DOMAIN,
            &as_str(&neighbour_rule, "routingRuleId")
        ),
        "expected an ARN naming {NEIGHBOUR_DOMAIN:?}, got {neighbour_rule}"
    );

    for case in rule_cases() {
        let label = case.label;
        let request = resolved(&case, &api_id);
        let (status, created) = create_routing_rule(&emulator, DOMAIN, &request).await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected 201 from CreateRoutingRule under {DOMAIN:?}, \
             got {status} {created}"
        );
        let routing_rule_id = as_str(&created, "routingRuleId");
        assert_eq!(
            as_str(&created, "routingRuleArn"),
            expected_arn(DOMAIN, &routing_rule_id),
            "case {label:?}: expected an ARN naming {DOMAIN:?}, got {created}"
        );

        // A read addressed at another domain name the account holds never reaches this
        // rule: it reports THAT domain name's own rule under the id, or nothing at all.
        let (status, elsewhere) =
            get_routing_rule(&emulator, NEIGHBOUR_DOMAIN, &routing_rule_id).await;
        if status == 200 {
            assert_eq!(
                strip_nulls(&elsewhere),
                strip_nulls(&neighbour_rule),
                "case {label:?}: a read under {NEIGHBOUR_DOMAIN:?} must report that domain \
                 name's own rule, got {elsewhere}"
            );
        } else {
            assert_not_found(
                &format!("{label}: read under {NEIGHBOUR_DOMAIN}"),
                status,
                &elsewhere,
                ROUTING_RULE_NOT_FOUND,
            );
        }

        // A domain name the account does not hold holds no rule to read, and takes none:
        // the missing resource is reported as the domain name it is, and the rule that
        // does exist is untouched.
        let absent = "unseeded-readback.example.com";
        let (status, body) = get_routing_rule(&emulator, absent, &routing_rule_id).await;
        assert_not_found(
            &format!("{label}: read under an absent domain name"),
            status,
            &body,
            INVALID_DOMAIN_NAME_IDENTIFIER,
        );
        let (status, body) = create_routing_rule(&emulator, absent, &request).await;
        assert_not_found(
            &format!("{label}: create under an absent domain name"),
            status,
            &body,
            INVALID_DOMAIN_NAME_IDENTIFIER,
        );
        let (status, read) = get_routing_rule(&emulator, DOMAIN, &routing_rule_id).await;
        assert_eq!(
            (status, strip_nulls(&read)),
            (200, strip_nulls(&created)),
            "case {label:?}: expected the stored rule to survive the refusals unchanged, \
             got {status} {read}"
        );
    }
}
