// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal of a TLS security policy outside the modelled value
//! set, on every resource whose `securityPolicy` a caller can write.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the PATCH
//! wrappers and the domain-name create helper are this crate's existing harness, reused
//! from `rest_api_lifecycle`, `patch_updates` and `domain_name_collection`.
//!
//! The RULE under test: a request that sets `securityPolicy` to a value the modelled
//! `SecurityPolicy` enum does not declare is refused with HTTP 400 /
//! `BadRequestException` carrying the one sentence API Gateway words for that condition —
//! fixed text with no variable part, so it quotes back neither the rejected value, nor
//! the patch path or op that carried it, nor the resource, nor the policies it would have
//! accepted — and the stored record keeps the policy it already had. Members whose
//! out-of-set value the service has no sentence of its own for keep the generic refusal.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: both
//! patchable resources (`UpdateDomainName` and `UpdateRestApi`), both ops that set a
//! single-valued member, out-of-set values derived from EVERY declared policy (case
//! changed, padded, suffixed) plus values in none of those forms, a list whose refused op
//! is not the first, and the whole declared value set on the accepting side.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::{
    API_KEY_SOURCE_TYPE_VALUES, ROUTING_MODE_VALUES, SECURITY_POLICY_VALUES,
};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::INVALID_SECURITY_POLICY;

use crate::domain_name_collection::created_domain;
use crate::patch_updates::{patch, replace};
use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};

/// The status and code every refusal below carries.
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// The policy the domain name under test is created with: a declared value that is NOT
/// the service's default, so a read-back proving the record unchanged cannot pass by
/// reporting a default.
const DOMAIN_POLICY: &str = "TLS_1_2";

/// The policy the REST API under test is created with, chosen for the same reason and
/// different from [`DOMAIN_POLICY`] so neither resource's expectation can satisfy the
/// other's.
const API_POLICY: &str = "SecurityPolicy_TLS13_1_2_2021_06";

/// One `patchOperations` entry, as a client spells it on the wire.
fn op(kind: &str, path: &str, value: &str) -> Value {
    json!({ "op": kind, "path": path, "value": value })
}

/// Assert `answer` is the modelled failure `(status, code)` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// A resource under test: how its update is addressed, how it is read back, and the
/// policy it was created with.
struct Resource {
    /// The resource's name, for the case label.
    kind: &'static str,
    /// The URI both `UpdateDomainName`/`UpdateRestApi` and the matching reader take.
    uri: String,
    /// The `securityPolicy` the create stored.
    policy: &'static str,
}

/// The two resources whose `securityPolicy` a caller writes, each created and ready to
/// patch.
async fn resources(gateway: &Gateway) -> Vec<Resource> {
    let domain_name = "security-policy.example.com";
    created_domain(
        gateway,
        domain_name,
        json!({
            // A regional record's endpoint is in place the moment it is created, so no
            // create span is running while a row patches it.
            "endpointConfiguration": { "types": ["REGIONAL"] },
            "regionalCertificateArn":
                "arn:aws:acm:us-west-2:000000000000:certificate/security-policy",
            "securityPolicy": DOMAIN_POLICY,
        }),
    )
    .await;

    let created = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": "security-policy", "securityPolicy": API_POLICY })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case CreateRestApi: expected 201, got {} with {:?}",
        created.status, created.body
    );
    let api = string_member(&created.body, "id", "CreateRestApi");

    vec![
        Resource {
            kind: "domain name",
            uri: format!("/domainnames/{domain_name}"),
            policy: DOMAIN_POLICY,
        },
        Resource {
            kind: "rest api",
            uri: format!("/restapis/{api}"),
            policy: API_POLICY,
        },
    ]
}

/// The `securityPolicy` the resource at `uri` reports NOW, read through its own
/// getter.
async fn stored_policy(gateway: &Gateway, uri: &str, case: &str) -> String {
    let read = gateway.call("GET", uri, None).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from the read, got {} with {:?}",
        read.status, read.body
    );
    string_member(&read.body, "securityPolicy", case)
}

/// Values the modelled `SecurityPolicy` enum does not declare, derived from the declared
/// set itself — `smithy_query(query="shape", shape="SecurityPolicy")`, as the crate's
/// generated catalog holds it — so every declared policy contributes rows and none of
/// them is a value this suite invented.
fn out_of_set_values() -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for declared in SECURITY_POLICY_VALUES {
        values.push(declared.to_lowercase());
        values.push(declared.to_uppercase());
        values.push(format!(" {declared} "));
        values.push(format!("{declared}_EXTRA"));
    }
    // Values in none of those forms: a TLS version the service does not offer, a policy
    // family name truncated before its date, another enum's value, a name no AWS policy
    // takes, and the empty string.
    for free in [
        "TLS_1_1",
        "SecurityPolicy_TLS13",
        "STRICT",
        "NOT_A_POLICY",
        "",
    ] {
        values.push(free.to_string());
    }
    values.retain(|value| !SECURITY_POLICY_VALUES.contains(&value.as_str()));
    values
}

#[tokio::test]
async fn an_out_of_set_security_policy_is_refused_with_the_services_own_sentence() {
    let gateway = Gateway::new();
    let under_test = resources(&gateway).await;
    let values = out_of_set_values();

    for resource in &under_test {
        for value in &values {
            // Both ops that set a single-valued member, so the refusal does not depend on
            // which one carried the value.
            for kind in ["replace", "add"] {
                let case =
                    format!("{} {kind} /securityPolicy {value:?}", resource.kind);
                let answer = patch(
                    &gateway,
                    &resource.uri,
                    vec![op(kind, "/securityPolicy", value)],
                )
                .await;
                assert_refusal(&answer, BAD_REQUEST, INVALID_SECURITY_POLICY, &case);
                // A refused patch changes nothing: the record still reports the policy it
                // was created with.
                let stored = stored_policy(&gateway, &resource.uri, &case).await;
                assert_eq!(
                    stored, resource.policy,
                    "case {case}: the refused patch must leave the stored policy at {:?}, got {stored:?}",
                    resource.policy
                );
            }
        }
    }
}

#[tokio::test]
async fn an_out_of_set_policy_refuses_the_whole_list_whatever_its_position() {
    let gateway = Gateway::new();
    let under_test = resources(&gateway).await;

    for resource in &under_test {
        // The refused op is the LAST of the list, so the ops before it must not be kept
        // either: the description a legal op would have set is read back afterwards.
        let case = format!("{}: a list whose last op is refused", resource.kind);
        let answer = patch(
            &gateway,
            &resource.uri,
            vec![
                replace("/securityPolicy", SECURITY_POLICY_VALUES[0]),
                replace("/securityPolicy", "TLS_1_1"),
            ],
        )
        .await;
        assert_refusal(&answer, BAD_REQUEST, INVALID_SECURITY_POLICY, &case);
        let stored = stored_policy(&gateway, &resource.uri, &case).await;
        assert_eq!(
            stored, resource.policy,
            "case {case}: no op of a refused list may be kept, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_member_with_no_sentence_of_its_own_keeps_the_generic_refusal() {
    let gateway = Gateway::new();
    let under_test = resources(&gateway).await;

    // One enum-typed member per resource that the service has no condition-specific
    // sentence for: its refusal still names the path, the values it would have taken and
    // the rejected value.
    let members: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "domain name",
            "/routingMode",
            "ROUTING_RULE",
            ROUTING_MODE_VALUES,
        ),
        (
            "rest api",
            "/apiKeySource",
            "COOKIE",
            API_KEY_SOURCE_TYPE_VALUES,
        ),
    ];

    for (kind, path, value, values) in &members {
        let resource = under_test
            .iter()
            .find(|resource| resource.kind == *kind)
            .expect("both resources are under test");
        let case = format!("{kind} replace {path} {value:?}");
        let answer = patch(&gateway, &resource.uri, vec![replace(path, value)]).await;
        let expected = format!(
            "{path} takes one of {}, got {value:?}",
            values.join(", ")
        );
        assert_refusal(&answer, BAD_REQUEST, &expected, &case);
        assert_ne!(
            answer.body["message"].as_str(),
            Some(INVALID_SECURITY_POLICY),
            "case {case}: another member must not borrow the security policy sentence"
        );
    }
}

#[tokio::test]
async fn every_declared_security_policy_is_still_accepted_and_read_back() {
    let gateway = Gateway::new();
    let under_test = resources(&gateway).await;

    for resource in &under_test {
        for declared in SECURITY_POLICY_VALUES {
            let case = format!("{} replace /securityPolicy {declared:?}", resource.kind);
            let answer = patch(
                &gateway,
                &resource.uri,
                vec![replace("/securityPolicy", declared)],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the modelled 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["securityPolicy"].as_str(),
                Some(*declared),
                "case {case}: the update must report the policy it set, got {:?}",
                answer.body["securityPolicy"]
            );
            let stored = stored_policy(&gateway, &resource.uri, &case).await;
            assert_eq!(
                stored, *declared,
                "case {case}: the accepted policy must be stored, got {stored:?}"
            );
        }
    }
}
