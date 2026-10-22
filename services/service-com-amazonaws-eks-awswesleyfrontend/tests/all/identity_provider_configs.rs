// SPDX-License-Identifier: CC0-1.0

//! WHERE the generic not-found wording belongs, and where it does not.
//!
//! `DescribeIdentityProviderConfig` resolves ONE resource — the
//! `(cluster, configuration)` pair — so it reports the service's generic not-found
//! sentence (`state::RESOURCE_NOT_FOUND_MESSAGE`; `identity_provider_config_not_found`
//! covers that rule over a table of absences). The operations that resolve a CLUSTER by
//! name, by contrast, name the cluster they could not find. This suite pins both halves
//! together, over a table of operations, so the wording cannot drift from one to the
//! other: it is the "applies here, not there" half of the rule, which neither of the
//! other suites asserts.
//!
//! The endpoint drivers are the ones `identity_provider_config_not_found` owns; the
//! request harness is `wire`'s.

use service_com_amazonaws_eks_awswesleyfrontend::state::RESOURCE_NOT_FOUND_MESSAGE;

use crate::cluster_lifecycle::{describe, REGION};
use crate::identity_provider_config_not_found::{
    active_cluster_in, assert_not_found, associate_in, describe_config_in,
};
use crate::wire::{call, emulator, encode_label, Answer};

/// The name no cluster in the store carries, used as the missing resource in every row.
const MISSING_CLUSTER: &str = "no-such-cluster";

/// The message the cluster-resolving operations report for that name.
fn cluster_not_found_message() -> serde_json::Value {
    serde_json::json!(format!("No cluster found for name: {MISSING_CLUSTER}."))
}

#[tokio::test]
async fn only_the_identity_provider_read_reports_the_generic_not_found_wording() {
    let router = emulator();
    // A real, ACTIVE cluster with a configuration on it, so every row below is about
    // the name it names rather than about an empty store.
    active_cluster_in(&router, REGION, "idp-present").await;
    let associated = associate_in(&router, REGION, "idp-present", "present-config").await;
    assert_eq!(
        associated.status, 200,
        "setup: the association must succeed, got {associated:?}"
    );

    // The identity-provider READ: the pair is one resource, so the answer names
    // neither half of it — whether the cluster or the configuration is the missing one.
    for (case, cluster, config_name) in [
        ("unknown cluster", MISSING_CLUSTER, "present-config"),
        ("unknown configuration", "idp-present", "absent-config"),
    ] {
        let answer = describe_config_in(&router, REGION, cluster, config_name).await;
        assert_not_found(case, &answer, &[cluster, config_name]);
    }

    // The operations that resolve a CLUSTER by name keep the wording that names it.
    // Rows differ along the operation and its HTTP shape (a path-only GET and DELETE,
    // and a POST carrying a body).
    let cluster_resolving: Vec<(&str, Answer)> = vec![
        ("DescribeCluster", describe(&router, MISSING_CLUSTER).await),
        (
            "AssociateIdentityProviderConfig",
            associate_in(&router, REGION, MISSING_CLUSTER, "whatever").await,
        ),
        (
            "DeleteCluster",
            call(
                &router,
                REGION,
                "DELETE",
                &format!("/clusters/{}", encode_label(MISSING_CLUSTER)),
                "",
            )
            .await,
        ),
    ];

    for (case, answer) in cluster_resolving {
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
            cluster_not_found_message(),
            "case {case:?}: an operation that resolves a cluster by name must name it, \
             got {:?}",
            answer.body["message"]
        );
        assert_ne!(
            answer.body["message"],
            serde_json::json!(RESOURCE_NOT_FOUND_MESSAGE),
            "case {case:?}: the identity-provider read's generic wording must not spread \
             to an operation that resolves a cluster by name"
        );
    }
}
