// SPDX-License-Identifier: CC0-1.0

//! What `GetTableAutoScalingSettings` answers for a table that EXISTS, driven end to end
//! over the service's own `awsJson1_0` wire protocol through the harness in
//! [`super::keyspace_table_lifecycle`].
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! 1. A report of a table that is in state is a 200 carrying the members the response
//!    models `@required` — `keyspaceName`, `tableName` and `resourceArn` — and carries the
//!    OPTIONAL `autoScalingSpecification` only when the table has auto scaling settings;
//!    having none is reported by leaving that member out, not by a failure. The modelled
//!    `ResourceNotFoundException` ("The operation tried to access a keyspace, table, or
//!    type that doesn't exist") is reserved for a keyspace or a table that is not in
//!    state.
//! 2. Reporting them is a read: two calls in a row answer the same body, and the table
//!    `GetTable` reports is unchanged by them.

use serde_json::{json, Value};

use super::keyspace_table_lifecycle::{
    assert_failure, assert_ok, minimal_schema, settle_wait, table_arn, Emulator, REGION,
};

/// A Region other than the request's own ([`REGION`]) a keyspace can be replicated in.
const OTHER_REGION: &str = "eu-west-1";

/// One `readCapacityAutoScaling`/`writeCapacityAutoScaling` value with auto scaling on.
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

/// One row of the table below.
struct Case {
    /// The case name, and the keyspace the row's table lives in.
    name: &'static str,
    /// The table's name — the rows span the modelled `TableName` shape
    /// (`len 1..48`, pattern `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`).
    table: String,
    /// The `replicationSpecification` the keyspace is created with, if any.
    replication: Option<Value>,
    /// The `capacitySpecification` the table is created with, if any.
    capacity: Option<Value>,
    /// The `autoScalingSpecification` the table is created with, if any.
    auto_scaling: Option<Value>,
    /// Whether the report must carry `autoScalingSpecification`.
    expect_specification: bool,
    /// The Regions the report must carry one `replicaSpecifications` entry each for, in
    /// order, or `None` when it must carry no entries at all.
    expect_regions: Option<Vec<&'static str>>,
}

/// RULE 1: a table that exists is reported, with `autoScalingSpecification` present only
/// when it has auto scaling settings.
///
/// The rows differ along what the rule generalises over: the capacity mode (omitted — the
/// modelled default — explicit `PAY_PER_REQUEST`, and `PROVISIONED` at differing units),
/// whether auto scaling was configured at all (absent, enabled, explicitly disabled),
/// the owning keyspace's replication strategy (single- and multi-Region), and the form of
/// the table's name.
#[tokio::test]
async fn a_table_without_auto_scaling_settings_is_reported_with_the_member_absent() {
    let provisioned = |read: i64, write: i64| {
        json!({
            "throughputMode": "PROVISIONED",
            "readCapacityUnits": read,
            "writeCapacityUnits": write,
        })
    };

    let cases: Vec<Case> = vec![
        Case {
            // No capacitySpecification: the modelled default PAY_PER_REQUEST, which takes
            // no auto scaling at all. The shortest name the shape allows.
            name: "default_capacity_mode",
            table: "a".to_string(),
            replication: None,
            capacity: None,
            auto_scaling: None,
            expect_specification: false,
            expect_regions: None,
        },
        Case {
            name: "explicit_pay_per_request",
            table: "9".to_string(),
            replication: None,
            capacity: Some(json!({"throughputMode": "PAY_PER_REQUEST"})),
            auto_scaling: None,
            expect_specification: false,
            expect_regions: None,
        },
        Case {
            name: "provisioned_without_auto_scaling",
            table: "Mixed_Case_9".to_string(),
            replication: None,
            capacity: Some(provisioned(5, 5)),
            auto_scaling: None,
            expect_specification: false,
            expect_regions: None,
        },
        Case {
            // The longest name the shape allows, at the minimum of the unit members.
            name: "provisioned_at_the_unit_minimum",
            table: "t".repeat(48),
            replication: None,
            capacity: Some(provisioned(1, 1)),
            auto_scaling: None,
            expect_specification: false,
            expect_regions: None,
        },
        Case {
            // A table of a MULTI_REGION keyspace is a multi-Region table, so it reports one
            // entry per replication Region — each with no settings of its own either.
            name: "multi_region_without_auto_scaling",
            table: "multi_region_tbl".to_string(),
            replication: Some(
                json!({"replicationStrategy": "MULTI_REGION", "regionList": [REGION, OTHER_REGION]}),
            ),
            capacity: Some(provisioned(5, 5)),
            auto_scaling: None,
            expect_specification: false,
            expect_regions: Some(vec![REGION, OTHER_REGION]),
        },
        Case {
            // The contrast rows: a table that HAS settings reports them, so the member's
            // absence above is the settings' absence and not a blanket omission.
            name: "provisioned_with_auto_scaling",
            table: "scaled_tbl".to_string(),
            replication: None,
            capacity: Some(provisioned(5, 5)),
            auto_scaling: Some(json!({"readCapacityAutoScaling": enabled(5, 50, 60.0)})),
            expect_specification: true,
            expect_regions: None,
        },
        Case {
            // "autoScalingDisabled: true" is a setting the table HAS: auto scaling is off
            // for that capacity, which is not the same as never having been configured.
            name: "provisioned_with_auto_scaling_disabled",
            table: "disabled_tbl".to_string(),
            replication: None,
            capacity: Some(provisioned(5, 5)),
            auto_scaling: Some(json!({
                "readCapacityAutoScaling": {"autoScalingDisabled": true},
                "writeCapacityAutoScaling": {"autoScalingDisabled": true},
            })),
            expect_specification: true,
            expect_regions: None,
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        let mut keyspace_request = json!({"keyspaceName": case.name});
        if let Some(replication) = &case.replication {
            keyspace_request["replicationSpecification"] = replication.clone();
        }
        assert_ok(
            case.name,
            emulator.call("CreateKeyspace", keyspace_request).await,
        );
        let mut table_request = json!({
            "keyspaceName": case.name,
            "tableName": case.table,
            "schemaDefinition": minimal_schema("pk"),
        });
        if let Some(capacity) = &case.capacity {
            table_request["capacitySpecification"] = capacity.clone();
        }
        if let Some(auto_scaling) = &case.auto_scaling {
            table_request["autoScalingSpecification"] = auto_scaling.clone();
        }
        assert_ok(case.name, emulator.call("CreateTable", table_request).await);
    }

    // Every table is past its transient status, so each one demonstrably exists and is
    // ACTIVE before its auto scaling settings are asked for.
    tokio::time::sleep(settle_wait()).await;

    for case in &cases {
        let identity = json!({"keyspaceName": case.name, "tableName": case.table});
        let table_report = assert_ok(case.name, emulator.call("GetTable", identity.clone()).await);
        assert_eq!(
            table_report["status"],
            json!("ACTIVE"),
            "case {:?}: expected the table to be ACTIVE before it is reported, got {table_report}",
            case.name
        );

        let report = assert_ok(
            case.name,
            emulator
                .call("GetTableAutoScalingSettings", identity.clone())
                .await,
        );
        for (member, expected) in [
            ("keyspaceName", json!(case.name)),
            ("tableName", json!(case.table)),
            ("resourceArn", json!(table_arn(case.name, &case.table))),
        ] {
            assert_eq!(
                report[member], expected,
                "case {:?}: expected {member} to be {expected}, got {}",
                case.name, report[member]
            );
        }
        assert_eq!(
            report.get("autoScalingSpecification").is_some(),
            case.expect_specification,
            "case {:?}: expected autoScalingSpecification present={}, got {report}",
            case.name,
            case.expect_specification
        );

        let got_regions: Option<Vec<String>> = report.get("replicaSpecifications").map(|entries| {
            entries
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: replicaSpecifications must be a list, got {entries}",
                        case.name
                    )
                })
                .iter()
                .map(|entry| entry["region"].as_str().unwrap_or_default().to_string())
                .collect()
        });
        let expected_regions: Option<Vec<String>> = case
            .expect_regions
            .as_ref()
            .map(|regions| regions.iter().map(|region| (*region).to_string()).collect());
        assert_eq!(
            got_regions, expected_regions,
            "case {:?}: expected replicaSpecifications for {expected_regions:?}, got {got_regions:?}",
            case.name
        );

        // RULE 2: the report is a read — asked twice it answers the same body, and the
        // table it was asked about is untouched.
        let again = assert_ok(
            case.name,
            emulator
                .call("GetTableAutoScalingSettings", identity.clone())
                .await,
        );
        assert_eq!(
            again, report,
            "case {:?}: a second report differs from the first, got {again}",
            case.name
        );
        let table_again = assert_ok(case.name, emulator.call("GetTable", identity).await);
        assert_eq!(
            table_again, table_report,
            "case {:?}: reporting the auto scaling settings changed the table, got \
             {table_again}",
            case.name
        );
    }
}

/// RULE 1's other side: the modelled not-found is for a resource that is not in state —
/// an absent keyspace, an absent table, and a table that has finished deleting.
#[tokio::test]
async fn auto_scaling_settings_of_a_resource_that_is_not_in_state_are_the_modelled_not_found() {
    let emulator = Emulator::start();
    let keyspace = "unconfigured_scaling_ks";
    assert_ok(
        "create the keyspace",
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await,
    );
    let deleted = "gone_tbl";
    assert_ok(
        "create the table to delete",
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": deleted,
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "delete the table",
        emulator
            .call(
                "DeleteTable",
                json!({"keyspaceName": keyspace, "tableName": deleted}),
            )
            .await,
    );
    tokio::time::sleep(settle_wait()).await;

    for (case, keyspace_name, table_name) in [
        ("an absent keyspace", "no_such_ks", "whatever"),
        ("an absent table", keyspace, "no_such_tbl"),
        ("a table that finished deleting", keyspace, deleted),
    ] {
        assert_failure(
            case,
            emulator
                .call(
                    "GetTableAutoScalingSettings",
                    json!({"keyspaceName": keyspace_name, "tableName": table_name}),
                )
                .await,
            404,
            "ResourceNotFoundException",
        );
    }
}
