// SPDX-License-Identifier: CC0-1.0

//! The modelled CLIENT errors of this service: every refusal here is a 4xx a client can
//! branch on, carrying the error code the model declares and a `Message` member.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>` —
//! so what is asserted is the wire envelope: the HTTP status, the `__type` error code and
//! the message. One engine per test, so a refusal that must store nothing is checked
//! against a store only that test wrote to.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! * a value outside a modelled enumerated type is refused by the boundary validator with
//!   HTTP 400 `ValidationException`, naming the member and its value set;
//! * `CreateApplication` provisions only the runtimes this API still creates applications
//!   for, and refuses the retired SQL runtime family with `UnsupportedOperationException`;
//! * an operation that looks an application up by name reports a miss with the error code
//!   its own model declares for that path — `ResourceNotFoundException` for the readers,
//!   `InvalidArgumentException` for `CreateApplicationSnapshot` — and never as a 5xx;
//! * `StartApplication` refuses an application whose stored configuration carries no code
//!   content with `InvalidApplicationConfigurationException`, and leaves it unstarted.

use serde_json::{json, Value};

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;

// The wire plumbing lives in `tests/all/harness.rs`, shared with the lifecycle suite, so
// both read one response envelope.
use super::harness::{
    call, create_body, error_code, error_message, router, ACCOUNT, SETTLE_WAIT,
};
use super::stated_enum_value_set::NAMED_RUNTIME_ENVIRONMENT_VALUES;

// ---------------------------------------------------------------------------
// Boundary validation: a value outside a modelled enum
// ---------------------------------------------------------------------------

/// A `RuntimeEnvironment` outside the modelled value set is refused with HTTP 400
/// `ValidationException`, in the envelope AWS's request validator uses: the offending
/// member, the value the request sent, and the constraint it failed.
///
/// The rows span the forms an out-of-set value takes — an unreleased version of a modelled
/// family, a modelled value in the wrong case, a value of no family at all, and the empty
/// string — because the rule is about enum MEMBERSHIP and not about one recorded value.
#[tokio::test]
async fn a_runtime_environment_outside_the_enum_is_refused_as_a_validation_exception() {
    let router = router();
    for value in [
        "FLINK-99_9",
        "SQL-9_9",
        "flink-1_20",
        "ZEPPELIN-FLINK-9_0",
        "NOT-A-RUNTIME",
        "",
    ] {
        let (status, body) = call(&router, "CreateApplication", &create_body("probe", value)).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400 for a value outside the enum, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {value:?}: expected ValidationException, got {body}"
        );
        let message = error_message(&body);
        // The envelope, the member path and the constraint are the requirement; what is
        // asserted here is WHICH values the set names. The ORDER it is stated in is its own
        // rule, asserted over several enums in `tests/all/error_envelope_wording.rs`.
        let expected_prefix = format!(
            "1 validation error detected: Value '{value}' at 'runtimeEnvironment' failed to \
             satisfy constraint: Member must satisfy enum value set: ["
        );
        assert!(
            message.starts_with(&expected_prefix) && message.ends_with(']'),
            "case {value:?}: expected a refusal naming the member, the value and the enum \
             constraint ({expected_prefix}…]), got {message:?}"
        );
        // The set the refusal names is the set the SERVICE names for the shape, which is
        // not every value the model declares — `tests/all/stated_enum_value_set.rs` owns
        // that rule and the table behind it.
        for named in NAMED_RUNTIME_ENVIRONMENT_VALUES {
            assert!(
                message.contains(named),
                "case {value:?}: expected the refusal to list the named value \
                 {named:?}, got {message:?}"
            );
        }
        for declared in RUNTIME_ENVIRONMENT_VALUES
            .iter()
            .filter(|declared| !NAMED_RUNTIME_ENVIRONMENT_VALUES.contains(declared))
        {
            assert!(
                !message.contains(declared),
                "case {value:?}: expected the refusal not to list {declared:?}, which the \
                 service does not name for this shape, got {message:?}"
            );
        }
    }

    // Nothing was stored by any of them.
    let (status, listed) = call(&router, "ListApplications", &json!({})).await;
    assert_eq!(status, 200, "ListApplications got {listed}");
    assert_eq!(
        listed["ApplicationSummaries"],
        json!([]),
        "a refused create must store nothing, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// CreateApplication: the runtimes this API still creates
// ---------------------------------------------------------------------------

/// Whether a runtime names a SQL application: the `SQL-` family of the modelled
/// `RuntimeEnvironment` enum, whichever version it carries.
fn is_sql(runtime: &str) -> bool {
    runtime.starts_with("SQL-")
}

/// `CreateApplication` refuses the SQL runtime family with HTTP 400
/// `UnsupportedOperationException` and a `Message`, and creates an application for every
/// other runtime the enum declares.
///
/// The table is the WHOLE enum, read from the generated
/// `RUNTIME_ENVIRONMENT_VALUES` (`src/ops/types/shared_types.rs`, which the model's
/// `RuntimeEnvironment` shape generates), so a runtime the issue never cited is covered
/// and a value added to the model later joins the table on its own.
#[tokio::test]
async fn create_refuses_the_sql_runtime_family_and_creates_every_other_runtime() {
    let router = router();
    for runtime in RUNTIME_ENVIRONMENT_VALUES {
        let name = format!("app-{}", runtime.replace(['-', '_'], "."));
        let (status, body) = call(&router, "CreateApplication", &create_body(&name, runtime)).await;
        if is_sql(runtime) {
            assert_eq!(
                status, 400,
                "case {runtime:?}: expected HTTP 400 for a SQL application, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedOperationException",
                "case {runtime:?}: expected UnsupportedOperationException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                "CreateApplication is not supported for SQL applications in this Region. You \
                 can use KDA Studio to build streaming applications with Flink SQL.",
                "case {runtime:?}: expected the unsupported-runtime message, got {body}"
            );

            // A refused create stores nothing: the reader still reports a miss.
            let (status, described) = call(
                &router,
                "DescribeApplication",
                &json!({"ApplicationName": name}),
            )
            .await;
            assert_eq!(
                (status, error_code(&described).as_str()),
                (400, "ResourceNotFoundException"),
                "case {runtime:?}: a refused create must store nothing, got {status} {described}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {runtime:?}: expected the application to be created, got {status} {body}"
        );
        assert_eq!(
            body["ApplicationDetail"]["RuntimeEnvironment"].as_str(),
            Some(*runtime),
            "case {runtime:?}: expected the stored runtime environment, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Lookup misses
// ---------------------------------------------------------------------------

/// Every operation that looks an application up by name answers a miss with the error code
/// its own model declares for that path, HTTP 400 and a `Message` naming the application:
/// `ResourceNotFoundException` for the readers and the lifecycle calls,
/// `InvalidArgumentException` for `CreateApplicationSnapshot`, which reports a missing
/// application as a bad argument and names the account it looked in.
///
/// The names span the forms `ApplicationName` admits (len 1..128, `^[a-zA-Z0-9_.-]+$`):
/// the shortest, one with each admitted separator, and the longest — so what is asserted
/// is the rule and not one recorded name. One STORED application is created first, so a
/// miss is a miss about the store and not about an empty one.
#[tokio::test]
async fn a_name_no_application_is_stored_under_is_refused_with_the_modelled_code() {
    let router = router();
    let (status, created) = call(
        &router,
        "CreateApplication",
        &create_body("stored-app", "FLINK-1_20"),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication got {created}");

    let longest = "n".repeat(128);
    let absent = ["a", "no-such-app", "probe.app_2-absent", longest.as_str()];

    for name in absent {
        // (case, operation, body, expected error code, expected message)
        let rows: [(&str, &str, Value, &str, String); 4] = [
            (
                "describe",
                "DescribeApplication",
                json!({"ApplicationName": name}),
                "ResourceNotFoundException",
                format!("Application '{name}' does not exist."),
            ),
            (
                "start",
                "StartApplication",
                json!({"ApplicationName": name}),
                "ResourceNotFoundException",
                format!("Application '{name}' does not exist."),
            ),
            (
                "stop",
                "StopApplication",
                json!({"ApplicationName": name}),
                "ResourceNotFoundException",
                format!("Application '{name}' does not exist."),
            ),
            (
                "snapshot",
                "CreateApplicationSnapshot",
                json!({"ApplicationName": name, "SnapshotName": "snap-1"}),
                "InvalidArgumentException",
                format!("Application {name} does not exist under accountId {ACCOUNT}"),
            ),
        ];

        for (case, operation, body, expected_code, expected_message) in rows {
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case}/{name:?}: expected HTTP 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case}/{name:?}: expected {expected_code}, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected_message,
                "case {case}/{name:?}: expected the Message to name the absent application, \
                 got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// StartApplication: a configuration it can start
// ---------------------------------------------------------------------------

/// An `ApplicationConfiguration` carrying `code`, or none at all when `code` is null.
fn configuration_with(code: Value) -> Value {
    if code.is_null() {
        return Value::Null;
    }
    json!({"ApplicationCodeConfiguration": code})
}

/// `StartApplication` starts an application whose stored configuration carries code
/// content and refuses one that carries none with HTTP 400
/// `InvalidApplicationConfigurationException`, leaving it unstarted.
///
/// The rows span every form `CodeContent` can take
/// (`smithy_query(query="shape",shape="CodeContent")`: inline text, a zip blob, an S3
/// object location) and every way it can be missing (no configuration at all, a code
/// configuration with no content, an empty text content, an empty zip), across the runtime
/// families — including the `ZEPPELIN-FLINK-` notebook family, whose code is entered in
/// the notebook rather than submitted, so it starts without any.
#[tokio::test]
async fn start_refuses_an_application_with_no_valid_code_content() {
    // (case, runtime, ApplicationCodeConfiguration, whether the start is accepted)
    let rows: [(&str, &str, Value, bool); 9] = [
        ("no configuration at all", "FLINK-1_20", Value::Null, false),
        (
            "a code configuration with no content",
            "FLINK-1_20",
            json!({"CodeContentType": "PLAINTEXT"}),
            false,
        ),
        (
            "an empty text content",
            "FLINK-1_20",
            json!({"CodeContentType": "PLAINTEXT", "CodeContent": {"TextContent": ""}}),
            false,
        ),
        (
            "an empty zip content",
            "FLINK-1_20",
            json!({"CodeContentType": "ZIPFILE", "CodeContent": {"ZipFileContent": ""}}),
            false,
        ),
        (
            "inline text content",
            "FLINK-1_20",
            json!({"CodeContentType": "PLAINTEXT", "CodeContent": {"TextContent": "env.execute();"}}),
            true,
        ),
        (
            "a zip blob",
            "FLINK-1_8",
            json!({"CodeContentType": "ZIPFILE", "CodeContent": {"ZipFileContent": "UEsDBAo="}}),
            true,
        ),
        (
            "an S3 object location",
            "FLINK-2_3",
            json!({
                "CodeContentType": "ZIPFILE",
                "CodeContent": {
                    "S3ContentLocation": {
                        "BucketARN": "arn:aws:s3:::kda-code",
                        "FileKey": "app.jar"
                    }
                }
            }),
            true,
        ),
        // A Studio notebook's code is in the notebook, so it needs none stored.
        (
            "a notebook runtime with no code content",
            "ZEPPELIN-FLINK-3_0",
            Value::Null,
            true,
        ),
        (
            "a streaming runtime of another family with no code content",
            "FLINK-1_11",
            Value::Null,
            false,
        ),
    ];

    for (case, runtime, code, accepted) in rows {
        let router = router();
        let name = "start-probe";
        let mut body = create_body(name, runtime);
        let configuration = configuration_with(code);
        if !configuration.is_null() {
            body["ApplicationConfiguration"] = configuration;
        }
        if runtime.starts_with("ZEPPELIN-FLINK-") {
            // A notebook is an INTERACTIVE application, as its runtime's doc states.
            body["ApplicationMode"] = json!("INTERACTIVE");
        }
        let (status, created) = call(&router, "CreateApplication", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} {created}"
        );

        let (status, started) =
            call(&router, "StartApplication", &json!({"ApplicationName": name})).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the start to be accepted, got {status} {started}"
            );
            assert!(
                started["OperationId"].as_str().is_some(),
                "case {case:?}: an accepted start answers with an OperationId, got {started}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {started}"
        );
        assert_eq!(
            error_code(&started),
            "InvalidApplicationConfigurationException",
            "case {case:?}: expected InvalidApplicationConfigurationException, got {started}"
        );
        assert_eq!(
            error_message(&started),
            format!(
                "Application '{name}' under account '{ACCOUNT}' does not have a valid \
                 configuration to start. Application is not configured with a valid code \
                 content. Please update the application with a valid code content."
            ),
            "case {case:?}: expected the start-readiness message, got {started}"
        );

        // A refused start leaves the application where it was: READY, not STARTING.
        let (status, described) = call(
            &router,
            "DescribeApplication",
            &json!({"ApplicationName": name}),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DescribeApplication got {described}");
        assert_eq!(
            described["ApplicationDetail"]["ApplicationStatus"].as_str(),
            Some("READY"),
            "case {case:?}: a refused start must not move the stored status, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// A snapshot of a RUNNING application is stored and read back, and every other request
/// for one is refused with the modelled code: a duplicate name is
/// `ResourceInUseException`, an application outside RUNNING is `InvalidRequestException`,
/// and a snapshot nothing stored is `ResourceNotFoundException`.
#[tokio::test]
async fn a_snapshot_of_a_running_application_is_stored_and_read_back() {
    let router = router();
    let name = "snapshot-app";
    let mut body = create_body(name, "FLINK-1_20");
    body["ApplicationConfiguration"] = json!({
        "ApplicationCodeConfiguration": {
            "CodeContentType": "PLAINTEXT",
            "CodeContent": {"TextContent": "env.execute();"}
        }
    });
    let (status, created) = call(&router, "CreateApplication", &body).await;
    assert_eq!(status, 200, "CreateApplication got {created}");

    // A READY application holds no running state, so there is nothing to snapshot yet:
    // the request is not a valid one for the application as it stands. The status gate's
    // own rule is asserted over every status in `tests/all/error_envelope_wording.rs`.
    let (status, refused) = call(
        &router,
        "CreateApplicationSnapshot",
        &json!({"ApplicationName": name, "SnapshotName": "snap-ready"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidRequestException"),
        "case not-running: expected InvalidRequestException, got {status} {refused}"
    );

    let (status, started) =
        call(&router, "StartApplication", &json!({"ApplicationName": name})).await;
    assert_eq!(status, 200, "StartApplication got {started}");
    tokio::time::sleep(SETTLE_WAIT).await;

    for snapshot in ["snap-1", "snap.2_final"] {
        let (status, taken) = call(
            &router,
            "CreateApplicationSnapshot",
            &json!({"ApplicationName": name, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {snapshot:?}: CreateApplicationSnapshot got {status} {taken}"
        );

        // The round trip: the reader reports the record the writer stored, in the
        // transient status a caller reading straight back must still see.
        let (status, described) = call(
            &router,
            "DescribeApplicationSnapshot",
            &json!({"ApplicationName": name, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {snapshot:?}: DescribeApplicationSnapshot got {status} {described}"
        );
        let details = &described["SnapshotDetails"];
        assert_eq!(
            details["SnapshotName"].as_str(),
            Some(snapshot),
            "case {snapshot:?}: expected the stored snapshot name, got {details}"
        );
        assert_eq!(
            details["SnapshotStatus"].as_str(),
            Some("CREATING"),
            "case {snapshot:?}: a snapshot is CREATING when the call answers, got {details}"
        );
        assert_eq!(
            details["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {snapshot:?}: expected the application version it was taken of, got {details}"
        );
        assert_eq!(
            details["RuntimeEnvironment"].as_str(),
            Some("FLINK-1_20"),
            "case {snapshot:?}: expected the application's runtime, got {details}"
        );
        assert!(
            details["SnapshotCreationTimestamp"]
                .as_i64()
                .is_some_and(|t| t > 0),
            "case {snapshot:?}: expected a creation timestamp, got {details}"
        );

        // The same name cannot be taken twice.
        let (status, duplicate) = call(
            &router,
            "CreateApplicationSnapshot",
            &json!({"ApplicationName": name, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            (status, error_code(&duplicate).as_str()),
            (400, "ResourceInUseException"),
            "case {snapshot:?}: a duplicate snapshot name is refused, got {status} {duplicate}"
        );
    }

    // READY becomes due on the clock, without any further call.
    tokio::time::sleep(SETTLE_WAIT).await;
    let (status, described) = call(
        &router,
        "DescribeApplicationSnapshot",
        &json!({"ApplicationName": name, "SnapshotName": "snap-1"}),
    )
    .await;
    assert_eq!(status, 200, "DescribeApplicationSnapshot got {described}");
    assert_eq!(
        described["SnapshotDetails"]["SnapshotStatus"].as_str(),
        Some("READY"),
        "case settled: the clock owed READY, got {described}"
    );

    // A snapshot name nothing stored, and a snapshot of another application's name.
    for (case, body) in [
        (
            "unknown snapshot",
            json!({"ApplicationName": name, "SnapshotName": "no-such-snapshot"}),
        ),
        (
            "unknown application",
            json!({"ApplicationName": "no-such-app", "SnapshotName": "snap-1"}),
        ),
    ] {
        let (status, response) = call(&router, "DescribeApplicationSnapshot", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {status} {response}"
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {case}: expected a Message naming what was not found, got {response}"
        );
    }
}
