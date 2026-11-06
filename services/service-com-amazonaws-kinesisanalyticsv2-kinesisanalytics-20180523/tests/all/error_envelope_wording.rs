// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the 4xx envelopes a client reads: the exception type the model
//! declares for each failure class, and the message AWS states for it.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>` —
//! so what is asserted is the wire envelope: the HTTP status, the `__type` a client
//! dispatches on, and the message text.
//!
//! The rules, each asserted over a SET of inputs rather than over one recorded request:
//!
//! * a create refused because the name is already stored states the resource the request
//!   named, quoted, and that it already exists, as a sentence ending in a period;
//! * a sentence about one application's SNAPSHOT quotes the snapshot the request named and
//!   names the application it belongs to plainly, as the scope the snapshot was looked up
//!   in;
//! * snapshot creation requires the application to be RUNNING, and a request for one
//!   against an application in any other status is the modelled
//!   `InvalidRequestException`, stating the status the application is in and the status the
//!   operation requires;
//! * a value outside an enumerated type's set is refused with that set stated in
//!   lexicographic order, whatever order the model declares its members in.

use serde_json::{json, Value};

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::{
    APPLICATION_MODE_VALUES, APPLICATION_RESTORE_TYPE_VALUES, CODE_CONTENT_TYPE_VALUES,
    LOG_LEVEL_VALUES, URL_TYPE_VALUES,
};

use super::harness::{
    call, create_body, create_startable, describe, error_code, error_message, router,
    with_code_content, ACCOUNT, SETTLE_WAIT,
};
use super::stated_enum_value_set::NAMED_RUNTIME_ENVIRONMENT_VALUES;

// ---------------------------------------------------------------------------
// A name the store already holds
// ---------------------------------------------------------------------------

/// A second `CreateApplication` for a stored name is refused with HTTP 400
/// `ResourceInUseException`, stating the name the request supplied — quoted — and that an
/// application already exists under it, and leaves the stored application as it was.
///
/// The rows span the `ApplicationName` space (len 1..128, `^[a-zA-Z0-9_.-]+$`): the
/// shortest name, one carrying every admitted separator, a digits-only name and the
/// longest, each re-created under a DIFFERENT runtime so the refusal is about the name and
/// nothing else.
#[tokio::test]
async fn a_second_create_of_a_stored_name_states_that_it_already_exists() {
    let longest = "n".repeat(128);
    // (case, name, the runtime it was stored with, the runtime the second create asks for)
    let rows: [(&str, &str, &str, &str); 4] = [
        ("the shortest name", "a", "FLINK-1_20", "FLINK-1_8"),
        (
            "every admitted separator",
            "probe.app_2-dup",
            "FLINK-1_8",
            "ZEPPELIN-FLINK-3_0",
        ),
        ("a digits-only name", "42", "FLINK-2_3", "FLINK-1_11"),
        (
            "the longest name",
            longest.as_str(),
            "FLINK-1_11",
            "FLINK-1_20",
        ),
    ];

    let router = router();
    for (case, name, stored_runtime, second_runtime) in rows {
        let (status, created) =
            call(&router, "CreateApplication", &create_body(name, stored_runtime)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} {created}"
        );

        let (status, refused) =
            call(&router, "CreateApplication", &create_body(name, second_runtime)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a stored name, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {case:?}: expected ResourceInUseException, got {refused}"
        );
        let expected = format!("Application '{name}' already exists.");
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // The refused create changed nothing: the stored application is still the one the
        // first call created.
        let stored = describe(&router, name).await;
        assert_eq!(
            stored["RuntimeEnvironment"].as_str(),
            Some(stored_runtime),
            "case {case:?}: a refused create must leave the stored application alone, got \
             {stored}"
        );
        assert_eq!(
            stored["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {case:?}: a refused create must not version the application, got {stored}"
        );
    }
}

// ---------------------------------------------------------------------------
// Snapshot creation and the application's status
// ---------------------------------------------------------------------------

/// Ask for a snapshot and return `(status, body)`.
async fn snapshot(
    router: &smithy_server_runtime::MultiServiceRouter,
    application: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "CreateApplicationSnapshot",
        &json!({"ApplicationName": application, "SnapshotName": name}),
    )
    .await
}

/// Read a snapshot back and return `(status, body)`.
async fn described_snapshot(
    router: &smithy_server_runtime::MultiServiceRouter,
    application: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeApplicationSnapshot",
        &json!({"ApplicationName": application, "SnapshotName": name}),
    )
    .await
}

/// Drive the rule over whatever status `application` is in at this instant, and return
/// that status: a snapshot of a RUNNING application is taken and readable, and in any
/// other status it is refused with the modelled `InvalidRequestException` stating that
/// status, storing nothing.
///
/// The status is READ rather than passed in, so a phase whose transient status the clock
/// has already settled is still asserted — against the status the service reports for it —
/// rather than racing the transition.
async fn assert_snapshot_rule(
    router: &smithy_server_runtime::MultiServiceRouter,
    application: &str,
    phase: &str,
) -> String {
    let status = describe(router, application).await["ApplicationStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let snapshot_name = format!("snap-at-{phase}");
    let (http_status, body) = snapshot(router, application, &snapshot_name).await;

    if status == "RUNNING" {
        assert_eq!(
            http_status, 200,
            "case {phase:?}/RUNNING: expected the snapshot to be taken, got {http_status} \
             {body}"
        );
        let (http_status, described) = described_snapshot(router, application, &snapshot_name).await;
        assert_eq!(
            (http_status, described["SnapshotDetails"]["SnapshotName"].as_str()),
            (200, Some(snapshot_name.as_str())),
            "case {phase:?}/RUNNING: expected the snapshot to be stored, got {http_status} \
             {described}"
        );
        return status;
    }

    assert_eq!(
        http_status, 400,
        "case {phase:?}/{status}: expected HTTP 400, got {http_status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "case {phase:?}/{status}: expected InvalidRequestException, got {body}"
    );
    let expected = format!(
        "Application {application} is in {status} status. Snapshot creation is only \
         allowed when application is in RUNNING status"
    );
    let actual = error_message(&body);
    assert_eq!(
        actual, expected,
        "case {phase:?}/{status}: expected {expected:?}, got {actual:?}"
    );

    let (http_status, described) = described_snapshot(router, application, &snapshot_name).await;
    assert_eq!(
        (http_status, error_code(&described).as_str()),
        (400, "ResourceNotFoundException"),
        "case {phase:?}/{status}: a refused snapshot must store nothing, got {http_status} \
         {described}"
    );
    status
}

/// `CreateApplicationSnapshot` takes a snapshot of a RUNNING application and refuses one
/// of an application in any other status with HTTP 400 `InvalidRequestException`, stating
/// the status it is in and the status required.
///
/// The phases span the lifecycle an application is driven through — a create (which leaves
/// it READY, where a snapshot is refused), straight after a start and straight after a
/// stop (the transient STARTING and STOPPING, or the settled status if the clock got there
/// first), and the settled RUNNING, where the snapshot is taken and read back — so the
/// rule is asserted about the status the service reports and not about one recorded run.
#[tokio::test]
async fn a_snapshot_outside_running_is_refused_as_an_invalid_request() {
    let router = router();
    let application = "gate-app";
    create_startable(&router, application, "FLINK-1_20").await;

    // A create leaves the application READY: it holds no running state yet, and nothing
    // has been asked of it, so no transition is pending.
    let status = assert_snapshot_rule(&router, application, "created").await;
    assert_eq!(
        status, "READY",
        "case \"created\": a create leaves the application READY, got {status:?}"
    );

    // Straight after the start: the start is asynchronous, so this is the transient
    // STARTING until the clock makes RUNNING due.
    let (http_status, started) = call(
        &router,
        "StartApplication",
        &json!({"ApplicationName": application}),
    )
    .await;
    assert_eq!(
        http_status, 200,
        "StartApplication got {http_status} {started}"
    );
    assert_snapshot_rule(&router, application, "starting").await;

    // RUNNING is the status a snapshot is taken in.
    tokio::time::sleep(SETTLE_WAIT).await;
    let status = assert_snapshot_rule(&router, application, "running").await;
    assert_eq!(
        status, "RUNNING",
        "case \"running\": the clock owed RUNNING, got {status:?}"
    );

    // Straight after the stop: on the way out of RUNNING there is no longer a running
    // state to capture.
    let (http_status, stopped) = call(
        &router,
        "StopApplication",
        &json!({"ApplicationName": application}),
    )
    .await;
    assert_eq!(http_status, 200, "StopApplication got {http_status} {stopped}");
    let status = assert_snapshot_rule(&router, application, "stopping").await;
    assert_ne!(
        status, "RUNNING",
        "case \"stopping\": a stopped application is no longer RUNNING, got {status:?}"
    );
}

// ---------------------------------------------------------------------------
// A snapshot the application holds no record of
// ---------------------------------------------------------------------------

/// `DescribeApplicationSnapshot` reports a snapshot the application holds no record of
/// with HTTP 400 `ResourceNotFoundException`, quoting the snapshot the request named and
/// naming the application plainly — the scope the snapshot was looked up in — while a
/// request naming an application that is not stored at all is answered about the
/// APPLICATION instead: the parameter violation the snapshot operations give an
/// application miss, stated over the account the name was resolved in.
///
/// The rows span the `SnapshotName` space (len 1..256, `^[a-zA-Z0-9_.-]+$`) and the
/// `ApplicationName` space, and one stored snapshot of each application is read back
/// successfully, so the rule is about absence and not about refusing every read.
#[tokio::test]
async fn a_snapshot_nothing_stored_is_stated_over_its_application() {
    let longest_snapshot = "s".repeat(256);
    let absent_snapshots = [
        ("the shortest name", "s"),
        ("every admitted separator", "snap.2_final-copy"),
        ("a digits-only name", "7"),
        ("the longest name", longest_snapshot.as_str()),
    ];

    let router = router();
    for application in ["probe-app", "probe.app_2-reader"] {
        create_startable(&router, application, "FLINK-1_20").await;
        let (status, started) = call(
            &router,
            "StartApplication",
            &json!({"ApplicationName": application}),
        )
        .await;
        assert_eq!(status, 200, "StartApplication got {status} {started}");
        tokio::time::sleep(SETTLE_WAIT).await;
        let (status, taken) = snapshot(&router, application, "stored-snap").await;
        assert_eq!(status, 200, "CreateApplicationSnapshot got {status} {taken}");

        // The snapshot that IS stored reads back, so a miss is a miss about this record.
        let (status, described) = described_snapshot(&router, application, "stored-snap").await;
        assert_eq!(
            (status, described["SnapshotDetails"]["SnapshotName"].as_str()),
            (200, Some("stored-snap")),
            "case {application:?}: expected the stored snapshot, got {status} {described}"
        );

        for (case, name) in absent_snapshots {
            let (status, refused) = described_snapshot(&router, application, name).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {refused}"
            );
            let expected =
                format!("Snapshot '{name}' does not exist for application {application}.");
            let actual = error_message(&refused);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(&format!("'{application}'")),
                "case {case:?}: the application a snapshot belongs to is named plainly, got \
                 {actual:?}"
            );
        }
    }

    // A name no application is stored under is reported about the APPLICATION, whichever
    // snapshot was asked for: the snapshot operations refuse it as a parameter that names
    // no application in this account.
    for (case, name) in absent_snapshots {
        let (status, refused) = described_snapshot(&router, "no-such-app", name).await;
        let expected = format!("Application 'no-such-app' does not exist under account '{ACCOUNT}'.");
        let actual = error_message(&refused);
        assert_eq!(
            (status, error_code(&refused).as_str(), actual.as_str()),
            (400, "InvalidArgumentException", expected.as_str()),
            "case {case:?}: expected {expected:?}, got {status} {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// The order an enumerated value set is stated in
// ---------------------------------------------------------------------------

/// The value set `values` is stated in: lexicographic order, whatever order it arrives in.
fn stated_order(values: &[&str]) -> String {
    let mut sorted: Vec<&str> = values.to_vec();
    sorted.sort_unstable();
    sorted.join(", ")
}

/// One member a request can carry a value outside its enumerated type's set in: where it
/// sits (`operation`, `path`), the request that carries it, the value submitted, and the
/// set the type admits.
struct EnumCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    path: &'static str,
    submitted: &'static str,
    values: &'static [&'static str],
}

/// A value outside a modelled enumerated type is refused with HTTP 400
/// `ValidationException` whose clause states the admissible set in LEXICOGRAPHIC order —
/// not the order the model declares the type's members in.
///
/// The rows span the enumerated types this model binds into a request and the ways a
/// member reaches one: two members of the top-level `CreateApplication` input, two nested
/// inside its `ApplicationConfiguration`, and one in each of two other operations — so the
/// rule is about every enum of this service rather than the one the issue cited. Each
/// expected set is the generated value list for that type
/// (`src/ops/types/shared_types.rs`, which the model's own enum shapes generate), sorted
/// here rather than taken as it is listed — except `RuntimeEnvironment`, whose set the
/// SERVICE names rather than the model (`stated_enum_value_set.rs` owns that rule).
#[tokio::test]
async fn an_enumerated_value_set_is_stated_in_lexicographic_order() {
    let base = create_body("enum-probe", "FLINK-1_20");
    let rows: [EnumCase; 6] = [
        EnumCase {
            case: "the runtime a create names",
            operation: "CreateApplication",
            body: create_body("enum-probe", "FLINK-99_9"),
            path: "runtimeEnvironment",
            submitted: "FLINK-99_9",
            values: NAMED_RUNTIME_ENVIRONMENT_VALUES,
        },
        EnumCase {
            case: "the mode a create names",
            operation: "CreateApplication",
            body: {
                let mut body = base.clone();
                body["ApplicationMode"] = json!("BATCH");
                body
            },
            path: "applicationMode",
            submitted: "BATCH",
            values: APPLICATION_MODE_VALUES,
        },
        EnumCase {
            case: "a member nested in the application configuration",
            operation: "CreateApplication",
            body: {
                let mut body = base.clone();
                body["ApplicationConfiguration"] = json!({
                    "ApplicationCodeConfiguration": {"CodeContentType": "JARFILE"}
                });
                body
            },
            path: "applicationConfiguration.applicationCodeConfiguration.codeContentType",
            submitted: "JARFILE",
            values: CODE_CONTENT_TYPE_VALUES,
        },
        EnumCase {
            case: "a member nested three structures deep",
            operation: "CreateApplication",
            body: {
                let mut body = base.clone();
                body["ApplicationConfiguration"] = json!({
                    "FlinkApplicationConfiguration": {
                        "MonitoringConfiguration": {
                            "ConfigurationType": "CUSTOM",
                            "LogLevel": "TRACE"
                        }
                    }
                });
                body
            },
            path: "applicationConfiguration.flinkApplicationConfiguration.\
                   monitoringConfiguration.logLevel",
            submitted: "TRACE",
            values: LOG_LEVEL_VALUES,
        },
        EnumCase {
            case: "the url type a presigned-url request names",
            operation: "CreateApplicationPresignedUrl",
            body: json!({"ApplicationName": "enum-probe", "UrlType": "FLINK_SQL_URL"}),
            path: "urlType",
            submitted: "FLINK_SQL_URL",
            values: URL_TYPE_VALUES,
        },
        EnumCase {
            case: "the restore type a start names",
            operation: "StartApplication",
            body: json!({
                "ApplicationName": "enum-probe",
                "RunConfiguration": {
                    "ApplicationRestoreConfiguration": {
                        "ApplicationRestoreType": "RESTORE_FROM_ANY_SNAPSHOT"
                    }
                }
            }),
            path: "runConfiguration.applicationRestoreConfiguration.applicationRestoreType",
            submitted: "RESTORE_FROM_ANY_SNAPSHOT",
            values: APPLICATION_RESTORE_TYPE_VALUES,
        },
    ];

    let router = router();
    let (status, created) = call(
        &router,
        "CreateApplication",
        &with_code_content(base.clone()),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication got {status} {created}");

    for EnumCase {
        case,
        operation,
        body,
        path,
        submitted,
        values,
    } in rows
    {
        let (status, refused) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at '{path}' failed to \
             satisfy constraint: Member must satisfy enum value set: [{}]",
            stated_order(values)
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // The model declares `RuntimeEnvironment` in release order, oldest family first, so
    // that order is what a set stated as the model declares it would open with.
    let (_, refused) = call(
        &router,
        "CreateApplication",
        &create_body("enum-probe", "FLINK-99_9"),
    )
    .await;
    let actual = error_message(&refused);
    assert!(
        !actual.contains("SQL-1_0, FLINK-1_6"),
        "case \"declaration order\": the set must not be stated as the model declares it, \
         got {actual:?}"
    );
}
