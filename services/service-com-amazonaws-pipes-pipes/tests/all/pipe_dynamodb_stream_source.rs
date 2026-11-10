// SPDX-License-Identifier: CC0-1.0

//! A pipe whose SOURCE is a DynamoDB stream, end to end over the wire.
//!
//! Pipes, DynamoDB, DynamoDB Streams, Amazon SQS and Step Functions are mounted on the
//! SAME engine and every call goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — because a change-data-capture topology is asserted where
//! a client sees it: the change is applied with `PutItem`/`DeleteItem` and the record
//! arrives at the target queue, with nothing in between asked to do anything.
//!
//! The rules asserted, each over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. The `SourceParameters` member a pipe needs is the one for the KIND of resource its
//!    `Source` names: a source type whose parameters shape declares a required member has
//!    no legal configuration without that member, so `CreatePipe` refuses one that omits
//!    it (`ValidationException`, HTTP 400, naming `SourceParameters.<Member>`) and stores
//!    nothing, while a source type whose shape requires nothing is accepted with no
//!    parameters at all. A source type configured by either of two members needs one of
//!    them, and a request carrying neither is refused naming both. The rows span every
//!    source type the model has parameters for.
//! 2. A RUNNING stream-sourced pipe reads its stream with no client action and delivers
//!    every change to its target, preserving the order the changes to one partition key
//!    were applied in. The rows differ along the starting position, whether the changes
//!    were applied before or after the pipe existed, and the batch size and batching
//!    window they are polled under.
//! 3. A pipe carrying `FilterCriteria` admits only the records its patterns match, and it
//!    does so BEFORE the enrichment: an enrichment that hands back what it was given
//!    delivers exactly the admitted records. The rows differ along the pattern — the
//!    change's own kind, a negation of it, and a value inside the record's new image.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{given_express_workflow, rpc, LOOK_AGAIN, QUIET};
use crate::pipe_lifecycle::{call, describe, list, listed_names, Reply, PAST_SETTLE, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies: the
/// engine carries no IAM store, which shows nothing about what the role may do.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-dynamodb-stream";

/// How long a wait for work the pipe has to do is given before it is a failure. Well
/// above the data plane's poll interval and the lifecycle settle span, so a row fails only
/// when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(25);

/// The partition key every row writes its changes to: one key, so the order the changes
/// reach the target in is the order they were applied in.
const KEY: &str = "order-1";

/// The attribute each change moves on, so a delivered record says WHICH change it is.
const VERSION: &str = "version";

/// What an enrichment that hands back exactly what it was given looks like: a `Pass`
/// state with neither `Parameters` nor `Result` answers its input, so what the target
/// receives is what the enrichment was handed.
const ECHO_DEFINITION: &str = r#"{
  "StartAt": "Echo",
  "States": {
    "Echo": {
      "Type": "Pass",
      "End": true
    }
  }
}"#;

/// Pipes and the services a DynamoDB-stream-sourced pipe reaches, on one engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One `awsJson` RPC call against DynamoDB.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "dynamodb", "DynamoDB_20120810", operation, body).await
}

/// One `awsJson` RPC call against Amazon SQS.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS", operation, body).await
}

/// A table called `name` with a stream of `view_type` turned on, waited out until it is
/// ACTIVE; answers the stream's ARN.
async fn given_streamed_table(router: &MultiServiceRouter, name: &str, view_type: &str) -> String {
    let (status, body) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateTable answered {status} {body}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, described) =
            dynamodb(router, "DescribeTable", &json!({ "TableName": name })).await;
        assert_eq!(
            status, 200,
            "case {name}: DescribeTable answered {status} {described}"
        );
        let active = described["Table"]["TableStatus"].as_str() == Some("ACTIVE");
        let stream = described["Table"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if active && !stream.is_empty() {
            return stream;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: expected the table to reach ACTIVE with a stream, got {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A FIFO queue called `<name>.fifo`; answers its url and the ARN a pipe names it by.
///
/// FIFO, because the order the pipe delivered the records in is what these rows assert:
/// a standard queue makes no promise about the order a receive hands messages back in,
/// so the queue itself has to keep it.
async fn given_fifo_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = sqs(
        router,
        "CreateQueue",
        &json!({
            "QueueName": format!("{name}.fifo"),
            "Attributes": {"FifoQueue": "true"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// The target parameters every row below delivers under: a FIFO queue needs the group its
/// sends are ordered by, and a deduplication token per message — the record's own
/// `eventID`, which a stream record carries one of per change.
fn fifo_target_parameters() -> Value {
    json!({
        "SqsQueueParameters": {
            "MessageGroupId": "changes",
            "MessageDeduplicationId": "$.eventID",
        },
    })
}

/// `PutItem` of the key at `version`, which is an INSERT the first time and a MODIFY
/// afterwards.
async fn put_version(router: &MultiServiceRouter, table: &str, version: usize) {
    let (status, body) = dynamodb(
        router,
        "PutItem",
        &json!({
            "TableName": table,
            "Item": {"pk": {"S": KEY}, VERSION: {"N": version.to_string()}},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {table}: PutItem {version} answered {status} {body}"
    );
}

/// `DeleteItem` of the key, which is a REMOVE.
async fn delete_key(router: &MultiServiceRouter, table: &str) {
    let (status, body) = dynamodb(
        router,
        "DeleteItem",
        &json!({ "TableName": table, "Key": {"pk": {"S": KEY}} }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {table}: DeleteItem answered {status} {body}"
    );
}

/// Apply `modifications` changes to the one key and then delete it: one INSERT, then
/// `modifications - 1` MODIFYs, then a REMOVE.
async fn apply_changes(router: &MultiServiceRouter, table: &str, modifications: usize) {
    for version in 1..=modifications {
        put_version(router, table, version).await;
    }
    delete_key(router, table).await;
}

/// The change kinds `apply_changes(_, _, modifications)` makes, in the order it makes
/// them.
fn applied_kinds(modifications: usize) -> Vec<String> {
    let mut kinds = vec!["INSERT".to_string()];
    kinds.extend((1..modifications).map(|_| "MODIFY".to_string()));
    kinds.push("REMOVE".to_string());
    kinds
}

/// `CreatePipe` for a stream-sourced pipe, answered as the lifecycle harness reads a
/// reply.
async fn create_pipe(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// Receive up to ten messages from the FIFO queue at `url` and DELETE each one, so the
/// next call hands over what is behind them.
async fn drain(router: &MultiServiceRouter, url: &str, case: &str) -> Vec<Value> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    let mut taken: Vec<Value> = Vec::new();
    for message in body["Messages"].as_array().cloned().unwrap_or_default() {
        let text = message["Body"].as_str().unwrap_or_default().to_string();
        let record: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!("case {case:?}: a delivered body is not a stream record ({error}): {text:?}")
        });
        taken.push(record);
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            let (status, answer) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteMessage answered {status} {answer}"
            );
        }
    }
    taken
}

/// Every record delivered to `url` until there are `want` of them or the budget lapses.
async fn delivered(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    want: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    let mut collected: Vec<Value> = Vec::new();
    loop {
        collected.extend(drain(router, url, case).await);
        if collected.len() >= want || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row of the `SourceParameters`-requirement table.
struct Required {
    case: &'static str,
    /// The `Source` the pipe names.
    source: &'static str,
    /// The `SourceParameters` the request carries, `Null` for a request that carries
    /// none.
    parameters: Value,
    /// The members the refusal must name, empty for a request that must be accepted.
    missing: &'static [&'static str],
}

/// RULE: the `SourceParameters` member a pipe needs is the one for the KIND of resource
/// its `Source` names — a source type whose own parameters shape declares a required
/// member (a stream's `StartingPosition`, a Kafka `TopicName`, a broker's `QueueName`) has
/// no legal configuration that omits the member, so `CreatePipe` refuses one that does
/// with the modelled `ValidationException` (HTTP 400) naming `SourceParameters.<Member>`
/// and stores nothing, while a source type whose shape requires nothing is accepted with
/// no parameters at all.
///
/// The rows span what the rule generalises over: every source type `PipeSourceParameters`
/// has a member for (the queue, both stream types, a broker, both Kafka forms — the member
/// names and which of them carry a required member are from the `PipeSource*Parameters`
/// shapes of the model), whether the request carries no `SourceParameters` at all or ones
/// for another source type, the accepted leg of each refused one, and the one source type
/// two members configure, where either satisfies the rule and neither is refused naming
/// both.
#[tokio::test]
async fn source_parameters_the_source_kind_requires_cannot_be_omitted() {
    let rows = vec![
        Required {
            case: "dynamodb-stream-without-parameters",
            source: "arn:aws:dynamodb:us-west-2:000000000000:table/orders/stream/2026-01-01T00:00:00.000",
            parameters: Value::Null,
            missing: &["DynamoDBStreamParameters"],
        },
        Required {
            case: "dynamodb-stream-given-another-source-type",
            source: "arn:aws:dynamodb:us-west-2:000000000000:table/orders/stream/2026-01-01T00:00:00.000",
            parameters: json!({"SqsQueueParameters": {"BatchSize": 1}}),
            missing: &["DynamoDBStreamParameters"],
        },
        Required {
            case: "dynamodb-stream-with-its-starting-position",
            source: "arn:aws:dynamodb:us-west-2:000000000000:table/orders/stream/2026-01-01T00:00:00.000",
            parameters: json!({"DynamoDBStreamParameters": {"StartingPosition": "LATEST"}}),
            missing: &[],
        },
        Required {
            case: "kinesis-stream-without-parameters",
            source: "arn:aws:kinesis:us-west-2:000000000000:stream/events",
            parameters: Value::Null,
            missing: &["KinesisStreamParameters"],
        },
        Required {
            case: "kinesis-stream-with-its-starting-position",
            source: "arn:aws:kinesis:us-west-2:000000000000:stream/events",
            parameters: json!({"KinesisStreamParameters": {"StartingPosition": "TRIM_HORIZON"}}),
            missing: &[],
        },
        Required {
            case: "managed-kafka-without-parameters",
            source: "arn:aws:kafka:us-west-2:000000000000:cluster/demo/abcd-1",
            parameters: Value::Null,
            missing: &["ManagedStreamingKafkaParameters"],
        },
        Required {
            case: "managed-kafka-with-its-topic",
            source: "arn:aws:kafka:us-west-2:000000000000:cluster/demo/abcd-1",
            parameters: json!({"ManagedStreamingKafkaParameters": {"TopicName": "orders"}}),
            missing: &[],
        },
        Required {
            case: "self-managed-kafka-without-parameters",
            source: "smk://b-1.broker.kafka.us-west-2.amazonaws.com:9092",
            parameters: Value::Null,
            missing: &["SelfManagedKafkaParameters"],
        },
        Required {
            case: "self-managed-kafka-with-its-topic",
            source: "smk://b-1.broker.kafka.us-west-2.amazonaws.com:9092",
            parameters: json!({"SelfManagedKafkaParameters": {"TopicName": "orders"}}),
            missing: &[],
        },
        Required {
            // `PipeSourceSqsQueueParameters` declares no required member, so a queue
            // source is configured by nothing at all.
            case: "queue-without-parameters",
            source: "arn:aws:sqs:us-west-2:000000000000:orders",
            parameters: Value::Null,
            missing: &[],
        },
        Required {
            // An Amazon MQ broker's ARN names the broker without saying which engine it
            // runs, so EITHER broker member makes the configuration legal — and carrying
            // neither is refused naming both candidates.
            case: "broker-without-parameters",
            source: "arn:aws:mq:us-west-2:000000000000:broker:orders:b-1234",
            parameters: Value::Null,
            missing: &["ActiveMQBrokerParameters", "RabbitMQBrokerParameters"],
        },
        Required {
            case: "broker-with-one-of-its-engines",
            source: "arn:aws:mq:us-west-2:000000000000:broker:orders:b-1234",
            parameters: json!({
                "RabbitMQBrokerParameters": {
                    "Credentials": {
                        "BasicAuth":
                            "arn:aws:secretsmanager:us-west-2:000000000000:secret:mq-abc",
                    },
                    "QueueName": "orders",
                },
            }),
            missing: &[],
        },
    ];

    let router = emulator();
    for row in rows {
        let mut body = json!({
            "Source": row.source,
            "Target": "arn:aws:sqs:us-west-2:000000000000:results",
            "RoleArn": ROLE,
            // STOPPED, because this table is about what the configuration may be and not
            // about what a running pipe does.
            "DesiredState": "STOPPED",
        });
        if !row.parameters.is_null() {
            body["SourceParameters"] = row.parameters.clone();
        }
        let created = create_pipe(&router, row.case, body).await;

        if row.missing.is_empty() {
            assert_eq!(
                created.status, 200,
                "case {:?}: this configuration must be accepted, got {created:?}",
                row.case
            );
            let described = describe(&router, row.case).await;
            assert_eq!(
                described.string("Source"),
                row.source,
                "case {:?}: the accepted pipe must read its Source back, got {described:?}",
                row.case
            );
            // What the read reports is the pipe's configuration: the members the request
            // carried, plus the `BatchSize` the pipe polls its source with where the
            // request named none. That rule and its table live in
            // `tests/all/pipe_reported_configuration.rs`; here it is enough that every
            // member the create stored still reads back.
            let reported = described.member("SourceParameters").clone();
            for (member, expected) in row
                .parameters
                .as_object()
                .into_iter()
                .flat_map(|members| members.iter())
            {
                let actual = reported.get(member).unwrap_or(&Value::Null);
                for (field, value) in expected
                    .as_object()
                    .into_iter()
                    .flat_map(|fields| fields.iter())
                {
                    assert_eq!(
                        actual.get(field),
                        Some(value),
                        "case {:?}: the accepted pipe must read {member}.{field} back as \
                         {value}, got {actual:?}",
                        row.case
                    );
                }
            }
            continue;
        }

        assert_eq!(
            (created.status, created.error_code.as_str()),
            (400, "ValidationException"),
            "case {:?}: expected 400 ValidationException, got {created:?}",
            row.case
        );
        let entries: Vec<String> = row
            .missing
            .iter()
            .map(|member| {
                format!(
                    r#"{{"Name":"SourceParameters.{member}","Message":"Missing required parameter."}}"#
                )
            })
            .collect();
        assert_eq!(
            created.message(),
            format!(
                r#"{{"ValidationErrorFields":[{}]}}"#,
                entries.join(",")
            ),
            "case {:?}: expected the ValidationErrorFields document for {:?}, got {:?}",
            row.case,
            row.missing,
            created.message()
        );
        assert_eq!(
            created.body["fieldList"],
            Value::Array(
                row.missing
                    .iter()
                    .map(|member| json!({
                        "name": format!("SourceParameters.{member}"),
                        "message": "Missing required parameter.",
                    }))
                    .collect()
            ),
            "case {:?}: expected fieldList to name {:?}, got {:?}",
            row.case,
            row.missing,
            created.body["fieldList"]
        );
        // The refusal has no effect: the pipe was never created.
        let described = describe(&router, row.case).await;
        assert_eq!(
            (described.status, described.error_code.as_str()),
            (404, "NotFoundException"),
            "case {:?}: a refused CreatePipe must have stored no pipe, got {described:?}",
            row.case
        );
        assert_eq!(
            described.message(),
            format!("Pipe {} does not exist.", row.case),
            "case {:?}: expected the absent-pipe sentence, got {:?}",
            row.case,
            described.message()
        );
        let listed = list(&router, "").await;
        assert!(
            !listed_names(&listed).contains(&row.case.to_string()),
            "case {:?}: a refused CreatePipe must leave the name unlisted, got {:?}",
            row.case,
            listed_names(&listed)
        );
    }
}

/// One row of the stream-delivery table.
struct Delivery {
    case: &'static str,
    /// `StartingPosition`: where the pipe starts reading the stream.
    starting_position: &'static str,
    /// Whether the changes are applied BEFORE the pipe exists, so what it reads is a
    /// backlog rather than a live change.
    applied_first: bool,
    /// `BatchSize`: how many records one poll takes.
    batch_size: usize,
    /// `MaximumBatchingWindowInSeconds`: how long a poll that has not filled its batch
    /// keeps collecting, so a batch can span several reads.
    window: usize,
    /// How many `PutItem`s the row makes before it deletes the key.
    modifications: usize,
}

/// RULE: a RUNNING pipe whose source is a DynamoDB stream reads the stream itself, with no
/// client action, and delivers every change to its target in the order the changes to one
/// partition key were applied — each as the stream record envelope, carrying the change's
/// kind, its own event id and the images the stream's view type captures.
///
/// The rows differ along what the rule generalises over: the starting position the pipe
/// reads from, whether the changes were applied before the pipe existed or while it was
/// running, the batch size and the batching window they are polled under (a window makes a
/// batch span several reads), and how many changes there are.
#[tokio::test]
async fn a_running_stream_sourced_pipe_delivers_every_change_in_applied_order() {
    let rows = vec![
        Delivery {
            case: "trim-horizon-backlog",
            starting_position: "TRIM_HORIZON",
            applied_first: true,
            batch_size: 1,
            window: 0,
            modifications: 5,
        },
        Delivery {
            case: "trim-horizon-live",
            starting_position: "TRIM_HORIZON",
            applied_first: false,
            batch_size: 10,
            window: 0,
            modifications: 3,
        },
        Delivery {
            case: "latest-live",
            starting_position: "LATEST",
            applied_first: false,
            batch_size: 2,
            window: 0,
            modifications: 2,
        },
        Delivery {
            // A batching window with more changes than one read can answer: the batch is
            // collected over several reads, and every change still arrives exactly once
            // and in order.
            case: "batched-over-a-window",
            starting_position: "TRIM_HORIZON",
            applied_first: false,
            batch_size: 3,
            window: 2,
            modifications: 7,
        },
    ];

    let router = emulator();
    for row in rows {
        let stream = given_streamed_table(&router, row.case, "NEW_AND_OLD_IMAGES").await;
        let (url, target) = given_fifo_queue(&router, row.case).await;
        if row.applied_first {
            apply_changes(&router, row.case, row.modifications).await;
        }
        let created = create_pipe(
            &router,
            row.case,
            json!({
                "Source": stream,
                "Target": target,
                "RoleArn": ROLE,
                "DesiredState": "RUNNING",
                "SourceParameters": {
                    "DynamoDBStreamParameters": {
                        "StartingPosition": row.starting_position,
                        "BatchSize": row.batch_size,
                        "MaximumBatchingWindowInSeconds": row.window,
                    },
                },
                "TargetParameters": fifo_target_parameters(),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe must succeed, got {created:?}",
            row.case
        );
        if !row.applied_first {
            // Past the settle span the pipe rests in RUNNING, so the changes below are
            // applied to a pipe that is already reading its stream.
            tokio::time::sleep(PAST_SETTLE).await;
            apply_changes(&router, row.case, row.modifications).await;
        }

        let wanted = row.modifications + 1;
        let records = delivered(&router, &url, row.case, wanted).await;
        assert_eq!(
            records.len(),
            wanted,
            "case {:?}: expected {wanted} delivered record(s), got {records:?}",
            row.case
        );

        let kinds: Vec<String> = records
            .iter()
            .map(|record| record["eventName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            kinds,
            applied_kinds(row.modifications),
            "case {:?}: expected the changes in applied order, got {kinds:?}",
            row.case
        );

        // The versions the modifications moved through, in order: the new image of each
        // write, and the OLD image of the delete — which is the last version written.
        let versions: Vec<String> = records
            .iter()
            .map(|record| {
                let image = if record["eventName"] == json!("REMOVE") {
                    &record["dynamodb"]["OldImage"]
                } else {
                    &record["dynamodb"]["NewImage"]
                };
                image[VERSION]["N"].as_str().unwrap_or_default().to_string()
            })
            .collect();
        let expected: Vec<String> = (1..=row.modifications)
            .chain(std::iter::once(row.modifications))
            .map(|version| version.to_string())
            .collect();
        assert_eq!(
            versions, expected,
            "case {:?}: expected the versions {expected:?}, got {versions:?}",
            row.case
        );

        let ids: BTreeSet<String> = records
            .iter()
            .map(|record| record["eventID"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids.len(),
            records.len(),
            "case {:?}: every record must carry a distinct eventID, got {ids:?}",
            row.case
        );

        for record in &records {
            assert_eq!(
                record["eventSource"],
                json!("aws:dynamodb"),
                "case {:?}: expected the DynamoDB event source, got {record}",
                row.case
            );
            assert_eq!(
                record["eventSourceARN"].as_str(),
                Some(stream.as_str()),
                "case {:?}: expected the stream ARN as the event source ARN, got {record}",
                row.case
            );
            assert_eq!(
                record["dynamodb"]["Keys"]["pk"]["S"],
                json!(KEY),
                "case {:?}: expected the record's keys, got {record}",
                row.case
            );
            assert_eq!(
                record["dynamodb"]["StreamViewType"],
                json!("NEW_AND_OLD_IMAGES"),
                "case {:?}: expected the stream's view type, got {record}",
                row.case
            );
        }
        // An INSERT has no old image, and a REMOVE no new one: a member the record does
        // not carry is absent rather than reported empty.
        assert_eq!(
            records[0]["dynamodb"]["OldImage"],
            Value::Null,
            "case {:?}: an INSERT carries no old image, got {:?}",
            row.case,
            records[0]
        );
        let last = records.len() - 1;
        assert_eq!(
            records[last]["dynamodb"]["NewImage"],
            Value::Null,
            "case {:?}: a REMOVE carries no new image, got {:?}",
            row.case,
            records[last]
        );

        // And nothing more arrives: the stream holds the changes the row made and no
        // record is delivered twice.
        tokio::time::sleep(QUIET).await;
        let extra = drain(&router, &url, row.case).await;
        assert!(
            extra.is_empty(),
            "case {:?}: expected no further delivery, got {extra:?}",
            row.case
        );
    }
}

/// One row of the filter table.
struct Filtered {
    case: &'static str,
    /// The event pattern the pipe filters its records by.
    pattern: Value,
    /// The change kinds the pattern admits, in applied order.
    admitted: &'static [&'static str],
}

/// RULE: a pipe carrying `FilterCriteria` admits only the records one of its patterns
/// matches, and it decides BEFORE the enrichment — so an enrichment that hands back
/// exactly what it was given delivers the admitted records and nothing else.
///
/// The rows differ along what the pattern reads, which is what the rule generalises over:
/// the change's own kind, a negation of a kind, and a value inside the record's new image.
#[tokio::test]
async fn filter_criteria_admit_only_matching_records_before_the_enrichment() {
    let rows = vec![
        Filtered {
            case: "inserts-only",
            pattern: json!({"eventName": ["INSERT"]}),
            admitted: &["INSERT"],
        },
        Filtered {
            case: "anything-but-remove",
            pattern: json!({"eventName": [{"anything-but": "REMOVE"}]}),
            admitted: &["INSERT", "MODIFY"],
        },
        Filtered {
            case: "a-value-of-the-new-image",
            pattern: json!({"dynamodb": {"NewImage": {VERSION: {"N": ["2"]}}}}),
            admitted: &["MODIFY"],
        },
    ];

    let router = emulator();
    for row in rows {
        let stream = given_streamed_table(&router, row.case, "NEW_AND_OLD_IMAGES").await;
        let (url, target) = given_fifo_queue(&router, row.case).await;
        let enrichment = given_express_workflow(&router, row.case, ECHO_DEFINITION).await;
        let created = create_pipe(
            &router,
            row.case,
            json!({
                "Source": stream,
                "Target": target,
                "RoleArn": ROLE,
                "DesiredState": "RUNNING",
                "SourceParameters": {
                    "DynamoDBStreamParameters": {
                        "StartingPosition": "TRIM_HORIZON",
                        "BatchSize": 1,
                    },
                    "FilterCriteria": {"Filters": [{"Pattern": row.pattern.to_string()}]},
                },
                "Enrichment": enrichment,
                "TargetParameters": fifo_target_parameters(),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe must succeed, got {created:?}",
            row.case
        );

        // One INSERT, one MODIFY and one REMOVE on the one key, so every row's pattern
        // sees the same three changes and only the ones it admits may arrive.
        apply_changes(&router, row.case, 2).await;

        let records = delivered(&router, &url, row.case, row.admitted.len()).await;
        let kinds: Vec<String> = records
            .iter()
            .map(|record| record["eventName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            kinds,
            row.admitted
                .iter()
                .map(|kind| (*kind).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected only the admitted changes, got {kinds:?}",
            row.case
        );
        // The enrichment handed back what it was given, so a record the filter dropped
        // could only be missing because it never reached the enrichment at all.
        tokio::time::sleep(QUIET).await;
        let extra = drain(&router, &url, row.case).await;
        assert!(
            extra.is_empty(),
            "case {:?}: a record the filter dropped must reach neither stage, got {extra:?}",
            row.case
        );
    }
}
