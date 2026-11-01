// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal this service picks for a request that names something it cannot
//! resolve, and WHAT that refusal is allowed to name back.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what it
//! generalises over rather than over one recorded request:
//!
//! * `CreateCapability` decides WHICH capability it was asked to create before it
//!   looks for anything the request names: a `type` outside the modelled
//!   `CapabilityType` value set names no capability type this API defines, so the
//!   request is `InvalidParameterException` (400) with fixed text and no resource
//!   identity member — whatever cluster the path names and whatever else the body
//!   carries. A type the model DOES declare takes the cluster-resolution path
//!   instead, which is what shows this is a choice of refusal and not a blanket 400.
//! * an operation addressed to ONE EKS Anywhere subscription reports an id the Region
//!   carries no subscription for in the terms of that resource, naming nothing from
//!   the request: `ResourceNotFoundException` (404) with the subscription's own
//!   sentence and no `subscriptionId`.
//! * an operation addressed to a resource INSIDE a cluster does not name the cluster
//!   it could not resolve — the cluster is not the resource it addresses — where an
//!   operation whose SUBJECT is the cluster does. The two are compared on the SAME
//!   cluster name, so the difference is the operation and nothing else.
//!
//! The wire harness ([`call`], [`emulator`], [`encode_label`], [`error_code`]) is the
//! shared one in `tests/all/wire.rs`, the cluster setup is `cluster_lifecycle`'s, and
//! the identity members a not-found payload may carry are `not_found_payload`'s list.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, CLUSTER_ACTIVATION_DELAY, INVALID_CAPABILITY_TYPE_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE, SUBSCRIPTION_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{minimal_request, REGION};
use crate::not_found_payload::IDENTITY_MEMBERS;
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The capability types the model declares, as the generated value set reports them:
/// `smithy_query query="shape" shape="CapabilityType"` is `ACK | ARGOCD | KRO`, read
/// here through `CAPABILITY_TYPE_VALUES` so this table cannot drift from the model.
const DECLARED_CAPABILITY_TYPES: &[&str] =
    service_com_amazonaws_eks_awswesleyfrontend::ops::types::CAPABILITY_TYPE_VALUES;

/// A role of the account this emulator serves, so the pass-role authorization (403) is
/// never what answers a row.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/eksRefusalProbeRole",
        engine::emulator_account_id()
    )
}

/// Create `name` and move it past the span the clock owes it, so it is `ACTIVE` and a
/// create against it is reachable.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &minimal_request(name).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {name:?} must succeed, got {created:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

async fn create_capability(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn list_capabilities(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        "",
    )
    .await
}

/// The smallest `CreateCapability` body, with the capability type left to the caller.
fn capability_request(name: &str, capability_type: &str) -> serde_json::Value {
    serde_json::json!({
        "capabilityName": name,
        "type": capability_type,
        "roleArn": role_arn(),
        "deletePropagationPolicy": "RETAIN",
    })
}

/// Every identity member of the refusal payload is absent.
fn assert_names_no_resource(case: &str, answer: &Answer) {
    for member in IDENTITY_MEMBERS {
        assert_eq!(
            answer.body[*member],
            serde_json::Value::Null,
            "case {case:?}: the refusal must not carry {member}, got {:?}",
            answer.body[*member]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `type` outside the modelled `CapabilityType` names no capability type this
// API defines, so `CreateCapability` refuses it 400 `InvalidParameterException` with
// the fixed sentence and no identity member — before it resolves the cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_undefined_capability_type_is_refused_before_the_cluster_is_resolved() {
    let router = emulator();
    // A real, ACTIVE cluster, so the rows that name it are refused for the TYPE rather
    // than for an empty store.
    active_cluster(&router, "refusal-real").await;

    // The rows differ along HOW the value misses the enum: unknown, a declared value
    // in the wrong case, one with padding, the empty string, a value longer than any
    // identifier, and one carrying the declared values as a list.
    let long_value = "Z".repeat(4_096);
    let undefined_types = [
        "EKS",
        "ack",
        "ArgoCD",
        " KRO",
        "",
        "ACK,KRO",
        long_value.as_str(),
    ];

    // The clusters differ along WHY (or whether) the cluster resolves, and along the
    // form of the name (`clusterName` is `length 1..100`, so both bounds are rows).
    let longest = "r".repeat(100);
    let clusters: Vec<(&str, &str, &str)> = vec![
        ("the real cluster", REGION, "refusal-real"),
        ("a cluster that was never created", REGION, "refusal-ghost"),
        ("a cluster name differing only in case", REGION, "REFUSAL-REAL"),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            longest.as_str(),
        ),
        ("a one-character cluster name", REGION, "r"),
        (
            "the real cluster name, signed for another region",
            "eu-west-1",
            "refusal-real",
        ),
    ];

    // The bodies differ along what ELSE travels with the undefined type: nothing, a
    // configuration, an idempotency token, and tags.
    let extras: [(&str, serde_json::Value); 4] = [
        ("no optional member", serde_json::json!({})),
        (
            "a configuration",
            serde_json::json!({ "configuration": { "ack": { "enableCrossNamespace": true } } }),
        ),
        (
            "an idempotency token",
            serde_json::json!({ "clientRequestToken": "refusal-token" }),
        ),
        (
            "a tag map",
            serde_json::json!({ "tags": { "owner": "refusal" } }),
        ),
    ];

    for undefined in undefined_types {
        for (why, region, cluster) in &clusters {
            for (extra_case, extra) in &extras {
                let mut body = capability_request("refusal-cap", undefined);
                for (key, value) in extra.as_object().expect("an object of extra members") {
                    body[key.as_str()] = value.clone();
                }
                let case = format!("type {undefined:?} on {why} with {extra_case}");
                let answer = create_capability(&router, region, cluster, &body).await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {answer:?}"
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException, got {code:?}"
                );
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(INVALID_CAPABILITY_TYPE_MESSAGE),
                    "case {case:?}: expected the fixed capability-type sentence, got {:?}",
                    answer.body["message"]
                );
                assert_names_no_resource(&case, &answer);
            }
        }
    }

    // Nothing above was created: the refusal is a refusal, not a silent write.
    let listed = list_capabilities(&router, "refusal-real").await;
    assert_eq!(
        listed.status, 200,
        "ListCapabilities on the real cluster must answer, got {listed:?}"
    );
    assert_eq!(
        listed.body["capabilities"],
        serde_json::json!([]),
        "a refused create must store nothing, got {:?}",
        listed.body["capabilities"]
    );
}

/// CONTROL, the other side of the same rule: a type the model DOES declare is not
/// refused here at all — it takes the cluster-resolution path, so an unknown cluster
/// is the 404 that names the cluster, and a real one creates a capability that reads
/// back.
#[tokio::test]
async fn a_declared_capability_type_takes_the_cluster_resolution_path() {
    let router = emulator();
    active_cluster(&router, "refusal-control").await;

    for declared in DECLARED_CAPABILITY_TYPES {
        // On a cluster that does not exist: the parent is what is reported, named.
        let body = capability_request("control-cap", declared);
        let answer = create_capability(&router, REGION, "refusal-nowhere", &body).await;
        assert_eq!(
            answer.status, 404,
            "case type {declared:?} on an unknown cluster: expected HTTP 404, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case type {declared:?} on an unknown cluster: expected \
             ResourceNotFoundException, got {code:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(cluster_not_found_message("refusal-nowhere").as_str()),
            "case type {declared:?}: expected the message that names the cluster, got {:?}",
            answer.body["message"]
        );

        // On the real cluster: created, and read back through `DescribeCapability`.
        let name = format!("control-{}", declared.to_lowercase());
        let body = capability_request(&name, declared);
        let created = create_capability(&router, REGION, "refusal-control", &body).await;
        assert_eq!(
            created.status, 200,
            "case type {declared:?} on the real cluster: expected 200, got {created:?}"
        );
        let read = call(
            &router,
            REGION,
            "GET",
            &format!("/clusters/refusal-control/capabilities/{name}"),
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case type {declared:?}: the stored capability must read back, got {read:?}"
        );
        assert_eq!(
            read.body["capability"]["type"],
            serde_json::json!(declared),
            "case type {declared:?}: the capability read back must carry the type it was \
             created with, got {:?}",
            read.body["capability"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an id the Region carries no EKS Anywhere subscription for is the 404 worded
// for THAT resource, and the payload names nothing the request sent.
// ---------------------------------------------------------------------------

async fn create_subscription(router: &MultiServiceRouter, name: &str, token: &str) -> Answer {
    let body = serde_json::json!({
        "name": name,
        "term": { "duration": 12, "unit": "MONTHS" },
        "licenseQuantity": 2,
        "clientRequestToken": token,
    });
    call(
        router,
        REGION,
        "POST",
        "/eks-anywhere-subscriptions",
        &body.to_string(),
    )
    .await
}

async fn describe_subscription(router: &MultiServiceRouter, region: &str, id: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/eks-anywhere-subscriptions/{}", encode_label(id)),
        "",
    )
    .await
}

#[tokio::test]
async fn an_unknown_subscription_id_is_refused_in_the_subscriptions_own_terms() {
    let router = emulator();
    // One real subscription, so a 404 below is about the id a row carries rather than
    // about an empty store — and so the control at the end has one to read back.
    let created = create_subscription(&router, "refusal-subscription", "refusal-sub-token").await;
    assert_eq!(
        created.status, 200,
        "setup: creating a subscription must succeed, got {created:?}"
    );
    let real_id = created.body["subscription"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !real_id.is_empty(),
        "setup: the created subscription must report an id, got {created:?}"
    );

    // The rows differ along the FORM of the id (a well-formed UUID, a value that is
    // not one, one character, a value longer than any id, a real id with a suffix)
    // and along WHY the Region carries no subscription for it — the last row is the
    // real id, read in a Region that does not hold it.
    let long_id = "f".repeat(512);
    let suffixed = format!("{real_id}-x");
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a well-formed UUID nothing carries",
            REGION,
            "11111111-2222-4333-8444-555555555555",
        ),
        ("a value that is not a UUID at all", REGION, "not-an-id"),
        ("a one-character id", REGION, "x"),
        ("an id longer than any this service mints", REGION, long_id.as_str()),
        ("the real id with a suffix", REGION, suffixed.as_str()),
        (
            "the real id, read in another Region",
            "eu-central-1",
            real_id.as_str(),
        ),
    ];

    for (case, region, id) in rows {
        let answer = describe_subscription(&router, region, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(SUBSCRIPTION_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the subscription's own sentence, got {:?}",
            answer.body["message"]
        );
        assert_names_no_resource(case, &answer);
    }

    // CONTROL: the rule is about ABSENCE — the subscription that IS there reads back
    // in the Region that holds it.
    let read = describe_subscription(&router, REGION, &real_id).await;
    assert_eq!(
        read.status, 200,
        "the stored subscription must read back, got {read:?}"
    );
    assert_eq!(
        read.body["subscription"]["id"],
        serde_json::json!(real_id),
        "the subscription read back must be the one that was created, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an operation addressed to a resource INSIDE a cluster does not name the
// cluster it could not resolve; an operation whose SUBJECT is the cluster does.
// ---------------------------------------------------------------------------

async fn disassociate(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    config_name: &str,
) -> Answer {
    let body = serde_json::json!({
        "identityProviderConfig": { "type": "oidc", "name": config_name },
    });
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/identity-provider-configs/disassociate",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

#[tokio::test]
async fn a_pair_addressing_operation_does_not_name_the_cluster_it_cannot_resolve() {
    let router = emulator();
    // A real, ACTIVE cluster, so the rows that name it are not-found about the
    // CONFIGURATION, and the rows that do not are not-found about the cluster.
    active_cluster(&router, "refusal-idp").await;

    // The rows differ along WHY the cluster cannot be resolved, along the form of the
    // name (both documented bounds of `clusterName`) and along the configuration name
    // the body carries.
    let longest = "i".repeat(100);
    let clusters: Vec<(&str, &str, &str)> = vec![
        ("a cluster that was never created", REGION, "refusal-idp-ghost"),
        ("a cluster name differing only in case", REGION, "REFUSAL-IDP"),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            longest.as_str(),
        ),
        ("a one-character cluster name", REGION, "i"),
        (
            "the real cluster name, signed for another region",
            "eu-west-1",
            "refusal-idp",
        ),
    ];

    for (why, region, cluster) in &clusters {
        for config_name in ["provider", "another-provider", "p"] {
            let case = format!("{why} / configuration {config_name:?}");
            let answer = disassociate(&router, region, cluster, config_name).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?}"
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(RESOURCE_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the generic not-found sentence, got {:?}",
                answer.body["message"]
            );
            assert_names_no_resource(&case, &answer);
        }
    }

    // THE OTHER SIDE of the rule, on the SAME names: the operations whose subject IS
    // the cluster report it, named — so what changed is which resource the operation
    // addresses, not the service's not-found payload as a whole.
    for (why, region, cluster) in &clusters {
        let described = call(
            &router,
            region,
            "GET",
            &format!("/clusters/{}", encode_label(cluster)),
            "",
        )
        .await;
        assert_eq!(
            described.status, 404,
            "case {why:?} / DescribeCluster: expected HTTP 404, got {described:?}"
        );
        assert_eq!(
            described.body["clusterName"].as_str(),
            Some(*cluster),
            "case {why:?} / DescribeCluster: the cluster's own read must name it, got {:?}",
            described.body["clusterName"]
        );
        assert_eq!(
            described.body["message"].as_str(),
            Some(cluster_not_found_message(cluster).as_str()),
            "case {why:?} / DescribeCluster: expected the message that names the cluster, \
             got {:?}",
            described.body["message"]
        );
    }
}
