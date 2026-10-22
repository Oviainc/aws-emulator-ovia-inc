// SPDX-License-Identifier: CC0-1.0

//! The not-found answer of `DescribeIdentityProviderConfig`, over a TABLE of ways the
//! resource a request names can be absent.
//!
//! A describe addresses ONE resource — the `(cluster, configuration)` pair — so every
//! row below is answered the same way: 404 `ResourceNotFoundException` with the
//! service's single generic not-found wording, which names neither half. The rows
//! differ along WHICH half is missing and HOW (a cluster that was never created, a
//! configuration name no cluster carries, a name differing only in case, a
//! configuration that exists only in another region, a cluster that has been deleted),
//! and one row holds the configuration that IS there, so the rule is about absence
//! rather than about the shape of the request.
//!
//! The request harness is `wire`'s; `cluster_lifecycle` owns the cluster drivers. The
//! two identity-provider endpoint drivers live here, as `pub(crate)`, so no suite needs
//! a second copy of them.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

pub(crate) async fn associate_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    config_name: &str,
) -> Answer {
    let body = serde_json::json!({
        "oidc": {
            "identityProviderConfigName": config_name,
            "issuerUrl": "https://oidc.example.com",
            "clientId": "kubernetes",
        },
    });
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/identity-provider-configs/associate",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

pub(crate) async fn describe_config_in(
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
            "/clusters/{}/identity-provider-configs/describe",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

/// Create `name` in `region` and wait out the span the clock owes it, so it is
/// `ACTIVE` — the state a cluster must be in before it can take an identity provider.
pub(crate) async fn active_cluster_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let created = call(
        router,
        region,
        "POST",
        "/clusters",
        &minimal_request(name).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {name:?} in {region} must succeed, got {created:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Assert the whole not-found answer of a describe: status, error code and the one
/// message this operation reports — the service's generic sentence
/// ([`RESOURCE_NOT_FOUND_MESSAGE`], the same one the access-entry reads use), which
/// must not echo what the request named.
pub(crate) fn assert_not_found(case: &str, answer: &Answer, named: &[&str]) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert!(
        answer
            .error_type
            .as_deref()
            .is_some_and(|actual| actual.ends_with("ResourceNotFoundException")),
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        answer.body["message"],
        serde_json::json!(RESOURCE_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the generic not-found message, got {:?}",
        answer.body["message"]
    );
    for value in named {
        assert!(
            !answer.body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(value),
            "case {case:?}: the message must not echo the request's {value:?}, got {:?}",
            answer.body["message"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `DescribeIdentityProviderConfig` whose `(cluster, configuration)` pair is
// not in the store for the region the request is signed for is answered 404
// `ResourceNotFoundException`, with the service's generic not-found message —
// whichever half is missing, and without echoing either name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_absent_configuration_is_the_same_not_found_answer() {
    let router = emulator();

    // One cluster carries a configuration, one carries none, and one of the same name
    // as the first lives in another region — so "not found" is never about an empty
    // store.
    active_cluster_in(&router, REGION, "idp-holder").await;
    active_cluster_in(&router, REGION, "idp-empty").await;
    let associated = associate_in(&router, REGION, "idp-holder", "present-config").await;
    assert_eq!(
        associated.status, 200,
        "setup: the association must succeed, got {associated:?}"
    );

    // (case, region, cluster, configuration name)
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a cluster that was never created",
            REGION,
            "no-such-cluster",
            "present-config",
        ),
        (
            "neither half exists",
            REGION,
            "no-such-cluster",
            "no-such-config",
        ),
        (
            "a known cluster that carries no configuration",
            REGION,
            "idp-empty",
            "present-config",
        ),
        (
            "a known cluster, a name it does not carry",
            REGION,
            "idp-holder",
            "absent-config",
        ),
        (
            "a name differing from the stored one only in case",
            REGION,
            "idp-holder",
            "PRESENT-CONFIG",
        ),
        (
            "a cluster name that differs only in case",
            REGION,
            "IDP-HOLDER",
            "present-config",
        ),
        (
            "the right names, signed for another region",
            "eu-west-1",
            "idp-holder",
            "present-config",
        ),
    ];

    for (case, region, cluster, config_name) in cases {
        let answer = describe_config_in(&router, region, cluster, config_name).await;
        assert_not_found(case, &answer, &[cluster, config_name]);
    }

    // The row that IS there: the rule is about absence, not about the request's shape.
    let found = describe_config_in(&router, REGION, "idp-holder", "present-config").await;
    assert_eq!(
        found.status, 200,
        "the stored configuration must still be found, got {found:?}"
    );
    assert_eq!(
        found.body["identityProviderConfig"]["oidc"]["identityProviderConfigName"],
        serde_json::json!("present-config"),
        "the found configuration must be the one that was associated, got {found:?}"
    );
}

/// A configuration lives on its cluster, so deleting the cluster takes it with it —
/// and the describe that followed the resource is then the same not-found answer.
#[tokio::test]
async fn a_configuration_is_not_found_once_its_cluster_is_deleted() {
    let router = emulator();
    for case in ["doomed", "doomed-too"] {
        let created = create(&router, &minimal_request(case)).await;
        assert_eq!(created.status, 200, "setup create {case:?}: {created:?}");
        tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
        let associated = associate_in(&router, REGION, case, "attached").await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: setup associate, got {associated:?}"
        );
        let found = describe_config_in(&router, REGION, case, "attached").await;
        assert_eq!(
            found.status, 200,
            "case {case:?}: the configuration must be readable before the delete, got {found:?}"
        );

        let deleted = call(
            &router,
            REGION,
            "DELETE",
            &format!("/clusters/{}", encode_label(case)),
            "",
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting the cluster must succeed, got {deleted:?}"
        );

        let answer = describe_config_in(&router, REGION, case, "attached").await;
        assert_not_found(case, &answer, &[case, "attached"]);
    }
}
