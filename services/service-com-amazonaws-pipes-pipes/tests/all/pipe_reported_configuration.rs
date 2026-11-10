// SPDX-License-Identifier: CC0-1.0

//! What a read of a pipe REPORTS about its configuration, end to end over the wire.
//!
//! Two rules, both read back through `DescribePipe` (and `DeletePipe` for the last one),
//! every assertion going through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — reusing the harness `pipe_lifecycle` and `pipe_update`
//! own rather than copying it.
//!
//! 1. A read reports the pipe's CONFIGURATION, not a record of which members its create
//!    request happened to mention: the configuration containers the model defines —
//!    `SourceParameters`, `EnrichmentParameters`, `TargetParameters` — and `Tags` are
//!    always present, empty where the pipe carries nothing under them, and the source's
//!    parameters carry the `BatchSize` the pipe polls its source with: the configured one,
//!    or the default for that kind of source where the configuration names none.
//! 2. `DeletePipe` answers with the RUN state the pipe is wound down to, `STOPPED`, while
//!    `CurrentState` moves to `DELETING`.
//!
//! The tables span what each rule generalises over: which containers the request carries
//! and which it leaves out, the source KIND (an Amazon SQS queue, a DynamoDB stream, a
//! Kinesis stream — the kinds `PipeSourceParameters` has a member for), whether the
//! source's parameters name a batch size and where in its `@range` that size sits, the
//! sibling members the container carries beside it, and the lifecycle calls the pipe has
//! been through (none, a stop, an update).

use serde_json::{json, Value};

use crate::pipe_lifecycle::{call, create, describe, minimal_body, router, Reply, PAST_SETTLE, REGION};
use crate::pipe_update::update;

const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";
const SOURCE_SQS: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const SOURCE_DDB: &str =
    "arn:aws:dynamodb:us-west-2:000000000000:table/orders/stream/2026-01-01T00:00:00.000";
const SOURCE_KINESIS: &str = "arn:aws:kinesis:us-west-2:000000000000:stream/events";
/// A Lambda function target: delivery is ONE call per batch, so no per-call entry limit
/// caps the batch size a row configures (`state::target_batch_size_cap`).
const TARGET_LAMBDA: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// `BatchSize`'s published bounds, from `PipeSourceSqsQueueParameters` in
/// `smithy-model/com-amazonaws-pipes-pipes.json` (`@range(min: 1, max: 10000)` on the
/// `LimitMax10000` shape it targets).
const BATCH_SIZE_MIN: i64 = 1;
const BATCH_SIZE_MAX: i64 = 10_000;

/// The `BatchSize` a source of each kind polls with when its parameters name none, as this
/// service's records report it: `state::DEFAULT_SQS_BATCH_SIZE` for an Amazon SQS queue and
/// `state::DEFAULT_STREAM_BATCH_SIZE` for a stream.
const DEFAULT_SQS_BATCH_SIZE: i64 =
    service_com_amazonaws_pipes_pipes::state::DEFAULT_SQS_BATCH_SIZE as i64;
const DEFAULT_STREAM_BATCH_SIZE: i64 =
    service_com_amazonaws_pipes_pipes::state::DEFAULT_STREAM_BATCH_SIZE as i64;

/// One row of the reported-configuration table.
struct Row {
    case: &'static str,
    /// The `Source` the pipe is created with.
    source: &'static str,
    /// The create members beyond `Source`/`Target`/`RoleArn`, as the request carries them.
    configured: Value,
    /// What `DescribePipe` must report for each of the four containers.
    source_parameters: Value,
    enrichment_parameters: Value,
    target_parameters: Value,
    tags: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The pipe the issue cited: nothing configured beyond the three required
            // members, so every container is reported empty — except the source's, which
            // carries the batch size an Amazon SQS source polls with.
            case: "sqs-source-nothing-configured",
            source: SOURCE_SQS,
            configured: json!({}),
            source_parameters: json!({"SqsQueueParameters": {"BatchSize": DEFAULT_SQS_BATCH_SIZE}}),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            case: "sqs-source-batch-size-at-its-minimum",
            source: SOURCE_SQS,
            configured: json!({
                "SourceParameters": {"SqsQueueParameters": {"BatchSize": BATCH_SIZE_MIN}},
            }),
            source_parameters: json!({"SqsQueueParameters": {"BatchSize": BATCH_SIZE_MIN}}),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            case: "sqs-source-batch-size-at-its-maximum",
            source: SOURCE_SQS,
            configured: json!({
                "SourceParameters": {"SqsQueueParameters": {"BatchSize": BATCH_SIZE_MAX}},
            }),
            source_parameters: json!({"SqsQueueParameters": {"BatchSize": BATCH_SIZE_MAX}}),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            // A sibling member of the source's own parameters, with no batch size beside
            // it: the sibling is kept and the batch size in force is reported.
            case: "sqs-source-batching-window-without-a-batch-size",
            source: SOURCE_SQS,
            configured: json!({
                "SourceParameters": {
                    "SqsQueueParameters": {"MaximumBatchingWindowInSeconds": 30},
                },
            }),
            source_parameters: json!({
                "SqsQueueParameters": {
                    "BatchSize": DEFAULT_SQS_BATCH_SIZE,
                    "MaximumBatchingWindowInSeconds": 30,
                },
            }),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            // `FilterCriteria` belongs to no source type, so a configuration carrying only
            // it still reports the source type's own parameters beside it.
            case: "sqs-source-filter-criteria-only",
            source: SOURCE_SQS,
            configured: json!({
                "SourceParameters": {
                    "FilterCriteria": {"Filters": [{"Pattern": "{\"body\":[{\"prefix\":\"a\"}]}"}]},
                },
            }),
            source_parameters: json!({
                "FilterCriteria": {"Filters": [{"Pattern": "{\"body\":[{\"prefix\":\"a\"}]}"}]},
                "SqsQueueParameters": {"BatchSize": DEFAULT_SQS_BATCH_SIZE},
            }),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            // Every container configured: each is reported as written, and the source's
            // keeps the batch size the request named.
            case: "sqs-source-everything-configured",
            source: SOURCE_SQS,
            configured: json!({
                "SourceParameters": {"SqsQueueParameters": {"BatchSize": 4}},
                "EnrichmentParameters": {"InputTemplate": "$.body"},
                "TargetParameters": {"InputTemplate": "<$.body>"},
                "Tags": {"env": "test"},
            }),
            source_parameters: json!({"SqsQueueParameters": {"BatchSize": 4}}),
            enrichment_parameters: json!({"InputTemplate": "$.body"}),
            target_parameters: json!({"InputTemplate": "<$.body>"}),
            tags: json!({"env": "test"}),
        },
        Row {
            // A stream source: its own parameters are required for the pipe to be legal at
            // all, and the batch size a stream source polls with is the stream default.
            case: "dynamodb-stream-source-without-a-batch-size",
            source: SOURCE_DDB,
            configured: json!({
                "SourceParameters": {"DynamoDBStreamParameters": {"StartingPosition": "LATEST"}},
            }),
            source_parameters: json!({
                "DynamoDBStreamParameters": {
                    "StartingPosition": "LATEST",
                    "BatchSize": DEFAULT_STREAM_BATCH_SIZE,
                },
            }),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            case: "dynamodb-stream-source-with-its-own-batch-size",
            source: SOURCE_DDB,
            configured: json!({
                "SourceParameters": {
                    "DynamoDBStreamParameters": {
                        "StartingPosition": "TRIM_HORIZON",
                        "BatchSize": 5,
                    },
                },
            }),
            source_parameters: json!({
                "DynamoDBStreamParameters": {"StartingPosition": "TRIM_HORIZON", "BatchSize": 5},
            }),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
        Row {
            // A source kind this emulator polls no source of: there is no batch size in
            // force to report, so the stored configuration is reported exactly as written.
            case: "kinesis-stream-source-is-reported-as-written",
            source: SOURCE_KINESIS,
            configured: json!({
                "SourceParameters": {"KinesisStreamParameters": {"StartingPosition": "LATEST"}},
            }),
            source_parameters: json!({
                "KinesisStreamParameters": {"StartingPosition": "LATEST"},
            }),
            enrichment_parameters: json!({}),
            target_parameters: json!({}),
            tags: json!({}),
        },
    ]
}

/// The `CreatePipe` body of one row: the three required members, the members the row
/// configures, and `DesiredState: STOPPED`, because these rows are about what a pipe
/// reports and not about what a running one does.
fn create_body(row: &Row) -> Value {
    let mut body = minimal_body(row.source, TARGET_LAMBDA);
    body["RoleArn"] = json!(ROLE);
    body["DesiredState"] = json!("STOPPED");
    for (member, value) in row
        .configured
        .as_object()
        .expect("the row's configured members are an object")
    {
        body[member] = value.clone();
    }
    body
}

/// Assert the four containers of one reply against a row.
fn assert_containers(described: &Reply, case: &str, row: &Row, after: &str) {
    for (member, expected) in [
        ("SourceParameters", &row.source_parameters),
        ("EnrichmentParameters", &row.enrichment_parameters),
        ("TargetParameters", &row.target_parameters),
        ("Tags", &row.tags),
    ] {
        assert_eq!(
            described.member(member),
            expected,
            "case {case:?}{after}: DescribePipe must report {member} as {expected}, got {:?}",
            described.member(member)
        );
    }
}

/// RULE: `DescribePipe` reports the pipe's configuration — every container present, empty
/// where the pipe carries nothing under it, and the source's parameters carrying the
/// `BatchSize` the pipe polls its source with.
#[tokio::test]
async fn describe_pipe_reports_every_configuration_container() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = router();
        let name = format!("reported{index}");
        let created = create(&router, &name, create_body(&row)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe must succeed, got {created:?}",
            row.case
        );

        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribePipe must find the pipe just created, got {described:?}",
            row.case
        );
        assert_containers(&described, row.case, &row, "");
    }
}

/// RULE: the same holds once the pipe has been through a lifecycle call and an update —
/// its configuration is not a side effect of the last call it answered.
#[tokio::test]
async fn a_lifecycle_call_and_an_update_leave_every_container_reported() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = router();
        let name = format!("lifecycle{index}");
        let created = create(&router, &name, create_body(&row)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe must succeed, got {created:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;

        // A stop, which moves the pipe's state and touches no configuration.
        let stopped = call(
            &router,
            "POST",
            &format!("/v1/pipes/{name}/stop"),
            None,
            REGION,
        )
        .await;
        assert_eq!(
            stopped.status, 200,
            "case {:?}: StopPipe must succeed, got {stopped:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;
        assert_containers(&describe(&router, &name).await, row.case, &row, " after a stop");

        // An update that carries only the member it must (`RoleArn`) replaces no
        // container, so every one of them still reads back.
        let updated = update(&router, &name, json!({"RoleArn": ROLE})).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: UpdatePipe must succeed, got {updated:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;
        assert_containers(
            &describe(&router, &name).await,
            row.case,
            &row,
            " after an update",
        );
    }
}

/// RULE: a container an `UpdatePipe` DOES carry replaces the stored one, and the read
/// reports the replacement with the batch size in force — the update path writes the same
/// configuration a create does.
#[tokio::test]
async fn an_update_that_replaces_a_container_is_reported_with_the_batch_size_in_force() {
    // (the case, the SourceParameters the update carries, what the read must report)
    let cases = [
        (
            "update-drops-the-batch-size",
            json!({"SqsQueueParameters": {"MaximumBatchingWindowInSeconds": 5}}),
            json!({
                "SqsQueueParameters": {
                    "BatchSize": DEFAULT_SQS_BATCH_SIZE,
                    "MaximumBatchingWindowInSeconds": 5,
                },
            }),
        ),
        (
            "update-names-a-batch-size",
            json!({"SqsQueueParameters": {"BatchSize": 7}}),
            json!({"SqsQueueParameters": {"BatchSize": 7}}),
        ),
        (
            "update-carries-only-filter-criteria",
            json!({"FilterCriteria": {"Filters": []}}),
            json!({
                "FilterCriteria": {"Filters": []},
                "SqsQueueParameters": {"BatchSize": DEFAULT_SQS_BATCH_SIZE},
            }),
        ),
    ];

    for (index, (case, sent, expected)) in cases.into_iter().enumerate() {
        let router = router();
        let name = format!("updated{index}");
        let mut body = minimal_body(SOURCE_SQS, TARGET_LAMBDA);
        body["RoleArn"] = json!(ROLE);
        body["DesiredState"] = json!("STOPPED");
        body["SourceParameters"] = json!({"SqsQueueParameters": {"BatchSize": 3}});
        let created = create(&router, &name, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );
        tokio::time::sleep(PAST_SETTLE).await;

        let updated = update(
            &router,
            &name,
            json!({"RoleArn": ROLE, "SourceParameters": sent}),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdatePipe must succeed, got {updated:?}"
        );
        tokio::time::sleep(PAST_SETTLE).await;

        let described = describe(&router, &name).await;
        assert_eq!(
            described.member("SourceParameters"),
            &expected,
            "case {case:?}: DescribePipe must report SourceParameters as {expected}, got {:?}",
            described.member("SourceParameters")
        );
    }
}

/// RULE: `DeletePipe` answers with the run state the pipe is wound down to — `STOPPED` —
/// while `CurrentState` moves to `DELETING`, whatever run state the pipe was in.
#[tokio::test]
async fn delete_pipe_reports_the_run_state_the_pipe_is_wound_down_to() {
    // The run states a pipe can be in when it is deleted, from `RequestedPipeState` in
    // `smithy-model/com-amazonaws-pipes-pipes.json`, plus the pipe still mid-move.
    for (case, desired, settle) in [
        ("running-pipe", "STOPPED", true),
        ("stopped-pipe", "STOPPED", true),
        ("pipe-still-creating", "RUNNING", false),
    ] {
        let router = router();
        let name = "doomed";
        let mut body = minimal_body(SOURCE_SQS, TARGET_LAMBDA);
        body["RoleArn"] = json!(ROLE);
        body["DesiredState"] = json!(desired);
        let created = create(&router, name, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );
        if settle {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        let deleted = call(&router, "DELETE", &format!("/v1/pipes/{name}"), None, REGION).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeletePipe must succeed, got {deleted:?}"
        );
        assert_eq!(
            deleted.string("DesiredState"),
            "STOPPED",
            "case {case:?}: DeletePipe must report DesiredState STOPPED, got {deleted:?}"
        );
        assert_eq!(
            deleted.string("CurrentState"),
            "DELETING",
            "case {case:?}: DeletePipe must report CurrentState DELETING, got {deleted:?}"
        );
    }
}
