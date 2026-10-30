// SPDX-License-Identifier: CC0-1.0

//! The two version READ paths answer from ONE catalog: `ListKafkaVersions`
//! publishes it and `GetCompatibleKafkaVersions` derives its matrix from the same
//! list, so neither can offer a version the other does not know.
//!
//! Driven end to end over the same `MultiServiceRouter` the emulator serves from,
//! and asserted from the WIRE alone: the expectations below are computed from what
//! `ListKafkaVersions` reported, not from the service's own catalog constant, so a
//! matrix derived from a second, drifting table would fail here.
//!
//! The invariants are checked EXHAUSTIVELY over every source the matrix reports —
//! the input space is the catalog itself, which is small enough to walk, and that
//! is stronger than sampling it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::kafka_harness::{Emulator, Reply};

/// A `KafkaVersionStatus` value: a version the service still offers.
const ACTIVE: &str = "ACTIVE";

/// What [`line`] answers for a build line that is neither ZooKeeper mode nor
/// KRaft: MSK publishes no newer member of such a line, and offers no upgrade off
/// it, so a source on one is terminal.
const OTHER_LINE: &str = "other";

/// `ListKafkaVersions`, which the model binds to `GET /v1/kafka-versions`.
async fn list_versions(emulator: &Emulator) -> Reply {
    emulator.call("GET", "/v1/kafka-versions", None).await
}

/// `GetCompatibleKafkaVersions` with no `ClusterArn` — the member is optional, and
/// omitting it asks for the whole matrix.
async fn matrix(emulator: &Emulator) -> Reply {
    emulator
        .call("GET", "/v1/compatible-kafka-versions", None)
        .await
}

/// The catalog as the wire reports it: the status published for each version,
/// which the catalog publishes once.
fn published(reply: &Reply) -> BTreeMap<String, String> {
    let mut catalog: BTreeMap<String, String> = BTreeMap::new();
    for entry in reply
        .at("/kafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a kafkaVersions list, got {}", reply.body))
    {
        let version = entry["version"]
            .as_str()
            .unwrap_or_else(|| panic!("an entry without a version: {entry}"))
            .to_string();
        let status = entry["status"]
            .as_str()
            .unwrap_or_else(|| panic!("an entry without a status: {entry}"))
            .to_string();
        assert!(
            catalog.insert(version.clone(), status).is_none(),
            "case version={version:?}: the catalog publishes a version once, got {}",
            reply.body
        );
    }
    catalog
}

/// The matrix as `(sourceVersion, targetVersions)` pairs, in the order reported.
fn rows(reply: &Reply) -> Vec<(String, Vec<String>)> {
    reply
        .at("/compatibleKafkaVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibleKafkaVersions list, got {}", reply.body))
        .iter()
        .map(|entry| {
            let source = entry["sourceVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry without a sourceVersion: {entry}"))
                .to_string();
            let targets = entry["targetVersions"]
                .as_array()
                .unwrap_or_else(|| panic!("an entry without targetVersions: {entry}"))
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

/// The `major.minor[.patch…]` release a version names, as the components that
/// order one release against another: a numeric component as itself, `x` ("the
/// newest patch of this minor") above every numbered patch, and the trailing
/// variant word dropped, because it names a build of the release.
fn release(version: &str) -> Vec<u32> {
    version
        .split('.')
        .filter_map(|part| match part.parse::<u32>() {
            Ok(number) => Some(number),
            Err(_) if part == "x" => Some(u32::MAX),
            Err(_) => None,
        })
        .collect()
}

/// The build line a version is on: `"kraft"` for the KRaft metadata-mode versions,
/// `""` for the ZooKeeper-mode ones — which the `.tiered` spelling of a release is
/// also on — and [`OTHER_LINE`] for any other build, which carries no upgrade path.
fn line(version: &str) -> &str {
    match version
        .rsplit('.')
        .next()
        .filter(|last| *last != "x" && last.parse::<u32>().is_err())
        .unwrap_or_default()
    {
        "" | "tiered" => "",
        "kraft" => "kraft",
        _ => OTHER_LINE,
    }
}

/// RULE: both read paths answer from one catalog — every source the matrix reports
/// is a version the listing publishes, every target is one the listing reports as
/// still offered, and the matrix covers every published entry.
#[tokio::test]
async fn the_matrix_is_derived_from_the_published_catalog() {
    let emulator = Emulator::new();
    let catalog = published(&list_versions(&emulator).await);
    let matrix = rows(&matrix(&emulator).await);

    assert!(
        !catalog.is_empty() && !matrix.is_empty(),
        "both read paths must answer something, got {} catalog entries and {} matrix rows",
        catalog.len(),
        matrix.len()
    );
    for (source, targets) in &matrix {
        assert!(
            catalog.contains_key(source),
            "case source={source:?}: a matrix source must be a published version, got {:?}",
            catalog.keys().collect::<Vec<_>>()
        );
        for target in targets {
            assert_eq!(
                catalog.get(target).map(String::as_str),
                Some(ACTIVE),
                "case source={source:?}: target {target:?} must be a version the catalog still \
                 offers, got {:?}",
                catalog.get(target)
            );
        }
    }
    // Every published version is a source the matrix answers for, so a client
    // choosing an upgrade finds the row for whatever its cluster runs.
    let sources: BTreeSet<&String> = matrix.iter().map(|(source, _)| source).collect();
    for version in catalog.keys() {
        assert!(
            sources.contains(version),
            "case version={version:?}: the matrix must answer for every published version, \
             got {sources:?}"
        );
    }
}

/// RULE: the matrix is a per-source upgrade GRAPH, not one blanket list: each
/// source's targets are the versions the catalog permits an upgrade to FROM it —
/// still offered, on that source's own build line and strictly newer, or the KRaft
/// version of the source's own release — so a terminal version reports an EMPTY
/// list and two sources on different lines are offered different things.
#[tokio::test]
async fn each_source_is_offered_only_the_upgrades_its_own_line_permits() {
    let emulator = Emulator::new();
    let catalog = published(&list_versions(&emulator).await);
    let matrix = rows(&matrix(&emulator).await);

    for (source, targets) in &matrix {
        // No self-upgrade, and no duplicate: a version is offered once or not at
        // all.
        let distinct: BTreeSet<&String> = targets.iter().collect();
        assert_eq!(
            distinct.len(),
            targets.len(),
            "case source={source:?}: a target must be offered once, got {targets:?}"
        );
        assert!(
            !targets.contains(source),
            "case source={source:?}: a version is not an upgrade of itself, got {targets:?}"
        );
        for target in targets {
            let same_line = line(target) == line(source) && line(source) != OTHER_LINE;
            let kraft_migration = line(source).is_empty()
                && line(target) == "kraft"
                && release(target) == release(source);
            assert!(
                same_line || kraft_migration,
                "case source={source:?}: target {target:?} is on another build line, or on a \
                 line with no upgrade path, so MSK does not offer it, got {targets:?}"
            );
            if same_line {
                assert!(
                    release(target) > release(source),
                    "case source={source:?}: target {target:?} is not a newer release, \
                     got {targets:?}"
                );
            }
        }
        // And every version the catalog permits is offered: the row is the whole
        // subset, not a sample of it.
        let permitted: BTreeSet<&String> = catalog
            .iter()
            .filter(|(_, status)| status.as_str() == ACTIVE)
            .map(|(version, _)| version)
            .filter(|version| {
                if line(version) == line(source) {
                    line(source) != OTHER_LINE && release(version) > release(source)
                } else {
                    line(source).is_empty()
                        && line(version) == "kraft"
                        && release(version) == release(source)
                }
            })
            .collect();
        assert_eq!(
            distinct, permitted,
            "case source={source:?}: expected exactly the permitted upgrades {permitted:?}, \
             got {targets:?}"
        );
    }

    // The graph really is per source: some source has nothing left to upgrade to,
    // and the rows do not all carry one shared list.
    assert!(
        matrix.iter().any(|(_, targets)| targets.is_empty()),
        "expected a terminal source with no upgrade target, got {matrix:?}"
    );
    let shapes: BTreeSet<&Vec<String>> = matrix.iter().map(|(_, targets)| targets).collect();
    assert!(
        shapes.len() > 1,
        "expected the matrix to offer different targets per source, got {matrix:?}"
    );
}

/// RULE: the catalog is published metadata, so it carries the variant lines the
/// service offers and both support statuses — a client can see which versions are
/// on the way out, and can select a KRaft version at all.
#[tokio::test]
async fn the_catalog_carries_the_variant_lines_and_both_statuses() {
    let emulator = Emulator::new();
    let catalog = published(&list_versions(&emulator).await);

    for status in [ACTIVE, "DEPRECATED"] {
        assert!(
            catalog.values().any(|served| served == status),
            "case status={status:?}: the catalog must publish it, got {catalog:?}"
        );
    }
    // Each build line the service publishes carries at least one entry, and the
    // KRaft line carries an offered one — a client that wants a KRaft cluster has
    // a version to ask for.
    let lines: BTreeSet<&str> = catalog.keys().map(|version| line(version)).collect();
    for expected in ["", "kraft"] {
        assert!(
            lines.contains(expected),
            "case line={expected:?}: the catalog must publish versions on it, got {lines:?}"
        );
    }
    assert!(
        catalog
            .iter()
            .any(|(version, status)| line(version) == "kraft" && status == ACTIVE),
        "the catalog must offer a KRaft version, got {catalog:?}"
    );
    // Every reported entry is one the published catalog holds, under a modelled
    // status.
    let listed = list_versions(&emulator).await;
    let entries = listed
        .at("/kafkaVersions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    for entry in &entries {
        let version = entry["version"].as_str().unwrap_or_default();
        assert!(
            catalog.contains_key(version),
            "case version={version:?}: every reported entry must be in the published catalog"
        );
        assert!(
            matches!(entry["status"].as_str(), Some(ACTIVE | "DEPRECATED")),
            "case version={version:?}: expected a modelled status, got {}",
            entry["status"]
        );
    }
    // Reading the catalog twice answers the same thing: it is published metadata,
    // not caller state.
    let again = published(&list_versions(&emulator).await);
    assert_eq!(
        again, catalog,
        "the catalog must not change because it was read"
    );
    assert_eq!(
        listed.at("/kafkaVersions"),
        &Value::Array(entries),
        "the listing must report the same entries on a second read"
    );
}
