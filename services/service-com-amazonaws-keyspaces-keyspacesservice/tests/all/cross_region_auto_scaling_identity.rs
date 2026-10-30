// SPDX-License-Identifier: CC0-1.0

//! A table's auto scaling settings are a resource of ONE Amazon Web Services Region,
//! driven end to end over both services' own wire protocols on ONE shared engine through
//! the two-service harness in [`super::auto_scaling_propagation`].
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a keyspace
//! and the tables it holds belong to the Region of the request that created them, so the
//! same keyspace and table names in another Region name a DIFFERENT table — and the
//! Application Auto Scaling identity its auto scaling settings live under is Region
//! qualified too. Therefore each Region's table holds its own scalable targets, policies
//! and reported windows, every write through either service touches only the Region it was
//! signed for, and a scalable target's ARN names the Region it is registered in.
//!
//! The rows differ along what the rule generalises over: how many Regions hold the same
//! names (two and three), which capacities each Region configures (read, write, both,
//! none), whether a Region turns auto scaling off, the unit bounds and target values each
//! Region configures, the keyspace and table names the `ResourceId` is built from, and —
//! for the second rule — WHICH write one Region performs (delete the table, delete the
//! keyspace, move the table out of provisioned capacity mode, re-configure the window).

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::auto_scaling_propagation::{assert_member, enabled, resource_id, Emulator};
use super::keyspace_table_lifecycle::{error_code, settle_wait};

/// One capacity's window, as a Region configures it: `minimumUnits`, `maximumUnits` and
/// the target-tracking policy's `targetValue`.
type Window = (i64, i64, f64);

/// What one Region configures for a keyspace/table name pair every Region of a row shares.
struct RegionSettings {
    /// The Region the creating request is signed for.
    region: &'static str,
    /// The read capacity's window, when that Region turns read auto scaling on.
    read: Option<Window>,
    /// The write capacity's window, when that Region turns write auto scaling on.
    write: Option<Window>,
    /// Whether the Region's request says `autoScalingDisabled: true` for the capacities it
    /// gives no window — a setting the table HAS, as opposed to never configuring one.
    disables_the_rest: bool,
}

/// One row: the names every Region of the row uses, and what each Region configures.
struct Case {
    name: &'static str,
    keyspace: &'static str,
    table: &'static str,
    regions: Vec<RegionSettings>,
}

/// The `autoScalingSpecification` a Region's `CreateTable` carries, or `None` when that
/// Region configures no auto scaling at all.
fn specification(settings: &RegionSettings) -> Option<Value> {
    let mut specification = json!({});
    for (member, window) in [
        ("readCapacityAutoScaling", settings.read),
        ("writeCapacityAutoScaling", settings.write),
    ] {
        match window {
            Some((minimum, maximum, target)) => {
                specification[member] = enabled(minimum, maximum, target);
            }
            None if settings.disables_the_rest => {
                specification[member] = json!({"autoScalingDisabled": true});
            }
            None => {}
        }
    }
    if specification
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        return None;
    }
    Some(specification)
}

/// The capacities a Region turned auto scaling ON for, in the order
/// `DescribeScalableTargets` reports them (by resource id, then by dimension, and the read
/// dimension sorts before the write one).
fn enabled_capacities(settings: &RegionSettings) -> Vec<(keyspaces::TableCapacity, Window)> {
    [
        (keyspaces::TableCapacity::Read, settings.read),
        (keyspaces::TableCapacity::Write, settings.write),
    ]
    .into_iter()
    .filter_map(|(capacity, window)| window.map(|window| (capacity, window)))
    .collect()
}

/// The response member a capacity's settings are reported under.
fn reported_member(capacity: keyspaces::TableCapacity) -> &'static str {
    match capacity {
        keyspaces::TableCapacity::Read => "readCapacityAutoScaling",
        keyspaces::TableCapacity::Write => "writeCapacityAutoScaling",
    }
}

/// Assert that `region` holds exactly the auto scaling settings IT configured for the
/// shared names: through Amazon Keyspaces' own report, and through the Region's
/// Application Auto Scaling scalable targets and policies.
async fn assert_region_holds_its_own(
    emulator: &Emulator,
    case: &str,
    keyspace: &str,
    table: &str,
    settings: &RegionSettings,
) {
    let region = settings.region;
    let name = format!("{case} in {region}");

    let (status, body) = emulator
        .keyspaces_in(
            region,
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected GetTableAutoScalingSettings to answer 200, got {status} {body}"
    );
    // The report identifies the table of THIS Region, whose ARN names it.
    assert_member(
        &name,
        &body,
        "/resourceArn",
        &json!(keyspaces::table_arn(region, keyspace, table)),
    );
    for capacity in keyspaces::TableCapacity::ALL {
        let member = reported_member(capacity);
        let base = format!("/autoScalingSpecification/{member}");
        let window = match capacity {
            keyspaces::TableCapacity::Read => settings.read,
            keyspaces::TableCapacity::Write => settings.write,
        };
        match window {
            Some((minimum, maximum, target)) => {
                assert_member(&name, &body, &format!("{base}/autoScalingDisabled"), &json!(false));
                assert_member(&name, &body, &format!("{base}/minimumUnits"), &json!(minimum));
                assert_member(&name, &body, &format!("{base}/maximumUnits"), &json!(maximum));
                assert_member(
                    &name,
                    &body,
                    &format!(
                        "{base}/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue"
                    ),
                    &json!(target),
                );
            }
            // A capacity this Region turned auto scaling OFF for is a setting it holds; one
            // it never configured has no settings in this Region at all, whatever another
            // Region configured under the same names.
            None if settings.disables_the_rest => {
                assert_member(&name, &body, &format!("{base}/autoScalingDisabled"), &json!(true));
            }
            None => assert_member(&name, &body, &base, &Value::Null),
        }
    }

    let expected = enabled_capacities(settings);
    let targets = emulator.cassandra_targets_in(region).await;
    assert_eq!(
        targets.len(),
        expected.len(),
        "case {name:?}: expected {} scalable target(s) in this Region, got {targets:?}",
        expected.len()
    );
    let policies = emulator.cassandra_policies_in(region).await;
    assert_eq!(
        policies.len(),
        expected.len(),
        "case {name:?}: expected {} scaling policy/policies in this Region, got {policies:?}",
        expected.len()
    );
    for (index, (capacity, (minimum, maximum, target))) in expected.iter().enumerate() {
        let target_body = &targets[index];
        let row = format!("{name} {}", capacity.scalable_dimension());
        assert_member(&row, target_body, "/ServiceNamespace", &json!("cassandra"));
        assert_member(
            &row,
            target_body,
            "/ResourceId",
            &json!(resource_id(keyspace, table)),
        );
        assert_member(
            &row,
            target_body,
            "/ScalableDimension",
            &json!(capacity.scalable_dimension()),
        );
        assert_member(&row, target_body, "/MinCapacity", &json!(minimum));
        assert_member(&row, target_body, "/MaxCapacity", &json!(maximum));
        // A scalable target is a REGIONAL resource, so its ARN names the Region it is
        // registered in and not whichever Region happened to register one first.
        let arn = target_body["ScalableTargetARN"].as_str().unwrap_or_default();
        assert!(
            arn.contains(&format!(":{region}:")),
            "case {row:?}: expected a scalable-target ARN naming {region}, got {arn:?}"
        );

        let policy = &policies[index];
        assert_member(
            &row,
            policy,
            "/ScalableDimension",
            &json!(capacity.scalable_dimension()),
        );
        assert_member(
            &row,
            policy,
            "/TargetTrackingScalingPolicyConfiguration/TargetValue",
            &json!(target),
        );
    }
}

/// RULE: the same keyspace and table names in several Regions are several tables, each
/// holding the auto scaling settings its OWN Region configured — so a later Region's
/// create registers its own scalable targets instead of overwriting another Region's.
#[tokio::test]
async fn same_named_tables_in_several_regions_hold_their_own_auto_scaling_settings() {
    let cases = vec![
        Case {
            name: "two Regions, read capacity only, far apart windows",
            keyspace: "app",
            table: "ledger",
            regions: vec![
                RegionSettings {
                    region: "us-east-1",
                    read: Some((1, 10, 70.0)),
                    write: None,
                    disables_the_rest: false,
                },
                RegionSettings {
                    region: "eu-west-1",
                    read: Some((100, 500, 50.0)),
                    write: None,
                    disables_the_rest: false,
                },
            ],
        },
        Case {
            name: "two Regions, both capacities, swapped windows",
            keyspace: "shared_ks",
            table: "Shared_9",
            regions: vec![
                RegionSettings {
                    region: "ap-southeast-1",
                    read: Some((2, 20, 40.0)),
                    write: Some((3, 30, 45.5)),
                    disables_the_rest: false,
                },
                RegionSettings {
                    region: "us-west-2",
                    read: Some((7, 70, 80.5)),
                    write: Some((9, 90, 25.0)),
                    disables_the_rest: false,
                },
            ],
        },
        Case {
            name: "three Regions, a different capacity configured in each",
            keyspace: "triple_ks",
            table: "t",
            regions: vec![
                RegionSettings {
                    region: "eu-central-1",
                    read: Some((5, 50, 60.0)),
                    write: None,
                    disables_the_rest: false,
                },
                RegionSettings {
                    region: "sa-east-1",
                    read: None,
                    write: Some((6, 60, 65.0)),
                    disables_the_rest: false,
                },
                RegionSettings {
                    region: "ca-central-1",
                    read: Some((8, 40000, 90.0)),
                    write: Some((1, 1, 20.0)),
                    disables_the_rest: false,
                },
            ],
        },
        Case {
            name: "one Region turns auto scaling off, the other leaves it on",
            keyspace: "mixed_ks",
            table: "mixed_tbl",
            regions: vec![
                RegionSettings {
                    region: "us-east-2",
                    read: None,
                    write: None,
                    disables_the_rest: true,
                },
                RegionSettings {
                    region: "ap-northeast-1",
                    read: Some((4, 44, 44.0)),
                    write: None,
                    disables_the_rest: false,
                },
            ],
        },
    ];

    for case in &cases {
        // One emulator per row, so what each Region reports is exactly what that row's
        // creates left behind.
        let emulator = Emulator::start();
        for settings in &case.regions {
            emulator
                .given_provisioned_table_in(
                    settings.region,
                    case.name,
                    case.keyspace,
                    case.table,
                    specification(settings).as_ref(),
                )
                .await;
        }
        tokio::time::sleep(settle_wait()).await;

        for settings in &case.regions {
            assert_region_holds_its_own(&emulator, case.name, case.keyspace, case.table, settings)
                .await;
        }
    }
}

/// RULE, read from the destructive side: a write in one Region reaches only that Region's
/// table, so a delete, a capacity-mode move or a re-configuration there leaves another
/// Region's same-named table, its settings, its scalable targets and its policies exactly
/// as they were.
#[tokio::test]
async fn a_write_in_one_region_leaves_another_regions_auto_scaling_settings_alone() {
    /// The Region whose settings must survive every row, and the names both Regions share.
    const HOME: &str = "us-east-1";
    const AWAY: &str = "eu-west-1";
    let (keyspace, table) = ("app", "ledger");

    /// One row: the write the AWAY Region performs, and what AWAY holds afterwards.
    struct Row {
        case: &'static str,
        operation: &'static str,
        request: Value,
        /// The read window AWAY reports after the write, or `None` when the write left it
        /// with no settings (or no table) at all.
        away_read_after: Option<Window>,
        /// Whether AWAY's table still exists after the write.
        away_table_survives: bool,
    }

    let home = RegionSettings {
        region: HOME,
        read: Some((1, 10, 70.0)),
        write: Some((2, 20, 30.0)),
        disables_the_rest: false,
    };
    let away = RegionSettings {
        region: AWAY,
        read: Some((100, 500, 50.0)),
        write: None,
        disables_the_rest: false,
    };

    let rows = vec![
        Row {
            case: "DeleteTable in the away Region",
            operation: "DeleteTable",
            request: json!({"keyspaceName": keyspace, "tableName": table}),
            away_read_after: None,
            away_table_survives: false,
        },
        Row {
            case: "DeleteKeyspace in the away Region",
            operation: "DeleteKeyspace",
            request: json!({"keyspaceName": keyspace}),
            away_read_after: None,
            away_table_survives: false,
        },
        Row {
            case: "UpdateTable out of PROVISIONED in the away Region",
            operation: "UpdateTable",
            request: json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"},
            }),
            away_read_after: None,
            away_table_survives: true,
        },
        Row {
            case: "UpdateTable re-configuring the window in the away Region",
            operation: "UpdateTable",
            request: json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "autoScalingSpecification": {"readCapacityAutoScaling": enabled(11, 22, 33.0)},
            }),
            away_read_after: Some((11, 22, 33.0)),
            away_table_survives: true,
        },
    ];

    for row in &rows {
        let emulator = Emulator::start();
        for settings in [&home, &away] {
            emulator
                .given_provisioned_table_in(
                    settings.region,
                    row.case,
                    keyspace,
                    table,
                    specification(settings).as_ref(),
                )
                .await;
        }
        tokio::time::sleep(settle_wait()).await;
        // Both Regions start out holding their own settings, so what the write changes is
        // measured against a known state.
        assert_region_holds_its_own(&emulator, row.case, keyspace, table, &home).await;
        assert_region_holds_its_own(&emulator, row.case, keyspace, table, &away).await;

        let (status, body) = emulator
            .keyspaces_in(AWAY, row.operation, row.request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected {} to answer 200, got {status} {body}",
            row.case, row.operation
        );
        tokio::time::sleep(settle_wait()).await;

        // The home Region is untouched: the same report, the same two targets and the same
        // two policies, in its own Region's ARNs.
        assert_region_holds_its_own(&emulator, row.case, keyspace, table, &home).await;
        // And the away Region holds exactly what its own write left it.
        let after = RegionSettings {
            region: AWAY,
            read: row.away_read_after,
            write: None,
            disables_the_rest: false,
        };
        if row.away_table_survives {
            assert_region_holds_its_own(&emulator, row.case, keyspace, table, &after).await;
        } else {
            let (status, body) = emulator
                .keyspaces_in(
                    AWAY,
                    "GetTableAutoScalingSettings",
                    json!({"keyspaceName": keyspace, "tableName": table}),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {:?}: expected the away table to be gone with the modelled not-found, \
                 got {status} {body}",
                row.case
            );
            let targets = emulator.cassandra_targets_in(AWAY).await;
            assert!(
                targets.is_empty(),
                "case {:?}: expected the away Region's targets to be retired, got {targets:?}",
                row.case
            );
        }
    }
}
