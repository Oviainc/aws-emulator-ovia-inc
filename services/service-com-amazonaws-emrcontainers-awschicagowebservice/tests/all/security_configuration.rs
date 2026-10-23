// SPDX-License-Identifier: CC0-1.0

//! EMR on EKS security configurations, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's other suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A create mints the resource, persists it, and the matching describe answers out of
//!    that record: the minted id and the ARN built from it, `createdAt` at the instant of
//!    the create, every member the request carried, and nothing the request omitted.
//! 2. An `@idempotencyToken` replayed names the resource the first call minted; a
//!    different token mints a resource of its own.
//! 3. A describe refuses an id the store does not hold — including the id of a DIFFERENT
//!    resource type — with the error its model declares, and never answers a canned
//!    record.
//! 4. A read does not change what the next reader sees.
//! 5. The store is bounded: at the emulator's retention cap the create is refused with
//!    the modelled `ValidationException` rather than growing the store, and the refusal
//!    leaves every stored record readable.
//! 6. A delete removes the configuration from state: it answers the id it deleted, a
//!    later describe of that id misses and is refused, and an id the store does not hold
//!    is refused rather than reported as deleted.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::CERTIFICATE_PROVIDER_TYPE_VALUES;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{
    assert_resource_id, field, instant_millis, text, Answer, Emr, REGION,
};

/// `POST /securityconfigurations` — `CreateSecurityConfiguration`.
async fn create_configuration(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/securityconfigurations", Some(body)).await
}

/// `GET /securityconfigurations/{id}` — `DescribeSecurityConfiguration`.
async fn describe_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// `DELETE /securityconfigurations/{id}` — `DeleteSecurityConfiguration`.
async fn delete_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// The `SecurityConfigurationArn` a configuration with this id must carry:
/// `arn:aws:emr-containers:{region}:{account}:/securityconfigurations/{id}`, in the region
/// the caller signed for.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:emr-containers:{REGION}:{}:/securityconfigurations/{id}",
        engine::emulator_account_id()
    )
}

/// One `CreateSecurityConfiguration` request.
struct ConfigCase {
    label: String,
    body: Value,
}

/// RULES 1, 3 (the ARN) and 4: a created security configuration is read back out of
/// state, with the members the request carried and without the ones it omitted.
///
/// The rows differ along everything `CreateSecurityConfiguration` generalises over: which
/// half of `securityConfigurationData` is populated and how deeply, whether a
/// `containerProvider` is given, how many tags there are, and the form and length of the
/// name and the client token.
#[tokio::test]
async fn a_created_security_configuration_is_described_from_state() {
    let emr = Emr::new();
    // `certificateProviderType`'s value catalog is the model's `CertificateProviderType`
    // enum, re-exported as `CERTIFICATE_PROVIDER_TYPE_VALUES` by `ops::types`; take the
    // value from there rather than writing a literal the model may not accept.
    let certificate_provider_type = CERTIFICATE_PROVIDER_TYPE_VALUES
        .first()
        .copied()
        .expect("the model declares at least one CertificateProviderType");

    let cases = vec![
        ConfigCase {
            label: "only the required members, with an empty configuration-data block".to_string(),
            body: json!({
                "name": "minimal",
                "clientToken": "sc-tok-minimal",
                "securityConfigurationData": {},
            }),
        },
        ConfigCase {
            label: "lake formation, a secure namespace, a container provider and tags"
                .to_string(),
            body: json!({
                "name": "lake.formation_1",
                "clientToken": "sc-tok-lake",
                "containerProvider": {
                    "id": "eks-lake",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "emr-lake", "nodeLabel": "spark-nodes"}},
                },
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {
                            "authorizedSessionTagValue": "Emr Session Tag",
                            "secureNamespaceInfo": {
                                "clusterId": "eks-secure-1",
                                "namespace": "emr-secure",
                            },
                            "queryEngineRoleArn":
                                "arn:aws:iam::000000000000:role/EMRQueryEngineRole",
                        },
                    },
                },
                "tags": {"team": "data", "env": "test"},
            }),
        },
        ConfigCase {
            label: "in-transit encryption with a TLS certificate".to_string(),
            body: json!({
                "name": "tls#config",
                "clientToken": "sc/tok.tls-1",
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "encryptionConfiguration": {
                            "inTransitEncryptionConfiguration": {
                                "tlsCertificateConfiguration": {
                                    "certificateProviderType": certificate_provider_type,
                                    "publicCertificateSecretArn":
                                        "arn:aws:secretsmanager:us-west-2:000000000000:secret:public-cert",
                                    "privateCertificateSecretArn":
                                        "arn:aws:secretsmanager:us-west-2:000000000000:secret:private-cert",
                                },
                            },
                        },
                    },
                },
                "tags": {"only": "one"},
            }),
        },
        ConfigCase {
            label: "identity-center authentication and no tags".to_string(),
            body: json!({
                "name": "identity-center",
                "clientToken": "sc-tok-idc",
                "securityConfigurationData": {
                    "authenticationConfiguration": {
                        "identityCenterConfiguration": {
                            "enableIdentityCenter": true,
                            "identityCenterApplicationAssignmentRequired": false,
                            "identityCenterInstanceARN":
                                "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                        },
                    },
                },
            }),
        },
        ConfigCase {
            label: "both halves of the configuration data, at the name and token length bounds"
                .to_string(),
            body: json!({
                // `ResourceNameString` is len 1..64 and `ClientToken` len 1..64: the
                // upper bound of each.
                "name": "n".repeat(64),
                "clientToken": "t".repeat(64),
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {"authorizedSessionTagValue": "Both"},
                    },
                    "authenticationConfiguration": {
                        "iamConfiguration": {
                            "systemRole": "arn:aws:iam::000000000000:role/EMRSystemRole",
                        },
                    },
                },
                "tags": {"a": "1", "b": "2", "c": "3"},
            }),
        },
    ];

    let before = state::now_millis();
    let mut ids = BTreeSet::new();
    for case in &cases {
        let label = case.label.as_str();
        let created = create_configuration(&emr, case.body.clone()).await;
        let created = created.ok(label).clone();

        let id = text(&created, "/id", label);
        assert_resource_id(label, "the minted security configuration id", &id);
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: id {id:?} was minted twice"
        );

        let arn = text(&created, "/arn", label);
        assert_eq!(
            arn,
            expected_arn(&id),
            "case {label:?}: CreateSecurityConfiguration answered the wrong arn"
        );
        // `SecurityConfigurationArn` is len 60..1024 (the model).
        assert!(
            (60..=1024).contains(&arn.len()),
            "case {label:?}: arn {arn:?} is {} characters, outside SecurityConfigurationArn's 60..1024",
            arn.len()
        );
        assert_eq!(
            created.get("name"),
            case.body.get("name"),
            "case {label:?}: CreateSecurityConfiguration answered the wrong name"
        );

        let described = describe_configuration(&emr, &id).await;
        let configuration = field(described.ok(label), "/securityConfiguration", label).clone();
        assert_eq!(
            text(&configuration, "/id", label),
            id,
            "case {label:?}: the described configuration is not the created one"
        );
        assert_eq!(
            text(&configuration, "/arn", label),
            arn,
            "case {label:?}: the described arn is not the created one"
        );

        // Every member the request carried, read back out of the record; every member it
        // omitted still absent.
        for member in ["name", "securityConfigurationData", "tags"] {
            let expected = match (member, case.body.get(member)) {
                // `tags` is a map the service always reports, empty when none were given.
                ("tags", None) => Some(json!({})),
                (_, other) => other.cloned(),
            };
            assert_eq!(
                configuration.get(member).cloned(),
                expected,
                "case {label:?}: {member} did not round-trip through state, got {configuration}"
            );
        }
        // `createdBy` names the identity that created the configuration: the caller
        // principal this emulator authenticates every request as. Its own rule and its
        // own table live in `crate::created_metadata_wire_form`.
        assert_eq!(
            configuration.get("createdBy").and_then(Value::as_str),
            Some(state::caller_identity_arn().as_str()),
            "case {label:?}: createdBy must name the caller principal: {configuration}"
        );
        // `containerProvider` is recorded behaviour rather than a modelled member, and
        // the create-time value is echoed on read; `crate::created_metadata_wire_form`
        // asserts that over the shapes a provider takes.
        assert_eq!(
            configuration.get("containerProvider").cloned(),
            case.body.get("containerProvider").cloned(),
            "case {label:?}: the create-time containerProvider must be echoed: {configuration}"
        );

        let created_at = instant_millis(&configuration, "/createdAt", label);
        let after = state::now_millis();
        assert!(
            created_at >= before - 1_000 && created_at <= after + 1_000,
            "case {label:?}: createdAt {created_at} is not within the window [{before}, {after}]"
        );

        // RULE 4: a read must not change what the next reader sees.
        let again = describe_configuration(&emr, &id).await;
        assert_eq!(
            field(again.ok(label), "/securityConfiguration", label),
            &configuration,
            "case {label:?}: describing a configuration twice answered two different records"
        );
    }

    assert_eq!(
        ids.len(),
        cases.len(),
        "every row must have minted its own security configuration, got {ids:?}"
    );
}

/// RULE 2: `clientToken` is `@idempotencyToken`, so the same token replayed returns the
/// configuration the first call minted — not a second one — and a different token mints
/// its own.
#[tokio::test]
async fn a_replayed_client_token_returns_the_first_security_configuration() {
    let emr = Emr::new();
    // Rows differ in the token's form and length, and in what the body carries beside it.
    let cases = [
        ("a plain token", "sc-idem-1".to_string(), json!({})),
        (
            "a token with punctuation",
            "sc/idem.2-a_b".to_string(),
            json!({"tags": {"a": "1"}}),
        ),
        (
            "a token at the 64-character bound",
            "s".repeat(64),
            json!({"securityConfigurationData": {
                "authenticationConfiguration": {
                    "iamConfiguration": {"systemRole": "arn:aws:iam::000000000000:role/EMRIdem"},
                },
            }}),
        ),
        ("a single-character token", "s".to_string(), json!({})),
    ];

    let mut ids = BTreeSet::new();
    for (label, token, extra) in &cases {
        let mut body = json!({
            "name": "idempotent",
            "clientToken": token,
            "securityConfigurationData": {},
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }

        let first = create_configuration(&emr, body.clone()).await;
        let first = first.ok(label).clone();
        let first_id = text(&first, "/id", label);
        let replay = create_configuration(&emr, body).await;
        let replay = replay.ok(label).clone();
        assert_eq!(
            text(&replay, "/id", label),
            first_id,
            "case {label:?}: replaying clientToken {token:?} minted a second configuration"
        );
        assert_eq!(
            replay.get("arn"),
            first.get("arn"),
            "case {label:?}: the replay answered a different arn"
        );
        assert_eq!(
            replay.get("name"),
            first.get("name"),
            "case {label:?}: the replay answered a different name"
        );
        ids.insert(first_id);
    }

    assert_eq!(
        ids.len(),
        cases.len(),
        "each distinct clientToken must mint its own configuration, got {ids:?}"
    );
}

/// RULE 3: `DescribeSecurityConfiguration` resolves its id out of state and refuses one
/// the store does not hold with the modelled `ResourceNotFoundException` (HTTP 400), so a
/// client can never read a record nothing created.
#[tokio::test]
async fn an_unknown_security_configuration_id_is_refused() {
    let emr = Emr::new();
    let created = create_configuration(
        &emr,
        json!({
            "name": "resolve",
            "clientToken": "sc-tok-resolve",
            "securityConfigurationData": {},
        }),
    )
    .await;
    let configuration = text(created.ok("the stored configuration"), "/id", "resolve");
    // A virtual cluster id: a resource of another type, so it is not a security
    // configuration even though it is a well-formed `ResourceIdString`.
    let cluster = emr.cluster("sc-cluster", "sc-tok-cluster").await;

    // Rows span the forms a `ResourceIdString` takes: both length bounds, a plausible
    // minted-width id, and the id of a different resource type.
    let cases = [
        ("a single-character id", "a".to_string()),
        ("a plausible minted id", "doesnotexist1".to_string()),
        ("an id at the 64-character bound", "z".repeat(64)),
        ("an all-digit id", "0123456789".to_string()),
        ("a virtual cluster's id", cluster.clone()),
    ];

    for (label, id) in &cases {
        describe_configuration(&emr, id)
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }

    // The configuration that IS stored is still readable: a refusal reads nothing out of
    // the store and writes nothing into it.
    let described = describe_configuration(&emr, &configuration).await;
    assert_eq!(
        text(
            field(
                described.ok("the stored configuration"),
                "/securityConfiguration",
                "resolve"
            ),
            "/id",
            "resolve"
        ),
        configuration,
        "the stored configuration must still describe after the refusals"
    );
}

/// RULE 5: the store is bounded. A security configuration has no retired form to reclaim,
/// so at [`state::MAX_SECURITY_CONFIGURATIONS`] the create is refused with the modelled
/// `ValidationException` instead of growing the store, and every stored record stays
/// readable.
#[tokio::test]
async fn security_configuration_creates_stop_at_the_retention_cap() {
    let emr = Emr::new();
    let case = "filling the retention cap";
    let mut ids = Vec::new();
    for index in 0..state::MAX_SECURITY_CONFIGURATIONS {
        let answer = create_configuration(
            &emr,
            json!({
                "name": format!("cap-{index}"),
                "clientToken": format!("sc-cap-tok-{index}"),
                "securityConfigurationData": {},
            }),
        )
        .await;
        ids.push(text(answer.ok(case), "/id", case));
    }
    assert_eq!(
        ids.iter().collect::<BTreeSet<_>>().len(),
        state::MAX_SECURITY_CONFIGURATIONS,
        "case {case:?}: every configuration up to the cap must be created with its own id"
    );

    create_configuration(
        &emr,
        json!({
            "name": "one-too-many",
            "clientToken": "sc-cap-tok-over",
            "securityConfigurationData": {},
        }),
    )
    .await
    .refused("a create past the cap", 400, "ValidationException");

    // The refusal changed nothing: the first, the middle and the last stored record all
    // still describe.
    for index in [0, state::MAX_SECURITY_CONFIGURATIONS / 2, ids.len() - 1] {
        let id = &ids[index];
        let described = describe_configuration(&emr, id).await;
        let configuration = field(described.ok(case), "/securityConfiguration", case);
        assert_eq!(
            text(configuration, "/id", case),
            *id,
            "case {case:?}: configuration {index} must still describe after the refused create"
        );
    }
}

/// RULE 6: `DeleteSecurityConfiguration` removes the configuration from state, so the
/// delete is observable through the only thing that can observe it — the next read.
///
/// The rows differ along what the deleted configuration carried (which half of
/// `securityConfigurationData`, whether it had a `containerProvider`, how many tags) and
/// along the form of the id handed to a delete that must be refused: both `ResourceIdString`
/// length bounds, an all-digit id, a plausible minted id, and the id of a configuration
/// that was already deleted.
#[tokio::test]
async fn deleting_a_security_configuration_removes_it_from_state() {
    let emr = Emr::new();
    let cases = [
        (
            "only the required members",
            json!({
                "name": "del-minimal",
                "clientToken": "sc-del-tok-minimal",
                "securityConfigurationData": {},
            }),
        ),
        (
            "a container provider and tags",
            json!({
                "name": "del-provider",
                "clientToken": "sc-del-tok-provider",
                "containerProvider": {
                    "id": "eks-del",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "emr-del"}},
                },
                "securityConfigurationData": {},
                "tags": {"team": "data", "env": "test"},
            }),
        ),
        (
            "authentication configuration and no tags",
            json!({
                "name": "del-auth",
                "clientToken": "sc-del-tok-auth",
                "securityConfigurationData": {
                    "authenticationConfiguration": {
                        "iamConfiguration": {
                            "systemRole": "arn:aws:iam::000000000000:role/EMRDelRole",
                        },
                    },
                },
            }),
        ),
    ];

    // A configuration that is NOT deleted, so the delete is shown to remove one record
    // rather than to empty the store.
    let keeper = create_configuration(
        &emr,
        json!({
            "name": "del-keeper",
            "clientToken": "sc-del-tok-keeper",
            "securityConfigurationData": {},
        }),
    )
    .await;
    let keeper = text(keeper.ok("the kept configuration"), "/id", "keeper");

    let mut deleted = Vec::new();
    for (label, body) in &cases {
        let created = create_configuration(&emr, body.clone()).await;
        let id = text(created.ok(label), "/id", label);
        // It describes before the delete, so the refusal below is the delete's effect.
        describe_configuration(&emr, &id).await.ok(label);

        let answer = delete_configuration(&emr, &id).await;
        assert_eq!(
            text(answer.ok(label), "/id", label),
            id,
            "case {label:?}: DeleteSecurityConfiguration must answer the id it deleted"
        );

        describe_configuration(&emr, &id)
            .await
            .refused(label, 400, "ResourceNotFoundException");
        deleted.push(id);
    }

    // The kept configuration is untouched by every delete above.
    let kept = describe_configuration(&emr, &keeper).await;
    assert_eq!(
        text(
            field(kept.ok("the kept configuration"), "/securityConfiguration", "keeper"),
            "/id",
            "keeper"
        ),
        keeper,
        "a delete must remove only the configuration it names"
    );

    // An id the store does not hold is refused: this operation models no not-found error,
    // so the modelled `ValidationException` is what refuses it — never a success naming an
    // id nothing deleted.
    let mut refusals: Vec<(String, String)> = vec![
        ("a single-character id".to_string(), "a".to_string()),
        (
            "a plausible minted id".to_string(),
            "doesnotexist1".to_string(),
        ),
        (
            "an id at the 64-character bound".to_string(),
            "z".repeat(64),
        ),
        ("an all-digit id".to_string(), "0123456789".to_string()),
    ];
    for id in &deleted {
        refusals.push(("an already-deleted id".to_string(), id.clone()));
    }
    for (label, id) in &refusals {
        delete_configuration(&emr, id)
            .await
            .refused(label, 400, "ValidationException");
    }

    // A `clientToken` whose configuration was deleted is free again: the create mints a
    // new configuration rather than replaying the deleted one.
    let reused = create_configuration(&emr, cases[0].1.clone()).await;
    let reused = text(reused.ok("a token reused after the delete"), "/id", "reuse");
    assert!(
        !deleted.contains(&reused),
        "a create after the delete must mint a new configuration, got the deleted {reused:?}"
    );
    describe_configuration(&emr, &reused)
        .await
        .ok("a token reused after the delete");
}
