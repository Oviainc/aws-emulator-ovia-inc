// SPDX-License-Identifier: CC0-1.0

//! `GetCompatibleKafkaVersions`, driven end to end over the shared
//! [`crate::kafka_harness`] — the same `MultiServiceRouter` the emulator serves
//! from.
//!
//! The RULE under test: the compatible target versions of a source version are
//! exactly the versions this service still offers (`ACTIVE` in
//! [`KAFKA_VERSION_CATALOG`]) that are strictly NEWER than the source — MSK
//! upgrades a cluster's Apache Kafka version in place, never downgrades, and never
//! upgrades to a version it has deprecated. With a `ClusterArn` the single source
//! version is the one the STORED cluster runs; without one, every catalogued
//! version is a source.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over: where the source sits in the catalog (oldest, deprecated, the
//! active boundary, the middle, the newest), how many components its version string
//! has, whether it carries a qualifier (`2.8.2.tiered`) or a `.x` patch name, and
//! whether the catalog lists it at all. The expected target lists are written out by
//! hand from the catalog and are then cross-checked against the catalog's own order,
//! so neither the handler nor its helper is asserted against itself.

use std::cmp::Ordering;

use serde_json::Value;
use service_com_amazonaws_kafka_kafka::state::{
    kafka_version_order, KAFKA_VERSION_ACTIVE, KAFKA_VERSION_CATALOG,
};

use crate::kafka_harness::{pct, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// `GetCompatibleKafkaVersions` for one cluster, as the model binds it:
/// `GET /v1/compatible-kafka-versions?clusterArn=<arn>`.
async fn compatible_for(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/compatible-kafka-versions?clusterArn={}", pct(arn)),
            None,
        )
        .await
}

/// The same call with no `clusterArn` — the member is optional, and omitting it
/// asks for the whole matrix.
async fn compatible_matrix(emulator: &Emulator) -> Reply {
    emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await
}

/// The `compatibleKafkaVersions` entries of a reply, as
/// `(sourceVersion, targetVersions)` pairs.
fn entries(reply: &Reply) -> Vec<(String, Vec<String>)> {
    reply.body["compatibleKafkaVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", reply.body))
        .iter()
        .map(|entry| {
            let source = entry["sourceVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("entry without a sourceVersion: {entry}"))
                .to_string();
            let targets = entry["targetVersions"]
                .as_array()
                .unwrap_or_else(|| panic!("entry without targetVersions: {entry}"))
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("a non-string target version: {entry}"))
                        .to_string()
                })
                .collect();
            (source, targets)
        })
        .collect()
}

/// The target versions the catalog ITSELF implies for a catalogued `source`: the
/// `ACTIVE` entries that come after it in the catalog, which is ordered oldest
/// first. Computed from the catalog's order alone — no version comparison — so it
/// is an independent check of the handler's answer.
fn targets_from_catalog_order(source: &str) -> Vec<String> {
    let index = KAFKA_VERSION_CATALOG
        .iter()
        .position(|entry| entry.version == source)
        .unwrap_or_else(|| panic!("{source} is not in the catalog"));
    KAFKA_VERSION_CATALOG[index + 1..]
        .iter()
        .filter(|entry| entry.status == KAFKA_VERSION_ACTIVE)
        .map(|entry| entry.version.to_string())
        .collect()
}

/// Every version the service still offers, oldest first — what any source older
/// than all of them must be able to upgrade to.
fn all_active_versions() -> Vec<String> {
    KAFKA_VERSION_CATALOG
        .iter()
        .filter(|entry| entry.status == KAFKA_VERSION_ACTIVE)
        .map(|entry| entry.version.to_string())
        .collect()
}

/// A minimal `CreateCluster` row running `kafka_version`.
fn cluster_on(name: &str, kafka_version: &'static str) -> ClusterRequest {
    ClusterRequest {
        kafka_version,
        ..ClusterRequest::named(name)
    }
}

/// The catalog is documented as ordered OLDEST FIRST, and both the matrix answer and
/// the expectations above read it that way — so the order itself is pinned here: every
/// entry must be strictly newer than the one before it under `kafka_version_order`.
/// An out-of-order insertion would otherwise desync the catalog from the comparison
/// the rule is computed with.
#[test]
fn the_catalog_is_ordered_oldest_first() {
    for pair in KAFKA_VERSION_CATALOG.windows(2) {
        let (older, newer) = (pair[0].version, pair[1].version);
        assert_eq!(
            kafka_version_order(older, newer),
            Ordering::Less,
            "case {older:?} -> {newer:?}: the catalog must run oldest first, got {:?}",
            kafka_version_order(older, newer)
        );
    }
}

/// RULE: with a `ClusterArn`, the answer is one entry whose `sourceVersion` is the
/// version the STORED cluster runs and whose `targetVersions` are the offered
/// versions strictly newer than it.
///
/// Rows: the oldest catalogued version, a deprecated version with four components,
/// the one just below the active boundary, the boundary itself, the qualified
/// `2.8.2.tiered`, a middle release, a `.x` patch name, the newest version (no
/// upgrade left) and a version the catalog does not list at all. The expected lists
/// are written out from the catalog documented in `src/state.rs`.
#[tokio::test]
async fn compatible_targets_are_the_offered_versions_newer_than_the_cluster() {
    let emulator = Emulator::new();
    let active = all_active_versions();

    let rows: Vec<(&'static str, Vec<String>)> = vec![
        // Older than every offered version: all of them are reachable.
        ("1.1.1", active.clone()),
        ("2.4.1.1", active.clone()),
        ("2.8.0", active.clone()),
        // The oldest offered version: itself excluded, everything above it in.
        (
            "2.8.1",
            vec![
                "2.8.2.tiered".to_string(),
                "3.1.1".to_string(),
                "3.2.0".to_string(),
                "3.3.1".to_string(),
                "3.3.2".to_string(),
                "3.4.0".to_string(),
                "3.5.1".to_string(),
                "3.6.0".to_string(),
                "3.7.x".to_string(),
                "3.8.x".to_string(),
                "3.9.x".to_string(),
            ],
        ),
        (
            "2.8.2.tiered",
            vec![
                "3.1.1".to_string(),
                "3.2.0".to_string(),
                "3.3.1".to_string(),
                "3.3.2".to_string(),
                "3.4.0".to_string(),
                "3.5.1".to_string(),
                "3.6.0".to_string(),
                "3.7.x".to_string(),
                "3.8.x".to_string(),
                "3.9.x".to_string(),
            ],
        ),
        (
            "3.6.0",
            vec![
                "3.7.x".to_string(),
                "3.8.x".to_string(),
                "3.9.x".to_string(),
            ],
        ),
        ("3.8.x", vec!["3.9.x".to_string()]),
        // The newest version there is: nothing to upgrade to.
        ("3.9.x", Vec::new()),
        // A version the catalog does not list, newer than every catalogued one:
        // still a valid source, with no target above it.
        ("9.9.9", Vec::new()),
    ];

    for (index, (version, expected)) in rows.iter().enumerate() {
        let request = cluster_on(&format!("compat-{index}"), version);
        let arn = emulator.create_ok(&request).await;
        let reply = compatible_for(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case version={version:?}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let answered = entries(&reply);
        assert_eq!(
            answered.len(),
            1,
            "case version={version:?}: expected exactly one entry, got {answered:?}"
        );
        let (source, targets) = &answered[0];
        assert_eq!(
            source, version,
            "case version={version:?}: expected the stored cluster's version as sourceVersion, got {source:?}"
        );
        assert_eq!(
            targets, expected,
            "case version={version:?}: expected targets {expected:?}, got {targets:?}"
        );
        // The source is never its own target (no self-upgrade), and no target is a
        // version the service no longer offers.
        assert!(
            !targets.contains(source),
            "case version={version:?}: the source version must not be a target, got {targets:?}"
        );
        for target in targets {
            let entry = KAFKA_VERSION_CATALOG
                .iter()
                .find(|entry| entry.version == *target);
            assert!(
                entry.is_some_and(|entry| entry.status == KAFKA_VERSION_ACTIVE),
                "case version={version:?}: target {target:?} is not an offered catalog version, got {targets:?}"
            );
        }
        // And, for a catalogued source, the answer agrees with the catalog's own
        // oldest-first order.
        if KAFKA_VERSION_CATALOG
            .iter()
            .any(|entry| entry.version == *version)
        {
            assert_eq!(
                targets,
                &targets_from_catalog_order(version),
                "case version={version:?}: the answer must match the catalog order, got {targets:?}"
            );
        }
    }
}

/// RULE: with no `ClusterArn` the answer is the whole matrix — one entry per
/// catalogued source version, in the catalog's order, each with the versions newer
/// than it that the service still offers.
#[tokio::test]
async fn the_matrix_covers_every_catalogued_source_version() {
    let emulator = Emulator::new();
    let reply = compatible_matrix(&emulator).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 for a matrix request, got {} {}",
        reply.status, reply.body
    );
    let answered = entries(&reply);
    assert_eq!(
        answered.len(),
        KAFKA_VERSION_CATALOG.len(),
        "expected one entry per catalogued version ({}), got {}",
        KAFKA_VERSION_CATALOG.len(),
        answered.len()
    );
    for (entry, catalogued) in answered.iter().zip(KAFKA_VERSION_CATALOG.iter()) {
        let (source, targets) = entry;
        assert_eq!(
            source, catalogued.version,
            "case source={source:?}: expected the catalog order, {} comes here",
            catalogued.version
        );
        assert_eq!(
            targets,
            &targets_from_catalog_order(source),
            "case source={source:?}: expected {:?}, got {targets:?}",
            targets_from_catalog_order(source)
        );
    }
    // The newest catalogued version has no upgrade target, whichever it is.
    let (newest, newest_targets) = answered
        .last()
        .expect("the catalog is not empty, so the matrix is not either");
    assert!(
        newest_targets.is_empty(),
        "case source={newest:?} (newest): expected no targets, got {newest_targets:?}"
    );
}

/// RULE: a reader that models both errors separates a `ClusterArn` it cannot parse
/// (`BadRequestException`, 400) from a well-formed one this endpoint's region holds
/// no cluster for (`NotFoundException`, 404) — including a cluster that exists in
/// another region.
#[tokio::test]
async fn an_unknown_or_malformed_cluster_arn_is_refused() {
    let emulator = Emulator::new();
    emulator.create_ok(&ClusterRequest::named("present")).await;
    // A cluster of the same shape in another region: well formed, and still not
    // this endpoint's.
    let elsewhere = emulator
        .call_in(
            "eu-west-1",
            "POST",
            "/v1/clusters",
            Some(ClusterRequest::named("elsewhere").to_json()),
        )
        .await;
    let elsewhere_arn = elsewhere.body["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the eu-west-1 create returned no clusterArn: {}", elsewhere.body))
        .to_string();

    // Rows: every way the ARN can fail — absent, another region's, not an ARN,
    // another service, another resource type, an empty region, a trailing segment.
    let rows: Vec<(String, u16, &str)> = vec![
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"),
            404,
            "NotFoundException",
        ),
        (elsewhere_arn, 404, "NotFoundException"),
        ("not-an-arn".to_string(), 400, "BadRequestException"),
        (
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:cluster/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka::{ACCOUNT}:cluster/present/abc-1"),
            400,
            "BadRequestException",
        ),
        (
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/present/abc-1/extra"),
            400,
            "BadRequestException",
        ),
    ];

    for (arn, status, code) in &rows {
        let reply = compatible_for(&emulator, arn).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case arn={arn:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
    }
}

/// RULE: reporting compatible versions is a READ — it changes nothing a later
/// caller sees, and it answers the same thing every time it is asked.
#[tokio::test]
async fn reporting_compatible_versions_changes_nothing() {
    let emulator = Emulator::new();
    // Rows: a source in the middle of the ladder and one at its top, so the
    // invariant is checked both with and without targets to report.
    for (index, version) in ["3.4.0", "3.9.x"].iter().enumerate() {
        let request = cluster_on(&format!("stable-{index}"), version);
        let arn = emulator.create_ok(&request).await;
        let before = emulator.describe(&arn).await;
        let first = entries(&compatible_for(&emulator, &arn).await);
        let second = entries(&compatible_for(&emulator, &arn).await);
        let after = emulator.describe(&arn).await;
        assert_eq!(
            first, second,
            "case version={version:?}: two reads must answer the same, got {first:?} then {second:?}"
        );
        for member in [
            "/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion",
            "/clusterInfo/currentVersion",
        ] {
            assert_eq!(
                before.at(member),
                after.at(member),
                "case version={version:?}: the read must not change {member}, got {} then {}",
                before.at(member),
                after.at(member)
            );
            assert_ne!(
                before.at(member),
                &Value::Null,
                "case version={version:?}: DescribeCluster must report {member}, got {}",
                before.body
            );
        }
    }
}
