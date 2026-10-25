// SPDX-License-Identifier: CC0-1.0

//! What happens to the Amazon Web Services Region specific settings of a multi-Region
//! table when `UpdateTable` changes the table's capacity mode, driven end to end over the
//! service's own `awsJson1_0` wire protocol through the shared
//! [`crate::keyspace_table_lifecycle::Emulator`] harness.
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! 1. Besides the Region it names, a `ReplicaSpecification` carries only that Region's
//!    `readCapacityUnits` ("The provisioned read capacity units for the multi-Region table
//!    in the specified Amazon Web Services Region") and its `readCapacityAutoScaling`, and
//!    both belong to `throughputMode:PROVISIONED` — the only mode whose read capacity is
//!    provisioned at all, and the only one a request may carry either setting for. So a
//!    table holds a per-Region setting only while it is in that mode: a change of capacity
//!    mode that takes the table out of it leaves the table with none, in every Region, and
//!    a change that leaves the table in it keeps them all.
//! 2. Dropped means dropped, not hidden: a later move back into provisioned mode reports
//!    the table's own read capacity in every Region, never an override from before.
//! 3. The state the reports describe is one a request could have produced: for a table
//!    that is no longer provisioned, the very overrides it used to hold are refused as
//!    input with the modelled `ValidationException`.

use serde_json::{json, Value};

use crate::column_types_and_auto_scaling::assert_member;
use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, Emulator, REGION,
};

/// Regions other than the request's own ([`REGION`]) a keyspace can be replicated in. Each
/// is within `RegionList`'s modelled member length (2..25).
const OTHER_REGIONS: [&str; 4] = ["eu-west-1", "ap-south-1", "us-east-2", "sa-east-1"];

/// The read capacity units every table below is created with, so each Region's override is
/// distinguishable from the table's own setting.
const TABLE_READ_UNITS: i64 = 5;

/// The write capacity units every table below is created with. The write capacity has no
/// per-Region override, because it "remains synchronized between all replicas".
const TABLE_WRITE_UNITS: i64 = 3;

/// The `readCapacityAutoScaling.minimumUnits` every table below is created with.
const TABLE_READ_MINIMUM: i64 = 5;

/// An `AutoScalingSettings` with auto scaling turned on: "This optional parameter enables
/// auto scaling for the table if set to `false`", and then `minimumUnits`, `maximumUnits`
/// and `scalingPolicy` are required.
fn enabled(minimum: i64, maximum: i64, target: f64) -> Value {
    json!({
        "autoScalingDisabled": false,
        "minimumUnits": minimum,
        "maximumUnits": maximum,
        "scalingPolicy": {
            "targetTrackingScalingPolicyConfiguration": {"targetValue": target},
        },
    })
}

/// A provisioned capacity specification: the mode that "requires `readCapacityUnits` and
/// `writeCapacityUnits` as input".
fn provisioned(read: i64, write: i64) -> Value {
    json!({
        "throughputMode": "PROVISIONED",
        "readCapacityUnits": read,
        "writeCapacityUnits": write,
    })
}

/// The other modelled `ThroughputMode` value, which takes no capacity units.
fn pay_per_request() -> Value {
    json!({"throughputMode": "PAY_PER_REQUEST"})
}

/// Create a `MULTI_REGION` keyspace over `regions` and, in it, a provisioned table with
/// table-level auto scaling and the per-Region overrides in `replicas`.
async fn create_multi_region_table(
    emulator: &Emulator,
    case: &str,
    name: &str,
    regions: &[&str],
    replicas: &Value,
) {
    assert_ok(
        case,
        emulator
            .call(
                "CreateKeyspace",
                json!({
                    "keyspaceName": name,
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": regions,
                    },
                }),
            )
            .await,
    );
    assert_ok(
        case,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": name,
                    "tableName": name,
                    "schemaDefinition": minimal_schema("pk"),
                    "capacitySpecification": provisioned(TABLE_READ_UNITS, TABLE_WRITE_UNITS),
                    "autoScalingSpecification": {
                        "readCapacityAutoScaling": enabled(TABLE_READ_MINIMUM, 50, 60.0),
                        "writeCapacityAutoScaling": enabled(TABLE_READ_MINIMUM, 50, 60.0),
                    },
                    "replicaSpecifications": replicas.clone(),
                }),
            )
            .await,
    );
}

/// `UpdateTable` for one table, carrying `members`.
async fn update_table(emulator: &Emulator, name: &str, members: Value) -> (u16, Value) {
    let mut request = members;
    request["keyspaceName"] = json!(name);
    request["tableName"] = json!(name);
    emulator.call("UpdateTable", request).await
}

/// `GetTable` for one table, as a 200 body.
async fn get_table(emulator: &Emulator, case: &str, name: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call(
                "GetTable",
                json!({"keyspaceName": name, "tableName": name}),
            )
            .await,
    )
}

/// `GetTableAutoScalingSettings` for one table, as a 200 body.
async fn get_auto_scaling(emulator: &Emulator, case: &str, name: &str) -> Value {
    assert_ok(
        case,
        emulator
            .call(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": name, "tableName": name}),
            )
            .await,
    )
}

/// The `replicaSpecifications` entries of a report, one per Region the table is replicated
/// in, asserted to be as many as `regions` and to follow their order.
fn replica_entries(case: &str, report: &Value, regions: &[&str]) -> Vec<Value> {
    let entries = report["replicaSpecifications"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a multi-Region table reports one entry per replication \
                 Region, got {report}"
            )
        });
    assert_eq!(
        entries.len(),
        regions.len(),
        "case {case:?}: expected one entry per Region of {regions:?}, got {report}"
    );
    for (entry, region) in entries.iter().zip(regions) {
        assert_eq!(
            entry["region"],
            json!(region),
            "case {case:?}: the entries follow the keyspace's Region list, got {report}"
        );
    }
    entries
}

/// Assert, for one table, that every Region reports `read_units` as its read capacity and
/// `read_minimum` as its `readCapacityAutoScaling.minimumUnits`, where a `None` means the
/// Region reports no such setting at all.
async fn assert_per_region_settings(
    emulator: &Emulator,
    case: &str,
    name: &str,
    regions: &[&str],
    read_units: &[Option<i64>],
    read_minimums: &[Option<i64>],
) {
    let report = get_table(emulator, case, name).await;
    let entries = replica_entries(case, &report, regions);
    for ((entry, region), expected) in entries.iter().zip(regions).zip(read_units) {
        let actual = &entry["capacitySpecification"]["readCapacityUnits"];
        match expected {
            Some(units) => assert_member(
                case,
                entry,
                "/capacitySpecification/readCapacityUnits",
                &json!(units),
            ),
            None => assert!(
                actual.is_null(),
                "case {case:?}: Region {region} reports no read capacity, got {actual} \
                 in {report}"
            ),
        }
        // Whatever the mode, the Region reports the table's mode and the table's write
        // capacity: neither is configurable per Region.
        assert_eq!(
            entry["capacitySpecification"]["throughputMode"],
            report["capacitySpecification"]["throughputMode"],
            "case {case:?}: Region {region} reports the table's capacity mode, got {report}"
        );
        assert_eq!(
            entry["capacitySpecification"]["writeCapacityUnits"],
            report["capacitySpecification"]["writeCapacityUnits"],
            "case {case:?}: Region {region} keeps the table's write capacity, got {report}"
        );
    }

    let scaling = get_auto_scaling(emulator, case, name).await;
    let scaling_entries = replica_entries(case, &scaling, regions);
    for ((entry, region), expected) in scaling_entries.iter().zip(regions).zip(read_minimums) {
        let actual = &entry["autoScalingSpecification"];
        match expected {
            Some(minimum) => assert_member(
                case,
                entry,
                "/autoScalingSpecification/readCapacityAutoScaling/minimumUnits",
                &json!(minimum),
            ),
            None => assert!(
                actual.is_null(),
                "case {case:?}: Region {region} reports no auto scaling settings, got \
                 {actual} in {scaling}"
            ),
        }
    }
}

/// RULE 1: a capacity-mode change that takes a table out of provisioned mode leaves it
/// with no per-Region read capacity or read capacity auto scaling setting in any Region,
/// and one that leaves the table in provisioned mode keeps every one of them.
///
/// RULE 3 as well, for the rows that leave provisioned mode: the overrides the table used
/// to hold are then refused as input, so the reports above describe a state a request could
/// have produced.
///
/// The rows differ along what the rule generalises over: which override was stored (read
/// capacity units, read capacity auto scaling, both), how many Regions carry one (one, two,
/// three), how many Regions the keyspace replicates to (two, three, five), where the
/// request's own Region sits in that list, and the mode the update moves the table to (the
/// other modelled `ThroughputMode` value, and provisioned again with different units).
#[tokio::test]
async fn a_capacity_mode_change_keeps_per_region_settings_only_while_provisioned() {
    struct Case {
        /// Also the keyspace name and the table name.
        name: &'static str,
        /// The keyspace's replication Regions, in order.
        regions: Vec<&'static str>,
        /// The per-Region overrides the table is created with.
        replicas: Value,
        /// The `capacitySpecification` the update carries.
        update: Value,
        /// The read capacity units each Region must report afterwards, in `regions` order.
        read_units: Vec<Option<i64>>,
        /// The `readCapacityAutoScaling.minimumUnits` each Region must report afterwards.
        read_minimums: Vec<Option<i64>>,
        /// Whether the update leaves the table in provisioned mode, which is the only mode
        /// the stored overrides belong to.
        stays_provisioned: bool,
    }

    let cases: Vec<Case> = vec![
        Case {
            // One Region, read capacity units only, over the modelled minimum regionList.
            name: "pay_per_request_drops_a_read_units_override",
            regions: vec![REGION, OTHER_REGIONS[0]],
            replicas: json!([{"region": OTHER_REGIONS[0], "readCapacityUnits": 9}]),
            update: pay_per_request(),
            read_units: vec![None, None],
            read_minimums: vec![None, None],
            stays_provisioned: false,
        },
        Case {
            // One Region, auto scaling override only, and the request's own Region last.
            name: "pay_per_request_drops_an_auto_scaling_override",
            regions: vec![OTHER_REGIONS[0], OTHER_REGIONS[1], REGION],
            replicas: json!([{
                "region": OTHER_REGIONS[1],
                "readCapacityAutoScaling": enabled(11, 4000, 75.0),
            }]),
            update: pay_per_request(),
            read_units: vec![None, None, None],
            read_minimums: vec![None, None, None],
            stays_provisioned: false,
        },
        Case {
            // Three Regions at once, one of them the request's own, in the middle of the
            // list, and the unit values at the modelled minimum and maximum ends.
            name: "pay_per_request_drops_overrides_in_every_region",
            regions: vec![OTHER_REGIONS[0], REGION, OTHER_REGIONS[1]],
            replicas: json!([
                {
                    "region": OTHER_REGIONS[0],
                    "readCapacityUnits": 1,
                    "readCapacityAutoScaling": enabled(2, 20, 20.0),
                },
                {"region": REGION, "readCapacityUnits": 40000},
                {
                    "region": OTHER_REGIONS[1],
                    "readCapacityUnits": 17,
                    "readCapacityAutoScaling": enabled(17, 170, 80.0),
                },
            ]),
            update: pay_per_request(),
            read_units: vec![None, None, None],
            read_minimums: vec![None, None, None],
            stays_provisioned: false,
        },
        Case {
            // A wider replication group, with only some of its Regions overridden.
            name: "pay_per_request_drops_overrides_in_five",
            regions: vec![
                OTHER_REGIONS[0],
                OTHER_REGIONS[1],
                OTHER_REGIONS[2],
                OTHER_REGIONS[3],
                REGION,
            ],
            replicas: json!([
                {"region": OTHER_REGIONS[1], "readCapacityUnits": 12},
                {
                    "region": OTHER_REGIONS[3],
                    "readCapacityUnits": 6,
                    "readCapacityAutoScaling": enabled(6, 60, 45.0),
                },
            ]),
            update: pay_per_request(),
            read_units: vec![None, None, None, None, None],
            read_minimums: vec![None, None, None, None, None],
            stays_provisioned: false,
        },
        Case {
            // The same update shape, but the mode the table ends in is still provisioned:
            // every override survives, over the table's NEW read capacity.
            name: "provisioned_again_keeps_every_override",
            regions: vec![REGION, OTHER_REGIONS[0]],
            replicas: json!([{
                "region": OTHER_REGIONS[0],
                "readCapacityUnits": 12,
                "readCapacityAutoScaling": enabled(12, 120, 70.0),
            }]),
            update: provisioned(8, 6),
            read_units: vec![Some(8), Some(12)],
            read_minimums: vec![Some(TABLE_READ_MINIMUM), Some(12)],
            stays_provisioned: true,
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        create_multi_region_table(
            &emulator,
            case.name,
            case.name,
            &case.regions,
            &case.replicas,
        )
        .await;
    }

    // A table takes an update only once the creation the clock owed it has settled.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        assert_ok(
            case.name,
            update_table(
                &emulator,
                case.name,
                json!({"capacitySpecification": case.update.clone()}),
            )
            .await,
        );
    }

    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        assert_per_region_settings(
            &emulator,
            case.name,
            case.name,
            &case.regions,
            &case.read_units,
            &case.read_minimums,
        )
        .await;

        // RULE 3: the overrides this table used to hold are input the service refuses for
        // the mode it is in now, so the reports above cannot be describing state a caller
        // could not have asked for. A table that is still provisioned takes them.
        let resubmitted = update_table(
            &emulator,
            case.name,
            json!({"replicaSpecifications": case.replicas.clone()}),
        )
        .await;
        match case.stays_provisioned {
            true => {
                assert_ok(case.name, resubmitted);
            }
            false => assert_failure(case.name, resubmitted, 400, "ValidationException"),
        }
    }
}

/// RULE 2: the per-Region settings a mode change dropped are gone, not hidden — a table
/// moved back into provisioned mode reports its OWN read capacity in every Region, and no
/// auto scaling settings at all until something configures them again.
///
/// The rows differ along which override the table held before (read capacity units, both
/// settings) and the read capacity the table is provisioned with on the way back.
#[tokio::test]
async fn a_dropped_per_region_setting_does_not_come_back_with_provisioned_mode() {
    // (case/keyspace/table name, the per-Region overrides stored first, the read capacity
    // the table is provisioned with afterwards)
    let rows: [(&str, Value, i64); 2] = [
        (
            "returning_with_a_units_override",
            json!([{"region": OTHER_REGIONS[0], "readCapacityUnits": 9}]),
            7,
        ),
        (
            "returning_with_both_overrides",
            json!([{
                "region": OTHER_REGIONS[0],
                "readCapacityUnits": 9,
                "readCapacityAutoScaling": enabled(9, 90, 65.0),
            }]),
            21,
        ),
    ];

    let emulator = Emulator::start();
    let regions = [REGION, OTHER_REGIONS[0]];

    for (name, replicas, _) in &rows {
        create_multi_region_table(&emulator, name, name, &regions, replicas).await;
    }

    tokio::time::sleep(settle_wait()).await;
    for (name, _, _) in &rows {
        assert_ok(
            name,
            update_table(
                &emulator,
                name,
                json!({"capacitySpecification": pay_per_request()}),
            )
            .await,
        );
    }

    tokio::time::sleep(settle_wait()).await;
    for (name, _, read_units) in &rows {
        assert_per_region_settings(&emulator, name, name, &regions, &[None, None], &[None, None])
            .await;
        assert_ok(
            name,
            update_table(
                &emulator,
                name,
                json!({"capacitySpecification": provisioned(*read_units, TABLE_WRITE_UNITS)}),
            )
            .await,
        );
    }

    tokio::time::sleep(settle_wait()).await;
    for (name, _, read_units) in &rows {
        // Every Region is back on the table's own read capacity, not on the override it
        // used to hold, and no auto scaling settings came back with the mode.
        assert_per_region_settings(
            &emulator,
            name,
            name,
            &regions,
            &[Some(*read_units), Some(*read_units)],
            &[None, None],
        )
        .await;
        let scaling = get_auto_scaling(&emulator, name, name).await;
        assert!(
            scaling["autoScalingSpecification"].is_null(),
            "case {name:?}: the move out of provisioned mode took the table's auto scaling \
             settings with it, got {scaling}"
        );
    }
}
