// SPDX-License-Identifier: CC0-1.0

//! What `DeleteSecurityConfiguration` resolves before it removes anything, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]) and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A resource another live resource of this service still REFERENCES is not removed: a
//!    virtual cluster holds the `securityConfigurationId` it was created with and reports
//!    it on every read, so a delete of that configuration is refused with the modelled
//!    `ValidationException` (HTTP 400 — the operation models no other client error) and
//!    removes nothing, while a configuration no live cluster names is deleted.
//! 2. The reference is released by the cluster's TEARDOWN, not by the call that starts it:
//!    it is still held while the cluster is `TERMINATING` and released once the clock has
//!    made it `TERMINATED` — and every referring cluster has to release it, not just one.
//! 3. The two ends of the resource's life agree: once the configuration no cluster names
//!    is deleted, a `CreateVirtualCluster` naming it is refused with
//!    `ResourceNotFoundException`, so no read ever reports a reference that resolves to
//!    nothing.

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};
use crate::virtual_cluster_scheduler_status::wait_for_the_clock;

/// `POST /securityconfigurations` — `CreateSecurityConfiguration`. `name`, `clientToken`
/// and `securityConfigurationData` are all `@required` (smithy_query: operation
/// `CreateSecurityConfiguration`).
async fn configuration(emr: &Emr, name: &str, token: &str, data: Value) -> String {
    let answer = emr
        .send(
            "POST",
            "/securityconfigurations",
            Some(json!({
                "name": name,
                "clientToken": token,
                "securityConfigurationData": data,
            })),
        )
        .await;
    text(answer.ok(name), "/id", name)
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

/// `POST /virtualclusters` — `CreateVirtualCluster`, optionally referencing a security
/// configuration and optionally claiming a Kubernetes namespace.
async fn create_cluster(
    emr: &Emr,
    name: &str,
    token: &str,
    provider_id: &str,
    namespace: Option<&str>,
    configuration_id: Option<&str>,
) -> Answer {
    let mut provider = json!({"type": "EKS", "id": provider_id});
    if let (Some(target), Some(namespace)) = (provider.as_object_mut(), namespace) {
        target.insert("info".to_string(), json!({"eksInfo": {"namespace": namespace}}));
    }
    let mut body = json!({"name": name, "clientToken": token, "containerProvider": provider});
    if let (Some(target), Some(id)) = (body.as_object_mut(), configuration_id) {
        target.insert("securityConfigurationId".to_string(), json!(id));
    }
    emr.send("POST", "/virtualclusters", Some(body)).await
}

/// The id of a cluster the create really minted.
async fn cluster_id(
    emr: &Emr,
    case: &str,
    name: &str,
    provider_id: &str,
    namespace: Option<&str>,
    configuration_id: Option<&str>,
) -> String {
    let answer = create_cluster(
        emr,
        name,
        &format!("tok-{name}"),
        provider_id,
        namespace,
        configuration_id,
    )
    .await;
    text(answer.ok(case), "/id", case)
}

/// `GET /virtualclusters/{id}` — the `state` and `securityConfigurationId` a cluster
/// reports.
async fn reported_cluster(emr: &Emr, case: &str, id: &str) -> (String, Option<String>) {
    let described = emr
        .send("GET", &format!("/virtualclusters/{id}"), None)
        .await;
    let cluster = field(described.ok(case), "/virtualCluster", case);
    (
        text(cluster, "/state", case),
        cluster
            .get("securityConfigurationId")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// `DELETE /virtualclusters/{id}` — `DeleteVirtualCluster`, which starts the teardown.
async fn delete_cluster(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/virtualclusters/{id}"), None)
        .await
}

/// One store a delete is attempted against.
struct InUseCase {
    label: &'static str,
    /// The `securityConfigurationData` of the configuration the clusters reference.
    data: Value,
    /// How many live virtual clusters reference it.
    referrers: usize,
    /// Whether those clusters claim a Kubernetes namespace.
    namespace: bool,
    /// How many live clusters reference a DIFFERENT configuration, so the refusal is shown
    /// to be about the reference and not about the store merely holding clusters.
    bystanders: usize,
}

/// RULE 1: a configuration a live virtual cluster references is not removed; one no live
/// cluster references is.
///
/// The rows differ along everything the rule generalises over: how many clusters hold the
/// reference, what the configuration's `securityConfigurationData` carries, whether the
/// holders claim a Kubernetes namespace, and how many unrelated clusters and
/// configurations the same store holds.
#[tokio::test]
async fn a_referenced_security_configuration_is_not_deleted() {
    let cases = [
        InUseCase {
            label: "one holder, an empty configuration-data block",
            data: json!({}),
            referrers: 1,
            namespace: false,
            bystanders: 0,
        },
        InUseCase {
            label: "one holder claiming a namespace, a lake-formation authorization half",
            data: json!({
                "authorizationConfiguration": {
                    "lakeFormationConfiguration": {"authorizedSessionTagValue": "Emr Session Tag"},
                },
            }),
            referrers: 1,
            namespace: true,
            bystanders: 1,
        },
        InUseCase {
            label: "two holders, an identity-center authentication half",
            data: json!({
                "authenticationConfiguration": {
                    "identityCenterConfiguration": {"enableIdentityCenter": true},
                },
            }),
            referrers: 2,
            namespace: false,
            bystanders: 0,
        },
        InUseCase {
            label: "three holders beside three unrelated clusters",
            data: json!({}),
            referrers: 3,
            namespace: true,
            bystanders: 3,
        },
    ];

    for case in &cases {
        let label = case.label;
        // One emulator per row, so each row's store holds only its own records.
        let emr = Emr::new();
        let held = configuration(&emr, "held", "tok-cfg-held", case.data.clone()).await;
        let other = configuration(&emr, "other", "tok-cfg-other", json!({})).await;
        let free = configuration(&emr, "free", "tok-cfg-free", case.data.clone()).await;

        let mut holders = Vec::new();
        for index in 0..case.referrers {
            let name = format!("holder-{index}");
            let namespace = case.namespace.then(|| format!("emr-holder-{index}"));
            holders.push(
                cluster_id(
                    &emr,
                    label,
                    &name,
                    &format!("eks-holder-{index}"),
                    namespace.as_deref(),
                    Some(held.as_str()),
                )
                .await,
            );
        }
        for index in 0..case.bystanders {
            let name = format!("bystander-{index}");
            cluster_id(
                &emr,
                label,
                &name,
                &format!("eks-bystander-{index}"),
                None,
                Some(other.as_str()),
            )
            .await;
        }

        let refused = delete_configuration(&emr, &held).await;
        refused.refused(label, 400, "ValidationException");
        // The refusal says which configuration it is about and which cluster still holds
        // it, out of the store rather than out of the request.
        let reported = text(&refused.body, "/message", label);
        assert!(
            reported.contains(&held) && holders.iter().any(|holder| reported.contains(holder)),
            "case {label:?}: the refusal must name the configuration and a holding cluster, \
             got {reported:?} for configuration {held:?} and holders {holders:?}"
        );

        // Nothing was removed: the configuration still describes, and every holder still
        // reports the reference it was created with.
        assert_eq!(
            text(
                field(
                    describe_configuration(&emr, &held).await.ok(label),
                    "/securityConfiguration",
                    label
                ),
                "/id",
                label
            ),
            held,
            "case {label:?}: the refused delete removed the configuration anyway"
        );
        for holder in &holders {
            let (state_now, reference) = reported_cluster(&emr, label, holder).await;
            assert_eq!(
                reference,
                Some(held.clone()),
                "case {label:?}: holder {holder:?} must still report its reference, state \
                 {state_now:?}"
            );
            assert_eq!(
                state_now, "RUNNING",
                "case {label:?}: the refused delete moved holder {holder:?} to {state_now:?}"
            );
        }

        // The other half of the rule: the configuration no cluster names is deleted, in
        // the very same store.
        assert_eq!(
            text(delete_configuration(&emr, &free).await.ok(label), "/id", label),
            free,
            "case {label:?}: an unreferenced configuration must still be deletable"
        );
        describe_configuration(&emr, &free).await.refused(
            &format!("{label}: the unreferenced configuration after its delete"),
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULES 2 and 3: the reference is released by the teardown of the cluster holding it, and
/// by EVERY holder's teardown. While a holder is `TERMINATING` the configuration is still
/// in use; once the clock has made the last holder `TERMINATED` the delete is honoured —
/// and from then on a create naming that id is refused, so the reference never dangles.
///
/// The rows differ in how many clusters hold the reference, which is what "the last holder"
/// generalises over.
#[tokio::test]
async fn a_configuration_is_deletable_once_every_holder_is_torn_down() {
    for referrers in [1usize, 2, 3] {
        let label = format!("{referrers} holder(s) of one configuration");
        let label = label.as_str();
        let emr = Emr::new();
        let held = configuration(&emr, "cycle", "tok-cfg-cycle", json!({})).await;

        let mut holders = Vec::new();
        for index in 0..referrers {
            holders.push(
                cluster_id(
                    &emr,
                    label,
                    &format!("cycle-{index}"),
                    &format!("eks-cycle-{index}"),
                    Some(&format!("emr-cycle-{index}")),
                    Some(held.as_str()),
                )
                .await,
            );
        }

        for (index, holder) in holders.iter().enumerate() {
            let remaining = referrers - index;
            delete_configuration(&emr, &held).await.refused(
                &format!("{label}: the delete with {remaining} holder(s) left"),
                400,
                "ValidationException",
            );

            delete_cluster(&emr, holder)
                .await
                .ok(&format!("{label}: starting the teardown of {holder}"));
            let (state_now, _) = reported_cluster(&emr, label, holder).await;
            assert_eq!(
                state_now, "TERMINATING",
                "case {label:?}: the delete of {holder:?} must leave it TERMINATING, got \
                 {state_now:?}"
            );
            // The teardown has started and the reference is still held: the cluster is
            // still registered and still reports it.
            delete_configuration(&emr, &held).await.refused(
                &format!("{label}: the delete while {holder} is TERMINATING"),
                400,
                "ValidationException",
            );
        }

        // The clock finishes every teardown, and the last released reference is what frees
        // the configuration.
        wait_for_the_clock();
        for holder in &holders {
            let (state_now, _) = reported_cluster(&emr, label, holder).await;
            assert_eq!(
                state_now, "TERMINATED",
                "case {label:?}: {holder:?} must be TERMINATED once the clock has passed, got \
                 {state_now:?}"
            );
        }
        assert_eq!(
            text(
                delete_configuration(&emr, &held).await.ok(label),
                "/id",
                label
            ),
            held,
            "case {label:?}: the delete must be honoured once no live cluster holds the \
             reference"
        );
        describe_configuration(&emr, &held).await.refused(
            &format!("{label}: the deleted configuration"),
            400,
            "ResourceNotFoundException",
        );

        // RULE 3: the create side agrees — the id that no longer resolves cannot be
        // persisted as a new cluster's reference either.
        create_cluster(
            &emr,
            "cycle-after",
            "tok-cycle-after",
            "eks-cycle-after",
            None,
            Some(held.as_str()),
        )
        .await
        .refused(
            &format!("{label}: a create naming the deleted configuration"),
            400,
            "ResourceNotFoundException",
        );
    }
}
