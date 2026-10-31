// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the application lifecycle slice: `CreateApplication`,
//! `DescribeApplication`, `ListApplications`, `StartApplication`, `StopApplication` and
//! `DeleteApplication`.
//!
//! Every test drives the service the way a client does — one `POST /` per call through
//! the real `MultiServiceRouter`, with `X-Amz-Target` naming the operation — so what is
//! asserted is the wire behaviour, not a handler's return value. One engine per test
//! means the store starts empty and every later assertion is about what an earlier call
//! actually persisted.

use std::sync::Arc;

use serde_json::{json, Value};

// The wire plumbing lives in `tests/all/harness.rs`, shared with every other suite in
// this crate, so both read one response envelope.
use super::harness::{
    call, call_in, create, create_body, create_startable, describe, error_code, router, router_on,
    ACCOUNT, SETTLE_WAIT,
};

// ---------------------------------------------------------------------------
// Create / Describe
// ---------------------------------------------------------------------------

/// A create is answered from the record it wrote, and a later describe reports exactly
/// the same record.
///
/// The table spans the whole class the rule covers: every `RuntimeEnvironment` family
/// this API still creates applications for
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")` — including values the issue
/// never cited; the retired SQL family is refused instead, which
/// `modeled_client_errors.rs` asserts), both `ApplicationMode` values and none, a
/// present / empty / absent `ApplicationDescription`, and the shortest and longest name
/// `ApplicationName` (len 1..128) admits.
#[tokio::test]
async fn create_persists_an_application_that_describe_reports_identically() {
    let router = router();
    let longest_name = "n".repeat(128);
    // (name, RuntimeEnvironment, ApplicationMode, ApplicationDescription)
    let cases: [(&str, &str, Option<&str>, Option<&str>); 7] = [
        ("flink-1-20-app", "FLINK-1_20", Some("STREAMING"), Some("a flink app")),
        ("flink-1-11-app", "FLINK-1_11", None, None),
        ("flink.1.19.app", "FLINK-1_19", Some("STREAMING"), Some("")),
        ("zeppelin-flink-3-0", "ZEPPELIN-FLINK-3_0", Some("INTERACTIVE"), Some("studio")),
        ("flink-2-3-app", "FLINK-2_3", Some("STREAMING"), None),
        ("a", "FLINK-1_6", None, Some("shortest name the model admits")),
        (longest_name.as_str(), "FLINK-1_8", Some("STREAMING"), None),
    ];

    let mut tokens: Vec<String> = Vec::new();
    for (name, runtime, mode, description) in cases {
        let mut body = create_body(name, runtime);
        if let Some(mode) = mode {
            body["ApplicationMode"] = json!(mode);
        }
        if let Some(description) = description {
            body["ApplicationDescription"] = json!(description);
        }

        let (status, response) = call(&router, "CreateApplication", &body).await;
        assert_eq!(status, 200, "case {name:?}: CreateApplication got {response}");
        let created = &response["ApplicationDetail"];

        assert_eq!(
            created["ApplicationARN"].as_str(),
            Some(format!("arn:aws:kinesisanalytics:us-east-1:{ACCOUNT}:application/{name}").as_str()),
            "case {name:?}: expected the application ARN for this name, got {created}"
        );
        assert_eq!(
            created["ApplicationStatus"].as_str(),
            Some("READY"),
            "case {name:?}: a new application is READY, got {created}"
        );
        assert_eq!(
            created["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {name:?}: a new application is version 1, got {created}"
        );
        assert_eq!(
            created["RuntimeEnvironment"].as_str(),
            Some(runtime),
            "case {name:?}: expected the stored runtime environment, got {created}"
        );
        // The mode the creator named, else the one this runtime implies — the rule
        // `application_mode_and_maintenance_window.rs` asserts over the whole runtime set.
        let expected_mode = mode.unwrap_or(if runtime.starts_with("ZEPPELIN-") {
            "INTERACTIVE"
        } else {
            "STREAMING"
        });
        assert_eq!(
            created["ApplicationMode"].as_str(),
            Some(expected_mode),
            "case {name:?}: expected the stored application mode, got {created}"
        );
        assert_eq!(
            created["ApplicationDescription"].as_str(),
            description,
            "case {name:?}: expected the stored description, got {created}"
        );
        assert_eq!(
            created["ServiceExecutionRole"].as_str(),
            Some("arn:aws:iam::000000000000:role/kda-service-role"),
            "case {name:?}: expected the stored service execution role, got {created}"
        );
        assert!(
            created["CreateTimestamp"].as_i64().is_some_and(|t| t > 0),
            "case {name:?}: expected a create timestamp, got {created}"
        );
        assert_eq!(
            created["ApplicationVersionCreateTimestamp"].as_i64(),
            created["CreateTimestamp"].as_i64(),
            "case {name:?}: version 1 was created with the application, got {created}"
        );
        let token = created["ConditionalToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: expected a conditional token, got {created}"))
            .to_string();
        assert!(
            !tokens.contains(&token),
            "case {name:?}: conditional token {token} was already handed to another application"
        );
        tokens.push(token);

        // The round trip: the reader returns the record the writer stored.
        let described = describe(&router, name).await;
        assert_eq!(
            &described, created,
            "case {name:?}: expected describe to report exactly what create returned, got {described}"
        );
    }
}

/// The region an ARN carries is the one the caller signed for, not a fixed literal.
#[tokio::test]
async fn an_application_arn_carries_the_region_the_caller_signed_for() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = router();
        let (status, body) = call_in(
            &router,
            region,
            "CreateApplication",
            &create_body("regional-app", "FLINK-1_20"),
        )
        .await;
        assert_eq!(status, 200, "case {region:?}: CreateApplication got {body}");
        assert_eq!(
            body["ApplicationDetail"]["ApplicationARN"].as_str(),
            Some(
                format!("arn:aws:kinesisanalytics:{region}:{ACCOUNT}:application/regional-app")
                    .as_str()
            ),
            "case {region:?}: expected an ARN in the signed region, got {body}"
        );
    }
}

/// An `ApplicationConfiguration` supplied at create is persisted and described, with the
/// ids and derived values the service assigns.
///
/// The runtime is a Flink one because this API no longer creates SQL applications (see
/// `modeled_client_errors.rs`); the configuration itself is unchanged, since what is under
/// test is that every member a creator sets is stored and described, whichever runtime the
/// application declares.
#[tokio::test]
async fn an_application_configuration_is_persisted_and_described() {
    let router = router();
    let body = json!({
        "ApplicationName": "configured-app",
        "RuntimeEnvironment": "FLINK-1_20",
        "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
        "ApplicationConfiguration": {
            "SqlApplicationConfiguration": {
                "Inputs": [{
                    "NamePrefix": "SOURCE_SQL_STREAM",
                    "KinesisStreamsInput": {"ResourceARN": "arn:aws:kinesis:us-east-1:000000000000:stream/in"},
                    "InputParallelism": {"Count": 2},
                    "InputSchema": {
                        "RecordFormat": {"RecordFormatType": "JSON"},
                        "RecordColumns": [{"Name": "ticker", "SqlType": "VARCHAR(4)"}]
                    }
                }],
                "Outputs": [{
                    "Name": "DESTINATION_SQL_STREAM",
                    "KinesisFirehoseOutput": {"ResourceARN": "arn:aws:firehose:us-east-1:000000000000:deliverystream/out"},
                    "DestinationSchema": {"RecordFormatType": "CSV"}
                }]
            },
            "ApplicationCodeConfiguration": {
                "CodeContentType": "PLAINTEXT",
                "CodeContent": {"TextContent": "SELECT 1;"}
            },
            "FlinkApplicationConfiguration": {
                "ParallelismConfiguration": {
                    "ConfigurationType": "CUSTOM",
                    "Parallelism": 4,
                    "ParallelismPerKPU": 2,
                    "AutoScalingEnabled": true
                },
                "MonitoringConfiguration": {"ConfigurationType": "CUSTOM", "LogLevel": "WARN", "MetricsLevel": "TASK"}
            },
            "EnvironmentProperties": {
                "PropertyGroups": [{"PropertyGroupId": "group-one", "PropertyMap": {"k": "v"}}]
            },
            "ApplicationSnapshotConfiguration": {"SnapshotsEnabled": true}
        },
        "CloudWatchLoggingOptions": [
            {"LogStreamARN": "arn:aws:logs:us-east-1:000000000000:log-group:kda:log-stream:app"}
        ],
        "Tags": [{"Key": "team", "Value": "analytics"}]
    });

    let (status, response) = call(&router, "CreateApplication", &body).await;
    assert_eq!(status, 200, "CreateApplication got {response}");
    let created = response["ApplicationDetail"].clone();
    let config = &created["ApplicationConfigurationDescription"];

    let input = &config["SqlApplicationConfigurationDescription"]["InputDescriptions"][0];
    assert_eq!(
        input["NamePrefix"].as_str(),
        Some("SOURCE_SQL_STREAM"),
        "case sql-input: expected the stored name prefix, got {input}"
    );
    assert_eq!(
        input["InAppStreamNames"],
        json!(["SOURCE_SQL_STREAM_001", "SOURCE_SQL_STREAM_002"]),
        "case sql-input: expected one in-app stream per unit of parallelism, got {input}"
    );
    assert_eq!(
        input["KinesisStreamsInputDescription"]["ResourceARN"].as_str(),
        Some("arn:aws:kinesis:us-east-1:000000000000:stream/in"),
        "case sql-input: expected the stored source stream, got {input}"
    );
    assert!(
        input["InputId"].as_str().is_some_and(|id| !id.is_empty()),
        "case sql-input: expected a server-assigned InputId, got {input}"
    );

    let output = &config["SqlApplicationConfigurationDescription"]["OutputDescriptions"][0];
    assert_eq!(
        output["Name"].as_str(),
        Some("DESTINATION_SQL_STREAM"),
        "case sql-output: expected the stored output name, got {output}"
    );
    assert_eq!(
        output["DestinationSchema"]["RecordFormatType"].as_str(),
        Some("CSV"),
        "case sql-output: expected the stored destination schema, got {output}"
    );

    let code = &config["ApplicationCodeConfigurationDescription"];
    assert_eq!(
        code["CodeContentDescription"]["TextContent"].as_str(),
        Some("SELECT 1;"),
        "case code: expected the stored code text, got {code}"
    );
    assert_eq!(
        code["CodeContentDescription"]["CodeSize"].as_i64(),
        Some("SELECT 1;".len() as i64),
        "case code: expected the code size computed from the content, got {code}"
    );

    let parallelism =
        &config["FlinkApplicationConfigurationDescription"]["ParallelismConfigurationDescription"];
    assert_eq!(
        parallelism["Parallelism"].as_i64(),
        Some(4),
        "case parallelism: expected the configured parallelism, got {parallelism}"
    );
    assert_eq!(
        parallelism["CurrentParallelism"].as_i64(),
        Some(4),
        "case parallelism: with no autoscaling the current parallelism is the configured one, got {parallelism}"
    );

    assert_eq!(
        config["EnvironmentPropertyDescriptions"]["PropertyGroupDescriptions"][0]
            ["PropertyGroupId"]
            .as_str(),
        Some("group-one"),
        "case environment: expected the stored property group, got {config}"
    );
    assert_eq!(
        config["ApplicationSnapshotConfigurationDescription"]["SnapshotsEnabled"].as_bool(),
        Some(true),
        "case snapshots: expected the stored snapshot setting, got {config}"
    );
    assert_eq!(
        created["CloudWatchLoggingOptionDescriptions"][0]["LogStreamARN"].as_str(),
        Some("arn:aws:logs:us-east-1:000000000000:log-group:kda:log-stream:app"),
        "case logging: expected the stored log stream, got {created}"
    );

    // Server-assigned ids are stored, not minted per call: a second read repeats them.
    let described = describe(&router, "configured-app").await;
    assert_eq!(
        described, created,
        "case round-trip: expected describe to repeat the stored configuration, got {described}"
    );
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// `ListApplications` walks every stored application exactly once, in name order,
/// whatever page size the caller asks for.
///
/// The table spans the page sizes `ListApplicationsInputLimit` admits (range 1..50,
/// `smithy_query(query="shape",shape="ListApplicationsInputLimit")`): both published
/// bounds, sizes above and below the number of applications, and an absent `Limit`.
#[tokio::test]
async fn list_applications_pages_every_stored_application_in_name_order() {
    let router = router();
    let names = ["delta", "alpha", "echo", "charlie", "bravo"];
    for name in names {
        create(&router, name, "FLINK-1_20").await;
    }
    let mut expected: Vec<&str> = names.to_vec();
    expected.sort_unstable();

    for limit in [None, Some(1), Some(2), Some(4), Some(5), Some(50)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(limit) = limit {
                body["Limit"] = json!(limit);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let (status, response) = call(&router, "ListApplications", &body).await;
            assert_eq!(status, 200, "case limit={limit:?}: ListApplications got {response}");

            let summaries = response["ApplicationSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case limit={limit:?}: expected ApplicationSummaries, got {response}"))
                .clone();
            if let Some(limit) = limit {
                assert!(
                    summaries.len() <= limit as usize,
                    "case limit={limit:?}: a page may not exceed the limit, got {} entries",
                    summaries.len()
                );
            }
            for summary in &summaries {
                assert_eq!(
                    summary["ApplicationStatus"].as_str(),
                    Some("READY"),
                    "case limit={limit:?}: expected the stored status in the summary, got {summary}"
                );
                assert_eq!(
                    summary["ApplicationVersionId"].as_i64(),
                    Some(1),
                    "case limit={limit:?}: expected the stored version in the summary, got {summary}"
                );
                assert_eq!(
                    summary["RuntimeEnvironment"].as_str(),
                    Some("FLINK-1_20"),
                    "case limit={limit:?}: expected the stored runtime in the summary, got {summary}"
                );
                let name = summary["ApplicationName"].as_str().unwrap_or_default();
                assert_eq!(
                    summary["ApplicationARN"].as_str(),
                    Some(
                        format!("arn:aws:kinesisanalytics:us-east-1:{ACCOUNT}:application/{name}")
                            .as_str()
                    ),
                    "case limit={limit:?}: expected the stored ARN in the summary, got {summary}"
                );
                seen.push(name.to_string());
            }

            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case limit={limit:?}: pagination did not terminate after {pages} pages"
            );
            match response["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        token.len() <= 128 && !token.is_empty(),
                        "case limit={limit:?}: a NextToken must satisfy len 1..128, got {token:?}"
                    );
                    assert!(
                        token.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)),
                        "case limit={limit:?}: a NextToken must match ^[a-zA-Z0-9_.-]+$, got {token:?}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case limit={limit:?}: expected every application exactly once in name order, got {seen:?}"
        );
    }
}

/// A `Limit` outside the modelled range is refused, at both published bounds' far side.
#[tokio::test]
async fn a_limit_outside_the_modelled_range_is_refused() {
    let router = router();
    create(&router, "an-app", "FLINK-1_20").await;
    for limit in [0, 51, -1, 1_000] {
        let (status, body) = call(&router, "ListApplications", &json!({"Limit": limit})).await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a value outside range 1..50 must be refused, got {status} {body}"
        );
    }
    // …and both published bounds themselves are accepted.
    for limit in [1, 50] {
        let (status, body) = call(&router, "ListApplications", &json!({"Limit": limit})).await;
        assert_eq!(
            status, 200,
            "case Limit={limit}: a published bound must be accepted, got {status} {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Start / Stop
// ---------------------------------------------------------------------------

/// Start and stop are asynchronous: each answers with an `OperationId` while the
/// application sits in the transient status, and the terminal status becomes due on the
/// clock without any further call.
#[tokio::test]
async fn start_and_stop_move_the_stored_status_through_the_transient_state() {
    let router = router();
    create_startable(&router, "lifecycle-app", "FLINK-1_20").await;

    let (status, started) = call(
        &router,
        "StartApplication",
        &json!({"ApplicationName": "lifecycle-app"}),
    )
    .await;
    assert_eq!(status, 200, "case start: StartApplication got {started}");
    let start_operation = started["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case start: expected an OperationId, got {started}"))
        .to_string();
    assert!(
        !start_operation.is_empty() && start_operation.len() <= 64,
        "case start: an OperationId must satisfy len 1..64, got {start_operation:?}"
    );

    assert_eq!(
        describe(&router, "lifecycle-app").await["ApplicationStatus"].as_str(),
        Some("STARTING"),
        "case start: a caller reading straight back must still see the transient status"
    );

    // While starting, the application is in use: neither a second start nor an
    // unforced stop is accepted.
    for (op, body, expected) in [
        (
            "StartApplication",
            json!({"ApplicationName": "lifecycle-app"}),
            "ResourceInUseException",
        ),
        (
            "StopApplication",
            json!({"ApplicationName": "lifecycle-app"}),
            "ResourceInUseException",
        ),
    ] {
        let (status, body) = call(&router, op, &body).await;
        assert_eq!(status, 400, "case starting/{op}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            expected,
            "case starting/{op}: expected {expected}, got {body}"
        );
    }

    std::thread::sleep(SETTLE_WAIT);
    let running = describe(&router, "lifecycle-app").await;
    assert_eq!(
        running["ApplicationStatus"].as_str(),
        Some("RUNNING"),
        "case start: the clock owed RUNNING, got {running}"
    );
    assert!(
        running["LastUpdateTimestamp"].as_i64().is_some(),
        "case start: the settled transition stamps LastUpdateTimestamp, got {running}"
    );

    let (status, stopped) = call(
        &router,
        "StopApplication",
        &json!({"ApplicationName": "lifecycle-app"}),
    )
    .await;
    assert_eq!(status, 200, "case stop: StopApplication got {stopped}");
    let stop_operation = stopped["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case stop: expected an OperationId, got {stopped}"))
        .to_string();
    assert_ne!(
        stop_operation, start_operation,
        "case stop: each operation gets its own id, got {stop_operation} twice"
    );

    assert_eq!(
        describe(&router, "lifecycle-app").await["ApplicationStatus"].as_str(),
        Some("STOPPING"),
        "case stop: a caller reading straight back must still see the transient status"
    );

    std::thread::sleep(SETTLE_WAIT);
    assert_eq!(
        describe(&router, "lifecycle-app").await["ApplicationStatus"].as_str(),
        Some("READY"),
        "case stop: the clock owed READY"
    );

    // The status a list reports is the same stored status describe reports.
    let (status, listed) = call(&router, "ListApplications", &json!({})).await;
    assert_eq!(status, 200, "case stop: ListApplications got {listed}");
    assert_eq!(
        listed["ApplicationSummaries"][0]["ApplicationStatus"].as_str(),
        Some("READY"),
        "case stop: expected the settled status in the summary, got {listed}"
    );
}

/// `Force` widens the statuses a stop is accepted from to the set its doc names, so a
/// forced stop of a still-starting application is accepted where an unforced one is not.
#[tokio::test]
async fn a_forced_stop_is_accepted_from_the_transient_starting_status() {
    let router = router();
    create_startable(&router, "force-app", "FLINK-1_20").await;
    call(
        &router,
        "StartApplication",
        &json!({"ApplicationName": "force-app"}),
    )
    .await;

    let (status, body) = call(
        &router,
        "StopApplication",
        &json!({"ApplicationName": "force-app", "Force": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case force-stop-while-starting: expected 200, got {status} {body}"
    );
    assert_eq!(
        describe(&router, "force-app").await["ApplicationStatus"].as_str(),
        Some("STOPPING"),
        "case force-stop-while-starting: expected the transient stopping status"
    );

    std::thread::sleep(SETTLE_WAIT);
    assert_eq!(
        describe(&router, "force-app").await["ApplicationStatus"].as_str(),
        Some("READY"),
        "case force-stop-while-starting: the clock owed READY"
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// A delete removes the application from every reader, and only when the caller names
/// the create timestamp it last read.
#[tokio::test]
async fn delete_removes_the_application_from_every_reader() {
    let router = router();
    for name in ["keep-one", "remove-me", "keep-two"] {
        create(&router, name, "FLINK-1_20").await;
    }
    let created = describe(&router, "remove-me").await;
    let create_timestamp = created["CreateTimestamp"]
        .as_i64()
        .expect("the stored create timestamp");

    // A create timestamp that does not identify the stored application is refused.
    for wrong in [create_timestamp - 1, create_timestamp + 60, 0] {
        let (status, body) = call(
            &router,
            "DeleteApplication",
            &json!({"ApplicationName": "remove-me", "CreateTimestamp": wrong}),
        )
        .await;
        assert_eq!(status, 400, "case CreateTimestamp={wrong}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidArgumentException",
            "case CreateTimestamp={wrong}: expected InvalidArgumentException, got {body}"
        );
    }
    assert_eq!(
        describe(&router, "remove-me").await["ApplicationName"].as_str(),
        Some("remove-me"),
        "case refused-delete: a refused delete must leave the application stored"
    );

    let (status, body) = call(
        &router,
        "DeleteApplication",
        &json!({"ApplicationName": "remove-me", "CreateTimestamp": create_timestamp}),
    )
    .await;
    assert_eq!(status, 200, "case delete: expected 200, got {status} {body}");

    let (status, body) = call(
        &router,
        "DescribeApplication",
        &json!({"ApplicationName": "remove-me"}),
    )
    .await;
    assert_eq!(status, 400, "case deleted-describe: expected 400, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case deleted-describe: expected ResourceNotFoundException, got {body}"
    );

    let (status, listed) = call(&router, "ListApplications", &json!({})).await;
    assert_eq!(status, 200, "case deleted-list: ListApplications got {listed}");
    let remaining: Vec<&str> = listed["ApplicationSummaries"]
        .as_array()
        .expect("ApplicationSummaries")
        .iter()
        .filter_map(|summary| summary["ApplicationName"].as_str())
        .collect();
    assert_eq!(
        remaining,
        vec!["keep-one", "keep-two"],
        "case deleted-list: expected the deleted application to be gone, got {remaining:?}"
    );
}

/// A running application has to settle back to READY before it can be deleted.
#[tokio::test]
async fn a_running_application_cannot_be_deleted() {
    let router = router();
    let created = create_startable(&router, "busy-app", "FLINK-1_20").await;
    let create_timestamp = created["CreateTimestamp"].as_i64().expect("create timestamp");
    call(
        &router,
        "StartApplication",
        &json!({"ApplicationName": "busy-app"}),
    )
    .await;

    for case in ["STARTING", "RUNNING"] {
        if case == "RUNNING" {
            std::thread::sleep(SETTLE_WAIT);
        }
        let observed = describe(&router, "busy-app").await["ApplicationStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(observed, case, "case {case}: expected the application to be {case}");
        let (status, body) = call(
            &router,
            "DeleteApplication",
            &json!({"ApplicationName": "busy-app", "CreateTimestamp": create_timestamp}),
        )
        .await;
        assert_eq!(status, 400, "case {case}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {case}: expected ResourceInUseException, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Every lifecycle operation refuses a name it holds no application for with
/// `ResourceNotFoundException` (HTTP 400), and refuses a second create of a stored name
/// with `ResourceInUseException`.
#[tokio::test]
async fn an_unknown_or_duplicate_application_is_refused_with_the_modelled_error() {
    let router = router();
    let created = create(&router, "known-app", "FLINK-1_20").await;
    let create_timestamp = created["CreateTimestamp"].as_i64().expect("create timestamp");

    // (case, operation, body, expected error code)
    let cases: [(&str, &str, Value, &str); 6] = [
        (
            "describe-unknown",
            "DescribeApplication",
            json!({"ApplicationName": "no-such-app"}),
            "ResourceNotFoundException",
        ),
        (
            "start-unknown",
            "StartApplication",
            json!({"ApplicationName": "no-such-app"}),
            "ResourceNotFoundException",
        ),
        (
            "stop-unknown",
            "StopApplication",
            json!({"ApplicationName": "no-such-app"}),
            "ResourceNotFoundException",
        ),
        (
            "delete-unknown",
            "DeleteApplication",
            json!({"ApplicationName": "no-such-app", "CreateTimestamp": create_timestamp}),
            "ResourceNotFoundException",
        ),
        (
            "stop-not-running",
            "StopApplication",
            json!({"ApplicationName": "known-app"}),
            "ResourceInUseException",
        ),
        (
            "create-duplicate",
            "CreateApplication",
            create_body("known-app", "FLINK-1_8"),
            "ResourceInUseException",
        ),
    ];

    for (case, operation, body, expected) in cases {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(status, 400, "case {case}: expected 400, got {status} {response}");
        assert_eq!(
            error_code(&response),
            expected,
            "case {case}: expected {expected}, got {response}"
        );
    }

    // A refused duplicate create leaves the stored application untouched.
    assert_eq!(
        describe(&router, "known-app").await["RuntimeEnvironment"].as_str(),
        Some("FLINK-1_20"),
        "case create-duplicate: a refused create must not overwrite the stored application"
    );
}

/// A create whose input breaks a modelled constraint is refused at the boundary with
/// HTTP 400 and stores nothing.
#[tokio::test]
async fn a_create_that_breaks_a_modelled_constraint_is_refused() {
    let router = router();
    // (case, body) — each row breaks exactly one modelled constraint.
    let cases: [(&str, Value); 6] = [
        (
            "unknown-runtime-environment",
            create_body("bad-app", "FLINK-9_9"),
        ),
        (
            "name-outside-the-pattern",
            create_body("not a valid name", "FLINK-1_20"),
        ),
        ("empty-name", create_body("", "FLINK-1_20")),
        (
            "role-not-an-arn",
            json!({
                "ApplicationName": "bad-app",
                "RuntimeEnvironment": "FLINK-1_20",
                "ServiceExecutionRole": "not-an-arn",
            }),
        ),
        (
            "missing-runtime-environment",
            json!({
                "ApplicationName": "bad-app",
                "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
            }),
        ),
        (
            "unknown-application-mode",
            json!({
                "ApplicationName": "bad-app",
                "RuntimeEnvironment": "FLINK-1_20",
                "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
                "ApplicationMode": "BATCH",
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, response) = call(&router, "CreateApplication", &body).await;
        assert_eq!(status, 400, "case {case}: expected 400, got {status} {response}");
    }

    let (status, listed) = call(&router, "ListApplications", &json!({})).await;
    assert_eq!(status, 200, "ListApplications got {listed}");
    assert_eq!(
        listed["ApplicationSummaries"],
        json!([]),
        "a refused create must store nothing, got {listed}"
    );
}

/// More user-defined tags than the documented ceiling is refused with
/// `TooManyTagsException`, and the ceiling itself is accepted.
#[tokio::test]
async fn more_user_tags_than_the_documented_ceiling_are_refused() {
    let router = router();
    // (case, tag count, expected http status)
    for (count, expected) in [(1_usize, 200), (50, 200), (51, 400), (120, 400)] {
        let name = format!("tagged-{count}");
        let tags: Vec<Value> = (0..count)
            .map(|n| json!({"Key": format!("k{n}"), "Value": format!("v{n}")}))
            .collect();
        let mut body = create_body(&name, "FLINK-1_20");
        body["Tags"] = json!(tags);
        let (status, response) = call(&router, "CreateApplication", &body).await;
        assert_eq!(
            status, expected,
            "case {count} tags: expected {expected}, got {status} {response}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "TooManyTagsException",
                "case {count} tags: expected TooManyTagsException, got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Bounds on caller-driven growth
// ---------------------------------------------------------------------------

/// `InputParallelism$Count` sizes the in-app stream name list, so it is bounded: the
/// published range (1..64) is honoured at both ends and one past each end is refused.
#[tokio::test]
async fn input_parallelism_is_bounded_by_the_modelled_range() {
    let router = router();
    // (Count, expected http status, expected number of in-app stream names)
    let cases: [(i64, u16, usize); 5] = [
        (1, 200, 1),
        (2, 200, 2),
        (64, 200, 64),
        (65, 400, 0),
        (0, 400, 0),
    ];
    for (count, expected_status, expected_names) in cases {
        let name = format!("parallel-{count}");
        // A Flink runtime: the SQL one is no longer created (see
        // `modeled_client_errors.rs`), while the input configuration under test is stored
        // and described the same way whichever runtime the application declares.
        let mut body = create_body(&name, "FLINK-1_20");
        body["ApplicationConfiguration"] = json!({
            "SqlApplicationConfiguration": {
                "Inputs": [{
                    "NamePrefix": "S",
                    "InputParallelism": {"Count": count},
                    "InputSchema": {
                        "RecordFormat": {"RecordFormatType": "JSON"},
                        "RecordColumns": [{"Name": "c", "SqlType": "VARCHAR(4)"}]
                    }
                }]
            }
        });
        let (status, response) = call(&router, "CreateApplication", &body).await;
        assert_eq!(
            status, expected_status,
            "case Count={count}: expected {expected_status}, got {status} {response}"
        );
        if expected_status != 200 {
            continue;
        }
        let input = &response["ApplicationDetail"]["ApplicationConfigurationDescription"]
            ["SqlApplicationConfigurationDescription"]["InputDescriptions"][0];
        let names = input["InAppStreamNames"]
            .as_array()
            .unwrap_or_else(|| panic!("case Count={count}: expected InAppStreamNames, got {input}"));
        assert_eq!(
            names.len(),
            expected_names,
            "case Count={count}: expected one in-app stream per unit of parallelism, got {input}"
        );
    }
}

/// The store holds every application for the life of the process, so a create past its
/// ceiling is refused with the modelled `LimitExceededException` instead of growing it.
#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused() {
    use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state;

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    create(&router, "seed-app", "FLINK-1_20").await;

    // Fill the store to its ceiling without issuing the calls that would take minutes:
    // the records are the same shape a create writes, cloned under a fresh name.
    let store = state::service_state(&*engine).expect("the service holds state");
    {
        let mut guard = store.lock();
        let template = guard.applications["seed-app"].clone();
        for n in 0..state::MAX_APPLICATIONS - 1 {
            let mut filler = template.clone();
            filler.name = format!("filler-{n:06}");
            guard.applications.insert(filler.name.clone(), filler);
        }
        assert_eq!(
            guard.applications.len(),
            state::MAX_APPLICATIONS,
            "case full-store: expected the store filled to its ceiling"
        );
    }

    let (status, response) = call(
        &router,
        "CreateApplication",
        &create_body("one-too-many", "FLINK-1_20"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case full-store: expected 400, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case full-store: expected LimitExceededException, got {response}"
    );
    assert_eq!(
        store.lock().applications.len(),
        state::MAX_APPLICATIONS,
        "case full-store: a refused create must not grow the store"
    );

    // Room again once something is deleted, so the cap is a ceiling and not a wedge.
    store.lock().applications.remove("filler-000000");
    let (status, response) = call(
        &router,
        "CreateApplication",
        &create_body("room-again", "FLINK-1_20"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case room-again: expected 200, got {status} {response}"
    );
}

/// One application's operation log is capped and trimmed oldest-first, so repeated
/// Start/Stop calls cannot grow a single record without limit.
#[tokio::test]
async fn the_operation_log_of_one_application_is_capped() {
    use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state;

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    create(&router, "logged-app", "FLINK-1_20").await;

    let store = state::service_state(&*engine).expect("the service holds state");
    let now = state::now_timestamp();
    let overflow = state::MAX_OPERATIONS_PER_APPLICATION + 5;
    let issued: Vec<String> = (0..overflow)
        .map(|_| {
            store
                .lock()
                .record_operation("logged-app", "StopApplication", now)
        })
        .collect();

    let retained: Vec<String> = store.lock().applications["logged-app"]
        .operations
        .iter()
        .map(|operation| operation.operation_id.clone())
        .collect();
    assert_eq!(
        retained.len(),
        state::MAX_OPERATIONS_PER_APPLICATION,
        "case {overflow} operations: expected the log capped at its ceiling, got {} entries",
        retained.len()
    );
    assert!(
        !retained.contains(&issued[0]),
        "case {overflow} operations: expected the oldest record trimmed first, but {} is still held",
        issued[0]
    );
    assert_eq!(
        retained.last(),
        issued.last(),
        "case {overflow} operations: expected the newest record retained, got {:?}",
        retained.last()
    );
}
