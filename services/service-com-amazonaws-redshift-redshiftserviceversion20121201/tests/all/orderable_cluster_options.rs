// SPDX-License-Identifier: CC0-1.0

//! The orderable-option catalog `DescribeOrderableClusterOptions` reports, end to
//! end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The suite asserts the RULE rather than one recorded request: the catalog is
//! the product of the offered engine versions (the ones `DescribeClusterVersions`
//! reports) with the two documented valid-value lists (cluster type, node type),
//! each option carrying the Availability Zones of the Region the request was
//! signed for; `ClusterVersion` and `NodeType` SELECT from that product; the
//! shared `MaxRecords` window and `Marker` cursor paginate it; and reading it
//! changes nothing. The node-type, cluster-type and zone lists are written out
//! here, quoting the source they come from, so the suite pins the catalog
//! independently of the constants the handler reads.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::static_catalog::listed_zones;

/// Every node type the catalog offers.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="NodeType")` — "Valid Values: dc2.large | dc2.8xlarge | rg.large |
/// rg.xlarge | rg.4xlarge | rg.12xlarge | ra3.large | ra3.xlplus | ra3.4xlarge |
/// ra3.16xlarge".
const NODE_TYPES: [&str; 10] = [
    "dc2.large",
    "dc2.8xlarge",
    "rg.large",
    "rg.xlarge",
    "rg.4xlarge",
    "rg.12xlarge",
    "ra3.large",
    "ra3.xlplus",
    "ra3.4xlarge",
    "ra3.16xlarge",
];

/// Every cluster type the catalog offers.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterType")` — "Valid Values: multi-node | single-node".
const CLUSTER_TYPES: [&str; 2] = ["multi-node", "single-node"];

/// Every engine version the catalog offers.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="ClusterVersion")` — "Only version 1.0 is currently available". The
/// suite also checks this list against the seeded catalog
/// `DescribeClusterVersions` reports, which is where the input's "Must be one of
/// the version returned from DescribeClusterVersions" points.
const CLUSTER_VERSIONS: [&str; 1] = ["1.0"];

/// One option as the assertions compare it: version, node type, cluster type.
type Option3 = (String, String, String);

/// One row of the filter table: the case name, the request's filter parameters,
/// and the `ClusterVersion` / `NodeType` those parameters select by.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Option<&'static str>,
    Option<&'static str>,
);

/// Every orderable option a `DescribeOrderableClusterOptions` body carries, in
/// document order.
///
/// Each option serializes exactly one `ClusterVersion`, one `ClusterType` and
/// one `NodeType`, so the three element sequences line up index by index.
fn listed_options(answer: &Answer) -> Vec<Option3> {
    let versions = all_text(&answer.body, "ClusterVersion");
    let cluster_types = all_text(&answer.body, "ClusterType");
    let node_types = all_text(&answer.body, "NodeType");
    assert_eq!(
        (versions.len(), cluster_types.len()),
        (node_types.len(), node_types.len()),
        "an option carries one of each member: got {} versions, {} cluster types and {} node types in body {}",
        versions.len(),
        cluster_types.len(),
        node_types.len(),
        answer.body
    );
    versions
        .into_iter()
        .zip(cluster_types)
        .zip(node_types)
        .map(|((version, cluster_type), node_type)| (version, node_type, cluster_type))
        .collect()
}

/// The options one `DescribeOrderableClusterOptions` call reports, asserting the
/// call succeeded.
async fn orderable(emulator: &Emulator, case: &str, params: &[(&str, &str)]) -> Vec<Option3> {
    let answer = emulator
        .call("DescribeOrderableClusterOptions", params)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    listed_options(&answer)
}

/// The whole catalog the rule describes: the product of the offered versions and
/// the two documented lists, nested version -> node type -> cluster type.
fn expected_catalog() -> Vec<Option3> {
    let mut expected = Vec::new();
    for version in CLUSTER_VERSIONS {
        for node_type in NODE_TYPES {
            for cluster_type in CLUSTER_TYPES {
                expected.push((
                    version.to_string(),
                    node_type.to_string(),
                    cluster_type.to_string(),
                ));
            }
        }
    }
    expected
}

/// The catalog narrowed by the same filters a request carries, which is what the
/// response must equal.
fn expected_filtered(version: Option<&str>, node_type: Option<&str>) -> Vec<Option3> {
    expected_catalog()
        .into_iter()
        .filter(|(catalog_version, catalog_node_type, _)| {
            version.is_none_or(|wanted| wanted == catalog_version)
                && node_type.is_none_or(|wanted| wanted == catalog_node_type)
        })
        .collect()
}

#[tokio::test]
async fn the_catalog_is_the_product_of_the_offered_versions_and_documented_lists() {
    let emulator = Emulator::new();

    let served = orderable(&emulator, "the whole catalog", &[]).await;
    let expected = expected_catalog();
    assert_eq!(
        served, expected,
        "case the whole catalog: expected every offered version/node-type/cluster-type combination, got {served:?}"
    );

    // The node-type and cluster-type constants the handler reads must be the
    // documented lists this suite pins.
    assert_eq!(
        (
            state::ORDERABLE_NODE_TYPES.to_vec(),
            state::ORDERABLE_CLUSTER_TYPES.to_vec()
        ),
        (NODE_TYPES.to_vec(), CLUSTER_TYPES.to_vec()),
        "case catalog constants: the handler's lists must match the documented ones"
    );

    // "Constraints: Must be one of the version returned from
    // DescribeClusterVersions" — the versions an option carries come from the one
    // seeded catalog that operation reports, so the two cannot disagree.
    let offered: Vec<String> = state::published_cluster_versions()
        .iter()
        .map(|version| version.cluster_version.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        offered,
        CLUSTER_VERSIONS.map(str::to_string).to_vec(),
        "case the offered versions: expected the documented engine versions, got {offered:?}"
    );
    let in_options: Vec<String> = served
        .iter()
        .map(|(version, _, _)| version.clone())
        .collect();
    assert!(
        in_options.iter().all(|version| offered.contains(version)),
        "case every option's version: expected each of {in_options:?} to be one the service offers ({offered:?})"
    );
}

#[tokio::test]
async fn every_filter_selects_from_the_catalog() {
    let emulator = Emulator::new();

    // Rows differ along which filter is present, which value it carries, and
    // whether that value is in the catalog at all — including node types the
    // issue never cited and a value differing only in case.
    let cases: Vec<FilterCase> = vec![
        ("no filter", vec![], None, None),
        (
            "the smallest node type",
            vec![("NodeType", "dc2.large")],
            None,
            Some("dc2.large"),
        ),
        (
            "a node type the issue did not cite",
            vec![("NodeType", "rg.4xlarge")],
            None,
            Some("rg.4xlarge"),
        ),
        (
            "the largest node type",
            vec![("NodeType", "ra3.16xlarge")],
            None,
            Some("ra3.16xlarge"),
        ),
        (
            "the only engine version",
            vec![("ClusterVersion", "1.0")],
            Some("1.0"),
            None,
        ),
        (
            "both filters together",
            vec![("ClusterVersion", "1.0"), ("NodeType", "ra3.xlplus")],
            Some("1.0"),
            Some("ra3.xlplus"),
        ),
        (
            "a node type nobody offers",
            vec![("NodeType", "zz.enormous")],
            None,
            Some("zz.enormous"),
        ),
        (
            "an engine version nobody offers",
            vec![("ClusterVersion", "2.0")],
            Some("2.0"),
            None,
        ),
        (
            "a node type differing only in case",
            vec![("NodeType", "RA3.XLPLUS")],
            None,
            Some("RA3.XLPLUS"),
        ),
        (
            "filters that agree on nothing",
            vec![("ClusterVersion", "2.0"), ("NodeType", "ra3.large")],
            Some("2.0"),
            Some("ra3.large"),
        ),
    ];

    for (case, params, version, node_type) in cases {
        let served = orderable(&emulator, case, &params).await;
        let expected = expected_filtered(version, node_type);
        assert_eq!(
            served, expected,
            "case {case}: expected {} option(s) selected from the catalog, got {served:?}",
            expected.len()
        );
    }
}

#[tokio::test]
async fn the_availability_zones_are_the_zones_of_the_signed_region() {
    let emulator = Emulator::new();

    // A Region's zones are `<region>` plus consecutive letters from `a`, as many
    // as that Region publishes — four for Oregon, three for a Region AWS
    // publishes the smallest count for. Written out here rather than taken from
    // the production helper, so a wrong zone list fails this suite; the whole
    // per-Region table and the platforms each zone advertises are pinned in the
    // `static_catalog` suite.
    const REGION_ZONES: [(&str, &[&str]); 3] = [
        ("us-west-2", &["us-west-2a", "us-west-2b", "us-west-2c", "us-west-2d"]),
        ("eu-central-1", &["eu-central-1a", "eu-central-1b", "eu-central-1c"]),
        (
            "ap-southeast-1",
            &["ap-southeast-1a", "ap-southeast-1b", "ap-southeast-1c"],
        ),
    ];

    for (region, zones) in REGION_ZONES {
        let answer = emulator
            .call_in_region(
                region,
                "DescribeOrderableClusterOptions",
                &[("NodeType", "ra3.large")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case zones in {region}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let options = listed_options(&answer);
        assert_eq!(
            options.len(),
            CLUSTER_TYPES.len(),
            "case zones in {region}: expected one option per cluster type, got {options:?}"
        );
        let expected: Vec<String> = zones.iter().map(|zone| zone.to_string()).collect();
        assert_eq!(
            state::region_availability_zones(region),
            expected,
            "case zones in {region}: the handler's zone helper must report {expected:?}"
        );
        // Each option carries the Region's whole zone list, in order, and every
        // zone carries the platforms a cluster in it can run on.
        let repeated: Vec<String> = std::iter::repeat_n(expected.clone(), options.len())
            .flatten()
            .collect();
        let served: Vec<String> = listed_zones(&answer.body)
            .into_iter()
            .map(|(name, platforms)| {
                assert_eq!(
                    platforms,
                    vec!["VPC".to_string()],
                    "case zone {name}: expected the VPC platform, got {platforms:?}"
                );
                name
            })
            .collect();
        assert_eq!(
            served, repeated,
            "case zones in {region}: expected each of the {} option(s) to carry {expected:?}, got {served:?}",
            options.len()
        );
    }
}

#[tokio::test]
async fn the_documented_max_records_window_and_marker_cursor_govern_the_page() {
    let emulator = Emulator::new();

    // "Default: 100 Constraints: minimum 20, maximum 100." — each published
    // bound, and one past each; only the minimum is enforced.
    for (case, max_records, accepted) in [
        ("one below the minimum", state::MAX_RECORDS_MIN - 1, false),
        ("at the minimum", state::MAX_RECORDS_MIN, true),
        ("inside the window", state::MAX_RECORDS_MIN + 1, true),
        ("at the maximum", state::MAX_RECORDS_MAX, true),
        ("one above the maximum", state::MAX_RECORDS_MAX + 1, true),
        ("zero", 0, false),
    ] {
        let value = max_records.to_string();
        let answer = emulator
            .call(
                "DescribeOrderableClusterOptions",
                &[("MaxRecords", &value)],
            )
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case MaxRecords {case} ({value}): expected accepted={accepted}, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
        if !accepted {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case MaxRecords {case} ({value}): expected a 400 refusal, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // The catalog is no longer than the smallest page the window allows, so a
    // page is never truncated and no `Marker` is handed out.
    let total = expected_catalog().len();
    assert!(
        total <= state::MAX_RECORDS_MIN as usize,
        "case page size: the catalog ({total} options) must fit the minimum page ({})",
        state::MAX_RECORDS_MIN
    );
    let smallest_page = state::MAX_RECORDS_MIN.to_string();
    let answer = emulator
        .call(
            "DescribeOrderableClusterOptions",
            &[("MaxRecords", &smallest_page)],
        )
        .await;
    assert_eq!(
        listed_options(&answer).len(),
        total,
        "case smallest page: expected the whole catalog, got {:?}",
        listed_options(&answer)
    );
    assert_eq!(
        answer.text("Marker"),
        None,
        "case smallest page: an untruncated page returns no Marker, got {:?}",
        answer.text("Marker")
    );

    // A cursor nobody handed out is refused rather than served as an offset.
    for (case, cursor) in [
        ("not a number", "not-a-cursor"),
        ("an invented offset", "7"),
        ("an offset with a forged tag", "20.00000000"),
    ] {
        let bogus = emulator
            .call("DescribeOrderableClusterOptions", &[("Marker", cursor)])
            .await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case marker {case} ({cursor:?}): expected a 400 refusal, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    let first = orderable(&emulator, "first read", &[]).await;
    let second = orderable(&emulator, "second read", &[]).await;
    assert_eq!(
        first, second,
        "case repeated read: the catalog is stable across reads, got {second:?}"
    );

    // What the account holds does not change what is orderable, and reading what
    // is orderable does not touch what the account holds.
    let created = emulator.create_minimal("orderable-probe").await;
    assert_eq!(
        created.status, 200,
        "case seed a cluster: expected 200, got {} with body {}",
        created.status, created.body
    );

    let after_create = orderable(&emulator, "read after a create", &[]).await;
    assert_eq!(
        after_create, first,
        "case read after a create: an account's clusters do not change the catalog, got {after_create:?}"
    );

    let described = emulator.call("DescribeClusters", &[]).await;
    assert_eq!(
        all_text(&described.body, "ClusterIdentifier"),
        vec!["orderable-probe".to_string()],
        "case clusters after the catalog reads: expected the one seeded cluster, got body {}",
        described.body
    );
}
