// SPDX-License-Identifier: CC0-1.0

//! `GetResourceMetadata`'s `Features` map, end to end over the wire.
//!
//! RULE under test: a feature of a monitored DB resource is turned on by the
//! service that OWNS the resource — `EnablePerformanceInsights` and
//! `DatabaseInsightsMode` are members of the database control plane's create and
//! modify requests, not of any Performance Insights request — so the status of
//! each reported feature is PROJECTED from the monitoring configuration that
//! service persisted for the resource, never measured here and never invented:
//! `ENABLED` where the setting the feature depends on is on, `DISABLED` where it
//! is off, `UNSUPPORTED` where the owning service cannot have the feature at
//! all, and `UNKNOWN` where this emulator holds no configuration for the data
//! source, which is the model's own value for "the feature status couldn't be
//! determined".
//!
//! The table below spans what the rule generalises over: Performance Insights on
//! and off, both `DatabaseInsightsMode` values (and the mode absent altogether,
//! as an Amazon DocumentDB owner records it), the mode in the case the caller
//! wrote it, both `ServiceType` values, and a data source with no configuration
//! on record at all. The PI-enabled advanced-mode instance a recorded unit polls
//! is ONE row among them. The hand-off from a real `CreateDBInstance` is covered
//! across services by `core/router/tests/all/pi_resource_features.rs`.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator_with_engine};

use service_com_amazonaws_pi_performanceinsightsv20180227::state;

/// The monitoring configuration one row registers, and the feature statuses it
/// must be projected onto.
struct Row {
    case: &'static str,
    /// The monitoring `ServiceType` that owns the data source.
    owner: &'static str,
    resource_id: &'static str,
    engine: &'static str,
    /// What the owning service persisted, or `None` when it recorded nothing.
    monitoring: Option<(bool, Option<&'static str>)>,
    /// The expected status of `DATABASE_INSIGHTS_ADVANCED_MODE`,
    /// `PERFORMANCE_ANALYSIS_REPORTS` and `SQL_LEVEL_STATISTICS`, in that order.
    expected: [&'static str; 3],
}

const ROWS: &[Row] = &[
    Row {
        case: "an RDS instance with Performance Insights on in advanced mode",
        owner: "RDS",
        resource_id: "db-ADVANCEDMODE00000001",
        engine: "mysql",
        monitoring: Some((true, Some("advanced"))),
        expected: [
            state::FEATURE_ENABLED,
            state::FEATURE_ENABLED,
            state::FEATURE_ENABLED,
        ],
    },
    Row {
        case: "the same mode in the case the caller wrote it",
        owner: "RDS",
        resource_id: "db-ADVANCEDMODECASED0002",
        engine: "postgres",
        monitoring: Some((true, Some("ADVANCED"))),
        expected: [
            state::FEATURE_ENABLED,
            state::FEATURE_ENABLED,
            state::FEATURE_ENABLED,
        ],
    },
    Row {
        case: "an RDS instance with Performance Insights on in standard mode",
        owner: "RDS",
        resource_id: "db-STANDARDMODE00000003",
        engine: "postgres",
        monitoring: Some((true, Some("standard"))),
        expected: [
            state::FEATURE_DISABLED,
            state::FEATURE_ENABLED,
            state::FEATURE_DISABLED,
        ],
    },
    Row {
        case: "an RDS cluster with Performance Insights off",
        owner: "RDS",
        resource_id: "cluster-PIOFF000004",
        engine: "aurora-mysql",
        monitoring: Some((false, Some("standard"))),
        expected: [
            state::FEATURE_DISABLED,
            state::FEATURE_DISABLED,
            state::FEATURE_DISABLED,
        ],
    },
    Row {
        case: "an RDS instance whose owner recorded advanced mode with PI off",
        owner: "RDS",
        resource_id: "db-ADVANCEDPIOFF0000005",
        engine: "mysql",
        monitoring: Some((false, Some("advanced"))),
        expected: [
            state::FEATURE_DISABLED,
            state::FEATURE_DISABLED,
            state::FEATURE_DISABLED,
        ],
    },
    Row {
        case: "a DocumentDB instance with Performance Insights on",
        owner: "DOCDB",
        resource_id: "db-DOCDBPION00000000006",
        engine: "docdb",
        monitoring: Some((true, None)),
        expected: [
            // No `DatabaseInsightsMode` exists for this owner, and the per-SQL
            // metric types are documented for "All engines except for Amazon
            // DocumentDB" — neither feature is merely off.
            state::FEATURE_UNSUPPORTED,
            state::FEATURE_ENABLED,
            state::FEATURE_UNSUPPORTED,
        ],
    },
    Row {
        case: "a DocumentDB instance with Performance Insights off",
        owner: "DOCDB",
        resource_id: "db-DOCDBPIOFF0000000007",
        engine: "docdb",
        monitoring: Some((false, None)),
        expected: [
            state::FEATURE_UNSUPPORTED,
            state::FEATURE_DISABLED,
            state::FEATURE_UNSUPPORTED,
        ],
    },
    Row {
        case: "a registered data source whose owner recorded no configuration",
        owner: "RDS",
        resource_id: "db-NOCONFIGRECORDED0008",
        engine: "mysql",
        monitoring: None,
        expected: [
            state::FEATURE_UNKNOWN,
            state::FEATURE_UNKNOWN,
            state::FEATURE_UNKNOWN,
        ],
    },
];

/// A data source of the documented form that no database service registered
/// here, so the directory holds no record of it at all.
const UNREGISTERED: &str = "db-ABCDEFGHIJKLMNOPQRSTU1VW2X";

/// Seed the shared data-source directory the way a database control plane does
/// when it mints a resource id and persists its monitoring settings.
fn register(engine: &engine::InMemoryEngine, row: &Row) {
    let directory = state::data_source_directory(engine)
        .expect("the shared data-source directory is reachable from the engine");
    let mut directory = directory.lock();
    directory.register(row.resource_id, row.owner, row.engine);
    if let Some((performance_insights_enabled, database_insights_mode)) = row.monitoring {
        directory.record_monitoring(
            row.resource_id,
            ::service_common::db_data_sources::MonitoringConfiguration {
                performance_insights_enabled,
                database_insights_mode: database_insights_mode.map(str::to_string),
            },
        );
    }
}

/// The `Status` of one feature in an answer.
fn status(answer: &Value, feature: &str) -> String {
    answer
        .get("Features")
        .and_then(|features| features.get(feature))
        .and_then(|metadata| metadata.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: every reported feature's status is the projection of the monitoring
/// configuration the owning service persisted for the data source.
#[tokio::test]
async fn every_feature_status_is_projected_from_the_owners_configuration() {
    let (emulator, engine) = emulator_with_engine();
    for row in ROWS {
        register(&engine, row);
    }

    for row in ROWS {
        let answer = call(
            &emulator,
            "GetResourceMetadata",
            json!({"ServiceType": row.owner, "Identifier": row.resource_id}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} {:?}",
            row.case, answer.status, answer.body
        );

        for (feature, expected) in state::REPORTED_FEATURES.iter().zip(row.expected) {
            let actual = status(&answer.body, feature);
            assert_eq!(
                actual, expected,
                "case {} / {feature}: expected {expected}, got {actual:?} in {:?}",
                row.case, answer.body
            );
        }

        // Exactly the features this emulator can decide, and no other key —
        // never a feature name under an invented status.
        let keys: Vec<String> = answer
            .member("Features")
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        let mut expected_keys: Vec<String> = state::REPORTED_FEATURES
            .iter()
            .map(|feature| (*feature).to_string())
            .collect();
        expected_keys.sort();
        let mut keys_sorted = keys.clone();
        keys_sorted.sort();
        assert_eq!(
            keys_sorted, expected_keys,
            "case {}: expected exactly the reported features, got {keys:?}",
            row.case
        );
    }
}

/// RULE: a data source the directory holds no record of leaves every feature
/// UNDECIDED — `UNKNOWN`, the model's own value for a status that could not be
/// determined — because a missing record is not evidence that a feature is off.
#[tokio::test]
async fn a_data_source_with_no_record_reports_every_feature_as_unknown() {
    let (emulator, engine) = emulator_with_engine();
    // One row IS registered, so the suite proves the two kinds of data source
    // coexist in one directory rather than that the directory is empty.
    register(&engine, &ROWS[0]);

    for service_type in ["RDS", "DOCDB"] {
        let answer = call(
            &emulator,
            "GetResourceMetadata",
            json!({"ServiceType": service_type, "Identifier": UNREGISTERED}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {service_type}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        for feature in state::REPORTED_FEATURES {
            // Per-SQL statistics are the one feature an Amazon DocumentDB data
            // source rules out whatever its configuration, so that row stays
            // UNSUPPORTED.
            let expected = if service_type == "DOCDB" && feature == state::FEATURE_SQL_LEVEL_STATISTICS
            {
                state::FEATURE_UNSUPPORTED
            } else {
                state::FEATURE_UNKNOWN
            };
            let actual = status(&answer.body, feature);
            assert_eq!(
                actual, expected,
                "case {service_type} / {feature}: expected {expected}, got {actual:?} in {:?}",
                answer.body
            );
        }
    }
}

/// RULE: reading the metadata reports a configuration, it does not change one —
/// so a repeated read answers identically, and the status a later caller sees
/// changes only when the OWNER records a different configuration.
#[tokio::test]
async fn reading_the_metadata_changes_no_feature_status() {
    let (emulator, engine) = emulator_with_engine();
    let row = &ROWS[2]; // Performance Insights on, standard mode.
    register(&engine, row);

    let request = json!({"ServiceType": row.owner, "Identifier": row.resource_id});
    let first = call(&emulator, "GetResourceMetadata", request.clone()).await;
    let second = call(&emulator, "GetResourceMetadata", request.clone()).await;
    assert_eq!(
        first.body, second.body,
        "case a repeated read: expected an identical answer, got {:?} then {:?}",
        first.body, second.body
    );

    // The owner moves the instance to advanced mode — the way `ModifyDBInstance`
    // records a changed setting — and the SAME read now reports the feature on.
    state::data_source_directory(&*engine)
        .expect("the shared data-source directory is reachable from the engine")
        .lock()
        .record_monitoring(
            row.resource_id,
            ::service_common::db_data_sources::MonitoringConfiguration {
                performance_insights_enabled: true,
                database_insights_mode: Some("advanced".to_string()),
            },
        );

    let after = call(&emulator, "GetResourceMetadata", request).await;
    for feature in state::REPORTED_FEATURES {
        let actual = status(&after.body, feature);
        assert_eq!(
            actual,
            state::FEATURE_ENABLED,
            "case after the owner enabled advanced mode / {feature}: expected {}, got \
             {actual:?} in {:?}",
            state::FEATURE_ENABLED,
            after.body
        );
    }
}
