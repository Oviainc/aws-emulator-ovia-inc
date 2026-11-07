// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the COVERAGE of the EKS add-on catalog, driven over the
//! wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test is: `DescribeAddonVersions` reports the Amazon EKS add-on
//! catalog, a PUBLICATION rather than per-account data, so for EVERY add-on
//! Amazon EKS publishes it answers with exactly one `AddonInfo` carrying that
//! add-on's `defaultNamespace` and the builds whose `compatibilities` name the
//! Kubernetes versions each supports — exactly one of them the `defaultVersion`
//! for any one of those versions — and that same catalog entry is what an install
//! of the add-on resolves its version and its namespace from.
//!
//! `addon_versions.rs` covers how the listing FILTERS and reports the catalog and
//! `addon_catalog_publication.rs` the configuration surface each published build
//! carries; this suite covers what the catalog's REACH implies for a client that
//! has to pick a cluster version and install on it: an add-on Amazon EKS
//! publishes but the seed omits answers an empty listing, so no version of it can
//! be discovered, resolved for a cluster version, or installed.
//!
//! The add-ons the catalog must publish are the ONE table
//! `addon_catalog_publication.rs` holds ([`PUBLISHED_ADDONS`], with each add-on's
//! published namespace), so the two suites cannot drift apart over which add-ons
//! exist.
//!
//! Every suite asserts the rule over a TABLE whose rows differ along what the
//! rule generalises over (which add-on, which Kubernetes version of the published
//! calendar, whether the add-on installs into the shared namespace or one of its
//! own), and each assertion names its case and prints the actual value. The
//! request harness and the add-on calls are the ones the add-on suites beside
//! this one already share.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::addon_catalog_publication::PUBLISHED_ADDONS;
use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::addon_versions::{
    active_cluster, addons, compatibilities_of, describe_addon_versions, string_member,
    versions_of, whole_catalog,
};
use crate::cluster_lifecycle::REGION;
use crate::cluster_versions::version_key;
use crate::wire::{call, emulator, encode_label};

/// The one `AddonInfo` the catalog publishes for `addon_name`, optionally for one
/// Kubernetes version — the lookup a client makes to resolve a version.
pub(crate) async fn published_addon(
    router: &MultiServiceRouter,
    addon_name: &str,
    kubernetes_version: Option<&str>,
) -> serde_json::Value {
    let mut query = format!("addonName={}", encode_label(addon_name));
    if let Some(version) = kubernetes_version {
        query.push_str(&format!("&kubernetesVersion={}", encode_label(version)));
    }
    let answer = describe_addon_versions(router, &query).await;
    assert_eq!(
        answer.status, 200,
        "case {addon_name:?} ({kubernetes_version:?}): expected HTTP 200 from \
         DescribeAddonVersions, got {answer:?}"
    );
    let listed = addons(&answer);
    assert_eq!(
        listed.len(),
        1,
        "case {addon_name:?} ({kubernetes_version:?}): a published add-on is reported as \
         exactly one AddonInfo, got {listed:?}"
    );
    listed[0].clone()
}

/// The Kubernetes versions one reported add-on's builds support, deduplicated.
pub(crate) fn supported_cluster_versions(addon: &serde_json::Value) -> BTreeSet<String> {
    versions_of(addon)
        .iter()
        .flat_map(|build| {
            compatibilities_of(build)
                .iter()
                .map(|entry| string_member(entry, "clusterVersion"))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The build the catalog publishes as the default of `addon` for the Kubernetes
/// version the listing was filtered to.
fn default_build(addon: &serde_json::Value) -> Option<String> {
    versions_of(addon)
        .iter()
        .find(|build| {
            compatibilities_of(build)
                .iter()
                .any(|entry| entry["defaultVersion"].as_bool() == Some(true))
        })
        .map(|build| string_member(build, "addonVersion"))
}

/// The Kubernetes versions `DescribeClusterVersions` reports in STANDARD support
/// — the versions a client picks a cluster version out of before resolving the
/// add-on builds published for it.
async fn standard_support_versions(router: &MultiServiceRouter) -> BTreeSet<String> {
    let answer = call(
        router,
        REGION,
        "GET",
        "/cluster-versions?versionStatus=STANDARD_SUPPORT",
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions(STANDARD_SUPPORT): got {answer:?}"
    );
    let rows = answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .clone();
    let versions: BTreeSet<String> = rows
        .iter()
        .map(|row| string_member(row, "clusterVersion"))
        .collect();
    assert!(
        !versions.is_empty(),
        "setup DescribeClusterVersions: expected versions in standard support, got {rows:?}"
    );
    versions
}

/// The newest Kubernetes version in standard support that EVERY published add-on
/// supports — the version a client picks when one cluster must take several
/// add-ons.
async fn version_every_addon_supports(router: &MultiServiceRouter) -> String {
    let mut shared = standard_support_versions(router).await;
    for addon in addons(&whole_catalog(router).await) {
        shared = shared
            .intersection(&supported_cluster_versions(&addon))
            .cloned()
            .collect();
    }
    let mut candidates: Vec<String> = shared.into_iter().collect();
    candidates.sort_by_key(|version| version_key(version));
    candidates.pop().unwrap_or_else(|| {
        panic!("no Kubernetes version in standard support is supported by the whole catalog")
    })
}

// ---------------------------------------------------------------------------
// RULE: every published add-on reports the namespace its workloads run in, which
// is the namespace an install of it goes into.
// ---------------------------------------------------------------------------

/// The table is the whole published catalog, whose rows differ in the kind of
/// add-on and in whether it installs into the shared `kube-system` namespace or
/// into one of its own. (That each name is published at all, as exactly one
/// `AddonInfo` with at least one build, is `addon_catalog_publication.rs`'s own
/// suite over this same table.)
#[tokio::test]
async fn every_published_addon_reports_the_namespace_it_installs_into() {
    let router = emulator();

    for (addon_name, default_namespace, kind) in PUBLISHED_ADDONS {
        let addon = published_addon(&router, addon_name, None).await;
        assert_eq!(
            string_member(&addon, "defaultNamespace"),
            *default_namespace,
            "case {addon_name:?} ({kind}): expected defaultNamespace {default_namespace:?}, \
             got {:?}",
            addon["defaultNamespace"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every published add-on supports Kubernetes versions the calendar carries
// in standard support, and for any one of them the catalog names exactly ONE
// build as that version's default.
// ---------------------------------------------------------------------------

/// The table is the whole published catalog crossed with every Kubernetes version
/// in standard support that the add-on supports, so the rows differ in both the
/// add-on and the cluster version the listing is filtered to.
#[tokio::test]
async fn every_published_addon_names_one_default_build_per_cluster_version() {
    let router = emulator();
    let standard_support = standard_support_versions(&router).await;

    for (addon_name, _, kind) in PUBLISHED_ADDONS {
        let addon = published_addon(&router, addon_name, None).await;
        assert_eq!(
            string_member(&addon, "addonName"),
            *addon_name,
            "case {addon_name:?} ({kind}): the reported add-on must be the one asked for, got \
             {addon:?}"
        );
        let supported = supported_cluster_versions(&addon);
        let usable: Vec<String> = supported.intersection(&standard_support).cloned().collect();
        assert!(
            !usable.is_empty(),
            "case {addon_name:?}: a published add-on supports a Kubernetes version in standard \
             support, got supported {supported:?} against {standard_support:?}"
        );

        for cluster_version in &usable {
            let filtered = published_addon(&router, addon_name, Some(cluster_version)).await;
            let defaults: Vec<String> = versions_of(&filtered)
                .iter()
                .filter(|build| {
                    compatibilities_of(build)
                        .iter()
                        .any(|entry| entry["defaultVersion"].as_bool() == Some(true))
                })
                .map(|build| string_member(build, "addonVersion"))
                .collect();
            assert_eq!(
                defaults.len(),
                1,
                "case {addon_name:?} on {cluster_version}: exactly one build is the default for \
                 a Kubernetes version, got {defaults:?}"
            );
            for build in &versions_of(&filtered) {
                let reported: Vec<String> = compatibilities_of(build)
                    .iter()
                    .map(|entry| string_member(entry, "clusterVersion"))
                    .collect();
                assert_eq!(
                    reported,
                    vec![cluster_version.clone()],
                    "case {addon_name:?} on {cluster_version}: a filtered listing reports that \
                     version's compatibility alone, got {reported:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the catalog entry is what an install resolves from — an add-on created
// without a version takes the build the catalog publishes as the default for the
// cluster's Kubernetes version, and without a `namespaceConfig` it goes into that
// add-on's own published `defaultNamespace`.
// ---------------------------------------------------------------------------

/// The rows differ in whose namespace the add-on installs into (the shared
/// `kube-system`, the add-on's own, a product's own) and in which kind of add-on
/// it is (one Amazon EKS builds, a community one, a machine-learning operator).
#[tokio::test]
async fn a_published_addon_installs_at_the_catalog_default_for_the_cluster_version() {
    let router = emulator();
    let cluster_version = version_every_addon_supports(&router).await;
    active_cluster(&router, "catalog-install", &cluster_version).await;

    let cases = [
        "cert-manager",
        "amazon-sagemaker-hyperpod-training-operator",
        "amazon-sagemaker-hyperpod-taskgovernance",
        "aws-network-flow-monitoring-agent",
        "prometheus-node-exporter",
        "vpc-cni",
    ];
    for addon_name in cases {
        let (_, default_namespace, _) = PUBLISHED_ADDONS
            .iter()
            .find(|(name, _, _)| *name == addon_name)
            .unwrap_or_else(|| panic!("case {addon_name:?} must be a published add-on"));

        // What the catalog publishes as this add-on's default build for the
        // cluster's Kubernetes version.
        let filtered = published_addon(&router, addon_name, Some(&cluster_version)).await;
        let expected_version = default_build(&filtered).unwrap_or_else(|| {
            panic!("case {addon_name:?}: no default build on {cluster_version}")
        });

        let created = create_addon(
            &router,
            "catalog-install",
            &serde_json::json!({ "addonName": addon_name }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {addon_name:?}: expected HTTP 200 from CreateAddon, got {created:?}"
        );
        assert_eq!(
            created.addon()["addonVersion"],
            serde_json::json!(expected_version),
            "case {addon_name:?}: expected the catalog's default build {expected_version:?}, \
             got {:?}",
            created.addon()["addonVersion"]
        );
        assert_eq!(
            created.addon()["namespaceConfig"]["namespace"],
            serde_json::json!(default_namespace),
            "case {addon_name:?}: expected the catalog's namespace {default_namespace:?}, got \
             {:?}",
            created.addon()["namespaceConfig"]
        );

        // The round trip: the installed add-on reads back what was written.
        let described = describe_addon(&router, "catalog-install", addon_name).await;
        assert_eq!(
            described.status, 200,
            "case {addon_name:?}: expected HTTP 200 from DescribeAddon, got {described:?}"
        );
        assert_eq!(
            described.addon()["addonVersion"],
            serde_json::json!(expected_version),
            "case {addon_name:?}: the read-back version must be the installed one, got {:?}",
            described.addon()["addonVersion"]
        );
        assert_eq!(
            described.addon()["namespaceConfig"]["namespace"],
            serde_json::json!(default_namespace),
            "case {addon_name:?}: the read-back namespace must be the installed one, got {:?}",
            described.addon()["namespaceConfig"]
        );
    }
}

/// A `namespaceConfig` the caller names still OVERRIDES the published default, so
/// the catalog decides only what an absent member leaves open. The rows differ in
/// whether the add-on's own default is the shared namespace or one of its own.
#[tokio::test]
async fn a_requested_namespace_overrides_the_catalog_default() {
    let router = emulator();
    let cluster_version = version_every_addon_supports(&router).await;
    active_cluster(&router, "namespace-override", &cluster_version).await;

    for (addon_name, requested) in [
        ("cert-manager", "certs"),
        ("vpc-cni", "networking"),
        ("amazon-sagemaker-hyperpod-training-operator", "training"),
    ] {
        let created = create_addon(
            &router,
            "namespace-override",
            &serde_json::json!({
                "addonName": addon_name,
                "namespaceConfig": { "namespace": requested },
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {addon_name:?}: expected HTTP 200 from CreateAddon, got {created:?}"
        );
        assert_eq!(
            created.addon()["namespaceConfig"]["namespace"],
            serde_json::json!(requested),
            "case {addon_name:?}: a requested namespace overrides the catalog default, got {:?}",
            created.addon()["namespaceConfig"]
        );
    }
}
