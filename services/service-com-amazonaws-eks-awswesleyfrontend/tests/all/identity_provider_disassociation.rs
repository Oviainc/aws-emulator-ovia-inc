// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisassociateIdentityProviderConfig`: the asynchronous
//! removal of one identity provider configuration from its cluster, driven over the
//! wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along the `type` the request names, which half of the
//! `(cluster, configuration)` pair is missing, and the casing of the one type the API
//! defines. Each assertion names its case and prints the actual value.
//!
//! The harness is the shared one in `tests/all/wire.rs`, with the cluster setup
//! `cluster_lifecycle` already owns.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, IDENTITY_PROVIDER_CONFIG_TYPE_OIDC, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// A little longer than the span the clock owes a transition, so a read after it is
/// certainly past the due instant.
fn past(delay: Duration) -> Duration {
    delay + Duration::from_millis(250)
}

async fn associate(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/identity-provider-configs/associate",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

async fn disassociate(
    router: &MultiServiceRouter,
    cluster: &str,
    config_type: &str,
    name: &str,
) -> Answer {
    let body = serde_json::json!({
        "identityProviderConfig": { "type": config_type, "name": name },
    });
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/identity-provider-configs/disassociate",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

async fn describe_config(
    router: &MultiServiceRouter,
    cluster: &str,
    config_type: &str,
    name: &str,
) -> Answer {
    let body = serde_json::json!({
        "identityProviderConfig": { "type": config_type, "name": name },
    });
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/identity-provider-configs/describe",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

/// Stand up an `ACTIVE` cluster carrying one `ACTIVE` OIDC configuration named
/// `provider`.
async fn cluster_with_provider(router: &MultiServiceRouter, cluster: &str) {
    let created = create(router, &minimal_request(cluster)).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {cluster:?}, got {created:?}"
    );
    tokio::time::sleep(past(CLUSTER_ACTIVATION_DELAY)).await;
    let associated = associate(
        router,
        cluster,
        &serde_json::json!({
            "oidc": {
                "identityProviderConfigName": "provider",
                "issuerUrl": "https://oidc.example.com",
                "clientId": "kubernetes",
            },
        }),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "setup: expected 200 from AssociateIdentityProviderConfig, got {associated:?}"
    );
    tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;
}

// ---------------------------------------------------------------------------
// RULE: the `type` names WHICH KIND of configuration is addressed, and the only kind
// this API defines is `oidc`; a request naming any other kind addresses no resource
// of any cluster and is refused as a bad parameter, before the cluster is resolved.
// ---------------------------------------------------------------------------

/// The table spans HOW a request misses that one type (an invented kind, a kind of
/// another identity system, the empty string, a value with padding) and pairs each
/// with a cluster that exists and one that does not — the same refusal either way,
/// because the type is judged first.
///
/// Source for the accepted value: `IdentityProviderConfig$type` ("the only type
/// available is `oidc`"), read through
/// [`IDENTITY_PROVIDER_CONFIG_TYPE_OIDC`].
#[tokio::test]
async fn a_type_this_api_does_not_define_is_refused_before_the_cluster_is_resolved() {
    let router = emulator();
    cluster_with_provider(&router, "idp-teardown").await;

    for unsupported in [
        "not-a-supported-type",
        "saml",
        "OIDC-v2",
        " oidc",
        "",
    ] {
        for cluster in ["idp-teardown", "no-such-cluster"] {
            let answer = disassociate(&router, cluster, unsupported, "provider").await;
            assert_eq!(
                answer.status, 400,
                "case type {unsupported:?} on {cluster:?}: expected HTTP 400, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "InvalidParameterException",
                "case type {unsupported:?} on {cluster:?}: expected \
                 InvalidParameterException, got {code:?}"
            );
        }
    }

    // The one type the API defines takes the cluster-resolution path instead, so a
    // cluster that does not exist is a not-found — and the comparison is
    // case-insensitive, so every casing of that token takes the same path.
    for spelling in [
        IDENTITY_PROVIDER_CONFIG_TYPE_OIDC,
        "OIDC",
        "Oidc",
    ] {
        let answer = disassociate(&router, "no-such-cluster", spelling, "provider").await;
        assert_eq!(
            answer.status, 404,
            "case type {spelling:?} on an unknown cluster: expected HTTP 404, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case type {spelling:?} on an unknown cluster: expected \
             ResourceNotFoundException, got {code:?}"
        );
    }

    // A configuration name the cluster does not carry is the same 404, on the cluster
    // that does exist.
    let answer = disassociate(&router, "idp-teardown", "oidc", "no-such-provider").await;
    assert_eq!(
        answer.status, 404,
        "case an unknown configuration name: expected HTTP 404, got {answer:?}"
    );
    let code = error_code(&answer);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case an unknown configuration name: expected ResourceNotFoundException, got {code:?}"
    );

    // Nothing above removed the configuration the cluster carries.
    let described = describe_config(&router, "idp-teardown", "oidc", "provider").await;
    assert_eq!(
        described.status, 200,
        "a refused disassociation must leave the configuration in place, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the disassociation is asynchronous — the configuration reports `DELETING`
// until the clock completes the update, and is gone from the cluster afterwards.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_disassociated_configuration_is_removed_only_once_the_clock_completes_the_update() {
    let router = emulator();
    cluster_with_provider(&router, "idp-removal").await;

    let started = disassociate(&router, "idp-removal", "oidc", "provider").await;
    assert_eq!(
        started.status, 200,
        "case the disassociation: expected 200, got {started:?}"
    );
    assert_eq!(
        started.body["update"]["type"],
        serde_json::json!("DisassociateIdentityProviderConfig"),
        "case the disassociation: expected that update type, got {:?}",
        started.body["update"]["type"]
    );
    assert_eq!(
        started.body["update"]["status"],
        serde_json::json!("InProgress"),
        "case the disassociation: expected update status InProgress, got {:?}",
        started.body["update"]["status"]
    );
    let update_id = started.body["update"]["id"]
        .as_str()
        .expect("the call answers with the id of the update it started")
        .to_owned();

    // While the update is in flight the configuration is still readable, and says it
    // is on its way out.
    let during = describe_config(&router, "idp-removal", "oidc", "provider").await;
    assert_eq!(
        during.status, 200,
        "case while the update runs: the configuration is still readable, got {during:?}"
    );
    assert_eq!(
        during.body["identityProviderConfig"]["oidc"]["status"],
        serde_json::json!("DELETING"),
        "case while the update runs: expected status DELETING, got {during:?}"
    );

    // A retry of the same call, carrying the same idempotency token, is answered with
    // the update the first call started rather than refused as a miss.
    let token_body = serde_json::json!({
        "identityProviderConfig": { "type": "oidc", "name": "provider" },
        "clientRequestToken": "token-teardown",
    });
    let with_token = call(
        &router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/identity-provider-configs/disassociate",
            encode_label("idp-removal")
        ),
        &token_body.to_string(),
    )
    .await;
    assert_eq!(
        with_token.status, 409,
        "case a second disassociation while the first runs: expected HTTP 409, got {with_token:?}"
    );

    tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;

    let after = describe_config(&router, "idp-removal", "oidc", "provider").await;
    assert_eq!(
        after.status, 404,
        "case once the update completes: the configuration is gone, got {after:?}"
    );
    let code = error_code(&after);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case once the update completes: expected ResourceNotFoundException, got {code:?}"
    );

    // The update a caller tracks the teardown through reports its completion.
    let described_update = call(
        &router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/updates/{update_id}",
            encode_label("idp-removal")
        ),
        "",
    )
    .await;
    assert_eq!(
        described_update.body["update"]["status"],
        serde_json::json!("Successful"),
        "case the recorded update: expected Successful, got {described_update:?}"
    );

    // And the cluster takes another identity provider afterwards: the removal freed
    // both the name and the one-update-at-a-time slot.
    let again = associate(
        &router,
        "idp-removal",
        &serde_json::json!({
            "oidc": {
                "identityProviderConfigName": "provider",
                "issuerUrl": "https://oidc.example.com",
                "clientId": "kubernetes",
            },
        }),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "case re-associating the freed name: expected 200, got {again:?}"
    );
}
