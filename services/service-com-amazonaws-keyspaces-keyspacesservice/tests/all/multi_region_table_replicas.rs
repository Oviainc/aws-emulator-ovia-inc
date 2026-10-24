// SPDX-License-Identifier: CC0-1.0

//! The Region-specific settings a report of a MULTI-REGION table carries, driven end to
//! end over the service's own `awsJson1_0` wire protocol through the harness in
//! [`super::keyspace_table_lifecycle`].
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! 1. A report of a table carries one Region-specific entry per Region the OWNING keyspace
//!    replicates the table to — that is, per Region of a `MULTI_REGION` keyspace's
//!    `replicationSpecification.regionList`, in the keyspace's own order — and none at all
//!    for a table of a keyspace under any other replication strategy, which is replicated
//!    in one Region only. `GetTable$replicaSpecifications` ("Returns the Amazon Web
//!    Services Region specific settings of all Regions a multi-Region table is replicated
//!    in") carries the table's status and capacity summary in each such Region;
//!    `GetTableAutoScalingSettings$replicaSpecifications` ("Returns the settings for all
//!    Regions the table is replicated in") carries its auto scaling settings there.
//! 2. The Regions are the keyspace's CURRENT replication Regions, whether the keyspace was
//!    created `MULTI_REGION` or widened into one by `UpdateKeyspace`, and the per-Region
//!    status is the table's status as the clock has settled it — so the entries move with
//!    the table from its transient status into `ACTIVE`.
//! 3. Reporting these settings is a read: it leaves the keyspace, the table and every
//!    later report exactly as they were.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::TABLE_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::keyspace_table_lifecycle::{
    assert_ok, minimal_schema, settle_wait, table_arn, Emulator, REGION,
};

/// Regions other than the request's own ([`REGION`]) a keyspace can be replicated in. Each
/// is within `RegionList`'s modelled member length (2..25).
const OTHER_REGIONS: [&str; 4] = ["eu-west-1", "ap-south-1", "us-east-2", "sa-east-1"];

/// A `MULTI_REGION` replication specification over `regions`.
fn multi_region(regions: &[&str]) -> Value {
    json!({"replicationStrategy": "MULTI_REGION", "regionList": regions})
}

/// `GetTable` for one table, as a 200 body.
async fn get_table(emulator: &Emulator, case: &str, keyspace: &str, table: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    )
}

/// `GetTableAutoScalingSettings` for one table, as a 200 body.
async fn get_auto_scaling(emulator: &Emulator, case: &str, keyspace: &str, table: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await,
    )
}

/// The `replicaSpecifications` of a report, as a list, or `None` when the report carries
/// none at all.
fn replicas(report: &Value) -> Option<Vec<Value>> {
    match &report["replicaSpecifications"] {
        Value::Null => None,
        other => Some(other.as_array().cloned().unwrap_or_else(|| {
            panic!("replicaSpecifications must be a list when present, got {other}")
        })),
    }
}

/// RULE 1 for `GetTable`: the summaries are the owning keyspace's replication Regions,
/// each carrying the table's status and capacity in that Region.
///
/// The rows differ along what the rule generalises over: the replication specification
/// (omitted, `SINGLE_REGION`, and `MULTI_REGION` over two, three and five Regions), the
/// position of the request's own Region inside the list, and the table's throughput mode
/// (the default `PAY_PER_REQUEST`, and `PROVISIONED` with differing units).
#[tokio::test]
async fn get_table_reports_one_replica_summary_per_replication_region() {
    struct Case {
        /// Also the keyspace name and the table name.
        name: &'static str,
        replication: Option<Value>,
        /// The `capacitySpecification` to create the table with, if any.
        capacity: Option<Value>,
        /// The Regions the report must carry one entry each for, in order, or `None` when
        /// it must carry no `replicaSpecifications` at all.
        expected: Option<Vec<&'static str>>,
    }

    let provisioned = |read: i64, write: i64| {
        json!({
            "throughputMode": "PROVISIONED",
            "readCapacityUnits": read,
            "writeCapacityUnits": write,
        })
    };

    let cases: Vec<Case> = vec![
        Case {
            // No replicationSpecification at all: a single-Region keyspace.
            name: "strategy_omitted",
            replication: None,
            capacity: None,
            expected: None,
        },
        Case {
            name: "single_region_provisioned",
            replication: Some(json!({"replicationStrategy": "SINGLE_REGION"})),
            capacity: Some(provisioned(5, 5)),
            expected: None,
        },
        Case {
            // The modelled minimum length of a regionList (2).
            name: "multi_region_pair",
            replication: Some(multi_region(&[REGION, OTHER_REGIONS[0]])),
            capacity: None,
            expected: Some(vec![REGION, OTHER_REGIONS[0]]),
        },
        Case {
            // The request's own Region in the MIDDLE of the list, so the entries follow the
            // keyspace's order rather than putting the caller's Region first.
            name: "multi_region_triple_provisioned",
            replication: Some(multi_region(&[
                OTHER_REGIONS[1],
                REGION,
                OTHER_REGIONS[2],
            ])),
            capacity: Some(provisioned(10, 7)),
            expected: Some(vec![OTHER_REGIONS[1], REGION, OTHER_REGIONS[2]]),
        },
        Case {
            // A wider group, with the request's own Region last.
            name: "multi_region_five_provisioned",
            replication: Some(multi_region(&[
                OTHER_REGIONS[0],
                OTHER_REGIONS[1],
                OTHER_REGIONS[2],
                OTHER_REGIONS[3],
                REGION,
            ])),
            capacity: Some(provisioned(1, 40000)),
            expected: Some(vec![
                OTHER_REGIONS[0],
                OTHER_REGIONS[1],
                OTHER_REGIONS[2],
                OTHER_REGIONS[3],
                REGION,
            ]),
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        let mut request = json!({"keyspaceName": case.name});
        if let Some(replication) = &case.replication {
            request["replicationSpecification"] = replication.clone();
        }
        assert_ok(case.name, emulator.call("CreateKeyspace", request).await);

        let mut create_table = json!({
            "keyspaceName": case.name,
            "tableName": case.name,
            "schemaDefinition": minimal_schema("pk"),
        });
        if let Some(capacity) = &case.capacity {
            create_table["capacitySpecification"] = capacity.clone();
        }
        assert_ok(
            case.name,
            emulator.call("CreateTable", create_table).await,
        );
    }

    // RULE 2: read first while the creation the clock owes is still pending, so the
    // per-Region status is the table's TRANSIENT status and not only ever `ACTIVE`.
    for case in &cases {
        let report = get_table(&emulator, case.name, case.name, case.name).await;
        assert_replicas(case.name, &report, case.expected.as_deref(), &emulator).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_CREATING),
            "case {:?}: a table whose creation was just accepted is CREATING, got {report}",
            case.name
        );
    }

    // RULE 2: the span has elapsed, so the table — and every per-Region entry for it — is
    // `ACTIVE`, and nothing else about the entries moved.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let report = get_table(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the creation span has elapsed, so the table is ACTIVE, got {report}",
            case.name
        );
        assert_replicas(case.name, &report, case.expected.as_deref(), &emulator).await;

        // RULE 3: the report is a read — the next one answers identically.
        let again = get_table(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report, again,
            "case {:?}: a settled table reads back identically, got {again}",
            case.name
        );
    }
}

/// Assert one `GetTable` body's `replicaSpecifications` against the Regions the owning
/// keyspace replicates the table to: one entry per Region, in order, each carrying the
/// table's own status and capacity summary in that Region.
async fn assert_replicas(
    case: &str,
    report: &Value,
    expected: Option<&[&str]>,
    emulator: &Emulator,
) {
    let got = replicas(report);
    let Some(expected) = expected else {
        assert!(
            got.is_none(),
            "case {case:?}: a table of a single-Region keyspace reports no \
             replicaSpecifications, got {report}"
        );
        return;
    };
    let entries = got.unwrap_or_else(|| {
        panic!(
            "case {case:?}: a table of a MULTI_REGION keyspace reports one entry per \
             replication Region, got {report}"
        )
    });
    assert_eq!(
        entries.len(),
        expected.len(),
        "case {case:?}: expected {} replica summaries for {expected:?}, got {report}",
        expected.len()
    );

    // The Regions a report carries are the keyspace's CURRENT replication Regions, which
    // `GetKeyspace` reports from the same stored list.
    let keyspace = assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": report["keyspaceName"]}))
            .await,
    );
    assert_eq!(
        keyspace["replicationRegions"],
        json!(expected),
        "case {case:?}: the keyspace is replicated in {expected:?}, got {keyspace}"
    );

    for (entry, region) in entries.iter().zip(expected) {
        assert_eq!(
            entry["region"],
            json!(region),
            "case {case:?}: the entries follow the keyspace's Region list, got {report}"
        );
        let status = entry["status"].as_str().unwrap_or_default();
        assert!(
            TABLE_STATUS_VALUES.contains(&status),
            "case {case:?}: status {status:?} is not a modelled TableStatus value, got {report}"
        );
        assert_eq!(
            entry["status"], report["status"],
            "case {case:?}: the table's status in {region} is the status it holds, got {report}"
        );
        assert_eq!(
            entry["capacitySpecification"], report["capacitySpecification"],
            "case {case:?}: with no per-Region override stored, the Region's capacity \
             summary is the table's, got {report}"
        );
        assert!(
            entry["warmThroughputSpecification"].is_null(),
            "case {case:?}: nothing pre-warmed this table, so no warm throughput is \
             reported per Region, got {report}"
        );
    }
}

/// RULE 1 for `GetTableAutoScalingSettings`: "Returns the settings for all Regions the
/// table is replicated in" — one entry per replication Region of the owning keyspace,
/// each carrying the table's auto scaling settings, and none for a single-Region table.
///
/// The rows differ along the replication specification (omitted, `MULTI_REGION` over two
/// and over three Regions) and along the auto scaling settings themselves (read only,
/// read and write, explicitly disabled).
#[tokio::test]
async fn get_table_auto_scaling_settings_reports_one_entry_per_replication_region() {
    struct Case {
        /// Also the keyspace name and the table name.
        name: &'static str,
        replication: Option<Value>,
        auto_scaling: Value,
        expected: Option<Vec<&'static str>>,
    }

    let enabled = |minimum: i64, maximum: i64, target: f64| {
        json!({
            "autoScalingDisabled": false,
            "minimumUnits": minimum,
            "maximumUnits": maximum,
            "scalingPolicy": {
                "targetTrackingScalingPolicyConfiguration": {"targetValue": target},
            },
        })
    };

    let cases: Vec<Case> = vec![
        Case {
            name: "auto_scaling_single_region",
            replication: None,
            auto_scaling: json!({
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            }),
            expected: None,
        },
        Case {
            name: "auto_scaling_pair_read_only",
            replication: Some(multi_region(&[REGION, OTHER_REGIONS[0]])),
            auto_scaling: json!({"readCapacityAutoScaling": enabled(1, 40000, 20.0)}),
            expected: Some(vec![REGION, OTHER_REGIONS[0]]),
        },
        Case {
            name: "auto_scaling_triple_disabled",
            replication: Some(multi_region(&[
                OTHER_REGIONS[1],
                OTHER_REGIONS[2],
                REGION,
            ])),
            auto_scaling: json!({
                "readCapacityAutoScaling": {"autoScalingDisabled": true},
                "writeCapacityAutoScaling": {"autoScalingDisabled": true},
            }),
            expected: Some(vec![OTHER_REGIONS[1], OTHER_REGIONS[2], REGION]),
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        let mut request = json!({"keyspaceName": case.name});
        if let Some(replication) = &case.replication {
            request["replicationSpecification"] = replication.clone();
        }
        assert_ok(case.name, emulator.call("CreateKeyspace", request).await);
        assert_ok(
            case.name,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": case.name,
                        "tableName": case.name,
                        "schemaDefinition": minimal_schema("pk"),
                        // "The optional auto scaling settings for a table in provisioned
                        // capacity mode."
                        "capacitySpecification": {
                            "throughputMode": "PROVISIONED",
                            "readCapacityUnits": 5,
                            "writeCapacityUnits": 5,
                        },
                        "autoScalingSpecification": case.auto_scaling,
                    }),
                )
                .await,
        );
    }

    for case in &cases {
        let report = get_auto_scaling(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report["resourceArn"],
            json!(table_arn(case.name, case.name)),
            "case {:?}: the report names the table it was asked for, got {report}",
            case.name
        );
        let got = replicas(&report);
        match &case.expected {
            None => assert!(
                got.is_none(),
                "case {:?}: a table of a single-Region keyspace reports no \
                 replicaSpecifications, got {report}",
                case.name
            ),
            Some(expected) => {
                let entries = got.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: a multi-Region table reports one entry per replication \
                         Region, got {report}",
                        case.name
                    )
                });
                assert_eq!(
                    entries.len(),
                    expected.len(),
                    "case {:?}: expected {} entries for {expected:?}, got {report}",
                    case.name,
                    expected.len()
                );
                for (entry, region) in entries.iter().zip(expected) {
                    assert_eq!(
                        entry["region"],
                        json!(region),
                        "case {:?}: the entries follow the keyspace's Region list, got {report}",
                        case.name
                    );
                    assert_eq!(
                        entry["autoScalingSpecification"], report["autoScalingSpecification"],
                        "case {:?}: with no per-Region override stored, the Region's auto \
                         scaling settings are the table's, got {report}",
                        case.name
                    );
                }
            }
        }

        // RULE 3: the report is a read — the next one answers identically.
        let again = get_auto_scaling(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report, again,
            "case {:?}: the settings read back identically, got {again}",
            case.name
        );
    }
}

/// RULE 2: the Regions a report carries are the keyspace's CURRENT replication Regions, so
/// a table that was created in a single-Region keyspace reports per-Region settings as soon
/// as `UpdateKeyspace` has widened that keyspace — over the Regions the update named, and
/// with the `UPDATING` status the replication puts the table in until the clock settles it.
///
/// The rows differ along how many Regions the update adds (one, and two at once).
#[tokio::test]
async fn widening_a_keyspace_gives_its_tables_per_region_settings() {
    // (keyspace/table name, the regionList UpdateKeyspace is given)
    let rows: [(&str, Vec<&str>); 2] = [
        ("widened_by_one", vec![REGION, OTHER_REGIONS[0]]),
        (
            "widened_by_two",
            vec![REGION, OTHER_REGIONS[1], OTHER_REGIONS[2]],
        ),
    ];

    let emulator = Emulator::start();

    for (name, _) in &rows {
        assert_ok(
            name,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": name}))
                .await,
        );
        assert_ok(
            name,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": name,
                        "tableName": name,
                        "schemaDefinition": minimal_schema("pk"),
                        // Every table of a keyspace being replicated "must use client-side
                        // timestamps for conflict resolution".
                        "clientSideTimestamps": {"status": "ENABLED"},
                    }),
                )
                .await,
        );

        // Before the widening, the table is replicated in one Region and reports nothing.
        let report = get_table(&emulator, name, name, name).await;
        assert!(
            replicas(&report).is_none(),
            "case {name:?}: a table of a single-Region keyspace reports no \
             replicaSpecifications, got {report}"
        );
    }

    // A keyspace takes an update only once it is ACTIVE.
    tokio::time::sleep(settle_wait()).await;

    for (name, regions) in &rows {
        assert_ok(
            name,
            emulator
                .call(
                    "UpdateKeyspace",
                    json!({
                        "keyspaceName": name,
                        "replicationSpecification": multi_region(regions),
                    }),
                )
                .await,
        );

        let report = get_table(&emulator, name, name, name).await;
        let entries = replicas(&report).unwrap_or_else(|| {
            panic!(
                "case {name:?}: a widened keyspace's table reports one entry per \
                 replication Region, got {report}"
            )
        });
        assert_eq!(
            entries.len(),
            regions.len(),
            "case {name:?}: expected {} entries for {regions:?}, got {report}",
            regions.len()
        );
        for (entry, region) in entries.iter().zip(regions) {
            assert_eq!(
                entry["region"],
                json!(region),
                "case {name:?}: the entries follow the Region list the update named, got {report}"
            );
            assert_eq!(
                entry["status"],
                json!(keyspaces::TABLE_STATUS_UPDATING),
                "case {name:?}: the table is UPDATING in every Region while Amazon Keyspaces \
                 replicates it, got {report}"
            );
        }
    }

    // The replication the clock owed has completed, so the table is ACTIVE in every Region.
    tokio::time::sleep(settle_wait()).await;
    for (name, regions) in &rows {
        let report = get_table(&emulator, name, name, name).await;
        let entries = replicas(&report).unwrap_or_else(|| {
            panic!("case {name:?}: the widened keyspace still has its Regions, got {report}")
        });
        assert_eq!(
            entries.len(),
            regions.len(),
            "case {name:?}: settling a status must not change the Region count, got {report}"
        );
        for entry in &entries {
            assert_eq!(
                entry["status"],
                json!(keyspaces::TABLE_STATUS_ACTIVE),
                "case {name:?}: the replication span has elapsed, so the table is ACTIVE in \
                 every Region, got {report}"
            );
        }
    }
}
