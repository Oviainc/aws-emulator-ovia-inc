// SPDX-License-Identifier: CC0-1.0

//! The table settings `UpdateTable` changes AFTER a table exists — auto scaling, the
//! Amazon Web Services Region specific settings of a multi-Region table, and warm
//! throughput — driven end to end over the service's own `awsJson1_0` wire protocol
//! through the shared [`crate::keyspace_table_lifecycle::Emulator`] harness, so what is
//! pinned is the emulator's observable behaviour: a change is believed only once the
//! operation that reports it answers with it.
//!
//! The rules under test, each exercised over a SET of inputs rather than one example:
//!
//! 1. Every setting `UpdateTableRequest` models is applied to the stored table and
//!    reported back by the operation that reads it — "Adds new columns to the table or
//!    updates one of the table's settings, for example capacity mode, auto scaling,
//!    encryption at rest, or point-in-time recovery". A well-formed call answers HTTP 200
//!    with the table's `resourceArn` and leaves the table `UPDATING` until the clock owes
//!    it `ACTIVE`; none of these members is answered with a status the operation does not
//!    model.
//! 2. The two capacities of `autoScalingSpecification` are configured independently of
//!    each other ("you can use UpdateTable to update the minimum and maximum values or the
//!    auto scaling policy settings independently"), so a request carrying one leaves the
//!    other's settings where they were.
//! 3. `replicaSpecifications` configures "the table's read capacity differently per Amazon
//!    Web Services Region": a Region it names reports that override, and every other
//!    Region the table is replicated in keeps reporting the table's own read capacity and
//!    auto scaling. The write capacity has no per-Region override, because it "remains
//!    synchronized between all replicas".
//! 4. `warmThroughputSpecification` is reported as the summary `GetTable` carries, whose
//!    status is "`AVAILABLE` when the configuration is active, and `UPDATING` when changes
//!    are being applied"; a later update of one unit count keeps the other.
//! 5. A request that breaks a rule the model cannot state is refused with the error the
//!    model declares for it — never with the scaffold's 501 — and the refusal leaves the
//!    table exactly as it was.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::ops::types::WARM_THROUGHPUT_STATUS_VALUES;
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::column_types_and_auto_scaling::assert_member;
use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, table_arn, Emulator, REGION,
};

/// Regions other than the request's own ([`REGION`]) a keyspace can be replicated in. Each
/// is within `RegionList`'s modelled member length (2..25).
const OTHER_REGIONS: [&str; 2] = ["eu-west-1", "ap-south-1"];

/// An `AutoScalingSettings` with auto scaling turned on: "This optional parameter enables
/// auto scaling for the table if set to `false`", and then "you must specify ...
/// `minimumUnits`, `maximumUnits` and `scalingPolicy`".
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

/// A provisioned capacity specification, the only mode whose read and write capacity are
/// provisioned at all.
fn provisioned(read: i64, write: i64) -> Value {
    json!({
        "throughputMode": "PROVISIONED",
        "readCapacityUnits": read,
        "writeCapacityUnits": write,
    })
}

/// `CreateKeyspace`, optionally with a replication specification.
async fn create_keyspace(emulator: &Emulator, name: &str, replication: Option<Value>) {
    let mut request = json!({"keyspaceName": name});
    if let Some(replication) = replication {
        request["replicationSpecification"] = replication;
    }
    assert_ok(name, emulator.call("CreateKeyspace", request).await);
}

/// `CreateTable` with the default minimal schema plus `members`.
async fn create_table(emulator: &Emulator, keyspace: &str, table: &str, members: Value) -> Value {
    let mut request = members;
    request["keyspaceName"] = json!(keyspace);
    request["tableName"] = json!(table);
    request["schemaDefinition"] = minimal_schema("pk");
    assert_ok(table, emulator.call("CreateTable", request).await)
}

/// `UpdateTable` for one table, carrying `members`.
async fn update_table(
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    members: Value,
) -> (u16, Value) {
    let mut request = members;
    request["keyspaceName"] = json!(keyspace);
    request["tableName"] = json!(table);
    emulator.call("UpdateTable", request).await
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

/// RULES 1 and 2: `UpdateTable` is the operation that changes a table's auto scaling after
/// creation, and `GetTableAutoScalingSettings` reports the NEW settings.
///
/// The rows differ along what the rule generalises over: whether the table had auto scaling
/// before the update, which capacity the update configures (read, write, both), whether it
/// turns auto scaling on or off, the unit bounds (the modelled minimum 1 and an equal
/// min/max among them) and the target value — including each published bound of the
/// `targetValue` range (20..=90, from `keyspaces::MIN_TARGET_VALUE` /
/// `keyspaces::MAX_TARGET_VALUE`).
#[tokio::test]
async fn update_table_applies_auto_scaling_and_reports_the_new_settings() {
    struct Case {
        /// The case name, which is also the table name.
        name: &'static str,
        /// The `autoScalingSpecification` the table is CREATED with, if any.
        create: Option<Value>,
        /// The `autoScalingSpecification` the update carries.
        update: Value,
        /// `(JSON pointer under autoScalingSpecification, expected value)` pairs the
        /// updated table must report.
        expect: Vec<(&'static str, Value)>,
    }

    let cases: Vec<Case> = vec![
        Case {
            // A table created without auto scaling: the update is its FIRST configuration,
            // which is what makes the setting reportable at all.
            name: "first_configuration_of_both_capacities",
            create: None,
            update: json!({
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            }),
            expect: vec![
                ("/readCapacityAutoScaling/minimumUnits", json!(5)),
                ("/readCapacityAutoScaling/maximumUnits", json!(50)),
                ("/readCapacityAutoScaling/autoScalingDisabled", json!(false)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(60.0),
                ),
                ("/writeCapacityAutoScaling/minimumUnits", json!(5)),
                ("/writeCapacityAutoScaling/maximumUnits", json!(50)),
            ],
        },
        Case {
            // RULE 2: only the write capacity is named, so the read capacity keeps the
            // settings the create gave it.
            name: "write_capacity_raised_on_its_own",
            create: Some(json!({
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            })),
            update: json!({"writeCapacityAutoScaling": enabled(10, 100, 70.0)}),
            expect: vec![
                ("/writeCapacityAutoScaling/minimumUnits", json!(10)),
                ("/writeCapacityAutoScaling/maximumUnits", json!(100)),
                (
                    "/writeCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(70.0),
                ),
                ("/readCapacityAutoScaling/minimumUnits", json!(5)),
                ("/readCapacityAutoScaling/maximumUnits", json!(50)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(60.0),
                ),
            ],
        },
        Case {
            // The bottom of the published targetValue range, and the modelled minimum of
            // the unit members (1).
            name: "read_capacity_at_the_bottom_of_the_target_range",
            create: Some(json!({"readCapacityAutoScaling": enabled(5, 50, 60.0)})),
            update: json!({"readCapacityAutoScaling": enabled(1, 40000, 20.0)}),
            expect: vec![
                ("/readCapacityAutoScaling/minimumUnits", json!(1)),
                ("/readCapacityAutoScaling/maximumUnits", json!(40000)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(20.0),
                ),
                ("/writeCapacityAutoScaling", Value::Null),
            ],
        },
        Case {
            // The top of the same range, with min == max.
            name: "read_capacity_at_the_top_of_the_target_range",
            create: None,
            update: json!({"readCapacityAutoScaling": enabled(7, 7, 90.0)}),
            expect: vec![
                ("/readCapacityAutoScaling/minimumUnits", json!(7)),
                ("/readCapacityAutoScaling/maximumUnits", json!(7)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(90.0),
                ),
            ],
        },
        Case {
            // "This optional parameter enables auto scaling for the table if set to false":
            // an update is also how auto scaling is turned OFF again.
            name: "auto_scaling_disabled_again",
            create: Some(json!({
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            })),
            update: json!({
                "readCapacityAutoScaling": {"autoScalingDisabled": true},
                "writeCapacityAutoScaling": {"autoScalingDisabled": true},
            }),
            expect: vec![
                ("/readCapacityAutoScaling/autoScalingDisabled", json!(true)),
                ("/writeCapacityAutoScaling/autoScalingDisabled", json!(true)),
            ],
        },
        Case {
            // The policy's optional knobs travel with the settings they belong to.
            name: "scale_in_and_cooldowns_updated",
            create: Some(json!({"readCapacityAutoScaling": enabled(5, 50, 60.0)})),
            update: json!({"readCapacityAutoScaling": {
                "autoScalingDisabled": false,
                "minimumUnits": 10,
                "maximumUnits": 100,
                "scalingPolicy": {
                    "targetTrackingScalingPolicyConfiguration": {
                        "targetValue": 45.5,
                        "disableScaleIn": true,
                        "scaleInCooldown": 120,
                        "scaleOutCooldown": 60,
                    },
                },
            }}),
            expect: vec![
                ("/readCapacityAutoScaling/minimumUnits", json!(10)),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue",
                    json!(45.5),
                ),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/disableScaleIn",
                    json!(true),
                ),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/scaleInCooldown",
                    json!(120),
                ),
                (
                    "/readCapacityAutoScaling/scalingPolicy/targetTrackingScalingPolicyConfiguration/scaleOutCooldown",
                    json!(60),
                ),
            ],
        },
    ];

    let emulator = Emulator::start();
    let keyspace = "update_auto_scaling_ks";
    create_keyspace(&emulator, keyspace, None).await;

    // Every row's table is provisioned — "The optional auto scaling settings for a table in
    // provisioned capacity mode" — and carries whatever auto scaling the row creates it
    // with.
    for case in &cases {
        let mut members = json!({"capacitySpecification": provisioned(5, 5)});
        if let Some(create) = &case.create {
            members["autoScalingSpecification"] = create.clone();
        }
        let created = create_table(&emulator, keyspace, case.name, members).await;
        assert_eq!(
            created["resourceArn"],
            json!(table_arn(keyspace, case.name)),
            "case {:?}: the create must answer with the table's ARN, got {created}",
            case.name
        );
    }
    // A table takes an update only once it is ACTIVE.
    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        let body = assert_ok(
            case.name,
            update_table(
                &emulator,
                keyspace,
                case.name,
                json!({"autoScalingSpecification": case.update.clone()}),
            )
            .await,
        );
        assert_eq!(
            body["resourceArn"],
            json!(table_arn(keyspace, case.name)),
            "case {:?}: the update answers with the modified table's ARN, got {body}",
            case.name
        );

        // RULE 1: the change is accepted asynchronously, so the table is UPDATING until the
        // clock owes it ACTIVE again — and the new settings are already what is reported.
        let report = get_table(&emulator, case.name, keyspace, case.name).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_UPDATING),
            "case {:?}: an accepted update leaves the table UPDATING, got {report}",
            case.name
        );
        assert_settings(&emulator, case.name, keyspace, case.name, &case.expect).await;
    }

    tokio::time::sleep(settle_wait()).await;
    for case in &cases {
        let report = get_table(&emulator, case.name, keyspace, case.name).await;
        assert_eq!(
            report["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: the update span has elapsed, so the table is ACTIVE, got {report}",
            case.name
        );
        // Updating the auto scaling changed nothing else about the table.
        assert_member(
            case.name,
            &report,
            "/capacitySpecification",
            &json!({
                "throughputMode": "PROVISIONED",
                "readCapacityUnits": 5,
                "writeCapacityUnits": 5,
            }),
        );
        assert_settings(&emulator, case.name, keyspace, case.name, &case.expect).await;
    }
}

/// Assert one table's `GetTableAutoScalingSettings` body against `expect`, a list of
/// pointers under `autoScalingSpecification`.
async fn assert_settings(
    emulator: &Emulator,
    case: &str,
    keyspace: &str,
    table: &str,
    expect: &[(&str, Value)],
) {
    let got = get_auto_scaling(emulator, case, keyspace, table).await;
    assert_member(case, &got, "/resourceArn", &json!(table_arn(keyspace, table)));
    for (pointer, expected) in expect {
        assert_member(
            case,
            &got,
            &format!("/autoScalingSpecification{pointer}"),
            expected,
        );
    }
}

/// RULE 3: `replicaSpecifications` configures the read capacity and the read capacity auto
/// scaling of a multi-Region table PER Region, and both reports carry the result — the
/// named Region's override, and the table's own settings everywhere else.
///
/// The rows differ along how many Regions one request names (one, two at once), which
/// override it carries (read capacity units, read capacity auto scaling, both, neither) and
/// the unit values (the modelled minimum 1 among them). Each row is applied to the SAME
/// table in sequence, so the rows also assert that a request naming one Region leaves the
/// Regions it does not name exactly as they were.
#[tokio::test]
async fn update_table_applies_per_region_replica_settings_and_reports_them_back() {
    struct Case {
        name: &'static str,
        /// The `replicaSpecifications` list the update carries.
        update: Value,
        /// The read capacity units each Region must report afterwards, in the keyspace's
        /// Region order.
        expected_read_units: [i64; 3],
        /// The `minimumUnits` each Region's `readCapacityAutoScaling` must report
        /// afterwards, or `None` where the Region reports the table's own settings.
        expected_read_minimums: [i64; 3],
    }

    let regions = [REGION, OTHER_REGIONS[0], OTHER_REGIONS[1]];
    let cases: Vec<Case> = vec![
        Case {
            // One Region, read capacity units only.
            name: "one_region_read_units",
            update: json!([{"region": OTHER_REGIONS[0], "readCapacityUnits": 9}]),
            expected_read_units: [5, 9, 5],
            expected_read_minimums: [5, 5, 5],
        },
        Case {
            // A second Region, with the read capacity auto scaling as well — and the first
            // Region's override must survive.
            name: "second_region_units_and_auto_scaling",
            update: json!([{
                "region": OTHER_REGIONS[1],
                "readCapacityUnits": 40000,
                "readCapacityAutoScaling": enabled(11, 4000, 75.0),
            }]),
            expected_read_units: [5, 9, 40000],
            expected_read_minimums: [5, 5, 11],
        },
        Case {
            // Two Regions at once, one of them the request's own, at the modelled minimum
            // of the unit members.
            name: "two_regions_at_once",
            update: json!([
                {"region": REGION, "readCapacityUnits": 1},
                {
                    "region": OTHER_REGIONS[0],
                    "readCapacityUnits": 3,
                    "readCapacityAutoScaling": enabled(2, 20, 20.0),
                },
            ]),
            expected_read_units: [1, 3, 40000],
            expected_read_minimums: [5, 2, 11],
        },
        Case {
            // An entry carrying neither override states that its Region uses the table's
            // own read capacity and auto scaling again.
            name: "a_region_reset_to_the_table_settings",
            update: json!([{"region": OTHER_REGIONS[0]}]),
            expected_read_units: [1, 5, 40000],
            expected_read_minimums: [5, 5, 11],
        },
    ];

    let emulator = Emulator::start();
    let keyspace = "replica_settings_ks";
    create_keyspace(
        &emulator,
        keyspace,
        Some(json!({"replicationStrategy": "MULTI_REGION", "regionList": regions})),
    )
    .await;
    let table = "replica_settings_tbl";
    let created = create_table(
        &emulator,
        keyspace,
        table,
        json!({
            "capacitySpecification": provisioned(5, 3),
            "autoScalingSpecification": {
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            },
        }),
    )
    .await;
    assert_eq!(
        created["resourceArn"],
        json!(table_arn(keyspace, table)),
        "the create must answer with the table's ARN, got {created}"
    );

    for case in &cases {
        tokio::time::sleep(settle_wait()).await;
        assert_ok(
            case.name,
            update_table(
                &emulator,
                keyspace,
                table,
                json!({"replicaSpecifications": case.update.clone()}),
            )
            .await,
        );

        let report = get_table(&emulator, case.name, keyspace, table).await;
        let entries = report["replicaSpecifications"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a multi-Region table reports one entry per replication \
                     Region, got {report}",
                    case.name
                )
            });
        assert_eq!(
            entries.len(),
            regions.len(),
            "case {:?}: expected one entry per Region of {regions:?}, got {report}",
            case.name
        );
        let scaling = get_auto_scaling(&emulator, case.name, keyspace, table).await;
        let scaling_entries = scaling["replicaSpecifications"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the auto scaling report carries one entry per replication \
                     Region, got {scaling}",
                    case.name
                )
            });

        for (index, region) in regions.iter().enumerate() {
            let entry = &entries[index];
            assert_eq!(
                entry["region"],
                json!(region),
                "case {:?}: the entries follow the keyspace's Region list, got {report}",
                case.name
            );
            assert_eq!(
                entry["capacitySpecification"]["readCapacityUnits"],
                json!(case.expected_read_units[index]),
                "case {:?}: Region {region} provisions {} read capacity units, got {report}",
                case.name,
                case.expected_read_units[index]
            );
            // The write capacity has no per-Region override: it "remains synchronized
            // between all replicas".
            assert_eq!(
                entry["capacitySpecification"]["writeCapacityUnits"],
                json!(3),
                "case {:?}: Region {region} keeps the table's write capacity, got {report}",
                case.name
            );

            let scaling_entry = &scaling_entries[index];
            assert_eq!(
                scaling_entry["region"],
                json!(region),
                "case {:?}: the auto scaling entries follow the same Region list, got {scaling}",
                case.name
            );
            assert_member(
                case.name,
                scaling_entry,
                "/autoScalingSpecification/readCapacityAutoScaling/minimumUnits",
                &json!(case.expected_read_minimums[index]),
            );
            // Only the READ capacity is configurable per Region, so every Region reports
            // the table's own write capacity auto scaling.
            assert_member(
                case.name,
                scaling_entry,
                "/autoScalingSpecification/writeCapacityAutoScaling/minimumUnits",
                &json!(5),
            );
        }

        // The table's own settings are untouched by a per-Region override.
        assert_member(case.name, &report, "/capacitySpecification/readCapacityUnits", &json!(5));
        assert_member(
            case.name,
            &scaling,
            "/autoScalingSpecification/readCapacityAutoScaling/minimumUnits",
            &json!(5),
        );
    }
}

/// RULE 4: `warmThroughputSpecification` is stored and reported as the summary `GetTable`
/// carries, for the table and for each Region it is replicated in, with the status the
/// clock owes it.
///
/// The rows differ along which unit counts one request carries (both, read only, write
/// only) and their values (the modelled minimum 1, and counts of the order the service
/// pre-warms), so the rule that an omitted unit keeps the configured one is asserted
/// across the class.
#[tokio::test]
async fn update_table_applies_warm_throughput_and_reports_the_summary() {
    // (case, the warmThroughputSpecification the update carries, expected read, expected write)
    let cases: [(&str, Value, i64, i64); 4] = [
        (
            "first_configuration_of_both_units",
            json!({"readUnitsPerSecond": 12000, "writeUnitsPerSecond": 4000}),
            12000,
            4000,
        ),
        (
            "read_units_updated_on_their_own",
            json!({"readUnitsPerSecond": 24000}),
            24000,
            4000,
        ),
        (
            "write_units_updated_on_their_own",
            json!({"writeUnitsPerSecond": 1}),
            24000,
            1,
        ),
        (
            "both_units_at_the_modelled_minimum",
            json!({"readUnitsPerSecond": 1, "writeUnitsPerSecond": 1}),
            1,
            1,
        ),
    ];

    let emulator = Emulator::start();
    let keyspace = "warm_throughput_ks";
    create_keyspace(
        &emulator,
        keyspace,
        Some(json!({
            "replicationStrategy": "MULTI_REGION",
            "regionList": [REGION, OTHER_REGIONS[0]],
        })),
    )
    .await;
    let table = "warm_throughput_tbl";
    create_table(
        &emulator,
        keyspace,
        table,
        json!({"capacitySpecification": provisioned(5, 5)}),
    )
    .await;

    // Nothing pre-warmed the table yet, so it reports no warm throughput at all.
    let report = get_table(&emulator, "before_any_update", keyspace, table).await;
    assert!(
        report["warmThroughputSpecification"].is_null(),
        "a table nothing pre-warmed reports no warmThroughputSpecification, got {report}"
    );

    for (case, members, expected_read, expected_write) in &cases {
        tokio::time::sleep(settle_wait()).await;
        assert_ok(
            case,
            update_table(
                &emulator,
                keyspace,
                table,
                json!({"warmThroughputSpecification": members.clone()}),
            )
            .await,
        );

        // "UPDATING when changes are being applied": the configuration the caller asked for
        // is reported straight away, in the status the clock owes it.
        let report = get_table(&emulator, case, keyspace, table).await;
        for (pointer, expected) in [
            ("/readUnitsPerSecond", json!(expected_read)),
            ("/writeUnitsPerSecond", json!(expected_write)),
            (
                "/status",
                json!(keyspaces::WARM_THROUGHPUT_STATUS_UPDATING),
            ),
        ] {
            assert_member(
                case,
                &report,
                &format!("/warmThroughputSpecification{pointer}"),
                &expected,
            );
        }
        let reported_status = report["warmThroughputSpecification"]["status"]
            .as_str()
            .unwrap_or_default();
        assert!(
            WARM_THROUGHPUT_STATUS_VALUES.contains(&reported_status),
            "case {case:?}: {reported_status:?} is not a modelled WarmThroughputStatus value, \
             got {report}"
        );
        // Warm throughput is configured for the TABLE, so each Region it is replicated in
        // reports the same summary.
        let entries = report["replicaSpecifications"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected per-Region entries, got {report}"));
        for entry in &entries {
            assert_eq!(
                entry["warmThroughputSpecification"], report["warmThroughputSpecification"],
                "case {case:?}: every replica reports the table's warm throughput, got {report}"
            );
        }

        // "AVAILABLE when the configuration is active": the span the clock owed has
        // elapsed, and the unit counts did not move with it.
        tokio::time::sleep(settle_wait()).await;
        let settled = get_table(&emulator, case, keyspace, table).await;
        for (pointer, expected) in [
            ("/readUnitsPerSecond", json!(expected_read)),
            ("/writeUnitsPerSecond", json!(expected_write)),
            (
                "/status",
                json!(keyspaces::WARM_THROUGHPUT_STATUS_AVAILABLE),
            ),
        ] {
            assert_member(
                case,
                &settled,
                &format!("/warmThroughputSpecification{pointer}"),
                &expected,
            );
        }
    }
}

/// RULE 1 for `CreateTable`: the same three settings are accepted at creation time and read
/// back, so the create and the update agree on what a table may carry.
#[tokio::test]
async fn create_table_stores_per_region_settings_and_warm_throughput() {
    let emulator = Emulator::start();
    let keyspace = "created_with_settings_ks";
    create_keyspace(
        &emulator,
        keyspace,
        Some(json!({
            "replicationStrategy": "MULTI_REGION",
            "regionList": [REGION, OTHER_REGIONS[0]],
        })),
    )
    .await;
    let table = "created_with_settings_tbl";
    let created = create_table(
        &emulator,
        keyspace,
        table,
        json!({
            "capacitySpecification": provisioned(5, 5),
            "autoScalingSpecification": {
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            },
            "replicaSpecifications": [{
                "region": OTHER_REGIONS[0],
                "readCapacityUnits": 17,
                "readCapacityAutoScaling": enabled(17, 170, 80.0),
            }],
            "warmThroughputSpecification": {
                "readUnitsPerSecond": 8000,
                "writeUnitsPerSecond": 2000,
            },
        }),
    )
    .await;
    assert_eq!(
        created["resourceArn"],
        json!(table_arn(keyspace, table)),
        "the create must answer with the table's ARN, got {created}"
    );

    tokio::time::sleep(settle_wait()).await;
    let report = get_table(&emulator, "created", keyspace, table).await;
    // (JSON pointer, expected value): the Region the create overrode, the Region it did
    // not, and the warm throughput — all read back through `GetTable`.
    for (pointer, expected) in [
        ("/replicaSpecifications/0/region", json!(REGION)),
        (
            "/replicaSpecifications/0/capacitySpecification/readCapacityUnits",
            json!(5),
        ),
        ("/replicaSpecifications/1/region", json!(OTHER_REGIONS[0])),
        (
            "/replicaSpecifications/1/capacitySpecification/readCapacityUnits",
            json!(17),
        ),
        ("/warmThroughputSpecification/readUnitsPerSecond", json!(8000)),
        (
            "/warmThroughputSpecification/writeUnitsPerSecond",
            json!(2000),
        ),
        (
            "/warmThroughputSpecification/status",
            json!(keyspaces::WARM_THROUGHPUT_STATUS_AVAILABLE),
        ),
    ] {
        assert_member("created", &report, pointer, &expected);
    }

    let scaling = get_auto_scaling(&emulator, "created", keyspace, table).await;
    for (pointer, expected) in [
        (
            "/replicaSpecifications/0/autoScalingSpecification/readCapacityAutoScaling/minimumUnits",
            json!(5),
        ),
        (
            "/replicaSpecifications/1/autoScalingSpecification/readCapacityAutoScaling/minimumUnits",
            json!(17),
        ),
        (
            "/replicaSpecifications/1/autoScalingSpecification/writeCapacityAutoScaling/minimumUnits",
            json!(5),
        ),
    ] {
        assert_member("created", &scaling, pointer, &expected);
    }
}

/// RULE 5: every request these three settings can break a documented rule with is refused
/// with the modelled error for it, and the refusal leaves the table — and both reports of
/// it — exactly as they were.
#[tokio::test]
async fn a_setting_that_breaks_a_documented_rule_is_the_modelled_error() {
    let emulator = Emulator::start();
    let multi = "refused_settings_multi_ks";
    let single = "refused_settings_single_ks";
    create_keyspace(
        &emulator,
        multi,
        Some(json!({
            "replicationStrategy": "MULTI_REGION",
            "regionList": [REGION, OTHER_REGIONS[0]],
        })),
    )
    .await;
    create_keyspace(&emulator, single, None).await;

    let provisioned_table = "provisioned_tbl";
    let on_demand_table = "on_demand_tbl";
    let single_region_table = "single_region_tbl";
    create_table(
        &emulator,
        multi,
        provisioned_table,
        json!({
            "capacitySpecification": provisioned(5, 5),
            "autoScalingSpecification": {"readCapacityAutoScaling": enabled(5, 50, 60.0)},
        }),
    )
    .await;
    // No capacitySpecification at all: "The default is throughput_mode:PAY_PER_REQUEST."
    create_table(&emulator, multi, on_demand_table, json!({})).await;
    create_table(
        &emulator,
        single,
        single_region_table,
        json!({"capacitySpecification": provisioned(5, 5)}),
    )
    .await;
    tokio::time::sleep(settle_wait()).await;

    // (case, keyspace, table, the request members beyond the two names, status, code)
    let refusals: Vec<(&str, &str, &str, Value, u16, &str)> = vec![
        // "The optional auto scaling settings for a table in provisioned capacity mode."
        (
            "auto scaling on a PAY_PER_REQUEST table",
            multi,
            on_demand_table,
            json!({"autoScalingSpecification": {"readCapacityAutoScaling": enabled(5, 50, 60.0)}}),
            400,
            "ValidationException",
        ),
        // Turned on, so "you must specify ... minimumUnits, maximumUnits and scalingPolicy".
        (
            "auto scaling enabled without its bounds",
            multi,
            provisioned_table,
            json!({"autoScalingSpecification": {
                "writeCapacityAutoScaling": {"autoScalingDisabled": false},
            }}),
            400,
            "ValidationException",
        ),
        (
            "minimumUnits above maximumUnits",
            multi,
            provisioned_table,
            json!({"autoScalingSpecification": {
                "readCapacityAutoScaling": enabled(50, 5, 60.0),
            }}),
            400,
            "ValidationException",
        ),
        // "You define targetValue as a percentage. A double between 20 and 90": one step
        // outside each published bound.
        (
            "targetValue below the published range",
            multi,
            provisioned_table,
            json!({"autoScalingSpecification": {
                "readCapacityAutoScaling": enabled(5, 50, keyspaces::MIN_TARGET_VALUE - 0.1),
            }}),
            400,
            "ValidationException",
        ),
        (
            "targetValue above the published range",
            multi,
            provisioned_table,
            json!({"autoScalingSpecification": {
                "readCapacityAutoScaling": enabled(5, 50, keyspaces::MAX_TARGET_VALUE + 0.1),
            }}),
            400,
            "ValidationException",
        ),
        // "The Amazon Web Services Region specific settings of a multi-Region table": a
        // table of a single-Region keyspace is replicated in one Region and has none.
        (
            "per-Region settings for a single-Region table",
            single,
            single_region_table,
            json!({"replicaSpecifications": [{"region": REGION, "readCapacityUnits": 5}]}),
            400,
            "ValidationException",
        ),
        (
            "a Region the table is not replicated in",
            multi,
            provisioned_table,
            json!({"replicaSpecifications": [
                {"region": OTHER_REGIONS[1], "readCapacityUnits": 5},
            ]}),
            400,
            "ValidationException",
        ),
        (
            "the same Region twice",
            multi,
            provisioned_table,
            json!({"replicaSpecifications": [
                {"region": REGION, "readCapacityUnits": 5},
                {"region": REGION, "readCapacityUnits": 6},
            ]}),
            400,
            "ValidationException",
        ),
        (
            "a per-Region read capacity override on a PAY_PER_REQUEST table",
            multi,
            on_demand_table,
            json!({"replicaSpecifications": [{"region": REGION, "readCapacityUnits": 5}]}),
            400,
            "ValidationException",
        ),
        (
            "a per-Region auto scaling that breaks its own rule",
            multi,
            provisioned_table,
            json!({"replicaSpecifications": [{
                "region": REGION,
                "readCapacityAutoScaling": enabled(5, 50, keyspaces::MAX_TARGET_VALUE + 5.0),
            }]}),
            400,
            "ValidationException",
        ),
        // Both members of the summary are `@required`, and a unit that was never configured
        // has no value to keep.
        (
            "warm throughput with only one unit count on a table that has none",
            multi,
            provisioned_table,
            json!({"warmThroughputSpecification": {"readUnitsPerSecond": 12000}}),
            400,
            "ValidationException",
        ),
        // "Note that you can only update one specific table setting per update operation."
        (
            "two settings at once",
            multi,
            provisioned_table,
            json!({
                "autoScalingSpecification": {"readCapacityAutoScaling": enabled(5, 50, 60.0)},
                "warmThroughputSpecification": {
                    "readUnitsPerSecond": 1,
                    "writeUnitsPerSecond": 1,
                },
            }),
            400,
            "ValidationException",
        ),
        // Modelled constraints, refused at the boundary before the handler runs:
        // `readUnitsPerSecond` and `readCapacityUnits` are each `range 1..`.
        (
            "warm throughput below its modelled minimum",
            multi,
            provisioned_table,
            json!({"warmThroughputSpecification": {
                "readUnitsPerSecond": 0,
                "writeUnitsPerSecond": 1,
            }}),
            400,
            "ValidationException",
        ),
        (
            "a per-Region read capacity below its modelled minimum",
            multi,
            provisioned_table,
            json!({"replicaSpecifications": [{"region": REGION, "readCapacityUnits": 0}]}),
            400,
            "ValidationException",
        ),
    ];

    for (case, keyspace, table, members, status, code) in refusals {
        let before = get_table(&emulator, case, keyspace, table).await;
        assert_failure(
            case,
            update_table(&emulator, keyspace, table, members).await,
            status,
            code,
        );
        assert_eq!(
            get_table(&emulator, case, keyspace, table).await,
            before,
            "case {case:?}: a refused update must leave the table exactly as it was"
        );
    }

    // A table with no auto scaling settings at all — the table's own or a Region's — still
    // has none to report after every refusal above.
    assert_failure(
        "a table with no auto scaling settings",
        emulator
            .call(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": multi, "tableName": on_demand_table}),
            )
            .await,
        404,
        "ResourceNotFoundException",
    );
}
