// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the identity `DeleteDomainName` addresses.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rule these cases assert: **`DeleteDomainName` ("Deletes a domain name") removes
//! the one custom domain name whose stored key equals the `DomainName` its URI carries,
//! answering the 202 Accepted the service answers such a teardown with (its model binds
//! `code: 204`), and refuses a name the account does not hold with
//! the modelled `NotFoundException` (404) while removing nothing** — the resource is keyed
//! by the whole name, so no other stored name is reached by a request naming a different
//! one. Because every child resource is addressed UNDER the domain
//! (`/v2/domainnames/{DomainName}/routingrules/{RoutingRuleId}`), a removed domain takes
//! its children with it rather than leaving a child nothing can address.
//!
//! The rows vary what the rule generalises over: how many labels the name has, which
//! characters it is built from, its length (including the longest name a create accepts,
//! and one longer than that), how the requested name RELATES to a held one (a different
//! top-level label, the held name's parent, a child under the held name, a shared
//! prefix), and whether the name was never minted or was minted and already deleted.
//! The recorded case that filed the issue — a delete of a name nothing minted — is one
//! row of the refusal table.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    DOMAIN_NAME_MAX_LENGTH, INVALID_DOMAIN_NAME_IDENTIFIER,
};

use crate::api_lifecycle::{as_str, emulator};
use crate::domain_name_deletion::{assert_not_found, seed_domain};
use crate::domain_name_lifecycle::{call, get_domain_name};

/// The names the account holds in every table below. They span the forms a custom domain
/// name takes: three labels, a deep subdomain, digits and hyphens under another top-level
/// label, and a name exactly [`DOMAIN_NAME_MAX_LENGTH`] characters long — the published
/// bound of `GetDomainName`'s `DomainName`
/// (`smithy_query(query="shape", shape="StringWithLengthBetween1And512")`).
fn held_names() -> Vec<String> {
    vec![
        "orders.example.com".to_string(),
        "a.b.c.deep.example.com".to_string(),
        "api-7.gateway-2.example.net".to_string(),
        longest_name(),
    ]
}

/// A name of exactly [`DOMAIN_NAME_MAX_LENGTH`] characters, built so the bound is read
/// from the service rather than written out here.
fn longest_name() -> String {
    let suffix = ".example.com";
    format!(
        "{}{suffix}",
        "x".repeat(DOMAIN_NAME_MAX_LENGTH - suffix.len())
    )
}

/// A fresh emulator holding every name in [`held_names`].
async fn seeded(case: &str) -> MultiServiceRouter {
    let emulator = emulator();
    for domain in held_names() {
        seed_domain(&emulator, case, &domain).await;
    }
    emulator
}

/// `DeleteDomainName` for one name.
async fn delete_domain_name(emulator: &MultiServiceRouter, domain: &str) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/domainnames/{domain}"),
        None,
    )
    .await
}

/// Every name `GetDomainNames` reports, so a refusal can be shown to have removed
/// nothing.
async fn listed_names(emulator: &MultiServiceRouter, case: &str) -> BTreeSet<String> {
    let (status, body) = call(emulator, "GET", "/v2/domainnames", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetDomainNames, got {status} {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "domainName"))
        .collect()
}

/// Assert a refusal is the modelled 404 `NotFoundException` of a custom domain name the
/// account does not hold. The 404, the `NotFoundException` code and the message are
/// asserted by the suite this borrows its assertion from, so both suites judge a refusal
/// by one standard.
fn assert_domain_not_found(case: &str, status: u16, body: &Value) {
    assert_not_found(case, status, body, INVALID_DOMAIN_NAME_IDENTIFIER);
}

#[tokio::test]
async fn deleting_a_domain_name_removes_exactly_the_name_its_uri_carries() {
    for target in held_names() {
        let case = format!("deleting {target:?} of {} held names", held_names().len());
        let case = case.as_str();
        let emulator = seeded(case).await;

        let (status, body) = delete_domain_name(&emulator, &target).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202 from DeleteDomainName, got {status} {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: the modelled output is empty, got {body}"
        );

        // Gone from its own reader and from the collection; every other held name is
        // exactly as it was.
        let (status, read) = get_domain_name(&emulator, &target).await;
        assert_domain_not_found(case, status, &read);
        let listed = listed_names(&emulator, case).await;
        let expected: BTreeSet<String> = held_names()
            .into_iter()
            .filter(|name| *name != target)
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected only the deleted name gone, got {listed:?}"
        );
        for name in &expected {
            let (status, held) = get_domain_name(&emulator, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {name:?} untouched, got {status} {held}"
            );
        }

        // A second delete of the same name is the refusal of a name the account no longer
        // holds.
        let (status, body) = delete_domain_name(&emulator, &target).await;
        assert_domain_not_found(case, status, &body);
    }
}

/// One row of the refusal table: a name the account does not hold, and how it relates to
/// the names it does.
struct AbsentCase {
    label: &'static str,
    name: String,
}

/// The rows span how an unheld name relates to a held one, and the lengths either side of
/// the published bound.
fn absent_cases() -> Vec<AbsentCase> {
    vec![
        AbsentCase {
            label: "a name nothing minted",
            name: "never.example.com".to_string(),
        },
        AbsentCase {
            label: "a held name under another top-level label",
            name: "orders.example.org".to_string(),
        },
        AbsentCase {
            label: "the parent of a held name",
            name: "example.com".to_string(),
        },
        AbsentCase {
            label: "a child label under a held name",
            name: "eu.orders.example.com".to_string(),
        },
        AbsentCase {
            label: "a held name's prefix, one label short",
            name: "b.c.deep.example.com".to_string(),
        },
        AbsentCase {
            label: "a single-character name",
            name: "x".to_string(),
        },
        AbsentCase {
            label: "a name longer than any the account can hold",
            name: format!("y{}", longest_name()),
        },
    ]
}

#[tokio::test]
async fn deleting_a_name_the_account_does_not_hold_is_refused_and_removes_nothing() {
    let emulator = seeded("the unheld-name table").await;
    let held: BTreeSet<String> = held_names().into_iter().collect();

    for AbsentCase { label, name } in absent_cases() {
        assert!(
            !held.contains(&name),
            "case {label:?}: the row must name a name the account does not hold, got {name:?}"
        );
        let (status, body) = delete_domain_name(&emulator, &name).await;
        assert_domain_not_found(label, status, &body);

        let listed = listed_names(&emulator, label).await;
        assert_eq!(
            listed, held,
            "case {label:?}: a refused delete must remove nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_domain_name_takes_its_routing_rules_with_it() {
    let case = "a domain holding two routing rules, beside a second domain";
    let emulator = emulator();
    let domain = "ruled.example.com";
    let other = "unruled.example.com";
    seed_domain(&emulator, case, domain).await;
    seed_domain(&emulator, case, other).await;

    // A rule's action invokes a stage, so the api it points at is seeded first.
    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "ruled-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let (status, stage) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {stage}"
    );

    let mut rule_ids = Vec::new();
    for (priority, base_path) in [(1, "orders"), (2, "carts")] {
        let (status, rule) = call(
            &emulator,
            "POST",
            &format!("/v2/domainnames/{domain}/routingrules"),
            Some(json!({
                "priority": priority,
                "actions": [{ "invokeApi": { "apiId": api_id, "stage": "$default" } }],
                "conditions": [{ "matchBasePaths": { "anyOf": [base_path] } }]
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRoutingRule, got {status} {rule}"
        );
        rule_ids.push(as_str(&rule, "routingRuleId"));
    }

    let (status, body) = delete_domain_name(&emulator, domain).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from DeleteDomainName, got {status} {body}"
    );

    // Neither the domain nor any rule addressed under it is reported afterwards: the
    // refusal is the domain's, because the path the rule was addressed at no longer
    // names a domain the account holds.
    let (status, read) = get_domain_name(&emulator, domain).await;
    assert_domain_not_found(case, status, &read);
    let (status, rules) = call(
        &emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules"),
        None,
    )
    .await;
    assert_domain_not_found(case, status, &rules);
    for rule_id in &rule_ids {
        let (status, rule) = call(
            &emulator,
            "GET",
            &format!("/v2/domainnames/{domain}/routingrules/{rule_id}"),
            None,
        )
        .await;
        assert_domain_not_found(case, status, &rule);
    }

    // The name is free again, and the recreated domain holds none of the deleted
    // domain's rules.
    seed_domain(&emulator, case, domain).await;
    let (status, rules) = call(
        &emulator,
        "GET",
        &format!("/v2/domainnames/{domain}/routingrules"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListRoutingRules, got {status} {rules}"
    );
    assert_eq!(
        rules["routingRules"].as_array(),
        Some(&Vec::new()),
        "case {case:?}: a recreated domain holds none of the deleted domain's rules, got {rules}"
    );
    for rule_id in &rule_ids {
        let (status, rule) = call(
            &emulator,
            "GET",
            &format!("/v2/domainnames/{domain}/routingrules/{rule_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted rule must not come back with its name, got {status} {rule}"
        );
    }

    // The second domain is untouched: a deletion removes one resource, not a kind of
    // resource.
    let (status, held) = get_domain_name(&emulator, other).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other domain untouched, got {status} {held}"
    );
}
