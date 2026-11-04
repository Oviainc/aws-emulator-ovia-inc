// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDomainHealth`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test is not one recorded call: `DescribeDomainHealth` reports a
//! projection of the stored domain record, where every CONFIGURED count comes from
//! the `ClusterConfig` member that defines it (and is the modelled `NotAvailable`
//! when the record holds no value for it), and every OBSERVED value — the zones
//! serving traffic, the elected master, the cluster health and the shard counts — is
//! reportable only once the cluster the clock owes the domain has come up.
//!
//! So the tests span the configurations the rule generalises over: a table of eleven
//! cluster shapes (single-zone, two and three zones — the published bounds of
//! `ZoneAwarenessConfig$AvailabilityZoneCount` — a standby zone, warm nodes,
//! dedicated masters, a domain with no cluster configuration at all, one whose zone
//! awareness names no zone count, and three whose count falls outside those bounds:
//! one below, one above and the largest a caller can send), the two provisioning
//! regimes (`Creating` and a settled domain with an update in flight), and a sweep
//! over node-and-zone combinations asserting the invariants the spread must satisfy
//! for every one of them.
//!
//! A name no domain is stored under is a second rule of its own: this operation
//! refuses it with the service's GENERIC envelope — the modelled `BaseException`,
//! HTTP 400, saying what was not found — asserted below over a set of such names
//! rather than over one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// The Region the test requests are signed for — the one the domains' Availability
/// Zones are named from.
const REGION: &str = "us-east-1";

/// `GET /2021-01-01/opensearch/domain/{DomainName}/health`.
async fn health(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{name}/health"), None)
        .await
}

/// `CreateDomain` for `name`, with `cluster` as its `ClusterConfig` when it is not
/// `Value::Null`, asserting the create was accepted.
async fn create_domain(emulator: &Emulator, case: &str, name: &str, cluster: &Value) {
    let mut body = json!({ "DomainName": name });
    if !cluster.is_null() {
        body["ClusterConfig"] = cluster.clone();
    }
    let answer = emulator.create(body).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected CreateDomain({name:?}) to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The health of `name`, asserting the call was answered.
async fn health_body(emulator: &Emulator, case: &str, name: &str) -> Value {
    let answer = health(emulator, name).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeDomainHealth({name:?}) to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// Whether a value satisfies the pattern every count member of this response carries
/// — `NumberOfAZs`, `NumberOfNodes` and `NumberOfShards` are all
/// `^((\d+)|(NotAvailable))$` (smithy_query query="shape" shape="NumberOfNodes").
fn is_modelled_count(value: &str) -> bool {
    value == NOT_AVAILABLE || (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The modelled "unable to fetch this information" value of every count member, and
/// the `NotAvailable` variant of `DomainHealth` and `ZoneStatus`.
const NOT_AVAILABLE: &str = "NotAvailable";

/// Every count member of the response and of its per-zone information, so one
/// assertion can hold the whole pattern constraint.
const COUNT_MEMBERS: &[&str] = &[
    "AvailabilityZoneCount",
    "ActiveAvailabilityZoneCount",
    "StandByAvailabilityZoneCount",
    "DataNodeCount",
    "MasterEligibleNodeCount",
    "WarmNodeCount",
    "TotalShards",
    "TotalUnAssignedShards",
];

/// The per-zone count members of `AvailabilityZoneInfo`.
const ZONE_COUNT_MEMBERS: &[&str] = &[
    "ConfiguredDataNodeCount",
    "AvailableDataNodeCount",
    "TotalShards",
    "TotalUnAssignedShards",
];

/// The `AvailabilityZoneInformation` of the single `EnvironmentInfo` a health answer
/// carries, or an empty list when it carries none.
fn zone_information(body: &Value) -> Vec<Value> {
    body["EnvironmentInformation"]
        .as_array()
        .and_then(|environments| environments.first())
        .and_then(|environment| environment["AvailabilityZoneInformation"].as_array())
        .cloned()
        .unwrap_or_default()
}

/// Assert every count the answer reports — response-level and per zone — satisfies
/// its modelled pattern.
fn assert_counts_are_modelled(case: &str, body: &Value) {
    for member in COUNT_MEMBERS {
        let value = string_member(body, member, case);
        assert!(
            is_modelled_count(&value),
            "case {case}: expected {member} to match ^((\\d+)|(NotAvailable))$, got {value:?}"
        );
    }
    for zone in zone_information(body) {
        for member in ZONE_COUNT_MEMBERS {
            let value = string_member(&zone, member, case);
            assert!(
                is_modelled_count(&value),
                "case {case}: expected zone {member} to match \
                 ^((\\d+)|(NotAvailable))$, got {value:?}"
            );
        }
    }
}

/// One expected zone: the name it is reported under, its `ZoneStatus` and the data
/// nodes configured in it.
struct ExpectedZone {
    name: &'static str,
    status: &'static str,
    configured_nodes: &'static str,
}

/// One row of the cluster-shape table: a `ClusterConfig` to create a domain with, and
/// the health a settled domain of that shape reports.
struct Row {
    case: &'static str,
    cluster: Value,
    zone_count: &'static str,
    active_zone_count: &'static str,
    standby_zone_count: &'static str,
    data_node_count: &'static str,
    dedicated_master: bool,
    master_eligible_node_count: &'static str,
    warm_node_count: &'static str,
    zones: Vec<ExpectedZone>,
}

fn zone(name: &'static str, status: &'static str, configured_nodes: &'static str) -> ExpectedZone {
    ExpectedZone {
        name,
        status,
        configured_nodes,
    }
}

/// The cluster shapes the rule has to hold over. The rows differ along the members
/// that define each count — the number of zones, whether one of them is a standby,
/// the data-node count and how evenly it divides, dedicated masters, warm nodes — and
/// two of them along the member being ABSENT, which is what makes a count the
/// modelled `NotAvailable` rather than an invented default.
fn cluster_shapes() -> Vec<Row> {
    vec![
        Row {
            case: "single-zone, two data nodes",
            cluster: json!({ "InstanceType": "r6g.large.search", "InstanceCount": 2 }),
            zone_count: "1",
            active_zone_count: "1",
            standby_zone_count: "0",
            data_node_count: "2",
            dedicated_master: false,
            // No dedicated master node, so every data node is master-eligible.
            master_eligible_node_count: "2",
            warm_node_count: "0",
            zones: vec![zone("us-east-1a", "Active", "2")],
        },
        Row {
            case: "two zones, dedicated masters",
            cluster: json!({
                "InstanceCount": 4,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
            }),
            zone_count: "2",
            active_zone_count: "2",
            standby_zone_count: "0",
            data_node_count: "4",
            dedicated_master: true,
            master_eligible_node_count: "3",
            warm_node_count: "0",
            zones: vec![
                zone("us-east-1a", "Active", "2"),
                zone("us-east-1b", "Active", "2"),
            ],
        },
        Row {
            case: "three zones with a standby, warm nodes",
            cluster: json!({
                "InstanceCount": 6,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
                "MultiAZWithStandbyEnabled": true,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
                "WarmEnabled": true,
                "WarmCount": 2,
            }),
            zone_count: "3",
            // One of the three zones is the standby, so two serve traffic.
            active_zone_count: "2",
            standby_zone_count: "1",
            data_node_count: "6",
            dedicated_master: true,
            master_eligible_node_count: "3",
            warm_node_count: "2",
            zones: vec![
                zone("us-east-1a", "Active", "2"),
                zone("us-east-1b", "Active", "2"),
                zone("us-east-1c", "StandBy", "2"),
            ],
        },
        Row {
            case: "no cluster configuration at all",
            cluster: Value::Null,
            // Zone awareness off (by absence) is one zone, which is a count, not an
            // unknown; the node counts the record does not carry are not.
            zone_count: "1",
            active_zone_count: "1",
            standby_zone_count: "0",
            data_node_count: NOT_AVAILABLE,
            dedicated_master: false,
            master_eligible_node_count: NOT_AVAILABLE,
            warm_node_count: "0",
            zones: vec![zone("us-east-1a", "Active", NOT_AVAILABLE)],
        },
        Row {
            case: "zone awareness without a zone count",
            cluster: json!({ "InstanceCount": 2, "ZoneAwarenessEnabled": true }),
            zone_count: NOT_AVAILABLE,
            active_zone_count: NOT_AVAILABLE,
            standby_zone_count: "0",
            data_node_count: "2",
            dedicated_master: false,
            master_eligible_node_count: "2",
            warm_node_count: "0",
            // The configuration names no number of zones, so there is no zone
            // information to report.
            zones: vec![],
        },
        Row {
            case: "seven data nodes over three zones",
            cluster: json!({
                "InstanceCount": 7,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
            }),
            zone_count: "3",
            active_zone_count: "3",
            standby_zone_count: "0",
            data_node_count: "7",
            dedicated_master: false,
            master_eligible_node_count: "7",
            warm_node_count: "0",
            // An uneven count spreads as evenly as it can: the remainder goes to the
            // first zones.
            zones: vec![
                zone("us-east-1a", "Active", "3"),
                zone("us-east-1b", "Active", "2"),
                zone("us-east-1c", "Active", "2"),
            ],
        },
        Row {
            case: "warm storage on without a warm count",
            cluster: json!({
                "InstanceCount": 2,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
                "WarmEnabled": true,
            }),
            zone_count: "1",
            active_zone_count: "1",
            standby_zone_count: "0",
            data_node_count: "2",
            dedicated_master: true,
            master_eligible_node_count: "3",
            warm_node_count: NOT_AVAILABLE,
            zones: vec![zone("us-east-1a", "Active", "2")],
        },
        Row {
            case: "dedicated masters on without a master count",
            cluster: json!({ "InstanceCount": 2, "DedicatedMasterEnabled": true }),
            zone_count: "1",
            active_zone_count: "1",
            standby_zone_count: "0",
            data_node_count: "2",
            dedicated_master: true,
            // Only the dedicated master nodes are master-eligible, and the record
            // carries no count of them.
            master_eligible_node_count: NOT_AVAILABLE,
            warm_node_count: "0",
            zones: vec![zone("us-east-1a", "Active", "2")],
        },
    ]
    .into_iter()
    .chain(out_of_range_zone_counts())
    .collect()
}

/// A zone count past each published bound of `ZoneAwarenessConfig$AvailabilityZoneCount`
/// ("Valid values are `2` and `3`"), and the largest one a caller can even send.
///
/// None of them names a supported number of zones, so each reports the modelled
/// `NotAvailable` and no zone information — which is also what keeps the answer
/// bounded, since the count is a request field the model puts no range on.
fn out_of_range_zone_counts() -> Vec<Row> {
    [
        ("one zone", 1),
        ("four zones", 4),
        ("the largest integer count", i32::MAX),
    ]
    .into_iter()
    .map(|(case, zones)| Row {
        case,
        cluster: json!({
            "InstanceCount": 2,
            "ZoneAwarenessEnabled": true,
            "ZoneAwarenessConfig": { "AvailabilityZoneCount": zones },
        }),
        zone_count: NOT_AVAILABLE,
        active_zone_count: NOT_AVAILABLE,
        standby_zone_count: "0",
        data_node_count: "2",
        dedicated_master: false,
        master_eligible_node_count: "2",
        warm_node_count: "0",
        zones: vec![],
    })
    .collect()
}

#[tokio::test]
async fn health_reports_the_configured_cluster_of_every_domain_shape() {
    let emulator = Emulator::new();
    let rows = cluster_shapes();

    for (index, row) in rows.iter().enumerate() {
        let name = format!("shape-{index}");
        create_domain(&emulator, row.case, &name, &row.cluster).await;
    }
    // One wait for every domain: the provisioning each create started falls due at
    // the same span, so by now every one of them is settled.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("shape-{index}");
        let body = health_body(&emulator, case, &name).await;
        assert_counts_are_modelled(case, &body);

        assert_eq!(
            (
                string_member(&body, "AvailabilityZoneCount", case),
                string_member(&body, "ActiveAvailabilityZoneCount", case),
                string_member(&body, "StandByAvailabilityZoneCount", case),
            ),
            (
                row.zone_count.to_string(),
                row.active_zone_count.to_string(),
                row.standby_zone_count.to_string(),
            ),
            "case {case}: expected the configured, active and standby zone counts \
             ({:?}, {:?}, {:?}), got {body:?}",
            row.zone_count,
            row.active_zone_count,
            row.standby_zone_count
        );
        assert_eq!(
            (
                string_member(&body, "DataNodeCount", case),
                string_member(&body, "MasterEligibleNodeCount", case),
                string_member(&body, "WarmNodeCount", case),
            ),
            (
                row.data_node_count.to_string(),
                row.master_eligible_node_count.to_string(),
                row.warm_node_count.to_string(),
            ),
            "case {case}: expected the data, master-eligible and warm node counts \
             ({:?}, {:?}, {:?}), got {body:?}",
            row.data_node_count,
            row.master_eligible_node_count,
            row.warm_node_count
        );
        assert_eq!(
            body["DedicatedMaster"].as_bool(),
            Some(row.dedicated_master),
            "case {case}: expected DedicatedMaster {:?}, got {:?}",
            row.dedicated_master,
            body["DedicatedMaster"]
        );

        // A settled domain's cluster is up, so every observed value is reported.
        assert_eq!(
            (
                string_member(&body, "DomainState", case),
                string_member(&body, "MasterNode", case),
                string_member(&body, "ClusterHealth", case),
            ),
            (
                "Active".to_string(),
                "Available".to_string(),
                "Green".to_string(),
            ),
            "case {case}: expected a settled domain to report an Active state, an \
             Available master node and Green health, got {body:?}"
        );
        // The emulator persists no index, so the domain holds no shard and none of
        // them is unassigned — which is what makes its health Green.
        assert_eq!(
            (
                string_member(&body, "TotalShards", case),
                string_member(&body, "TotalUnAssignedShards", case),
            ),
            ("0".to_string(), "0".to_string()),
            "case {case}: expected a domain with no index to report no shard, got {body:?}"
        );

        let reported_zones = zone_information(&body);
        assert_eq!(
            reported_zones.len(),
            row.zones.len(),
            "case {case}: expected {} zone(s) of information, got {:?}",
            row.zones.len(),
            body["EnvironmentInformation"]
        );
        for (expected, actual) in row.zones.iter().zip(reported_zones.iter()) {
            assert_eq!(
                (
                    string_member(actual, "AvailabilityZoneName", case),
                    string_member(actual, "ZoneStatus", case),
                    string_member(actual, "ConfiguredDataNodeCount", case),
                    string_member(actual, "AvailableDataNodeCount", case),
                ),
                (
                    expected.name.to_string(),
                    expected.status.to_string(),
                    expected.configured_nodes.to_string(),
                    // Every configured node of a cluster that is up is available.
                    expected.configured_nodes.to_string(),
                ),
                "case {case}: expected zone {:?} to be {:?} with {:?} configured and \
                 available data nodes, got {actual:?}",
                expected.name,
                expected.status,
                expected.configured_nodes
            );
        }
    }
}

#[tokio::test]
async fn health_of_a_domain_still_creating_withholds_what_the_cluster_has_not_reached() {
    let emulator = Emulator::new();
    let case = "a domain read straight after CreateDomain";
    let name = "still-creating";
    let cluster = json!({
        "InstanceCount": 4,
        "ZoneAwarenessEnabled": true,
        "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
    });
    create_domain(&emulator, case, name, &cluster).await;

    // No wait: the provisioning span is deliberately not zero, so the cluster has not
    // come up yet.
    let body = health_body(&emulator, case, name).await;
    assert_counts_are_modelled(case, &body);

    assert_eq!(
        (
            string_member(&body, "DomainState", case),
            string_member(&body, "MasterNode", case),
            string_member(&body, "ClusterHealth", case),
            string_member(&body, "ActiveAvailabilityZoneCount", case),
            string_member(&body, "TotalShards", case),
            string_member(&body, "TotalUnAssignedShards", case),
        ),
        (
            "Processing".to_string(),
            // The master is elected from the master-eligible nodes the configuration
            // places, which is known from the moment the create is accepted.
            "Available".to_string(),
            // The shard allocation is an OBSERVATION of the running cluster, and there
            // is none to read yet.
            NOT_AVAILABLE.to_string(),
            // Which zones serve traffic follows from the configuration too: the zones
            // the domain is placed in, less the ones held in standby.
            "2".to_string(),
            NOT_AVAILABLE.to_string(),
            NOT_AVAILABLE.to_string(),
        ),
        "case {case}: expected a creating domain to report its configured cluster and no \
         observation of the running one, got {body:?}"
    );
    // What was CONFIGURED is known from the moment the create is accepted.
    assert_eq!(
        (
            string_member(&body, "AvailabilityZoneCount", case),
            string_member(&body, "StandByAvailabilityZoneCount", case),
            string_member(&body, "DataNodeCount", case),
            string_member(&body, "MasterEligibleNodeCount", case),
        ),
        (
            "2".to_string(),
            "0".to_string(),
            "4".to_string(),
            "4".to_string()
        ),
        "case {case}: expected the configured counts of a creating domain, got {body:?}"
    );
    // Per-zone information is an observation of the running cluster — how many of each
    // zone's nodes are available, and how its shards are allocated — so a domain whose
    // cluster is not up reports none at all.
    assert!(
        body["EnvironmentInformation"].is_null(),
        "case {case}: expected no zone information before the cluster is up, got {:?}",
        body["EnvironmentInformation"]
    );

    // Once the provisioning the clock owed it falls due, the same read reports the
    // cluster.
    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = health_body(&emulator, case, name).await;
    assert_eq!(
        (
            string_member(&settled, "DomainState", case),
            string_member(&settled, "MasterNode", case),
            string_member(&settled, "ClusterHealth", case),
            string_member(&settled, "ActiveAvailabilityZoneCount", case),
        ),
        (
            "Active".to_string(),
            "Available".to_string(),
            "Green".to_string(),
            "2".to_string()
        ),
        "case {case}: expected a settled domain to report its cluster, got {settled:?}"
    );
}

#[tokio::test]
async fn an_update_in_flight_is_processing_on_a_cluster_that_is_still_up() {
    let emulator = Emulator::new();
    let case = "a domain read straight after UpdateDomainConfig";
    let name = "updating-domain";
    create_domain(&emulator, case, name, &json!({ "InstanceCount": 2 })).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let updated = emulator
        .update_config(name, json!({ "ClusterConfig": { "InstanceCount": 3 } }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected UpdateDomainConfig to answer 200, got {} with {:?}",
        updated.status, updated.body
    );

    let body = health_body(&emulator, case, name).await;
    assert_counts_are_modelled(case, &body);
    assert_eq!(
        (
            string_member(&body, "DomainState", case),
            // The running cluster is not torn down by a configuration change, so it
            // still has an elected master and readable health.
            string_member(&body, "MasterNode", case),
            string_member(&body, "ClusterHealth", case),
            string_member(&body, "DataNodeCount", case),
        ),
        (
            "Processing".to_string(),
            "Available".to_string(),
            "Green".to_string(),
            "3".to_string(),
        ),
        "case {case}: expected an update in flight to report Processing over a cluster \
         that is up, got {body:?}"
    );
}

#[tokio::test]
async fn the_reported_zones_account_for_every_configured_data_node() {
    let emulator = Emulator::new();
    // The rule generalises over the node count and the number of zones it is spread
    // across, so sweep both: counts that divide evenly, counts that do not, and one
    // node per zone.
    let node_counts = [1, 2, 3, 4, 6, 7, 9];
    let zone_counts = [1, 2, 3];
    let mut cases = Vec::new();

    for nodes in node_counts {
        for zones in zone_counts {
            let name = format!("sweep-{nodes}-{zones}");
            let case = format!("{nodes} data node(s) over {zones} zone(s)");
            let cluster = if zones == 1 {
                json!({ "InstanceCount": nodes })
            } else {
                json!({
                    "InstanceCount": nodes,
                    "ZoneAwarenessEnabled": true,
                    "ZoneAwarenessConfig": { "AvailabilityZoneCount": zones },
                })
            };
            create_domain(&emulator, &case, &name, &cluster).await;
            cases.push((case, name, nodes, zones));
        }
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, name, nodes, zones) in &cases {
        let body = health_body(&emulator, case, name).await;
        assert_counts_are_modelled(case, &body);

        let reported = zone_information(&body);
        assert_eq!(
            reported.len(),
            *zones as usize,
            "case {case}: expected one zone of information per configured zone, got {reported:?}"
        );
        assert_eq!(
            string_member(&body, "AvailabilityZoneCount", case),
            zones.to_string(),
            "case {case}: expected AvailabilityZoneCount {zones}, got {body:?}"
        );

        let per_zone: Vec<i64> = reported
            .iter()
            .map(|zone| {
                string_member(zone, "ConfiguredDataNodeCount", case)
                    .parse()
                    .unwrap_or_else(|error| {
                        panic!("case {case}: expected a numeric zone node count: {error}")
                    })
            })
            .collect();
        let total: i64 = per_zone.iter().sum();
        assert_eq!(
            total, *nodes,
            "case {case}: expected the zones to account for all {nodes} data nodes, \
             got {per_zone:?}"
        );
        let widest = per_zone.iter().max().copied().unwrap_or_default()
            - per_zone.iter().min().copied().unwrap_or_default();
        assert!(
            widest <= 1,
            "case {case}: expected the data nodes spread evenly across the zones, \
             got {per_zone:?}"
        );

        // Active plus standby is every zone the domain is configured in.
        let active: i64 = string_member(&body, "ActiveAvailabilityZoneCount", case)
            .parse()
            .unwrap_or_else(|error| panic!("case {case}: expected a numeric count: {error}"));
        let standby: i64 = string_member(&body, "StandByAvailabilityZoneCount", case)
            .parse()
            .unwrap_or_else(|error| panic!("case {case}: expected a numeric count: {error}"));
        assert_eq!(
            active + standby,
            *zones,
            "case {case}: expected the active and standby zones to add up to {zones}, \
             got {active} + {standby}"
        );

        // Each zone is named once, for the Region the domain was created in.
        let names: BTreeSet<String> = reported
            .iter()
            .map(|zone| string_member(zone, "AvailabilityZoneName", case))
            .collect();
        assert_eq!(
            names.len(),
            reported.len(),
            "case {case}: expected a distinct name per zone, got {names:?}"
        );
        for zone_name in &names {
            assert!(
                zone_name.starts_with(REGION) && zone_name.len() == REGION.len() + 1,
                "case {case}: expected a zone of {REGION}, got {zone_name:?}"
            );
        }
    }
}

#[tokio::test]
async fn health_reports_the_same_cluster_the_domain_record_does() {
    let emulator = Emulator::new();
    let case = "health against DescribeDomain";
    let name = "round-trip";
    create_domain(
        &emulator,
        case,
        name,
        &json!({
            "InstanceCount": 6,
            "ZoneAwarenessEnabled": true,
            "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
            "DedicatedMasterEnabled": true,
            "DedicatedMasterCount": 3,
        }),
    )
    .await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // The created resource read back through the implemented reader.
    let described = emulator.describe(name).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeDomain to answer 200, got {} with {:?}",
        described.status, described.body
    );
    let cluster = described.body["DomainStatus"]["ClusterConfig"].clone();

    let body = health_body(&emulator, case, name).await;
    assert_eq!(
        string_member(&body, "DataNodeCount", case),
        cluster["InstanceCount"].to_string(),
        "case {case}: expected the health to report the stored InstanceCount {:?}, got {body:?}",
        cluster["InstanceCount"]
    );
    assert_eq!(
        string_member(&body, "MasterEligibleNodeCount", case),
        cluster["DedicatedMasterCount"].to_string(),
        "case {case}: expected the health to report the stored DedicatedMasterCount {:?}, \
         got {body:?}",
        cluster["DedicatedMasterCount"]
    );
    assert_eq!(
        body["DedicatedMaster"].as_bool(),
        cluster["DedicatedMasterEnabled"].as_bool(),
        "case {case}: expected the health to report the stored DedicatedMasterEnabled {:?}, \
         got {:?}",
        cluster["DedicatedMasterEnabled"],
        body["DedicatedMaster"]
    );

    // A read changes nothing a later caller sees: the second answer is the first.
    let again = health_body(&emulator, case, name).await;
    assert_eq!(
        again, body,
        "case {case}: expected a second DescribeDomainHealth to answer the same, \
         got {again:?} after {body:?}"
    );
}

#[tokio::test]
async fn health_of_an_unknown_domain_is_the_generic_envelope() {
    let emulator = Emulator::new();
    // One created domain, so the refusal is about the name asked for and not about an
    // empty store.
    create_domain(
        &emulator,
        "a known domain",
        "known-domain",
        &json!({ "InstanceCount": 1 }),
    )
    .await;
    // A domain that WAS stored and is not any more is the same miss as one that never
    // was.
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/known-domain"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(known-domain): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // The rule holds for every name no domain is stored under, whatever its shape
    // inside `DomainName`'s own constraint: the shortest and the longest it allows,
    // one near-miss of a name that was created, and a deleted one.
    for name in [
        "abc",
        &"d".repeat(28),
        "known-domain-2",
        "zzz-nothing-here",
        "known-domain",
    ] {
        let answer = health(&emulator, name).await;
        assert_eq!(
            answer.status, 400,
            "case DescribeDomainHealth({name:?}): expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "BaseException",
            "case DescribeDomainHealth({name:?}): expected BaseException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(format!("Domain not found: {name}").as_str()),
            "case DescribeDomainHealth({name:?}): expected the miss to name the domain asked \
             for, got {:?}",
            answer.body
        );
    }
}
