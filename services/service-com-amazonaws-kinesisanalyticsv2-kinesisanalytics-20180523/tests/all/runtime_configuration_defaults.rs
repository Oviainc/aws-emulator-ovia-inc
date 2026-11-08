// SPDX-License-Identifier: CC0-1.0

//! The defaults an application is described under depend on the KIND of application the
//! service runs it as.
//!
//! The rule under test: `CheckpointingEnabled` and `SnapshotsEnabled` switch features of a
//! Managed Service for Apache Flink application — the model documents each as describing
//! "whether checkpointing is enabled for a Managed Service for Apache Flink application"
//! and "whether snapshots are enabled for a Managed Service for Apache Flink application"
//! (`smithy_query(query="doc",shape="CheckpointConfiguration",member="CheckpointingEnabled")`
//! / `shape="ApplicationSnapshotConfiguration",member="SnapshotsEnabled"`) — while an
//! application the service runs in `INTERACTIVE` mode is a Studio notebook rather than such
//! an application (`CreateApplication$ApplicationMode`). So an application whose request
//! said nothing about either runs them only when it STREAMS, and every operation that
//! returns `ApplicationConfigurationDescription` reports the one stored answer.
//!
//! The rows span every `RuntimeEnvironment` the enum admits a create for, both ways the
//! mode is decided (named by the request, implied by the runtime), and a configuration that
//! switches each feature on explicitly.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;

use super::harness::{call, create_body, describe, router};

/// The `RuntimeEnvironment` family a Studio notebook runs on, as the enum spells it
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`: `ZEPPELIN-FLINK-<major>_<minor>`).
const NOTEBOOK_FAMILY: &str = "ZEPPELIN-";

/// The `RuntimeEnvironment` family of a SQL application, which this API no longer creates
/// applications for, so no row drives a create with one.
const SQL_FAMILY: &str = "SQL-";

/// What the two switches must report for an application of `runtime` created with no
/// configuration naming them: on for a Managed Service for Apache Flink application, off
/// for a Studio notebook.
fn features_run(runtime: &str) -> bool {
    !runtime.starts_with(NOTEBOOK_FAMILY)
}

/// The `CheckpointingEnabled` of the described checkpoint configuration.
fn checkpointing_enabled(detail: &Value) -> Option<bool> {
    detail
        .pointer(
            "/ApplicationConfigurationDescription/FlinkApplicationConfigurationDescription\
             /CheckpointConfigurationDescription/CheckpointingEnabled",
        )
        .and_then(Value::as_bool)
}

/// The `SnapshotsEnabled` of the described snapshot configuration.
fn snapshots_enabled(detail: &Value) -> Option<bool> {
    detail
        .pointer(
            "/ApplicationConfigurationDescription\
             /ApplicationSnapshotConfigurationDescription/SnapshotsEnabled",
        )
        .and_then(Value::as_bool)
}

/// One member of the described checkpoint configuration, for the values that are the same
/// for every kind of application.
fn checkpoint_member<'a>(detail: &'a Value, member: &str) -> &'a Value {
    &detail["ApplicationConfigurationDescription"]["FlinkApplicationConfigurationDescription"]
        ["CheckpointConfigurationDescription"][member]
}

/// Create an application, asserting the call was accepted, and answer its detail.
async fn create_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    runtime: &str,
    mode: Option<&str>,
    configuration: Option<Value>,
) -> Value {
    let mut body = create_body(name, runtime);
    if let Some(mode) = mode {
        body["ApplicationMode"] = json!(mode);
    }
    if let Some(configuration) = configuration {
        body["ApplicationConfiguration"] = configuration;
    }
    let (status, created) = call(router, "CreateApplication", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateApplication to be accepted, got {status} {created}"
    );
    created["ApplicationDetail"].clone()
}

/// Update one application with a configuration change that names NEITHER switch, and
/// answer the detail the update reported.
async fn update_unrelated_configuration(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version: i64,
) -> Value {
    let (status, updated) = call(
        router,
        "UpdateApplication",
        &json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": version,
            "ApplicationConfigurationUpdate": {
                "EnvironmentPropertyUpdates": {
                    "PropertyGroups": [
                        {"PropertyGroupId": "producer", "PropertyMap": {"topic": "orders"}}
                    ]
                }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateApplication to be accepted, got {status} {updated}"
    );
    updated["ApplicationDetail"].clone()
}

/// Every runtime the enum admits a create for, described with the defaults its kind of
/// application runs under — on create, on describe, and on an update that names neither
/// switch.
#[tokio::test]
async fn each_runtime_is_described_with_the_defaults_its_kind_of_application_runs() {
    let router = router();
    for runtime in RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .filter(|runtime| !runtime.starts_with(SQL_FAMILY))
    {
        let case = format!("{runtime} with no configuration at all");
        let name = format!("defaults-{}", runtime.to_lowercase());
        let expected = features_run(runtime);

        let created = create_with(&router, &case, &name, runtime, None, None).await;
        assert_eq!(
            checkpointing_enabled(&created),
            Some(expected),
            "case {case:?}: expected the create to report CheckpointingEnabled {expected}, \
             got {created}"
        );
        assert_eq!(
            snapshots_enabled(&created),
            Some(expected),
            "case {case:?}: expected the create to report SnapshotsEnabled {expected}, \
             got {created}"
        );
        // The rest of the DEFAULT checkpoint table is the same for every kind of
        // application: the two intervals the `ConfigurationType` doc publishes.
        for (member, value) in [
            ("ConfigurationType", json!("DEFAULT")),
            ("CheckpointInterval", json!(60_000)),
            ("MinPauseBetweenCheckpoints", json!(5_000)),
        ] {
            assert_eq!(
                checkpoint_member(&created, member),
                &value,
                "case {case:?}: expected {member} to be {value}, got {created}"
            );
        }

        let described = describe(&router, &name).await;
        assert_eq!(
            described, created,
            "case {case:?}: expected the describe to repeat the created detail, got {described}"
        );

        let updated = update_unrelated_configuration(&router, &case, &name, 1).await;
        assert_eq!(
            (checkpointing_enabled(&updated), snapshots_enabled(&updated)),
            (Some(expected), Some(expected)),
            "case {case:?}: expected an unrelated update to carry both switches through \
             unchanged, got {updated}"
        );
        let redescribed = describe(&router, &name).await;
        assert_eq!(
            (
                checkpointing_enabled(&redescribed),
                snapshots_enabled(&redescribed)
            ),
            (Some(expected), Some(expected)),
            "case {case:?}: expected the stored switches to survive the update, got \
             {redescribed}"
        );
    }
}

/// The mode the application RUNS in is what decides, whether the request named it or left
/// it to the runtime.
#[tokio::test]
async fn the_mode_the_application_runs_in_decides_the_defaults() {
    // (case, runtime, the ApplicationMode the create names, the mode the application runs
    // in, whether the two features run)
    let rows: [(&str, &str, Option<&str>, &str, bool); 4] = [
        (
            "a streaming runtime naming its mode",
            "FLINK-1_20",
            Some("STREAMING"),
            "STREAMING",
            true,
        ),
        (
            "a streaming runtime naming no mode",
            "FLINK-1_8",
            None,
            "STREAMING",
            true,
        ),
        (
            "a notebook runtime naming its mode",
            "ZEPPELIN-FLINK-3_0",
            Some("INTERACTIVE"),
            "INTERACTIVE",
            false,
        ),
        (
            "a notebook runtime naming no mode",
            "ZEPPELIN-FLINK-1_0",
            None,
            "INTERACTIVE",
            false,
        ),
    ];

    let router = router();
    for (case, runtime, named_mode, mode, expected) in rows {
        let name = format!("mode-{}", case.replace(' ', "-"));
        let created = create_with(&router, case, &name, runtime, named_mode, None).await;
        assert_eq!(
            created["ApplicationMode"].as_str(),
            Some(mode),
            "case {case:?}: expected the application to run in {mode}, got {created}"
        );
        assert_eq!(
            (checkpointing_enabled(&created), snapshots_enabled(&created)),
            (Some(expected), Some(expected)),
            "case {case:?}: expected both switches to be {expected}, got {created}"
        );
    }
}

/// A request that switches a feature on is reported as given, on every kind of
/// application: the per-kind values are DEFAULTS, not a ceiling.
#[tokio::test]
async fn a_request_that_switches_a_feature_on_is_reported_as_given() {
    // (case, runtime, the ApplicationConfiguration the create carries, expected
    // CheckpointingEnabled, expected SnapshotsEnabled)
    let rows: [(&str, &str, Value, bool, bool); 4] = [
        (
            "a notebook that enables snapshots",
            "ZEPPELIN-FLINK-3_0",
            json!({"ApplicationSnapshotConfiguration": {"SnapshotsEnabled": true}}),
            false,
            true,
        ),
        (
            "a notebook with a CUSTOM checkpoint configuration that enables checkpointing",
            "ZEPPELIN-FLINK-2_0",
            json!({
                "FlinkApplicationConfiguration": {
                    "CheckpointConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "CheckpointingEnabled": true
                    }
                }
            }),
            true,
            false,
        ),
        (
            "a streaming application that disables snapshots",
            "FLINK-1_19",
            json!({"ApplicationSnapshotConfiguration": {"SnapshotsEnabled": false}}),
            true,
            false,
        ),
        (
            "a streaming application with a CUSTOM checkpoint configuration that disables \
             checkpointing",
            "FLINK-2_2",
            json!({
                "FlinkApplicationConfiguration": {
                    "CheckpointConfiguration": {
                        "ConfigurationType": "CUSTOM",
                        "CheckpointingEnabled": false
                    }
                }
            }),
            false,
            true,
        ),
    ];

    let router = router();
    for (case, runtime, configuration, checkpointing, snapshots) in rows {
        let name = format!("explicit-{}", runtime.to_lowercase());
        let created =
            create_with(&router, case, &name, runtime, None, Some(configuration)).await;
        assert_eq!(
            (checkpointing_enabled(&created), snapshots_enabled(&created)),
            (Some(checkpointing), Some(snapshots)),
            "case {case:?}: expected (CheckpointingEnabled, SnapshotsEnabled) to be \
             ({checkpointing}, {snapshots}), got {created}"
        );
        let described = describe(&router, &name).await;
        assert_eq!(
            (
                checkpointing_enabled(&described),
                snapshots_enabled(&described)
            ),
            (Some(checkpointing), Some(snapshots)),
            "case {case:?}: expected the describe to report the stored values, got {described}"
        );
    }
}

/// An update that names a switch moves THAT one and leaves the other at its stored value,
/// on an application whose defaults turned both off.
#[tokio::test]
async fn an_update_that_names_one_switch_moves_only_that_one() {
    let router = router();
    let name = "notebook-switches";
    let created = create_with(
        &router,
        "a notebook",
        name,
        "ZEPPELIN-FLINK-3_0",
        None,
        None,
    )
    .await;
    assert_eq!(
        (checkpointing_enabled(&created), snapshots_enabled(&created)),
        (Some(false), Some(false)),
        "expected a notebook to run neither feature, got {created}"
    );

    let (status, updated) = call(
        &router,
        "UpdateApplication",
        &json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": 1,
            "ApplicationConfigurationUpdate": {
                "ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": true}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the update to be accepted, got {status} {updated}"
    );
    let detail = updated["ApplicationDetail"].clone();
    assert_eq!(
        snapshots_enabled(&detail),
        Some(true),
        "expected the update to enable snapshots, got {detail}"
    );
    assert_eq!(
        checkpointing_enabled(&detail),
        Some(false),
        "expected checkpointing to stay at the value the notebook runs under, got {detail}"
    );

    let described = describe(&router, name).await;
    assert_eq!(
        (snapshots_enabled(&described), checkpointing_enabled(&described)),
        (Some(true), Some(false)),
        "expected the describe to report what the update stored, got {described}"
    );
}
