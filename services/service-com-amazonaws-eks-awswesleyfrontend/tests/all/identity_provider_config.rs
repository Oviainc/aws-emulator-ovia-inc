// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS identity-provider slice:
//! `AssociateIdentityProviderConfig` and `DescribeIdentityProviderConfig`, driven over
//! the wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded request.
//! The rows differ along what each rule generalises over: which optional OIDC members
//! are supplied, the shape of the issuer URL, how many required claims and tags travel
//! with the configuration, which documented constraint a bad request breaks, and the
//! `type`/`name` pair a read addresses a configuration by.
//!
//! The request harness (`emulator`, `call`, `create`, `encode_label`, …) is the one
//! `cluster_lifecycle` already owns; this suite adds only the two
//! identity-provider endpoints.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CLUSTER_ACTIVATION_DELAY, MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER, UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// The canonical account every ARN this emulator mints carries
/// (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A little longer than the span the clock owes a transition, so a read after it is
/// certainly past the due instant.
fn past(delay: Duration) -> Duration {
    delay + Duration::from_millis(250)
}

/// Wait out the span the clock owes an in-flight update before it is `Successful` and
/// the configuration it associates is `ACTIVE`. A cluster takes one update at a time,
/// so this is also what frees it for the next association.
async fn settle_update() {
    tokio::time::sleep(past(UPDATE_COMPLETION_DELAY)).await;
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

/// The update a caller tracks an association through, read back through
/// `DescribeUpdate` — the same record the association wrote onto the cluster.
async fn describe_update(router: &MultiServiceRouter, cluster: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}/updates/{update_id}", encode_label(cluster)),
        "",
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

/// Create `name` and wait out the span the clock owes it, so it is `ACTIVE` — the
/// state a cluster must be in before it can take an identity provider.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let created = create(router, &minimal_request(name)).await;
    assert_eq!(
        created.status, 200,
        "cluster {name:?}: expected 200 from CreateCluster, got {created:?}"
    );
    tokio::time::sleep(past(CLUSTER_ACTIVATION_DELAY)).await;
}

/// The smallest `oidc` object the operation accepts: its three required members.
fn minimal_oidc(name: &str) -> serde_json::Value {
    serde_json::json!({
        "identityProviderConfigName": name,
        "issuerUrl": "https://oidc.example.com",
        "clientId": "kubernetes",
    })
}

/// A version-4 UUID in the canonical hyphenated form, which is what `Update$id`
/// documents ("A UUID that is used to track the update").
fn is_uuid_v4(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase())
            })
        && groups[2].starts_with('4')
        && matches!(groups[3].as_bytes().first(), Some(b'8' | b'9' | b'a' | b'b'))
}

/// The id EKS puts last in an identity provider configuration's ARN, in the same
/// 32-hex-uppercase shape this service gives a cluster's resource id.
fn is_resource_id(value: &str) -> bool {
    value.len() == 32
        && value
            .chars()
            .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_lowercase())
}

fn oidc_of(answer: &Answer) -> &serde_json::Value {
    &answer.body["identityProviderConfig"]["oidc"]
}

// ---------------------------------------------------------------------------
// RULE: associating an identity provider writes the configuration the request
// describes onto its cluster and reports it through an asynchronous `Update`; a
// later describe answers exactly the members that were written, plus the
// server-generated ARN and status.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional OIDC members are
/// supplied at all, the form of the issuer URL (bare host vs. a path component, which
/// the model allows), how many required claims and tags travel along, and the
/// prefix-disabling `-` the model documents.
#[tokio::test]
async fn an_associated_identity_provider_reads_back_exactly_as_stored() {
    let router = emulator();

    let cases: Vec<(&str, &str, serde_json::Value, serde_json::Value)> = vec![
        (
            "only the three required OIDC members",
            "idp-minimal",
            minimal_oidc("minimal-provider"),
            serde_json::Value::Null,
        ),
        (
            "every optional member, an issuer URL with a path component",
            "idp-full",
            serde_json::json!({
                "identityProviderConfigName": "full_provider-1",
                "issuerUrl": "https://login.example.org/realms/eks",
                "clientId": "eks-audience",
                "usernameClaim": "email",
                "usernamePrefix": "oidc-user:",
                "groupsClaim": "groups",
                "groupsPrefix": "oidc:",
                "requiredClaims": { "hd": "example.org", "azp": "eks-audience" },
            }),
            serde_json::json!({ "env": "dev", "team": "platform" }),
        ),
        (
            "a single required claim and a single tag, the low end of both maps",
            "idp-one-of-each",
            serde_json::json!({
                "identityProviderConfigName": "one-of-each",
                "issuerUrl": "https://accounts.example.net",
                "clientId": "audience-1",
                "requiredClaims": { "only": "claim" },
            }),
            serde_json::json!({ "only": "tag" }),
        ),
        (
            "prefixing disabled with the documented `-` value",
            "idp-dash-prefix",
            serde_json::json!({
                "identityProviderConfigName": "dash-prefix",
                "issuerUrl": "https://issuer.example.com/oidc",
                "clientId": "audience-2",
                "usernameClaim": "sub",
                "usernamePrefix": "-",
                "groupsPrefix": "-",
            }),
            serde_json::Value::Null,
        ),
    ];

    for (_, cluster, _, _) in &cases {
        let created = create(&router, &minimal_request(cluster)).await;
        assert_eq!(
            created.status, 200,
            "cluster {cluster:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    tokio::time::sleep(past(CLUSTER_ACTIVATION_DELAY)).await;

    for (case, cluster, oidc, tags) in &cases {
        let mut request = serde_json::json!({ "oidc": oidc });
        if !tags.is_null() {
            request["tags"] = tags.clone();
        }
        let associated = associate(&router, cluster, &request).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected 200 from AssociateIdentityProviderConfig, got \
             {associated:?}"
        );

        // The asynchronous update the association is reported through.
        let update = &associated.body["update"];
        let update_id = update["id"].as_str().unwrap_or_default().to_owned();
        assert!(
            is_uuid_v4(&update_id),
            "case {case:?}: expected Update.id to be a v4 UUID, got {update_id:?}"
        );
        assert_eq!(
            update["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: a fresh association is InProgress, got {:?}",
            update["status"]
        );
        assert_eq!(
            update["type"],
            serde_json::json!("AssociateIdentityProviderConfig"),
            "case {case:?}: expected the AssociateIdentityProviderConfig update type, \
             got {:?}",
            update["type"]
        );
        assert!(
            update["createdAt"].is_number(),
            "case {case:?}: expected an epoch-seconds Update.createdAt, got {:?}",
            update["createdAt"]
        );
        let params = update["params"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            params.len(),
            1,
            "case {case:?}: expected one update param, got {params:?}"
        );
        assert_eq!(
            params[0]["type"],
            serde_json::json!("IdentityProviderConfig"),
            "case {case:?}: expected the IdentityProviderConfig param type, got {:?}",
            params[0]["type"]
        );
        let param_value: serde_json::Value =
            serde_json::from_str(params[0]["value"].as_str().unwrap_or("null"))
                .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            param_value,
            serde_json::json!({
                "type": "oidc",
                "name": oidc["identityProviderConfigName"],
            }),
            "case {case:?}: the update param names the configuration it associates, \
             got {param_value:?}"
        );

        // The update lives on the cluster, so the caller tracks the association
        // through `DescribeUpdate` and reads back the very same record.
        let tracked = describe_update(&router, cluster, &update_id).await;
        assert_eq!(
            tracked.status, 200,
            "case {case:?}: expected 200 from DescribeUpdate for the association \
             update, got {tracked:?}"
        );
        assert_eq!(
            tracked.body["update"], *update,
            "case {case:?}: DescribeUpdate must answer the update the association \
             reported, got {:?}",
            tracked.body["update"]
        );

        // `TagMap` is bounded 1..50, so a configuration with no tags reports no map.
        if tags.is_null() {
            assert!(
                associated.body["tags"].is_null(),
                "case {case:?}: expected no tags member when none were sent, got {:?}",
                associated.body["tags"]
            );
        } else {
            assert_eq!(
                associated.body["tags"], *tags,
                "case {case:?}: expected the tags that were sent, got {:?}",
                associated.body["tags"]
            );
        }

        // The read-back: every member the request supplied, unchanged.
        let name = oidc["identityProviderConfigName"]
            .as_str()
            .expect("the case names its configuration");
        let described = describe_config(&router, cluster, "oidc", name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeIdentityProviderConfig, got \
             {described:?}"
        );
        let read = oidc_of(&described);
        for member in [
            "identityProviderConfigName",
            "issuerUrl",
            "clientId",
            "usernameClaim",
            "usernamePrefix",
            "groupsClaim",
            "groupsPrefix",
            "requiredClaims",
        ] {
            let expected = oidc.get(member).cloned().unwrap_or(serde_json::Value::Null);
            assert_eq!(
                read[member], expected,
                "case {case:?}: expected {member} {expected:?}, got {:?}",
                read[member]
            );
        }
        assert_eq!(
            read["clusterName"],
            serde_json::json!(cluster),
            "case {case:?}: expected clusterName {cluster:?}, got {:?}",
            read["clusterName"]
        );
        assert_eq!(
            read["tags"], *tags,
            "case {case:?}: expected tags {tags:?}, got {:?}",
            read["tags"]
        );
        assert_eq!(
            read["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a just-associated configuration is CREATING, got {:?}",
            read["status"]
        );

        // The ARN: the configuration is a sub-resource of its cluster, so the cluster,
        // the type and the name are path components and the server-minted id is last.
        let arn = read["identityProviderConfigArn"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let prefix =
            format!("arn:aws:eks:{REGION}:{ACCOUNT}:identityproviderconfig/{cluster}/oidc/{name}/");
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
        );
        assert!(
            is_resource_id(&arn[prefix.len()..]),
            "case {case:?}: expected a 32-hex id last in the ARN, got {arn:?}"
        );

        // A second read answers identically: reading changes nothing.
        let twice = describe_config(&router, cluster, "oidc", name).await;
        assert_eq!(
            oidc_of(&twice),
            read,
            "case {case:?}: a second read must answer identically"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the association is asynchronous — the configuration is `CREATING` and its
// update `InProgress` until the span the clock owes them has passed, and then both
// have moved, without any later call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_settles_when_the_clock_owes_it() {
    let router = emulator();
    let cluster = "idp-settling";
    active_cluster(&router, cluster).await;

    // The rows differ in whether the association carries tags, which is what the
    // answer's `tags` member generalises over; neither changes when it settles. They
    // run one after the other because a cluster takes one update at a time.
    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        ("no tags", "settling-plain", serde_json::Value::Null),
        (
            "with tags",
            "settling-tagged",
            serde_json::json!({ "env": "test" }),
        ),
    ];

    for (case, name, tags) in &cases {
        let mut request = serde_json::json!({ "oidc": minimal_oidc(name) });
        if !tags.is_null() {
            request["tags"] = tags.clone();
        }
        let associated = associate(&router, cluster, &request).await;
        assert_eq!(
            associated.status, 200,
            "case {case:?}: expected 200 from AssociateIdentityProviderConfig, got \
             {associated:?}"
        );
        let update_id = associated.body["update"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        // A caller acting straight after the call still sees the transient state: the
        // span is never zero.
        let immediately = describe_config(&router, cluster, "oidc", name).await;
        assert_eq!(
            oidc_of(&immediately)["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a describe straight after the associate still sees \
             CREATING, got {:?}",
            oidc_of(&immediately)["status"]
        );
        let tracked = describe_update(&router, cluster, &update_id).await;
        assert_eq!(
            tracked.body["update"]["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: the association update is still InProgress, got {:?}",
            tracked.body["update"]["status"]
        );

        settle_update().await;

        let described = describe_config(&router, cluster, "oidc", name).await;
        assert_eq!(
            oidc_of(&described)["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: once the delay has passed the configuration is ACTIVE, \
             got {:?}",
            oidc_of(&described)["status"]
        );
        let settled = describe_update(&router, cluster, &update_id).await;
        assert_eq!(
            settled.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {case:?}: the association update is Successful once the \
             configuration is ACTIVE, got {:?}",
            settled.body["update"]["status"]
        );

        // Reading again moves nothing further: the transition was owed once.
        let again = describe_config(&router, cluster, "oidc", name).await;
        assert_eq!(
            oidc_of(&again),
            oidc_of(&described),
            "case {case:?}: a second read must answer identically"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `clientRequestToken` is the idempotency key of the CALL — a resend of it is
// answered with the update that call minted — while a second association under a name
// the cluster already carries, with any other token or none, is a 409 conflict.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resent_token_replays_the_association_and_anything_else_conflicts() {
    let router = emulator();
    let cluster = "idp-idempotency";
    active_cluster(&router, cluster).await;

    let first = associate(
        &router,
        cluster,
        &serde_json::json!({
            "oidc": minimal_oidc("replayed"),
            "clientRequestToken": "token-replayed",
            "tags": { "env": "dev" },
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "expected 200 from the first AssociateIdentityProviderConfig, got {first:?}"
    );
    let first_id = first.body["update"]["id"].clone();
    // A cluster takes one update at a time, so the rows below judge the token and the
    // stored name rather than the in-flight update.
    settle_update().await;

    // The rows differ along the one thing the rule turns on: the token the second call
    // sends — the same one, another one, none, a blank one — and, for the last row,
    // the same token with another configuration name.
    let cases: Vec<(&str, serde_json::Value, u16, &str, bool)> = vec![
        (
            "the same token — a retry, answered with the first update",
            serde_json::json!({
                "oidc": minimal_oidc("replayed"),
                "clientRequestToken": "token-replayed",
                "tags": { "env": "dev" },
            }),
            200,
            "",
            true,
        ),
        (
            "a different token — a conflict",
            serde_json::json!({
                "oidc": minimal_oidc("replayed"),
                "clientRequestToken": "token-other",
            }),
            409,
            "ResourceInUseException",
            false,
        ),
        (
            "no token at all — a conflict",
            serde_json::json!({ "oidc": minimal_oidc("replayed") }),
            409,
            "ResourceInUseException",
            false,
        ),
        (
            "a blank token, which identifies nothing — a conflict",
            serde_json::json!({
                "oidc": minimal_oidc("replayed"),
                "clientRequestToken": "   ",
            }),
            409,
            "ResourceInUseException",
            false,
        ),
        (
            "the same token with another name — still that one call, so still its \
             update",
            serde_json::json!({
                "oidc": minimal_oidc("replayed-elsewhere"),
                "clientRequestToken": "token-replayed",
            }),
            200,
            "",
            true,
        ),
        (
            "a fresh token and a fresh name — a second configuration of its own",
            serde_json::json!({
                "oidc": minimal_oidc("second-config"),
                "clientRequestToken": "token-second",
            }),
            200,
            "",
            false,
        ),
    ];

    for (case, request, expected_status, expected_error, replays_first) in &cases {
        let answer = associate(&router, cluster, request).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_error.is_empty() {
            if *replays_first {
                assert_eq!(
                    answer.body["update"]["id"], first_id,
                    "case {case:?}: a retry answers the update the first call minted, \
                     got {:?}",
                    answer.body["update"]["id"]
                );
                assert_eq!(
                    answer.body["tags"],
                    serde_json::json!({ "env": "dev" }),
                    "case {case:?}: a retry answers the tags the first call did, got \
                     {:?}",
                    answer.body["tags"]
                );
            } else {
                assert_ne!(
                    answer.body["update"]["id"], first_id,
                    "case {case:?}: a new configuration gets its own update, got {:?}",
                    answer.body["update"]["id"]
                );
            }
        } else {
            assert!(
                answer
                    .error_type
                    .as_deref()
                    .is_some_and(|actual| actual.ends_with(expected_error)),
                "case {case:?}: expected {expected_error}, got {answer:?}"
            );
        }
    }

    // Only the two configurations the accepted rows asked for were stored: a replay
    // creates nothing.
    settle_update().await;
    for (name, expected_status) in [
        ("replayed", 200),
        ("second-config", 200),
        ("replayed-elsewhere", 404),
    ] {
        let described = describe_config(&router, cluster, "oidc", name).await;
        assert_eq!(
            described.status, expected_status,
            "case {name:?}: expected HTTP {expected_status} from \
             DescribeIdentityProviderConfig, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a configuration is a sub-resource of its cluster, so a request naming no
// cluster of this account in this region is answered 404 `ResourceNotFoundException`
// — whatever the rest of the request says, and whichever of the two operations it is.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_for_an_unknown_cluster_is_not_found() {
    let router = emulator();
    // One real, ACTIVE cluster exists, so "not found" is about the name, not about an
    // empty store. It is created in `REGION`, so naming it from another region is a
    // row of this table too.
    active_cluster(&router, "idp-present").await;

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "a name no cluster has",
            "no-such-cluster",
            serde_json::json!({ "oidc": minimal_oidc("whatever") }),
        ),
        (
            "a name no cluster has, with a request that is also invalid",
            "another-missing-cluster",
            serde_json::json!({
                "oidc": {
                    "identityProviderConfigName": "whatever",
                    "issuerUrl": "http://not-https.example.com",
                    "clientId": "audience",
                },
            }),
        ),
        (
            "a name that differs only in case from a real cluster",
            "IDP-PRESENT",
            serde_json::json!({ "oidc": minimal_oidc("whatever") }),
        ),
    ];

    for (case, cluster, request) in &cases {
        let answer = associate(&router, cluster, request).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from \
             AssociateIdentityProviderConfig, got {answer:?}"
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
            serde_json::json!(format!("No cluster found for name: {cluster}.")),
            "case {case:?}: expected the message to name the missing cluster, got {:?}",
            answer.body["message"]
        );

        let described = describe_config(&router, cluster, "oidc", "whatever").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: expected HTTP 404 from \
             DescribeIdentityProviderConfig, got {described:?}"
        );
        assert!(
            described
                .error_type
                .as_deref()
                .is_some_and(|actual| actual.ends_with("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException, got {described:?}"
        );
    }

    // The same cluster name, signed for another region, names no cluster either.
    let elsewhere = call(
        &router,
        "eu-west-1",
        "POST",
        "/clusters/idp-present/identity-provider-configs/associate",
        &serde_json::json!({ "oidc": minimal_oidc("whatever") }).to_string(),
    )
    .await;
    assert_eq!(
        elsewhere.status, 404,
        "case \"another region\": a cluster name is unique per region, so this names \
         no cluster, got {elsewhere:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: every constraint the model documents for the OIDC object and none of which
// is expressible as a modelled constraint — a non-empty name, issuer and client id,
// an `https://` issuer URL with no query parameters, and prefixes that do not contain
// `system:` — is refused 400 `InvalidParameterException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_oidc_object_that_breaks_a_documented_constraint_is_refused() {
    let router = emulator();
    let cluster = "idp-validation";
    active_cluster(&router, cluster).await;

    // Each row breaks exactly one documented constraint; the baseline row — LAST,
    // because it is the only one that starts an update and a cluster takes one at a
    // time — proves the same request without that one defect is accepted.
    let cases: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "an http:// issuer URL",
            serde_json::json!({
                "identityProviderConfigName": "plain-http",
                "issuerUrl": "http://oidc.example.com",
                "clientId": "audience",
            }),
            400,
        ),
        (
            "an issuer URL with no scheme at all",
            serde_json::json!({
                "identityProviderConfigName": "no-scheme",
                "issuerUrl": "oidc.example.com",
                "clientId": "audience",
            }),
            400,
        ),
        (
            "an issuer URL carrying query parameters",
            serde_json::json!({
                "identityProviderConfigName": "with-query",
                "issuerUrl": "https://oidc.example.com/realm?tenant=eks",
                "clientId": "audience",
            }),
            400,
        ),
        (
            "a username prefix containing system:",
            serde_json::json!({
                "identityProviderConfigName": "username-prefix",
                "issuerUrl": "https://oidc.example.com",
                "clientId": "audience",
                "usernamePrefix": "system:",
            }),
            400,
        ),
        (
            "a groups prefix containing system:",
            serde_json::json!({
                "identityProviderConfigName": "groups-prefix",
                "issuerUrl": "https://oidc.example.com",
                "clientId": "audience",
                "groupsPrefix": "my-system:groups",
            }),
            400,
        ),
        (
            "an empty configuration name",
            serde_json::json!({
                "identityProviderConfigName": "",
                "issuerUrl": "https://oidc.example.com",
                "clientId": "audience",
            }),
            400,
        ),
        (
            "an empty issuer URL",
            serde_json::json!({
                "identityProviderConfigName": "empty-issuer",
                "issuerUrl": "",
                "clientId": "audience",
            }),
            400,
        ),
        (
            "an empty client id",
            serde_json::json!({
                "identityProviderConfigName": "empty-client-id",
                "issuerUrl": "https://oidc.example.com",
                "clientId": "   ",
            }),
            400,
        ),
        ("the baseline, accepted", minimal_oidc("baseline"), 200),
    ];

    for (case, oidc, expected_status) in &cases {
        let answer = associate(&router, cluster, &serde_json::json!({ "oidc": oidc })).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if *expected_status == 400 {
            assert!(
                answer
                    .error_type
                    .as_deref()
                    .is_some_and(|actual| actual.ends_with("InvalidParameterException")),
                "case {case:?}: expected InvalidParameterException, got {answer:?}"
            );
            // A refused request persists nothing.
            let name = oidc["identityProviderConfigName"].as_str().unwrap_or("");
            let described = describe_config(&router, cluster, "oidc", name).await;
            assert_eq!(
                described.status, 404,
                "case {case:?}: a refused association stores nothing, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster takes an identity provider only once its control plane is running,
// so a cluster that is not `ACTIVE` is refused 400 `InvalidRequestException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_that_is_not_active_yet_cannot_take_an_identity_provider() {
    let router = emulator();

    // The rows differ in the cluster's shape, not in its status: the rule is about the
    // status alone, whichever cluster carries it.
    let clusters = ["still-creating", "Still_Creating9"];
    for cluster in clusters {
        let created = create(&router, &minimal_request(cluster)).await;
        assert_eq!(
            created.status, 200,
            "case {cluster:?}: expected 200 from CreateCluster, got {created:?}"
        );
        let answer = associate(
            &router,
            cluster,
            &serde_json::json!({ "oidc": minimal_oidc("too-early") }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {cluster:?}: expected HTTP 400 while the cluster is CREATING, got \
             {answer:?}"
        );
        assert!(
            answer
                .error_type
                .as_deref()
                .is_some_and(|actual| actual.ends_with("InvalidRequestException")),
            "case {cluster:?}: expected InvalidRequestException, got {answer:?}"
        );
    }

    // Once the clock has made them `ACTIVE`, the very same call is accepted.
    tokio::time::sleep(past(CLUSTER_ACTIVATION_DELAY)).await;
    for cluster in clusters {
        let answer = associate(
            &router,
            cluster,
            &serde_json::json!({ "oidc": minimal_oidc("not-too-early") }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {cluster:?}: expected 200 once the cluster is ACTIVE, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster runs one asynchronous update at a time, so a second association
// started while one is in flight is refused 409 `ResourceInUseException` — and the
// very same call is accepted once the clock has completed that update.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_takes_one_association_at_a_time() {
    let router = emulator();
    let cluster = "idp-serialised";
    active_cluster(&router, cluster).await;

    let first = associate(
        &router,
        cluster,
        &serde_json::json!({ "oidc": minimal_oidc("in-flight") }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "expected 200 from the first AssociateIdentityProviderConfig, got {first:?}"
    );

    // The rows differ in the name the second call asks for — another name entirely and
    // one that only differs in case — so what refuses them is the in-flight update,
    // not the stored name.
    let contenders = ["another-provider", "In-Flight"];
    for name in contenders {
        let answer = associate(
            &router,
            cluster,
            &serde_json::json!({ "oidc": minimal_oidc(name) }),
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {name:?}: expected HTTP 409 while an update is in flight, got \
             {answer:?}"
        );
        assert!(
            answer
                .error_type
                .as_deref()
                .is_some_and(|actual| actual.ends_with("ResourceInUseException")),
            "case {name:?}: expected ResourceInUseException, got {answer:?}"
        );
    }

    settle_update().await;

    for name in contenders {
        let answer = associate(
            &router,
            cluster,
            &serde_json::json!({ "oidc": minimal_oidc(name) }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected 200 once the in-flight update completed, got \
             {answer:?}"
        );
        settle_update().await;
    }
}

// ---------------------------------------------------------------------------
// RULE: a read addresses a configuration by the `type`/`name` pair of
// `IdentityProviderConfig`; `oidc` is the only type this API defines, and a name the
// cluster does not carry is not found.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_read_addresses_a_configuration_by_type_and_name() {
    let router = emulator();
    let cluster = "idp-addressing";
    active_cluster(&router, cluster).await;

    let associated = associate(
        &router,
        cluster,
        &serde_json::json!({ "oidc": minimal_oidc("addressed") }),
    )
    .await;
    assert_eq!(
        associated.status, 200,
        "expected 200 from AssociateIdentityProviderConfig, got {associated:?}"
    );

    // The rows span the type (the documented token, its case variants, and types the
    // API does not define) and the name (stored vs. not stored).
    let cases: Vec<(&str, &str, &str, u16, &str)> = vec![
        (
            "the documented type and the stored name",
            "oidc",
            "addressed",
            200,
            "",
        ),
        (
            "the documented type in upper case",
            "OIDC",
            "addressed",
            200,
            "",
        ),
        (
            "a type this API does not define",
            "saml",
            "addressed",
            400,
            "InvalidParameterException",
        ),
        (
            "an empty type",
            "",
            "addressed",
            400,
            "InvalidParameterException",
        ),
        (
            "the documented type and a name the cluster does not carry",
            "oidc",
            "not-associated",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a name that differs only in case from the stored one",
            "oidc",
            "ADDRESSED",
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, config_type, name, expected_status, expected_error) in &cases {
        let answer = describe_config(&router, cluster, config_type, name).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_error.is_empty() {
            assert_eq!(
                oidc_of(&answer)["identityProviderConfigName"],
                serde_json::json!(name),
                "case {case:?}: expected the configuration named {name:?}, got {:?}",
                oidc_of(&answer)
            );
        } else {
            assert!(
                answer
                    .error_type
                    .as_deref()
                    .is_some_and(|actual| actual.ends_with(expected_error)),
                "case {case:?}: expected {expected_error}, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: one cluster holds at most
// `MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER` configurations — the ceiling that
// bounds the store — and the association past it is refused rather than retained.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_ceiling_bounds_how_many_configurations_one_cluster_holds() {
    let router = emulator();
    let cluster = "idp-ceiling";
    active_cluster(&router, cluster).await;

    for index in 0..MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER {
        let name = format!("config-{index}");
        let answer = associate(
            &router,
            cluster,
            &serde_json::json!({ "oidc": minimal_oidc(&name) }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected 200 up to the ceiling, got {answer:?}"
        );
        // A cluster takes one update at a time, so each association is let settle
        // before the next one is asked for.
        settle_update().await;
    }

    let over = associate(
        &router,
        cluster,
        &serde_json::json!({ "oidc": minimal_oidc("one-too-many") }),
    )
    .await;
    assert_eq!(
        over.status, 400,
        "case \"one past the ceiling\": expected HTTP 400, got {over:?}"
    );
    assert!(
        over.error_type
            .as_deref()
            .is_some_and(|actual| actual.ends_with("InvalidRequestException")),
        "case \"one past the ceiling\": expected InvalidRequestException, got {over:?}"
    );
    let described = describe_config(&router, cluster, "oidc", "one-too-many").await;
    assert_eq!(
        described.status, 404,
        "case \"one past the ceiling\": the refused association stored nothing, got \
         {described:?}"
    );
}
