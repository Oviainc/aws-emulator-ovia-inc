// SPDX-License-Identifier: CC0-1.0

//! What `CreateTable` does with the two members a multi-Region table is given its
//! per-Region capacity and its pre-warmed throughput with — `replicaSpecifications` and
//! `warmThroughputSpecification` — driven end to end over the service's own `awsJson1_0`
//! wire protocol through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness,
//! so what is pinned is the emulator's observable behaviour rather than its internals.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. Every optional member `CreateTableRequest` models is ordinary input: a well-formed
//!    create that carries it answers HTTP 200 with the table's `resourceArn` and stores the
//!    table, whichever subset of those members it names. `CreateTable` models exactly
//!    `AccessDeniedException`, `ConflictException`, `InternalServerException`,
//!    `ResourceNotFoundException`, `ServiceQuotaExceededException` and
//!    `ValidationException` — no not-implemented error of any status — so naming a
//!    modelled member is never itself a reason to refuse the create.
//! 2. What a create stores, a read reports. `replicaSpecifications` configures "the table's
//!    read capacity differently per Amazon Web Services Region": a Region an entry gives
//!    `readCapacityUnits` reports that override through `GetTable` and one it gives
//!    `readCapacityAutoScaling` reports that override through
//!    `GetTableAutoScalingSettings`, while a Region the create left alone — named by an
//!    entry carrying neither override, or named by no entry at all — keeps reporting the
//!    table's own. `warmThroughputSpecification`'s two unit counts come back as
//!    `GetTable`'s `warmThroughputSpecification` summary, for the table and for every
//!    Region it is replicated in.
//! 3. The pre-warming is a transition the clock owes, like the table's own creation: the
//!    summary reports `UPDATING` "when changes are being applied" and `AVAILABLE` "when
//!    the configuration is active", and the unit counts do not move when it settles.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::{
    TABLE_STATUS_VALUES, WARM_THROUGHPUT_STATUS_VALUES,
};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::column_types_and_auto_scaling::assert_member;
use crate::keyspace_table_lifecycle::{
    assert_ok, minimal_schema, settle_wait, table_arn, Emulator, REGION,
};

/// Regions other than the request's own ([`REGION`]) a keyspace can be replicated in. Each
/// is within `region`'s modelled member length (2..25).
const OTHER_REGIONS: [&str; 4] = ["eu-west-1", "ap-south-1", "us-east-2", "sa-east-1"];

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
/// `writeCapacityUnits` as input", and the only one whose read capacity a
/// `ReplicaSpecification` can override.
fn provisioned(read: i64, write: i64) -> Value {
    json!({
        "throughputMode": "PROVISIONED",
        "readCapacityUnits": read,
        "writeCapacityUnits": write,
    })
}

/// `CreateKeyspace`, replicated over `regions` when the case names more than one.
async fn create_keyspace(emulator: &Emulator, case: &str, name: &str, regions: Option<&[&str]>) {
    let mut request = json!({"keyspaceName": name});
    if let Some(regions) = regions {
        request["replicationSpecification"] = json!({
            "replicationStrategy": "MULTI_REGION",
            "regionList": regions,
        });
    }
    assert_ok(case, emulator.call("CreateKeyspace", request).await);
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

/// The `tableName`s `ListTables` reports for one keyspace.
async fn listed_tables(emulator: &Emulator, case: &str, keyspace: &str) -> Vec<String> {
    let body = assert_ok(
        case,
        emulator
            .call("ListTables", json!({"keyspaceName": keyspace}))
            .await,
    );
    body["tables"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListTables must report a `tables` list: {body}"))
        .iter()
        .map(|summary| summary["tableName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// What one Region of a created table must report: its own read capacity units, and the
/// `minimumUnits` of the read capacity auto scaling settings that apply there (`null` for a
/// Region whose table and entry configure no auto scaling at all).
struct RegionExpectation {
    region: &'static str,
    read_capacity_units: Value,
    read_auto_scaling_minimum: Value,
}

/// One create, and everything the two reads of the table it stored must then answer with.
struct Case {
    /// The case name, which is also the keyspace name and the table name.
    name: &'static str,
    /// The Regions the owning keyspace is replicated in, or `None` for a keyspace under
    /// the default single-Region strategy.
    regions: Option<&'static [&'static str]>,
    /// The `capacitySpecification` the create carries, if any.
    capacity: Option<Value>,
    /// The table-level `autoScalingSpecification` the create carries, if any.
    auto_scaling: Option<Value>,
    /// The `replicaSpecifications` list the create carries, if any.
    replicas: Option<Value>,
    /// The `warmThroughputSpecification` the create carries, if any.
    warm: Option<Value>,
    /// One entry per Region the table is replicated in, in the keyspace's order; empty for
    /// a table of a single-Region keyspace, which reports no `replicaSpecifications`.
    per_region: Vec<RegionExpectation>,
    /// The `(readUnitsPerSecond, writeUnitsPerSecond)` the warm throughput summary must
    /// report, or `None` when nothing pre-warmed the table.
    warm_units: Option<(i64, i64)>,
}

/// RULES 1, 2 and 3: a create carrying `replicaSpecifications` and/or
/// `warmThroughputSpecification` is served, and both reads of the table report what it
/// stored.
///
/// The rows differ along what the rules generalise over: which of the two members the
/// create names (each on its own, both together, neither), how many entries the list
/// carries (one, one per Region of a three-Region keyspace), which override each entry
/// carries (`readCapacityUnits` only, `readCapacityAutoScaling` only, both, neither), the
/// keyspace's replication strategy (single-Region and `MULTI_REGION` over two and three
/// Regions), the table's capacity mode (`PROVISIONED` and the default `PAY_PER_REQUEST`)
/// and the warm unit counts (the modelled minimum 1, and counts of the order the service
/// pre-warms).
#[tokio::test]
async fn create_table_stores_the_per_region_and_warm_throughput_members_it_is_given() {
    let two_regions: &[&str] = &[REGION, OTHER_REGIONS[0]];
    let three_regions: &[&str] = &[OTHER_REGIONS[1], REGION, OTHER_REGIONS[2]];
    let also_two_regions: &[&str] = &[REGION, OTHER_REGIONS[3]];

    let cases: Vec<Case> = vec![
        Case {
            // One entry, overriding only the read capacity units, for one of two Regions.
            name: "replica_units_for_one_region",
            regions: Some(two_regions),
            capacity: Some(provisioned(5, 3)),
            auto_scaling: None,
            replicas: Some(json!([{"region": OTHER_REGIONS[0], "readCapacityUnits": 11}])),
            warm: None,
            per_region: vec![
                RegionExpectation {
                    region: REGION,
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(null),
                },
                RegionExpectation {
                    region: OTHER_REGIONS[0],
                    read_capacity_units: json!(11),
                    read_auto_scaling_minimum: json!(null),
                },
            ],
            warm_units: None,
        },
        Case {
            // One entry overriding only the read capacity AUTO SCALING: the Region keeps
            // the table's own read capacity units and takes the entry's scaling bounds.
            name: "replica_auto_scaling_only",
            regions: Some(two_regions),
            capacity: Some(provisioned(5, 3)),
            auto_scaling: Some(json!({"readCapacityAutoScaling": enabled(5, 50, 60.0)})),
            replicas: Some(json!([{
                "region": OTHER_REGIONS[0],
                "readCapacityAutoScaling": enabled(9, 90, 70.0),
            }])),
            warm: None,
            per_region: vec![
                RegionExpectation {
                    region: REGION,
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(5),
                },
                RegionExpectation {
                    region: OTHER_REGIONS[0],
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(9),
                },
            ],
            warm_units: None,
        },
        Case {
            // Both members at once, one entry per Region of a three-Region keyspace, with
            // the request's own Region in the middle of the list.
            name: "both_members_every_region",
            regions: Some(three_regions),
            capacity: Some(provisioned(7, 4)),
            auto_scaling: Some(json!({
                "readCapacityAutoScaling": enabled(7, 70, 60.0),
                "writeCapacityAutoScaling": enabled(7, 70, 60.0),
            })),
            replicas: Some(json!([
                {
                    "region": OTHER_REGIONS[1],
                    "readCapacityUnits": 21,
                    "readCapacityAutoScaling": enabled(21, 210, 80.0),
                },
                {"region": REGION, "readCapacityUnits": 22},
                {"region": OTHER_REGIONS[2], "readCapacityUnits": 23},
            ])),
            warm: Some(json!({"readUnitsPerSecond": 24000, "writeUnitsPerSecond": 9000})),
            per_region: vec![
                RegionExpectation {
                    region: OTHER_REGIONS[1],
                    read_capacity_units: json!(21),
                    read_auto_scaling_minimum: json!(21),
                },
                RegionExpectation {
                    region: REGION,
                    read_capacity_units: json!(22),
                    read_auto_scaling_minimum: json!(7),
                },
                RegionExpectation {
                    region: OTHER_REGIONS[2],
                    read_capacity_units: json!(23),
                    read_auto_scaling_minimum: json!(7),
                },
            ],
            warm_units: Some((24000, 9000)),
        },
        Case {
            // An entry carrying NEITHER override states that its Region uses the table's
            // own read capacity and auto scaling, which is what a Region with no entry
            // does.
            name: "replica_entry_with_neither_override",
            regions: Some(also_two_regions),
            capacity: Some(provisioned(5, 3)),
            auto_scaling: None,
            replicas: Some(json!([{"region": OTHER_REGIONS[3]}])),
            warm: None,
            per_region: vec![
                RegionExpectation {
                    region: REGION,
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(null),
                },
                RegionExpectation {
                    region: OTHER_REGIONS[3],
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(null),
                },
            ],
            warm_units: None,
        },
        Case {
            // Warm throughput on its own, at the modelled minimum of each unit count
            // ("The minimum value is 1", `WarmThroughputSpecification`), on a table of a
            // single-Region keyspace: pre-warming is a table setting, not a per-Region one.
            name: "warm_at_the_modelled_minimum",
            regions: None,
            capacity: Some(provisioned(5, 3)),
            auto_scaling: None,
            replicas: None,
            warm: Some(json!({"readUnitsPerSecond": 1, "writeUnitsPerSecond": 1})),
            per_region: Vec::new(),
            warm_units: Some((1, 1)),
        },
        Case {
            // `WarmThroughputSpecification` names no capacity mode, unlike
            // `autoScalingSpecification` ("for a table in provisioned capacity mode"), so
            // a table under the default `PAY_PER_REQUEST` mode is pre-warmed too — and
            // reports no capacity units, because that mode provisions none.
            name: "warm_on_a_pay_per_request_table",
            regions: None,
            capacity: None,
            auto_scaling: None,
            replicas: None,
            warm: Some(json!({"readUnitsPerSecond": 12000, "writeUnitsPerSecond": 4000})),
            per_region: Vec::new(),
            warm_units: Some((12000, 4000)),
        },
        Case {
            // The control row: the same keyspace and capacity, with NEITHER member named.
            name: "neither_member_named",
            regions: Some(two_regions),
            capacity: Some(provisioned(5, 3)),
            auto_scaling: None,
            replicas: None,
            warm: None,
            per_region: vec![
                RegionExpectation {
                    region: REGION,
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(null),
                },
                RegionExpectation {
                    region: OTHER_REGIONS[0],
                    read_capacity_units: json!(5),
                    read_auto_scaling_minimum: json!(null),
                },
            ],
            warm_units: None,
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        create_keyspace(&emulator, case.name, case.name, case.regions).await;

        let mut request = json!({
            "keyspaceName": case.name,
            "tableName": case.name,
            "schemaDefinition": minimal_schema("pk"),
        });
        for (member, value) in [
            ("capacitySpecification", case.capacity.as_ref()),
            ("autoScalingSpecification", case.auto_scaling.as_ref()),
            ("replicaSpecifications", case.replicas.as_ref()),
            ("warmThroughputSpecification", case.warm.as_ref()),
        ] {
            if let Some(value) = value {
                request[member] = value.clone();
            }
        }

        // RULE 1: the create is served, and answers with the ARN of the table it stored.
        let (status, body) = emulator.call("CreateTable", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: a create naming modelled members is served, got {status} {body}",
            case.name
        );
        assert_eq!(
            body["resourceArn"],
            json!(table_arn(case.name, case.name)),
            "case {:?}: the create answers with the table's ARN, got {body}",
            case.name
        );

        // RULE 1: a table was really stored, so the keyspace lists it.
        let listed = listed_tables(&emulator, case.name, case.name).await;
        assert_eq!(
            listed,
            vec![case.name.to_string()],
            "case {:?}: the created table must be listed, got {listed:?}",
            case.name
        );
    }

    // RULES 2 and 3, while the transitions the clock owes are still pending: the settings
    // the create stored are reported straight away, in the transient statuses.
    for case in &cases {
        let report = get_table(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_CREATING),
            "case {:?}: a table whose create was just accepted is CREATING, got {report}",
            case.name
        );
        assert_settings(
            case,
            &report,
            &get_auto_scaling(&emulator, case.name, case.name, case.name).await,
            keyspaces::TABLE_STATUS_CREATING,
            keyspaces::WARM_THROUGHPUT_STATUS_UPDATING,
        );
    }

    // RULE 3: the spans have elapsed, so the table is ACTIVE and the pre-warming is
    // AVAILABLE — with every unit count exactly where the create put it.
    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let report = get_table(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the creation span has elapsed, so the table is ACTIVE, got {report}",
            case.name
        );
        let scaling = get_auto_scaling(&emulator, case.name, case.name, case.name).await;
        assert_settings(
            case,
            &report,
            &scaling,
            keyspaces::TABLE_STATUS_ACTIVE,
            keyspaces::WARM_THROUGHPUT_STATUS_AVAILABLE,
        );

        // Both reports are reads: the next pair answers identically.
        let again = get_table(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            report, again,
            "case {:?}: a settled table reads back identically, got {again}",
            case.name
        );
        let scaling_again = get_auto_scaling(&emulator, case.name, case.name, case.name).await;
        assert_eq!(
            scaling, scaling_again,
            "case {:?}: the auto scaling settings read back identically, got {scaling_again}",
            case.name
        );
    }
}

/// Assert one case's stored settings against the pair of reports that carry them: the
/// per-Region read capacity and warm throughput from `GetTable`, and the per-Region read
/// capacity auto scaling from `GetTableAutoScalingSettings`.
fn assert_settings(
    case: &Case,
    report: &Value,
    scaling: &Value,
    expected_table_status: &str,
    expected_warm_status: &str,
) {
    let name = case.name;

    match &case.warm_units {
        None => assert!(
            report["warmThroughputSpecification"].is_null(),
            "case {name:?}: nothing pre-warmed this table, so it reports no \
             warmThroughputSpecification, got {report}"
        ),
        Some((read, write)) => {
            for (pointer, expected) in [
                ("readUnitsPerSecond", json!(read)),
                ("writeUnitsPerSecond", json!(write)),
                ("status", json!(expected_warm_status)),
            ] {
                assert_member(
                    name,
                    report,
                    &format!("/warmThroughputSpecification/{pointer}"),
                    &expected,
                );
            }
            let reported = report["warmThroughputSpecification"]["status"]
                .as_str()
                .unwrap_or_default();
            assert!(
                WARM_THROUGHPUT_STATUS_VALUES.contains(&reported),
                "case {name:?}: {reported:?} is not a modelled WarmThroughputStatus value, \
                 got {report}"
            );
        }
    }

    if case.per_region.is_empty() {
        assert!(
            report["replicaSpecifications"].is_null(),
            "case {name:?}: a table of a single-Region keyspace reports no \
             replicaSpecifications, got {report}"
        );
        assert!(
            scaling["replicaSpecifications"].is_null(),
            "case {name:?}: a table of a single-Region keyspace reports no per-Region auto \
             scaling settings, got {scaling}"
        );
        return;
    }

    for (source, body) in [("GetTable", report), ("GetTableAutoScalingSettings", scaling)] {
        let entries = body["replicaSpecifications"].as_array().unwrap_or_else(|| {
            panic!(
                "case {name:?}: {source} must report one entry per replication Region, \
                 got {body}"
            )
        });
        assert_eq!(
            entries.len(),
            case.per_region.len(),
            "case {name:?}: {source} must report {} entries, got {body}",
            case.per_region.len()
        );
    }

    for (index, expected) in case.per_region.iter().enumerate() {
        let region = expected.region;
        for (source, body) in [("GetTable", report), ("GetTableAutoScalingSettings", scaling)] {
            let reported = body.pointer(&format!("/replicaSpecifications/{index}/region"));
            assert_eq!(
                reported,
                Some(&json!(region)),
                "case {name:?}: {source} entry {index} must be the keyspace's Region \
                 {region:?}, got {reported:?} in {body}"
            );
        }

        assert_member(
            name,
            report,
            &format!("/replicaSpecifications/{index}/capacitySpecification/readCapacityUnits"),
            &expected.read_capacity_units,
        );
        assert_member(
            name,
            scaling,
            &format!(
                "/replicaSpecifications/{index}/autoScalingSpecification\
                 /readCapacityAutoScaling/minimumUnits"
            ),
            &expected.read_auto_scaling_minimum,
        );

        let status = report
            .pointer(&format!("/replicaSpecifications/{index}/status"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            TABLE_STATUS_VALUES.contains(&status),
            "case {name:?}: entry {index} status {status:?} is not a modelled TableStatus \
             value, got {report}"
        );
        assert_eq!(
            status, expected_table_status,
            "case {name:?}: the table's status in {region:?} is the status it holds, got {report}"
        );

        // Warm throughput is configured for the TABLE (`WarmThroughputSpecification` carries
        // no Region, and `ReplicaSpecification` no warm throughput), so every Region reports
        // the table's summary.
        let per_region_warm =
            report.pointer(&format!("/replicaSpecifications/{index}/warmThroughputSpecification"));
        assert_eq!(
            per_region_warm.unwrap_or(&Value::Null),
            &report["warmThroughputSpecification"],
            "case {name:?}: every replica reports the table's warm throughput, got {report}"
        );
    }
}
