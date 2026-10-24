// SPDX-License-Identifier: CC0-1.0

//! `UpdatePipe`, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — reusing the harness in `tests/all/pipe_lifecycle.rs`
//! rather than copying it, so what is tested is the HTTP behaviour of `UpdatePipe` read
//! back through `DescribePipe`/`ListPipes` and not a Rust helper.
//!
//! The tables span the class each rule generalises over: which members the request
//! carries, the form and type of their values, the lifecycle state the pipe is in, and
//! the number of concurrent writers.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{
    call, create, describe, list, listed_names, minimal_body, router, Reply, PAST_SETTLE, REGION,
};

const ROLE_A: &str = "arn:aws:iam::000000000000:role/pipes-role";
const ROLE_B: &str = "arn:aws:iam::000000000000:role/pipes-other";
const TARGET_A: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";
const TARGET_B: &str = "arn:aws:sqs:us-west-2:000000000000:results";
const SOURCE_SQS: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const SOURCE_KINESIS: &str = "arn:aws:kinesis:us-west-2:000000000000:stream/events";
const ENRICHMENT: &str = "arn:aws:lambda:us-west-2:000000000000:function:enrich";

/// One `UpdatePipe` call: `PUT /v1/pipes/{Name}`. Source: the `UpdatePipe` http binding
/// in `OPERATIONS.md`.
async fn update(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(
        router,
        "PUT",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await
}

/// A `CreatePipe` body carrying the three required members plus `extra`.
fn create_body(source: &str, target: &str, extra: Value) -> Value {
    let mut body = minimal_body(source, target);
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        body[member.clone()] = value.clone();
    }
    body
}

/// One row of the member-by-member update table.
struct Case {
    /// Also the pipe's name, so every row is independent.
    name: String,
    create: Value,
    update: Value,
    /// Members `DescribePipe` must report with exactly this value after the update.
    expected: Value,
    /// Members `DescribePipe` must report nothing for after the update.
    absent: Vec<&'static str>,
}

/// RULE: `UpdatePipe` applies to the stored pipe exactly the members the request carries
/// and leaves every member it omits unchanged; a service-specific parameters object the
/// request carries replaces the stored one atomically (a member it omits returns to its
/// system default), keeping only the members that object is created with and the update
/// shape cannot express.
///
/// The rows differ along what that rule generalises over: which members the request
/// carries, their value (text, unicode text, the empty string, an ARN, numbers, nested
/// structures), which parameters branch is updated, and the pipe's name length.
#[tokio::test]
async fn update_pipe_changes_only_the_members_the_request_carries() {
    let cases = vec![
        Case {
            name: "description-only".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({"Description": "before", "Tags": {"env": "test"}}),
            ),
            update: json!({"RoleArn": ROLE_A, "Description": "after"}),
            expected: json!({
                "Description": "after",
                "Target": TARGET_A,
                "RoleArn": ROLE_A,
                "Tags": {"env": "test"},
            }),
            absent: vec![],
        },
        Case {
            name: "unicode-description".to_string(),
            create: create_body(SOURCE_SQS, TARGET_A, json!({"Description": "before"})),
            update: json!({
                "RoleArn": ROLE_A,
                "Description": "données — 日本語 ✅ 🚀 naïve",
            }),
            expected: json!({"Description": "données — 日本語 ✅ 🚀 naïve"}),
            absent: vec![],
        },
        Case {
            name: "empty-description".to_string(),
            create: create_body(SOURCE_SQS, TARGET_A, json!({"Description": "before"})),
            // `PipeDescription` is `@length(min: 0)`, so the empty string is a value a
            // description can hold rather than a way of removing it.
            update: json!({"RoleArn": ROLE_A, "Description": ""}),
            expected: json!({"Description": ""}),
            absent: vec![],
        },
        Case {
            name: "role-arn-only".to_string(),
            create: create_body(SOURCE_SQS, TARGET_A, json!({"Description": "kept"})),
            update: json!({"RoleArn": ROLE_B}),
            expected: json!({
                "RoleArn": ROLE_B,
                "Description": "kept",
                "Target": TARGET_A,
            }),
            absent: vec![],
        },
        Case {
            name: "target-replaced".to_string(),
            create: create_body(SOURCE_SQS, TARGET_A, json!({"Description": "kept"})),
            update: json!({"RoleArn": ROLE_A, "Target": TARGET_B}),
            expected: json!({"Target": TARGET_B, "Description": "kept"}),
            absent: vec![],
        },
        Case {
            name: "target-parameters-overridden-as-one".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_B,
                json!({
                    "TargetParameters": {
                        "InputTemplate": "$.detail",
                        "SqsQueueParameters": {"MessageGroupId": "g1"},
                    },
                }),
            ),
            update: json!({
                "RoleArn": ROLE_A,
                "TargetParameters": {"InputTemplate": "$.id"},
            }),
            // `SqsQueueParameters` was not re-specified, so it is back at its default.
            expected: json!({"TargetParameters": {"InputTemplate": "$.id"}}),
            absent: vec![],
        },
        Case {
            name: "enrichment-removed".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({
                    "Enrichment": ENRICHMENT,
                    "EnrichmentParameters": {"InputTemplate": "$.detail"},
                }),
            ),
            // `OptionalArn` admits the empty string, which is how an enrichment resource
            // is removed.
            update: json!({"RoleArn": ROLE_A, "Enrichment": ""}),
            expected: json!({}),
            absent: vec!["Enrichment"],
        },
        Case {
            name: "enrichment-added".to_string(),
            create: minimal_body(SOURCE_SQS, TARGET_A),
            update: json!({
                "RoleArn": ROLE_A,
                "Enrichment": ENRICHMENT,
                "EnrichmentParameters": {"InputTemplate": "$.detail"},
            }),
            expected: json!({
                "Enrichment": ENRICHMENT,
                "EnrichmentParameters": {"InputTemplate": "$.detail"},
            }),
            absent: vec![],
        },
        Case {
            name: "kms-key-removed".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({"KmsKeyIdentifier": "alias/pipes-key"}),
            ),
            // The operation doc: specifying an empty string moves the pipe back to the
            // Amazon Web Services owned key, so no customer managed key is reported.
            update: json!({"RoleArn": ROLE_A, "KmsKeyIdentifier": ""}),
            expected: json!({}),
            absent: vec!["KmsKeyIdentifier"],
        },
        Case {
            name: "kms-key-replaced".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({"KmsKeyIdentifier": "alias/pipes-key"}),
            ),
            update: json!({
                "RoleArn": ROLE_A,
                "KmsKeyIdentifier": "arn:aws:kms:us-west-2:000000000000:key/abcd1234",
            }),
            expected: json!({
                "KmsKeyIdentifier": "arn:aws:kms:us-west-2:000000000000:key/abcd1234",
            }),
            absent: vec![],
        },
        Case {
            name: "source-parameters-overridden-as-one".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({
                    "SourceParameters": {
                        "FilterCriteria": {"Filters": [{"Pattern": "{\"source\":[\"demo\"]}"}]},
                        "SqsQueueParameters": {
                            "BatchSize": 7,
                            "MaximumBatchingWindowInSeconds": 3,
                        },
                    },
                }),
            ),
            update: json!({
                "RoleArn": ROLE_A,
                "SourceParameters": {"SqsQueueParameters": {"BatchSize": 9}},
            }),
            // Neither the filter criteria nor the batching window was re-specified, so
            // both are back at their defaults.
            expected: json!({"SourceParameters": {"SqsQueueParameters": {"BatchSize": 9}}}),
            absent: vec![],
        },
        Case {
            name: "source-parameters-keep-create-only-members".to_string(),
            create: create_body(
                SOURCE_KINESIS,
                TARGET_A,
                json!({
                    "SourceParameters": {
                        "KinesisStreamParameters": {
                            "StartingPosition": "LATEST",
                            "BatchSize": 5,
                            "MaximumRetryAttempts": 4,
                        },
                    },
                }),
            ),
            update: json!({
                "RoleArn": ROLE_A,
                "SourceParameters": {"KinesisStreamParameters": {"BatchSize": 11}},
            }),
            // `UpdatePipeSourceKinesisStreamParameters` has no `StartingPosition`, so the
            // created one is kept; `MaximumRetryAttempts` it CAN express, so dropping it
            // returns it to its default.
            expected: json!({
                "SourceParameters": {
                    "KinesisStreamParameters": {
                        "StartingPosition": "LATEST",
                        "BatchSize": 11,
                    },
                },
            }),
            absent: vec![],
        },
        Case {
            name: "log-configuration-replaced".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({
                    "LogConfiguration": {
                        "Level": "INFO",
                        "CloudwatchLogsLogDestination": {
                            "LogGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/aws/pipes",
                        },
                    },
                }),
            ),
            update: json!({
                "RoleArn": ROLE_A,
                "LogConfiguration": {
                    "Level": "ERROR",
                    "S3LogDestination": {
                        "BucketName": "pipe-logs",
                        "BucketOwner": "000000000000",
                        "OutputFormat": "json",
                    },
                },
            }),
            expected: json!({
                "LogConfiguration": {
                    "Level": "ERROR",
                    "S3LogDestination": {
                        "BucketName": "pipe-logs",
                        "BucketOwner": "000000000000",
                        "OutputFormat": "json",
                    },
                },
            }),
            absent: vec![],
        },
        Case {
            name: "desired-state-and-description".to_string(),
            create: create_body(
                SOURCE_SQS,
                TARGET_A,
                json!({"DesiredState": "RUNNING", "Description": "before"}),
            ),
            update: json!({
                "RoleArn": ROLE_A,
                "DesiredState": "STOPPED",
                "Description": "stopping",
            }),
            expected: json!({"Description": "stopping", "DesiredState": "STOPPED"}),
            absent: vec![],
        },
        Case {
            // The longest name `PipeName` allows (`@length(max: 64)`).
            name: "u".repeat(64),
            create: minimal_body(SOURCE_SQS, TARGET_A),
            update: json!({"RoleArn": ROLE_A, "Description": "longest name"}),
            expected: json!({"Description": "longest name"}),
            absent: vec![],
        },
    ];

    for case in cases {
        let name = case.name.as_str();
        let router = router();
        let created = create(&router, name, case.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreatePipe must succeed, got {created:?}"
        );
        // Past the settle span the pipe is out of CREATING, so the update is accepted.
        tokio::time::sleep(PAST_SETTLE).await;

        let updated = update(&router, name, case.update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {name:?}: UpdatePipe must succeed, got {updated:?}"
        );
        assert_eq!(
            updated.string("Arn"),
            created.string("Arn"),
            "case {name:?}: an update must not re-mint the ARN, got {updated:?}"
        );
        assert_eq!(
            updated.string("Name"),
            name,
            "case {name:?}: UpdatePipe must echo the name, got {updated:?}"
        );
        assert_eq!(
            updated.number("CreationTime"),
            created.number("CreationTime"),
            "case {name:?}: CreationTime must never change, got {updated:?}"
        );
        assert!(
            updated.number("LastModifiedTime") > created.number("LastModifiedTime"),
            "case {name:?}: the update must advance LastModifiedTime past {}, got {:?}",
            created.number("LastModifiedTime"),
            updated.number("LastModifiedTime")
        );

        // An update that names no `DesiredState` leaves the pipe's own one alone.
        let expected_desired = case
            .update
            .get("DesiredState")
            .or_else(|| case.create.get("DesiredState"))
            .and_then(Value::as_str)
            .unwrap_or("RUNNING")
            .to_string();
        assert_eq!(
            updated.string("DesiredState"),
            expected_desired,
            "case {name:?}: DesiredState must be {expected_desired}, got {updated:?}"
        );

        tokio::time::sleep(PAST_SETTLE).await;
        let described = describe(&router, name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: DescribePipe must still find the pipe, got {described:?}"
        );
        for (member, expected) in case
            .expected
            .as_object()
            .expect("the case's expectations are an object")
        {
            assert_eq!(
                described.member(member),
                expected,
                "case {name:?}: DescribePipe must report {member} as {expected}, got {:?}",
                described.member(member)
            );
        }
        for member in &case.absent {
            assert_eq!(
                described.member(member),
                &Value::Null,
                "case {name:?}: {member} was removed, so DescribePipe must report none, \
                 got {:?}",
                described.member(member)
            );
        }
        // `Source` is not an `UpdatePipe` member, so no update can move it.
        assert_eq!(
            described.member("Source"),
            case.create
                .get("Source")
                .expect("every case creates a source"),
            "case {name:?}: Source is not updatable, got {:?}",
            described.member("Source")
        );
        assert_eq!(
            described.string("CurrentState"),
            expected_desired,
            "case {name:?}: past the settle span the pipe must be {expected_desired}, \
             got {described:?}"
        );
    }
}

/// RULE: an accepted update puts the pipe into `UPDATING` at once and the move into the
/// state it is meant to be in is owed to the clock, so a caller acting straight after the
/// call still sees `UPDATING` and one past the span sees the settled state.
///
/// The rows differ along the pipe's stored desired state and whether the update names one.
#[tokio::test]
async fn update_pipe_reports_updating_then_settles_into_the_desired_state() {
    // (case, the created DesiredState, the DesiredState the update names, the settled state)
    let rows: Vec<(&str, Option<&str>, Option<&str>, &str)> = vec![
        ("default-untouched", None, None, "RUNNING"),
        ("running-untouched", Some("RUNNING"), None, "RUNNING"),
        ("running-to-stopped", Some("RUNNING"), Some("STOPPED"), "STOPPED"),
        ("stopped-untouched", Some("STOPPED"), None, "STOPPED"),
        ("stopped-to-running", Some("STOPPED"), Some("RUNNING"), "RUNNING"),
        ("stopped-restated", Some("STOPPED"), Some("STOPPED"), "STOPPED"),
    ];

    for (case, created_state, requested_state, settled) in rows {
        let router = router();
        let mut body = minimal_body(SOURCE_SQS, TARGET_A);
        if let Some(created_state) = created_state {
            body["DesiredState"] = json!(created_state);
        }
        let created = create(&router, case, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );
        tokio::time::sleep(PAST_SETTLE).await;

        let mut update_body = json!({"RoleArn": ROLE_A});
        if let Some(requested_state) = requested_state {
            update_body["DesiredState"] = json!(requested_state);
        }
        let updated = update(&router, case, update_body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdatePipe must succeed, got {updated:?}"
        );
        assert_eq!(
            updated.string("CurrentState"),
            "UPDATING",
            "case {case:?}: an accepted update reports UPDATING, got {updated:?}"
        );
        assert_eq!(
            updated.string("DesiredState"),
            settled,
            "case {case:?}: DesiredState must be {settled}, got {updated:?}"
        );

        let immediately = describe(&router, case).await;
        assert_eq!(
            immediately.string("CurrentState"),
            "UPDATING",
            "case {case:?}: a caller acting straight after the update still sees \
             UPDATING, got {immediately:?}"
        );

        // Nothing sleeps in the handler; the due instant is stored, so the move shows up
        // on the first read taken past it.
        tokio::time::sleep(PAST_SETTLE).await;
        let settled_read = describe(&router, case).await;
        assert_eq!(
            settled_read.string("CurrentState"),
            settled,
            "case {case:?}: past the settle span the pipe must be {settled}, \
             got {settled_read:?}"
        );
        assert_eq!(
            settled_read.number("LastModifiedTime"),
            immediately.number("LastModifiedTime"),
            "case {case:?}: settling a state move must not re-stamp LastModifiedTime, \
             got {settled_read:?}"
        );
    }
}

/// One row of the mid-transition table: the pipe/case name, the HTTP method of the call
/// that starts the transition (empty for the CREATING row, which the create itself
/// starts), that call's URI suffix under `/v1/pipes/{Name}`, and the transient state it
/// leaves the pipe in.
type TransitionRow = (&'static str, &'static str, &'static str, &'static str);

/// RULE: a pipe whose last call left it mid-transition is not reconfigurable — until that
/// move lands, `UpdatePipe` is refused with `ConflictException` (409) and the stored
/// record is left exactly as it was; once it has landed the same update is accepted.
///
/// The rows differ along which call started the transition, hence which transient state
/// the pipe is in.
#[tokio::test]
async fn update_pipe_during_a_transition_is_refused_and_changes_nothing() {
    let rows: Vec<TransitionRow> = vec![
        ("creating", "", "", "CREATING"),
        ("stopping", "POST", "/stop", "STOPPING"),
        ("starting", "POST", "/start", "STARTING"),
        ("updating", "PUT", "", "UPDATING"),
    ];

    for (case, method, suffix, transient) in rows {
        let router = router();
        let created = create(
            &router,
            case,
            create_body(SOURCE_SQS, TARGET_A, json!({"Description": "kept"})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        if !method.is_empty() {
            // Let the CREATING move land first, then start the one under test.
            tokio::time::sleep(PAST_SETTLE).await;
            let uri = format!("/v1/pipes/{case}{suffix}");
            let body = if method == "PUT" {
                Some(json!({"RoleArn": ROLE_A, "Description": "kept"}))
            } else {
                None
            };
            let starting = call(&router, method, &uri, body, REGION).await;
            assert_eq!(
                starting.status, 200,
                "case {case:?}: the call that starts the transition must succeed, \
                 got {starting:?}"
            );
            assert_eq!(
                starting.string("CurrentState"),
                transient,
                "case {case:?}: that call must leave the pipe {transient}, got {starting:?}"
            );
        }

        let refused = update(
            &router,
            case,
            json!({"RoleArn": ROLE_B, "Description": "must not land"}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: an update while {transient} must be 409 ConflictException, \
             got {refused:?}"
        );

        let described = describe(&router, case).await;
        assert_eq!(
            described.string("Description"),
            "kept",
            "case {case:?}: a refused update must not have changed the record, \
             got {described:?}"
        );
        assert_eq!(
            described.string("RoleArn"),
            ROLE_A,
            "case {case:?}: a refused update must not have changed the record, \
             got {described:?}"
        );

        // Once the move has landed the very same update is accepted.
        tokio::time::sleep(PAST_SETTLE).await;
        let accepted = update(
            &router,
            case,
            json!({"RoleArn": ROLE_B, "Description": "lands now"}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: past the transition the update must be accepted, \
             got {accepted:?}"
        );
        let reread = describe(&router, case).await;
        assert_eq!(
            reread.string("Description"),
            "lands now",
            "case {case:?}: the accepted update must be readable back, got {reread:?}"
        );
    }
}

/// RULE: `UpdatePipe` names a pipe that must already exist — a name nothing created is
/// `NotFoundException` (404) and no pipe by that name comes into being.
///
/// The rows differ along the form of the absent name: plain, differing only in case from
/// a stored one, the longest the model allows, and one carrying the other characters
/// `PipeName`'s pattern admits.
#[tokio::test]
async fn update_pipe_on_an_unknown_name_is_not_found_and_creates_nothing() {
    let router = router();
    let created = create(&router, "stored", minimal_body(SOURCE_SQS, TARGET_A)).await;
    assert_eq!(
        created.status, 200,
        "the fixture pipe must be created, got {created:?}"
    );

    let long_name = "n".repeat(64);
    let rows: Vec<&str> = vec![
        "never-created",
        // `PipeName` is case-sensitive, so this is a different pipe from "stored".
        "STORED",
        long_name.as_str(),
        "dotted.name_with-every.allowed_char0",
    ];
    for name in rows {
        let refused = update(
            &router,
            name,
            json!({"RoleArn": ROLE_A, "Description": "for a pipe that is not there"}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (404, "NotFoundException"),
            "case {name:?}: UpdatePipe on an unknown name must be 404 NotFoundException, \
             got {refused:?}"
        );
        let described = describe(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: the refused update must not have created a pipe, \
             got {described:?}"
        );
        let listed = listed_names(&list(&router, "").await);
        assert_eq!(
            listed,
            vec!["stored".to_string()],
            "case {name:?}: ListPipes must still report only the fixture pipe, got {listed:?}"
        );
    }
}

/// RULE: an update outside what the service accepts is refused — `ValidationException`
/// (400) — and the stored record is left as it was.
///
/// The rows differ along which rule the request breaks: a required member, a modelled
/// length, a modelled pattern, a modelled enum, and the one rule the model cannot state
/// (a `SourceParameters` branch whose create-only members the pipe holds nothing for).
#[tokio::test]
async fn update_pipe_refuses_a_request_outside_what_the_service_accepts() {
    let router = router();
    let created = create(
        &router,
        "guarded",
        create_body(
            SOURCE_SQS,
            TARGET_A,
            json!({
                "Description": "kept",
                "SourceParameters": {"SqsQueueParameters": {"BatchSize": 4}},
            }),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture pipe must be created, got {created:?}"
    );
    tokio::time::sleep(PAST_SETTLE).await;

    let too_long = "d".repeat(513);
    let rows: Vec<(&str, Value)> = vec![
        ("RoleArn missing", json!({"Description": "no role"})),
        (
            "RoleArn outside its pattern",
            json!({"RoleArn": "not-an-arn"}),
        ),
        (
            "Description longer than PipeDescription allows",
            json!({"RoleArn": ROLE_A, "Description": too_long}),
        ),
        (
            "DesiredState outside RequestedPipeState",
            json!({"RoleArn": ROLE_A, "DesiredState": "PAUSED"}),
        ),
        (
            "Target shorter than Arn allows",
            json!({"RoleArn": ROLE_A, "Target": ""}),
        ),
        (
            // The pipe holds no DynamoDB stream parameters, so the members only
            // `CreatePipe` can state (the starting position) have nowhere to come from.
            "SourceParameters branch the pipe has none of",
            json!({
                "RoleArn": ROLE_A,
                "SourceParameters": {"DynamoDBStreamParameters": {"BatchSize": 2}},
            }),
        ),
    ];

    for (case, body) in rows {
        let refused = update(&router, "guarded", body).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
    }

    let described = describe(&router, "guarded").await;
    assert_eq!(
        described.string("Description"),
        "kept",
        "a refused update must not have changed the record, got {described:?}"
    );
    assert_eq!(
        described.string("RoleArn"),
        ROLE_A,
        "a refused update must not have changed the record, got {described:?}"
    );
    assert_eq!(
        described.member("SourceParameters"),
        &json!({"SqsQueueParameters": {"BatchSize": 4}}),
        "a refused update must not have changed the record, got {described:?}"
    );
}

/// RULE: updates racing on one pipe are serialised — at least one is accepted, every
/// refusal is the modelled conflict, and the stored record ends as ONE writer's whole
/// update rather than a mix of several.
///
/// The rows differ along the number of writers racing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_leave_one_writers_update_whole() {
    for writers in [2usize, 3, 8] {
        let router = Arc::new(router());
        let name = format!("raced-{writers}");
        let created = create(&router, &name, minimal_body(SOURCE_SQS, TARGET_A)).await;
        assert_eq!(
            created.status, 200,
            "case {writers}: CreatePipe must succeed, got {created:?}"
        );
        // Out of CREATING, so the race is between the updates and nothing else.
        tokio::time::sleep(PAST_SETTLE).await;

        let mut tasks = Vec::new();
        for writer in 0..writers {
            let router = Arc::clone(&router);
            let name = name.clone();
            tasks.push(tokio::spawn(async move {
                // Three members moved together: a record carrying one writer's value for
                // one of them and another writer's for a second would be corrupt.
                let body = json!({
                    "Description": format!("writer-{writer}"),
                    "Target": format!("arn:aws:sqs:us-west-2:000000000000:results-{writer}"),
                    "RoleArn": format!("arn:aws:iam::000000000000:role/writer-{writer}"),
                });
                update(&router, &name, body).await
            }));
        }

        let mut accepted = 0usize;
        for task in tasks {
            let reply = task.await.expect("the update task must not panic");
            if reply.status == 200 {
                accepted += 1;
            } else {
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (409, "ConflictException"),
                    "case {writers}: a racing update is either accepted or the modelled \
                     conflict, got {reply:?}"
                );
            }
        }
        assert!(
            accepted >= 1,
            "case {writers}: at least one racing UpdatePipe must be accepted, \
             got {accepted} of {writers}"
        );

        tokio::time::sleep(PAST_SETTLE).await;
        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {writers}: DescribePipe must still find the pipe, got {described:?}"
        );
        let landed = described.string("Description");
        let writer = landed
            .strip_prefix("writer-")
            .and_then(|index| index.parse::<usize>().ok())
            .unwrap_or_else(|| {
                panic!(
                    "case {writers}: the stored Description must be one writer's, \
                     got {described:?}"
                )
            });
        assert!(
            writer < writers,
            "case {writers}: the stored Description names writer {writer}, got {described:?}"
        );
        assert_eq!(
            described.string("Target"),
            format!("arn:aws:sqs:us-west-2:000000000000:results-{writer}"),
            "case {writers}: Target must come from the same writer as Description \
             ({writer}), got {described:?}"
        );
        assert_eq!(
            described.string("RoleArn"),
            format!("arn:aws:iam::000000000000:role/writer-{writer}"),
            "case {writers}: RoleArn must come from the same writer as Description \
             ({writer}), got {described:?}"
        );
    }
}
