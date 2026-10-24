// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED pipes call says, end to end over the wire.
//!
//! The status and the error code of these refusals are pinned by the suites that own the
//! operations (`pipe_lifecycle`, `pipe_update`, `pipe_tagging`); this suite pins the
//! BODIES — the three message rules this service builds from the request:
//!
//! * a validation refusal reports the refused request fields as a `ValidationErrorFields`
//!   JSON document in the exception's `message`, and the same entries in the modelled
//!   `fieldList`; for `SourceParameters` the entry names the member that was supplied and
//!   the source type the pipe resolves to;
//! * an absent pipe is reported by ONE sentence naming the pipe the request asked for, the
//!   same sentence whichever operation was asked for it;
//! * an `UpdatePipe` that lands while a move is already in flight is reported by ONE
//!   sentence naming the pipe, whichever move that is, leaking none of the pipe's states.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — reusing the harness `pipe_lifecycle`, `pipe_update` and
//! `pipe_tagging` already own rather than copying it.

use serde_json::{json, Value};

use crate::pipe_lifecycle::{call, create, describe, router, Reply, PAST_SETTLE, REGION};
use crate::pipe_tagging::{encode, pipe_arn};
use crate::pipe_update::{create_body, update};

const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";
const TARGET: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// The `PipeState` values a refusal must not leak. Source: the `PipeState` enum of
/// `smithy-model/com-amazonaws-pipes-pipes.json`.
const PIPE_STATES: &[&str] = &[
    "CREATE_FAILED",
    "CREATE_ROLLBACK_FAILED",
    "CREATING",
    "DELETE_FAILED",
    "DELETE_ROLLBACK_FAILED",
    "DELETING",
    "RUNNING",
    "STARTING",
    "START_FAILED",
    "STOPPED",
    "STOPPING",
    "STOP_FAILED",
    "UPDATE_FAILED",
    "UPDATE_ROLLBACK_FAILED",
    "UPDATING",
];

/// The exception message a refusal carries, however this protocol's error body spells the
/// modelled `message` member.
fn message(reply: &Reply) -> String {
    for member in ["message", "Message"] {
        if let Some(text) = reply.member(member).as_str() {
            return text.to_string();
        }
    }
    String::new()
}

/// The `fieldList` entries a refusal carries, as `(name, message)` pairs.
fn field_list(reply: &Reply) -> Vec<(String, String)> {
    reply
        .member("fieldList")
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    let read = |member: &str| {
                        field
                            .get(member)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (read("name"), read("message"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The document a validation refusal's message is: one `ValidationErrorFields` entry per
/// refused request field. Spelled out here, independently of the handler, so what the test
/// pins is the envelope rather than the code that built it.
fn validation_error_fields(entries: &[(&str, &str)]) -> String {
    let fields = entries
        .iter()
        .map(|(name, message)| format!(r#"{{"Name":"{name}","Message":"{message}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"ValidationErrorFields":[{fields}]}}"#)
}

/// One row of the source-type table.
struct Mismatch {
    /// Also the pipe's name, so every row is independent.
    case: &'static str,
    /// The `Source` the pipe is created with, which fixes its source type.
    source: &'static str,
    /// The `SourceParameters` the pipe is created with, or `Value::Null` for none.
    created: Value,
    /// The `SourceParameters` member the update supplies, and that member's value.
    member: &'static str,
    supplied: Value,
    /// The source type the pipe resolves to, which the refusal must name.
    source_type: &'static str,
}

/// RULE: a pipe's source type is fixed when it is created — `UpdatePipe` carries no
/// `Source` and `UpdatePipeSourceParameters` holds exactly one member per source type — so
/// `SourceParameters` the source cannot have is refused with `ValidationException` (400)
/// whose body is this service's `ValidationErrorFields` document (and `fieldList`) naming
/// the member that was supplied and the source type the pipe resolves to, leaving the
/// stored record exactly as it was.
///
/// The rows differ along what that rule generalises over: the pipe's source type (queue,
/// two stream types, a broker, both Kafka forms — one per source type the model has
/// parameters for), which other type's member is supplied, and whether the pipe was
/// created with any `SourceParameters` at all.
#[tokio::test]
async fn source_parameters_of_another_source_type_are_refused_naming_the_member_and_the_type() {
    let rows = vec![
        Mismatch {
            case: "sqs-given-dynamodb",
            source: "arn:aws:sqs:us-west-2:000000000000:orders",
            created: json!({"SqsQueueParameters": {"BatchSize": 1}}),
            member: "DynamoDBStreamParameters",
            supplied: json!({"BatchSize": 5}),
            source_type: "sqs",
        },
        Mismatch {
            case: "sqs-given-kinesis",
            source: "arn:aws:sqs:us-west-2:000000000000:orders",
            created: json!({"SqsQueueParameters": {"BatchSize": 1}}),
            member: "KinesisStreamParameters",
            supplied: json!({"BatchSize": 5}),
            source_type: "sqs",
        },
        Mismatch {
            case: "kinesis-given-sqs",
            source: "arn:aws:kinesis:us-west-2:000000000000:stream/events",
            created: json!({
                "KinesisStreamParameters": {"StartingPosition": "LATEST", "BatchSize": 2},
            }),
            member: "SqsQueueParameters",
            supplied: json!({"BatchSize": 3}),
            source_type: "kinesis",
        },
        Mismatch {
            case: "dynamodb-given-kinesis",
            source: "arn:aws:dynamodb:us-west-2:000000000000:table/orders/stream/2026-01-01",
            created: json!({
                "DynamoDBStreamParameters": {"StartingPosition": "TRIM_HORIZON"},
            }),
            member: "KinesisStreamParameters",
            supplied: json!({"BatchSize": 4}),
            source_type: "dynamodb",
        },
        Mismatch {
            case: "mq-given-sqs",
            source: "arn:aws:mq:us-west-2:000000000000:broker:orders:b-1234",
            created: json!({
                "ActiveMQBrokerParameters": {
                    "Credentials": {
                        "BasicAuth":
                            "arn:aws:secretsmanager:us-west-2:000000000000:secret:mq-abc",
                    },
                    "QueueName": "orders",
                },
            }),
            member: "SqsQueueParameters",
            supplied: json!({"BatchSize": 6}),
            source_type: "mq",
        },
        Mismatch {
            case: "kafka-given-rabbitmq",
            source: "arn:aws:kafka:us-west-2:000000000000:cluster/demo/abcd-1",
            created: json!({
                "ManagedStreamingKafkaParameters": {"TopicName": "orders", "BatchSize": 7},
            }),
            member: "RabbitMQBrokerParameters",
            supplied: json!({
                "Credentials": {
                    "BasicAuth": "arn:aws:secretsmanager:us-west-2:000000000000:secret:mq-abc",
                },
            }),
            source_type: "kafka",
        },
        Mismatch {
            // `Source` is not an ARN here — the `smk://` form its pattern also admits —
            // so the source type is that form's own rather than an ARN namespace.
            case: "selfmanagedkafka-given-msk",
            source: "smk://b-1.broker.kafka.us-west-2.amazonaws.com:9092",
            created: json!({
                "SelfManagedKafkaParameters": {"TopicName": "orders", "BatchSize": 8},
            }),
            member: "ManagedStreamingKafkaParameters",
            supplied: json!({"BatchSize": 9}),
            source_type: "selfmanagedkafka",
        },
        Mismatch {
            // The member DOES belong to this pipe's source type, but the pipe was created
            // with no parameters for it, so the members only `CreatePipe` can state (the
            // starting position) have nothing to be carried over from: the source cannot
            // hold the parameters as sent either.
            case: "kinesis-without-created-parameters",
            source: "arn:aws:kinesis:us-west-2:000000000000:stream/events",
            created: Value::Null,
            member: "KinesisStreamParameters",
            supplied: json!({"BatchSize": 10}),
            source_type: "kinesis",
        },
    ];

    for row in rows {
        let router = router();
        let mut extra = json!({"Description": "kept"});
        if !row.created.is_null() {
            extra["SourceParameters"] = row.created.clone();
        }
        let created = create(
            &router,
            row.case,
            create_body(row.source, TARGET, extra),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the fixture pipe must be created, got {created:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;

        let refused = update(
            &router,
            row.case,
            json!({
                "RoleArn": ROLE,
                "SourceParameters": {row.member: row.supplied},
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {:?}: expected 400 ValidationException, got {refused:?}",
            row.case
        );
        let reason = format!(
            "Invalid source parameter provided for source {}.",
            row.source_type
        );
        assert_eq!(
            message(&refused),
            validation_error_fields(&[(row.member, &reason)]),
            "case {:?}: expected the ValidationErrorFields document naming {} and source {}, \
             got {refused:?}",
            row.case,
            row.member,
            row.source_type
        );
        assert_eq!(
            field_list(&refused),
            vec![(row.member.to_string(), reason.clone())],
            "case {:?}: the fieldList must carry the same entry as the message, got {refused:?}",
            row.case
        );

        // The refusal changed nothing: what the pipe was created with is what it still has.
        let described = describe(&router, row.case).await;
        assert_eq!(
            described.member("SourceParameters"),
            &row.created,
            "case {:?}: a refused update must leave the stored SourceParameters as they \
             were, got {described:?}",
            row.case
        );
    }
}

/// RULE: a same-type `SourceParameters` update is still accepted — the refusal above is
/// about the source type, not about updating source parameters.
#[tokio::test]
async fn source_parameters_of_the_pipes_own_source_type_still_apply() {
    let router = router();
    let created = create(
        &router,
        "ownsourcetype",
        create_body(
            "arn:aws:kinesis:us-west-2:000000000000:stream/events",
            TARGET,
            json!({
                "SourceParameters": {
                    "KinesisStreamParameters": {"StartingPosition": "LATEST", "BatchSize": 2},
                },
            }),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture pipe must be created, got {created:?}"
    );
    tokio::time::sleep(PAST_SETTLE).await;

    let accepted = update(
        &router,
        "ownsourcetype",
        json!({
            "RoleArn": ROLE,
            "SourceParameters": {"KinesisStreamParameters": {"BatchSize": 12}},
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a same-type SourceParameters update must be accepted, got {accepted:?}"
    );
    tokio::time::sleep(PAST_SETTLE).await;

    let described = describe(&router, "ownsourcetype").await;
    assert_eq!(
        described.member("SourceParameters"),
        &json!({
            "KinesisStreamParameters": {"StartingPosition": "LATEST", "BatchSize": 12},
        }),
        "the accepted update must be readable back, got {described:?}"
    );
}

/// RULE: an absent pipe is reported by ONE sentence built from the request — the pipe the
/// caller named interpolated into it — the same sentence whichever operation was asked for
/// that pipe.
///
/// The rows differ along which operation is called (every one that addresses a single
/// pipe, by name and by ARN) and along the form of the absent name: plain, the longest
/// `PipeName` allows, one carrying the other characters its pattern admits, and one
/// differing from a stored name only in case.
#[tokio::test]
async fn an_absent_pipe_is_reported_by_one_sentence_naming_it() {
    let router = router();
    let created = create(
        &router,
        "stored",
        create_body("arn:aws:sqs:us-west-2:000000000000:orders", TARGET, json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture pipe must be created, got {created:?}"
    );

    let long_name = "g".repeat(64);
    for name in ["ghost", &long_name, "ghost.with-other_chars", "STORED"] {
        let arn = pipe_arn(name);
        let tags_uri = format!("/tags/{}", encode(&arn));
        // (the case, method, uri, body) of every operation that addresses one pipe.
        let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("DescribePipe", "GET", format!("/v1/pipes/{name}"), None),
            (
                "UpdatePipe",
                "PUT",
                format!("/v1/pipes/{name}"),
                Some(json!({"RoleArn": ROLE})),
            ),
            (
                "StartPipe",
                "POST",
                format!("/v1/pipes/{name}/start"),
                None,
            ),
            ("StopPipe", "POST", format!("/v1/pipes/{name}/stop"), None),
            ("DeletePipe", "DELETE", format!("/v1/pipes/{name}"), None),
            ("ListTagsForResource", "GET", tags_uri.clone(), None),
            (
                "TagResource",
                "POST",
                tags_uri.clone(),
                Some(json!({"tags": {"env": "test"}})),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("{tags_uri}?tagKeys=env"),
                None,
            ),
        ];

        for (case, method, uri, body) in rows {
            let refused = call(&router, method, &uri, body, REGION).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case} {name:?}: expected 404 NotFoundException, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                format!("Pipe {name} does not exist."),
                "case {case} {name:?}: expected the one absent-pipe sentence naming the \
                 pipe, got {refused:?}"
            );
        }
    }
}

/// RULE: an `UpdatePipe` that lands while a move is already in flight is refused with
/// `ConflictException` (409) reporting ONE sentence naming the pipe — the same sentence
/// whichever move is in flight, leaking none of the pipe's own states — with `resourceId`
/// and `resourceType` naming the pipe, as the model requires of all three members.
///
/// The rows differ along which call started the transition, hence which state the pipe is
/// passing through when the update arrives.
#[tokio::test]
async fn an_update_during_a_move_in_flight_reports_one_sentence_naming_the_pipe() {
    // (the case and pipe name, the method and URI suffix of the call that starts the
    // transition — empty for the row the create itself starts).
    let rows: Vec<(&str, &str, &str)> = vec![
        ("conflictcreating", "", ""),
        ("conflictstopping", "POST", "/stop"),
        ("conflictstarting", "POST", "/start"),
        ("conflictupdating", "PUT", ""),
    ];

    for (case, method, suffix) in rows {
        let router = router();
        let created = create(
            &router,
            case,
            create_body("arn:aws:sqs:us-west-2:000000000000:orders", TARGET, json!({})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the fixture pipe must be created, got {created:?}"
        );

        if !method.is_empty() {
            // Out of CREATING first, so the transition in flight is the one this row names.
            tokio::time::sleep(PAST_SETTLE).await;
            let body = if method == "PUT" {
                Some(json!({"RoleArn": ROLE}))
            } else {
                None
            };
            let started = call(
                &router,
                method,
                &format!("/v1/pipes/{case}{suffix}"),
                body,
                REGION,
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: the call that starts the transition must succeed, \
                 got {started:?}"
            );
        }

        let refused = update(&router, case, json!({"RoleArn": ROLE})).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Pipe {case} is updating concurrently."),
            "case {case:?}: expected the one concurrent-update sentence, got {refused:?}"
        );
        for state in PIPE_STATES {
            assert!(
                !message(&refused).contains(state),
                "case {case:?}: the refusal must leak no pipe state, but it names {state}: \
                 {refused:?}"
            );
        }
        assert_eq!(
            (
                refused.string("resourceId"),
                refused.string("resourceType")
            ),
            (case.to_string(), "pipe".to_string()),
            "case {case:?}: the conflict must name the resource it is about, got {refused:?}"
        );
    }
}
