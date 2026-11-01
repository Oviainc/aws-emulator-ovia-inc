// SPDX-License-Identifier: CC0-1.0

//! The cluster topology a create asks for, as every reader echoes it back.
//!
//! RULE: `ClusterConfig` is a container the request SETS and the service reports back member
//! for member — every member the request carried comes back with that value, and the two
//! booleans below are independent of one another. `smithy_query(query="shape",
//! shape="ClusterConfig")` declares `DedicatedMasterEnabled` ("indicates whether dedicated
//! master nodes are enabled for the cluster") and `MultiAZWithStandbyEnabled` ("a boolean
//! that indicates whether a multi-AZ domain is turned on with a standby AZ") as two
//! independent members, so neither stands in for the other: a client that reads its topology
//! back must see the choice it made, or drift detection and idempotent reconciliation have
//! nothing to compare.
//!
//! The table spans the topologies a domain is created with — a single node, a single node
//! stating both booleans off, zone-awareness with dedicated masters, a standby zone with warm
//! nodes, a standby explicitly off, and no container at all — and each row is read back
//! through every reader of the stored record: the create's own answer, `DescribeDomain` and
//! `DescribeDomainConfig`.
//!
//! What a reader reports for a member the request did NOT set is deliberately not asserted:
//! that is the service's own defaulting, which the behaviour behind this rule does not cover,
//! so the rule is about the members a request SETS.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness is this crate's one test harness ([`super::domain_lifecycle`]).

use serde_json::{json, Value};

use super::domain_lifecycle::Emulator;

/// One topology to round-trip: the `ClusterConfig` the create carries, and the domain it is
/// created under.
struct Row {
    case: &'static str,
    domain: &'static str,
    /// The container the request carries, or `None` for a create that carries none.
    cluster: Option<Value>,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a single node stating neither boolean",
            domain: "cc-single",
            cluster: Some(json!({ "InstanceType": "t3.small.search", "InstanceCount": 1 })),
        },
        Row {
            case: "a single node stating both booleans off",
            domain: "cc-single-off",
            cluster: Some(json!({
                "InstanceType": "t3.small.search",
                "InstanceCount": 1,
                "DedicatedMasterEnabled": false,
                "ZoneAwarenessEnabled": false,
            })),
        },
        Row {
            case: "zone awareness with dedicated masters",
            domain: "cc-dedicated",
            cluster: Some(json!({
                "InstanceCount": 4,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
            })),
        },
        Row {
            case: "a standby zone with warm nodes",
            domain: "cc-standby",
            cluster: Some(json!({
                "InstanceCount": 6,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 3 },
                "MultiAZWithStandbyEnabled": true,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
                "WarmEnabled": true,
                "WarmCount": 2,
            })),
        },
        Row {
            case: "a standby zone turned off explicitly",
            domain: "cc-standby-off",
            cluster: Some(json!({
                "InstanceCount": 2,
                "MultiAZWithStandbyEnabled": false,
            })),
        },
        Row {
            case: "no ClusterConfig at all",
            domain: "cc-absent",
            cluster: None,
        },
    ]
}

/// RULE: the cluster topology a create asked for round-trips through every reader of the
/// stored domain — each member it set reported back with that value, and the two independent
/// booleans never standing in for one another.
#[tokio::test]
async fn the_requested_cluster_topology_round_trips_through_every_reader() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let mut body = json!({
            "DomainName": row.domain,
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
        });
        if let Some(cluster) = &row.cluster {
            body["ClusterConfig"] = cluster.clone();
        }
        let requested = row
            .cluster
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let created = emulator.create(body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateDomain, got {} with {:?}",
            created.status, created.body
        );
        let described = emulator.describe(row.domain).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeDomain, got {} with {:?}",
            described.status, described.body
        );
        let config = emulator.describe_config(row.domain).await;
        assert_eq!(
            config.status, 200,
            "case {case:?}: expected 200 from DescribeDomainConfig, got {} with {:?}",
            config.status, config.body
        );

        // Every reader of the stored record reports the container — `ClusterConfig` is a
        // member of the required `DomainStatus` — with every member the request set carrying
        // the value it set.
        for (reader, reported) in [
            (
                "CreateDomain",
                &created.body["DomainStatus"]["ClusterConfig"],
            ),
            (
                "DescribeDomain",
                &described.body["DomainStatus"]["ClusterConfig"],
            ),
            (
                "DescribeDomainConfig",
                &config.body["DomainConfig"]["ClusterConfig"]["Options"],
            ),
        ] {
            assert!(
                reported.is_object(),
                "case {case:?} ({reader}): expected a ClusterConfig container, got {reported:?}"
            );
            for (member, written) in &requested {
                assert_eq!(
                    &reported[member], written,
                    "case {case:?} ({reader}): expected ClusterConfig.{member} to round-trip \
                     as {written:?}, got {:?}",
                    reported[member]
                );
            }
            // The two booleans are INDEPENDENT members, so one the request set is never
            // answered under the other's name: where only one of the pair was set, the other
            // carries nothing.
            for (member, other) in [
                ("DedicatedMasterEnabled", "MultiAZWithStandbyEnabled"),
                ("MultiAZWithStandbyEnabled", "DedicatedMasterEnabled"),
            ] {
                let Some(written) = requested.get(member) else {
                    continue;
                };
                assert_eq!(
                    &reported[member], written,
                    "case {case:?} ({reader}): expected {member} as set ({written:?}), got {:?}",
                    reported[member]
                );
                assert!(
                    requested.contains_key(other) || reported[other].is_null(),
                    "case {case:?} ({reader}): {other} was never set, so {member} must not be \
                     answered under it, got {:?}",
                    reported[other]
                );
            }
        }
    }
}
