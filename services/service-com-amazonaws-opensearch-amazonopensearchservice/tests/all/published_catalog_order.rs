// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER and the SHAPE in which this service publishes its
//! reference catalogs: `ListVersions`, `GetCompatibleVersions`,
//! `DescribeInstanceTypeLimits` and `DescribeReservedInstanceOfferings`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the per-operation calls from the suites that own
//! those operations ([`super::compatible_versions`], [`super::instance_type_limits`],
//! [`super::reserved_instance_offerings`]); this suite adds no second copy of them.
//!
//! The rules under test, each over a SET of inputs rather than one recorded call:
//!
//! * `ListVersions` publishes the supported engine releases NEWEST FIRST: the answer
//!   descends by engine line, then major, then minor, so the current release leads and
//!   the oldest Elasticsearch release is last;
//! * `GetCompatibleVersions` publishes an upgrade path that walks ONE release line and
//!   leaves it only from its end: a source's targets are the later releases of its own
//!   line, plus — from the newest release of that line only — every release of the line
//!   that immediately succeeds it, and nothing two lines ahead — over a table of sources
//!   spanning both engines, a line with later minors, a release at the end of its line,
//!   the engine-line crossing and the newest release;
//! * `DescribeInstanceTypeLimits` publishes each storage limit PAIR ceiling before
//!   floor, over a table of volume sub-types and instance sizes, and the ceiling value
//!   really is the larger of the pair;
//! * `DescribeReservedInstanceOfferings` publishes ONE hourly recurring charge per
//!   offering whatever its payment option, zero exactly where the term is paid up
//!   front, over the whole catalog.

use serde_json::Value;

use super::compatible_versions::{compatible, entries, line, rank, target_versions};
use super::domain_lifecycle::{string_member, Emulator, PREFIX};
use super::instance_type_limits::{describe_limits, limits_by_role};
use super::reserved_instance_offerings::whole_catalog;
use super::upgrade_history::settled_domain;

/// The engine release the limit rows below are asked on: one that offers every family
/// they name.
const LIMITS_VERSION: &str = "OpenSearch_2.19";

/// The payment option whose whole term is paid up front, so its recurring charge is a
/// zero amount. Source: `ReservedInstancePaymentOption`
/// (`smithy_query(query="shape",shape="ReservedInstancePaymentOption")`).
const ALL_UPFRONT: &str = "ALL_UPFRONT";

/// `ListVersions`: `GET /2021-01-01/opensearch/versions`.
async fn published_versions(emulator: &Emulator) -> Vec<String> {
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/versions"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case the published catalog: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Versions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case the published catalog: expected a Versions array, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case the published catalog: expected a string version, got {value:?}")
                })
                .to_owned()
        })
        .collect()
}

/// The `LimitName`s of one storage type, in the order the answer publishes them.
fn limit_names(storage_type: &Value) -> Vec<String> {
    storage_type["StorageTypeLimits"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|limit| limit["LimitName"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The single value of the `index`th limit of one storage type.
fn limit_value(storage_type: &Value, index: usize, case: &str) -> i64 {
    storage_type["StorageTypeLimits"][index]["LimitValues"][0]
        .as_str()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a numeric limit value at {index}, got {:?}",
                storage_type["StorageTypeLimits"]
            )
        })
}

#[tokio::test]
async fn the_engine_version_catalog_is_published_newest_first() {
    let emulator = Emulator::new();
    let catalog = published_versions(&emulator).await;
    assert!(
        catalog.len() >= 4,
        "case catalog size: these assertions need several published releases, got \
         {catalog:?}"
    );

    // Strictly descending: each entry ranks above the one after it, so the current
    // release leads and no release appears twice.
    for pair in catalog.windows(2) {
        assert!(
            rank(&pair[0]) > rank(&pair[1]),
            "case {} then {}: expected the catalog newest first, got ranks {:?} and {:?}",
            pair[0],
            pair[1],
            rank(&pair[0]),
            rank(&pair[1])
        );
    }

    // The newest release of the whole catalog leads it, and the oldest closes it —
    // computed from the set rather than named, so the rule is about the ORDER and not
    // about which releases are published.
    let newest = catalog
        .iter()
        .max_by_key(|version| rank(version))
        .expect("the catalog is not empty");
    let oldest = catalog
        .iter()
        .min_by_key(|version| rank(version))
        .expect("the catalog is not empty");
    assert_eq!(
        catalog.first(),
        Some(newest),
        "case the leading entry: expected the newest published release, got {:?}",
        catalog.first()
    );
    assert_eq!(
        catalog.last(),
        Some(oldest),
        "case the last entry: expected the oldest published release, got {:?}",
        catalog.last()
    );

    // The successor engine line leads the one it succeeded: every OpenSearch release
    // precedes every Elasticsearch release.
    let last_opensearch = catalog
        .iter()
        .rposition(|version| version.starts_with("OpenSearch_"));
    let first_elasticsearch = catalog
        .iter()
        .position(|version| version.starts_with("Elasticsearch_"));
    assert!(
        matches!(
            (last_opensearch, first_elasticsearch),
            (Some(last), Some(first)) if last < first
        ),
        "case the engine lines: expected the OpenSearch line to lead the Elasticsearch \
         one, got {catalog:?}"
    );

    // Several minors of the newest major are published, so "newest first" is a
    // statement about minors as numbers and not only about majors.
    let (newest_generation, newest_major, _) = rank(newest);
    let newest_line: Vec<&String> = catalog
        .iter()
        .filter(|version| {
            let (generation, major, _) = rank(version);
            (generation, major) == (newest_generation, newest_major)
        })
        .collect();
    assert!(
        newest_line.len() >= 2,
        "case the newest release line: expected several minors published, got \
         {newest_line:?}"
    );
}

#[tokio::test]
async fn the_upgrade_path_steps_one_release_line_at_a_time() {
    let emulator = Emulator::new();
    let catalog = published_versions(&emulator).await;

    // The suite's OWN statement of the rule, as the targets each source publishes: a
    // source reaches the later releases of its own line, and the whole of the next line
    // only when it is the newest release of its own. One row per shape the rule has: a
    // source in the middle of a line whose line runs on (Elasticsearch 7.7 and
    // Elasticsearch 7.10, both of which reach the OpenSearch 1 releases because those
    // continue the Elasticsearch 7 line), a release at the END of its line, which is
    // where the path leaves it (OpenSearch 1.3), a source in the middle of a line that
    // reaches no further line (OpenSearch 1.0), and the newest release, which has nowhere
    // to go. [`super::reference_catalogs`] holds the rule over a wider table of sources,
    // including the end of an Elasticsearch line.
    let rows: [(&str, &[&str]); 5] = [
        (
            "Elasticsearch_7.7",
            &[
                "Elasticsearch_7.8",
                "Elasticsearch_7.9",
                "Elasticsearch_7.10",
                "OpenSearch_1.0",
                "OpenSearch_1.1",
                "OpenSearch_1.2",
                "OpenSearch_1.3",
            ],
        ),
        (
            "Elasticsearch_7.10",
            &[
                "OpenSearch_1.0",
                "OpenSearch_1.1",
                "OpenSearch_1.2",
                "OpenSearch_1.3",
            ],
        ),
        (
            "OpenSearch_1.0",
            &["OpenSearch_1.1", "OpenSearch_1.2", "OpenSearch_1.3"],
        ),
        (
            "OpenSearch_1.3",
            &[
                "OpenSearch_2.3",
                "OpenSearch_2.5",
                "OpenSearch_2.7",
                "OpenSearch_2.9",
                "OpenSearch_2.11",
                "OpenSearch_2.13",
                "OpenSearch_2.15",
                "OpenSearch_2.17",
                "OpenSearch_2.19",
            ],
        ),
        (
            catalog.first().expect("the catalog is not empty").as_str(),
            &[],
        ),
    ];

    for (index, (source, expected)) in rows.iter().enumerate() {
        // A domain running the source version is how one source is asked about: the
        // filtered answer reports that version's row.
        let name = format!("upgrade-line-{index}");
        settled_domain(&emulator, &name, source).await;
        let answer = compatible(&emulator, Some(&name)).await;
        let rows_for_domain = entries(&answer, source);
        assert_eq!(
            rows_for_domain.len(),
            1,
            "case {source}: naming a domain limits the answer to its one version, got \
             {rows_for_domain:?}"
        );
        let targets = target_versions(&rows_for_domain[0], source);
        assert_eq!(
            targets,
            expected.iter().map(|v| (*v).to_string()).collect::<Vec<_>>(),
            "case {source}: expected the later releases of its own line and the whole of \
             the next line, got {targets:?}"
        );

        // No target is more than one release line ahead of its source: a domain crosses
        // one line boundary per upgrade, the Elasticsearch 7 and OpenSearch 1 releases
        // being one line.
        let source_line = line(source);
        let lines_ahead = |target: &str| -> usize {
            let target_line = line(target);
            catalog
                .iter()
                .map(|version| line(version))
                .filter(|candidate| *candidate > source_line && *candidate <= target_line)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        };
        for target in &targets {
            assert!(
                lines_ahead(target) <= 1,
                "case {source} -> {target}: expected a target at most one release line \
                 ahead, got {} lines ahead",
                lines_ahead(target)
            );
        }
    }

    // The published table itself starts at the oldest release the service still offers
    // an upgrade from, and every row it carries leads somewhere.
    let table = entries(&compatible(&emulator, None).await, "published table");
    for entry in &table {
        let source = string_member(entry, "SourceVersion", "published table");
        assert!(
            !target_versions(entry, &source).is_empty(),
            "case {source}: expected every listed source to have a target, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn storage_limit_pairs_are_published_ceiling_before_floor() {
    let emulator = Emulator::new();

    // The limit names each volume sub-type publishes, in order. Source:
    // `StorageTypeLimit$LimitName`'s own documentation — a volume-size pair for every
    // sub-type, an IOPS pair where IOPS are provisioned per volume, and a throughput
    // pair for the sub-type whose throughput is. Each pair is published ceiling first.
    let expected: [(&str, &[&str]); 4] = [
        ("standard", &["MaximumVolumeSize", "MinimumVolumeSize"]),
        ("gp2", &["MaximumVolumeSize", "MinimumVolumeSize"]),
        (
            "io1",
            &[
                "MaximumVolumeSize",
                "MinimumVolumeSize",
                "MaximumIops",
                "MinimumIops",
            ],
        ),
        (
            "gp3",
            &[
                "MaximumVolumeSize",
                "MinimumVolumeSize",
                "MaximumIops",
                "MinimumIops",
                "MaximumThroughput",
                "MinimumThroughput",
            ],
        ),
    ];

    // One row per instance size class, so the ordering is not one type's: the smallest
    // published size, a size the scale differentiates, the largest, and a
    // previous-generation family, which is the one that publishes the magnetic and
    // provisioned-IOPS sub-types too.
    for instance_type in [
        "t3.nano.search",
        "m5.large.search",
        "m5.2xlarge.search",
        "m5.24xlarge.search",
        "m4.2xlarge.search",
    ] {
        let case = format!("{LIMITS_VERSION} {instance_type}");
        let map = limits_by_role(
            &describe_limits(&emulator, LIMITS_VERSION, instance_type, "").await,
            &case,
        );
        // The data role is the one a volume attaches to, and the published counts are
        // the role's own.
        let counts = &map["data"]["InstanceLimits"]["InstanceCountLimits"];
        assert_eq!(
            counts["MinimumInstanceCount"].as_i64(),
            Some(1),
            "case {case}: expected the published data-node floor, got {counts:?}"
        );
        assert_eq!(
            counts["MaximumInstanceCount"].as_i64(),
            Some(20),
            "case {case}: expected the published data-node ceiling, got {counts:?}"
        );

        // A volume attaches to the domain's data nodes, so the storage limits are the
        // data role's and the master role publishes none at all.
        assert!(
            map["master"]["StorageTypes"].is_null(),
            "case {case}: expected a dedicated master to publish no storage type, got \
             {:?}",
            map["master"]
        );

        // The storage limits are the data role's, so that is the block the ordering is
        // read from.
        let storage = map["data"]["StorageTypes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !storage.is_empty(),
            "case {case}: expected the data role to publish its storage types, got {:?}",
            map["data"]
        );
        for entry in &storage {
            let sub_type = entry["StorageSubTypeName"].as_str().unwrap_or_default();
            let case = format!("{case} {sub_type}");
            let (_, expected_names) = expected
                .iter()
                .find(|(name, _)| *name == sub_type)
                .unwrap_or_else(|| panic!("case {case}: unexpected storage sub-type"));
            assert_eq!(
                limit_names(entry),
                expected_names
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>(),
                "case {case}: expected each limit pair published ceiling before floor, \
                 got {entry:?}"
            );
            // The names are not merely swapped labels: the value published first is
            // the larger one of each pair.
            for pair in 0..expected_names.len() / 2 {
                let ceiling = limit_value(entry, pair * 2, &case);
                let floor = limit_value(entry, pair * 2 + 1, &case);
                assert!(
                    ceiling >= floor,
                    "case {case}: expected the ceiling of pair {pair} to be the larger \
                     value, got {ceiling} before {floor}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_offering_publishes_one_hourly_recurring_charge() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, "recurring charges").await;
    assert!(
        !catalog.is_empty(),
        "case the offering catalog: expected the published offerings, got none"
    );

    let mut prepaid = 0_usize;
    let mut billed = 0_usize;
    for offering in &catalog {
        let instance_type = string_member(offering, "InstanceType", "offering");
        let payment_option = string_member(offering, "PaymentOption", &instance_type);
        let duration = offering["Duration"].as_i64().unwrap_or_default();
        let case = format!("{instance_type} / {duration}s / {payment_option}");

        let charges = offering["RecurringCharges"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a RecurringCharges array"));
        assert_eq!(
            charges.len(),
            1,
            "case {case}: expected the one recurring charge every offering publishes, got \
             {charges:?}"
        );
        assert_eq!(
            charges[0]["RecurringChargeFrequency"].as_str(),
            Some("Hourly"),
            "case {case}: expected an hourly recurring charge, got {:?}",
            charges[0]
        );
        let amount = charges[0]["RecurringChargeAmount"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a numeric charge amount"));
        if payment_option == ALL_UPFRONT {
            prepaid += 1;
            assert!(
                amount.abs() < f64::EPSILON,
                "case {case}: a term paid up front leaves a zero hourly charge, got {amount}"
            );
        } else {
            billed += 1;
            assert!(
                amount > 0.0,
                "case {case}: a term billed hourly charges something, got {amount}"
            );
        }
    }

    // The catalog really spans both sides of the rule, so neither branch above went
    // untested.
    assert!(
        prepaid > 0 && billed > 0,
        "case the offering catalog: expected prepaid and hourly-billed offerings, got \
         {prepaid} and {billed}"
    );
}
