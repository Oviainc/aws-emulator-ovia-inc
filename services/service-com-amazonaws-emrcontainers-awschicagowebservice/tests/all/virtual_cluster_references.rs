// SPDX-License-Identifier: CC0-1.0

//! What `CreateVirtualCluster` resolves before it mints anything, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]) and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A reference to another resource of this service is resolved out of that resource's
//!    store before the create mints anything: a `securityConfigurationId` naming no stored
//!    security configuration is refused with the modelled `ResourceNotFoundException` and
//!    persists nothing, while one that names a stored configuration is accepted and read
//!    back by `DescribeVirtualCluster`.
//! 2. A virtual cluster maps 1:1 onto a Kubernetes namespace of a container cluster: a
//!    create naming a `(containerProvider.id, namespace)` pair a live virtual cluster
//!    already holds is refused, while a pair differing in either component is accepted.
//! 3. The refusal is about OTHER clusters: a replayed `@idempotencyToken` still answers
//!    with the cluster it minted, which is the very holder of that namespace.
//! 4. A namespace is released by the teardown of the cluster holding it, not by the call
//!    that starts the teardown: it is still held while the cluster is `TERMINATING` and
//!    free once the clock has made it `TERMINATED`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{assert_resource_id, field, text, Answer, Emr};
use crate::virtual_cluster_scheduler_status::wait_for_the_clock;

/// `POST /virtualclusters` — `CreateVirtualCluster`.
async fn create_cluster(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/virtualclusters", Some(body)).await
}

/// `GET /virtualclusters/{id}` — `DescribeVirtualCluster`.
async fn describe_cluster(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/virtualclusters/{id}"), None).await
}

/// `DELETE /virtualclusters/{id}` — `DeleteVirtualCluster`.
async fn delete_cluster(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/virtualclusters/{id}"), None)
        .await
}

/// One `CreateVirtualCluster` body: a name, a token, a container provider, and a
/// `securityConfigurationId` when the row carries one.
fn cluster_body(
    name: &str,
    token: &str,
    provider_id: &str,
    namespace: Option<&str>,
    security_configuration_id: Option<&str>,
) -> Value {
    let mut provider = json!({"type": "EKS", "id": provider_id});
    if let (Some(target), Some(namespace)) = (provider.as_object_mut(), namespace) {
        target.insert("info".to_string(), json!({"eksInfo": {"namespace": namespace}}));
    }
    let mut body = json!({"name": name, "clientToken": token, "containerProvider": provider});
    if let (Some(target), Some(id)) = (body.as_object_mut(), security_configuration_id) {
        target.insert("securityConfigurationId".to_string(), json!(id));
    }
    body
}

/// `POST /securityconfigurations` — the id of a security configuration this service really
/// holds, so a cluster referencing it names a resource that exists.
async fn stored_configuration(emr: &Emr, name: &str, token: &str, data: Value) -> String {
    let created = create_configuration(emr, name, token, data).await;
    let id = text(created.ok(name), "/id", name);
    assert_resource_id(name, "the minted security configuration id", &id);
    id
}

async fn create_configuration(emr: &Emr, name: &str, token: &str, data: Value) -> Answer {
    emr.send(
        "POST",
        "/securityconfigurations",
        Some(json!({
            "name": name,
            "clientToken": token,
            "securityConfigurationData": data,
        })),
    )
    .await
}

/// The `securityConfigurationId` a described cluster reports, if any.
async fn reported_configuration(emr: &Emr, case: &str, id: &str) -> Option<String> {
    let described = describe_cluster(emr, id).await;
    field(described.ok(case), "/virtualCluster", case)
        .get("securityConfigurationId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// RULE 1: a `securityConfigurationId` that names no stored security configuration is
/// refused with `ResourceNotFoundException` (HTTP 400) and nothing is persisted — the same
/// request with a reference that DOES resolve is then accepted under the very token the
/// refusal was issued for.
///
/// The rows span the forms a `ResourceIdString` takes (both published length bounds, an
/// all-digit id, a plausible minted-width id), plus the id of a resource of a DIFFERENT
/// type and the id of a configuration that was deleted — each well formed, none naming a
/// stored security configuration.
#[tokio::test]
async fn an_unresolvable_security_configuration_id_is_refused() {
    let emr = Emr::new();
    let present = stored_configuration(&emr, "resolves", "tok-sc-resolves", json!({})).await;

    // A virtual cluster id: another resource type's id, well formed as a
    // `ResourceIdString` yet naming no security configuration.
    let other_type = text(
        create_cluster(
            &emr,
            cluster_body("other-type", "tok-other-type", "eks-other-type", None, None),
        )
        .await
        .ok("a cluster whose id is offered as a configuration id"),
        "/id",
        "a cluster whose id is offered as a configuration id",
    );

    // A deleted configuration: it resolved once and must not resolve any more.
    let deleted = stored_configuration(&emr, "deleted", "tok-sc-deleted", json!({})).await;
    emr.send("DELETE", &format!("/securityconfigurations/{deleted}"), None)
        .await
        .ok("deleting the configuration the rows below reference");

    let cases: [(&str, String); 6] = [
        // `ResourceIdString` is len 1..64, `^[0-9a-z]+$` (smithy_query: operation
        // `CreateVirtualCluster`, input `securityConfigurationId`): both bounds, and
        // values inside them.
        ("the shortest id the model allows", "a".to_string()),
        ("the longest id the model allows", "z".repeat(64)),
        ("an all-digit id", "0123456789".to_string()),
        ("a plausible minted id", "doesnotexist1".to_string()),
        ("a virtual cluster's id", other_type.clone()),
        ("a deleted configuration's id", deleted.clone()),
    ];

    for (index, (label, configuration_id)) in cases.iter().enumerate() {
        assert_ne!(
            configuration_id, &present,
            "case {label:?}: the row must name an id no stored configuration carries"
        );
        let name = format!("ref-{index}");
        let token = format!("tok-ref-{index}");
        let provider = format!("eks-ref-{index}");
        create_cluster(
            &emr,
            cluster_body(
                &name,
                &token,
                &provider,
                None,
                Some(configuration_id.as_str()),
            ),
        )
        .await
        .refused(label, 400, "ResourceNotFoundException");

        // The refusal persisted nothing: the very same request, with the only unresolvable
        // member replaced by a reference that resolves, is accepted — so neither a cluster
        // nor the `clientToken` that names one was spent by the refused call.
        let accepted = create_cluster(
            &emr,
            cluster_body(&name, &token, &provider, None, Some(present.as_str())),
        )
        .await;
        let id = text(accepted.ok(label), "/id", label);
        assert_resource_id(label, "the minted virtual cluster id", &id);
        assert_eq!(
            reported_configuration(&emr, label, &id).await,
            Some(present.clone()),
            "case {label:?}: the resolved reference must be read back out of state"
        );
    }

    // The cluster whose id was offered as a configuration id is still there, and still
    // carries no reference of its own.
    assert_eq!(
        reported_configuration(&emr, "a cluster that references nothing", &other_type).await,
        None,
        "a cluster created without a securityConfigurationId must report none"
    );
}

/// RULE 1, the accepting half: every stored security configuration is a reference a create
/// accepts, whatever shape the configuration has and however many clusters name it.
///
/// The rows differ in the configuration's `securityConfigurationData` (empty, an
/// authorization half, an authentication half), which is what one stored configuration can
/// differ from another in.
#[tokio::test]
async fn a_stored_security_configuration_is_accepted_as_a_reference() {
    let emr = Emr::new();
    let cases: [(&str, Value); 3] = [
        ("an empty configuration-data block", json!({})),
        (
            "a lake-formation authorization half",
            json!({
                "authorizationConfiguration": {
                    "lakeFormationConfiguration": {"authorizedSessionTagValue": "Emr Session Tag"},
                },
            }),
        ),
        (
            "an identity-center authentication half",
            json!({
                "authenticationConfiguration": {
                    "identityCenterConfiguration": {"enableIdentityCenter": true},
                },
            }),
        ),
    ];

    let mut referenced = BTreeSet::new();
    for (index, (label, data)) in cases.iter().enumerate() {
        let configuration = stored_configuration(
            &emr,
            &format!("shape-{index}"),
            &format!("tok-shape-{index}"),
            data.clone(),
        )
        .await;
        // TWO clusters name the same configuration: a security configuration is a template
        // any number of virtual clusters may reference, so resolving it is not consuming
        // it.
        for copy in 0..2 {
            let name = format!("shaped-{index}-{copy}");
            let created = create_cluster(
                &emr,
                cluster_body(
                    &name,
                    &format!("tok-shaped-{index}-{copy}"),
                    &format!("eks-shaped-{index}-{copy}"),
                    None,
                    Some(configuration.as_str()),
                ),
            )
            .await;
            let id = text(created.ok(label), "/id", label);
            assert_eq!(
                reported_configuration(&emr, label, &id).await,
                Some(configuration.clone()),
                "case {label:?}: copy {copy} must report the configuration it referenced"
            );
            assert!(
                referenced.insert(id.clone()),
                "case {label:?}: id {id:?} was minted twice"
            );
        }
    }
    assert_eq!(
        referenced.len(),
        cases.len() * 2,
        "every row must have minted its own clusters, got {referenced:?}"
    );
}

/// One second `CreateVirtualCluster` against a container cluster whose namespace `emr-one`
/// is already registered.
struct PairCase {
    label: &'static str,
    provider_id: &'static str,
    namespace: Option<&'static str>,
    accepted: bool,
}

/// RULE 2: the `(containerProvider.id, namespace)` pair is what a virtual cluster occupies.
/// A second create naming the same pair is refused with `ValidationException` (HTTP 400);
/// a pair differing in either component, or naming no namespace at all, is accepted.
///
/// The rows differ along each component of the pair, and in whether a namespace is named at
/// all: `ContainerProvider$info` and `EksInfo$namespace` are both optional (smithy_query:
/// shape `EksInfo`), so a request may claim no namespace, and then has none to collide on.
#[tokio::test]
async fn a_namespace_carries_at_most_one_virtual_cluster() {
    let emr = Emr::new();
    let held = "the first cluster of eks-pair/emr-one";
    let first = text(
        create_cluster(
            &emr,
            cluster_body("pair-first", "tok-pair-first", "eks-pair", Some("emr-one"), None),
        )
        .await
        .ok(held),
        "/id",
        held,
    );

    let cases = [
        PairCase {
            label: "the same provider and the same namespace",
            provider_id: "eks-pair",
            namespace: Some("emr-one"),
            accepted: false,
        },
        PairCase {
            label: "the same provider, a different namespace",
            provider_id: "eks-pair",
            namespace: Some("emr-two"),
            accepted: true,
        },
        PairCase {
            label: "a different provider, the same namespace",
            provider_id: "eks-other",
            namespace: Some("emr-one"),
            accepted: true,
        },
        PairCase {
            label: "the same provider, no namespace at all",
            provider_id: "eks-pair",
            namespace: None,
            accepted: true,
        },
        PairCase {
            label: "the same provider, no namespace at all, again",
            provider_id: "eks-pair",
            namespace: None,
            accepted: true,
        },
        PairCase {
            // `KubernetesNamespace` is len 1..63, `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`: the
            // shortest value the shape admits, unclaimed so far.
            label: "the shortest namespace the model allows",
            provider_id: "eks-pair",
            namespace: Some("a"),
            accepted: true,
        },
        PairCase {
            label: "the shortest namespace the model allows, already claimed",
            provider_id: "eks-pair",
            namespace: Some("a"),
            accepted: false,
        },
    ];

    let mut ids = BTreeSet::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let answer = create_cluster(
            &emr,
            cluster_body(
                &format!("pair-{index}"),
                &format!("tok-pair-{index}"),
                case.provider_id,
                case.namespace,
                None,
            ),
        )
        .await;
        if !case.accepted {
            answer.refused(label, 400, "ValidationException");
            continue;
        }
        let id = text(answer.ok(label), "/id", label);
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: id {id:?} was minted twice"
        );
        let described = describe_cluster(&emr, &id).await;
        assert_eq!(
            text(field(described.ok(label), "/virtualCluster", label), "/state", label),
            "RUNNING",
            "case {label:?}: an accepted create must leave a RUNNING cluster"
        );
    }

    // RULE 3: the refusals are about OTHER clusters. The holder's own token still names
    // the cluster it minted rather than being refused by that cluster's namespace.
    let replay = create_cluster(
        &emr,
        cluster_body("pair-first", "tok-pair-first", "eks-pair", Some("emr-one"), None),
    )
    .await;
    assert_eq!(
        text(replay.ok("the holder's token replayed"), "/id", "the holder's token replayed"),
        first,
        "a replayed clientToken must answer the cluster it minted, not a namespace refusal"
    );
    assert!(
        !ids.contains(&first),
        "the replay must not have minted a second cluster, got {ids:?}"
    );
}

/// RULE 4: the namespace is released by the TEARDOWN, not by the call that starts it. While
/// the holder is `TERMINATING` the pair is still taken; once the clock has made it
/// `TERMINATED` the same pair is free, and the cluster that takes it is a new one.
#[tokio::test]
async fn a_torn_down_cluster_releases_its_namespace() {
    let emr = Emr::new();
    let label = "eks-cycle/emr-cycle";
    let first = text(
        create_cluster(
            &emr,
            cluster_body(
                "cycle-first",
                "tok-cycle-first",
                "eks-cycle",
                Some("emr-cycle"),
                None,
            ),
        )
        .await
        .ok(label),
        "/id",
        label,
    );

    delete_cluster(&emr, &first).await.ok(label);
    let state_now = text(
        field(
            describe_cluster(&emr, &first).await.ok(label),
            "/virtualCluster",
            label,
        ),
        "/state",
        label,
    );
    assert_eq!(
        state_now, "TERMINATING",
        "case {label:?}: the delete must leave the cluster TERMINATING, got {state_now:?}"
    );
    // Phase one: the teardown has started and the namespace is still registered.
    create_cluster(
        &emr,
        cluster_body(
            "cycle-while-terminating",
            "tok-cycle-terminating",
            "eks-cycle",
            Some("emr-cycle"),
            None,
        ),
    )
    .await
    .refused(label, 400, "ValidationException");

    // Phase two: the clock has finished the teardown, so the namespace is free.
    wait_for_the_clock();
    let second = text(
        create_cluster(
            &emr,
            cluster_body(
                "cycle-second",
                "tok-cycle-second",
                "eks-cycle",
                Some("emr-cycle"),
                None,
            ),
        )
        .await
        .ok(label),
        "/id",
        label,
    );
    assert_ne!(
        second, first,
        "case {label:?}: the namespace's new cluster must be a new resource"
    );
    let terminated = text(
        field(
            describe_cluster(&emr, &first).await.ok(label),
            "/virtualCluster",
            label,
        ),
        "/state",
        label,
    );
    assert_eq!(
        terminated, "TERMINATED",
        "case {label:?}: the cluster that released the namespace must be TERMINATED, got \
         {terminated:?}"
    );
    let still_running = text(
        field(
            describe_cluster(&emr, &second).await.ok(label),
            "/virtualCluster",
            label,
        ),
        "/state",
        label,
    );
    assert_eq!(
        still_running, "RUNNING",
        "case {label:?}: the cluster that took the namespace must be RUNNING, got \
         {still_running:?}"
    );
}
