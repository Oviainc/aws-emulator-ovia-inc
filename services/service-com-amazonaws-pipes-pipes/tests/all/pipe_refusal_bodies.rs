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
//! * an absent pipe is reported by ONE sentence per addressing form — the name-addressed
//!   control-plane operations name the pipe the request asked for, the ARN-addressed
//!   tagging operations state the constant resource-agnostic sentence;
//! * a name this account already holds is reported by ONE sentence naming it;
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

/// The exception message a refusal carries. The reading of it belongs to the harness, so
/// every suite reads it the same way.
fn message(reply: &Reply) -> String {
    reply.message()
}

/// The `fieldList` entries a refusal carries, as `(name, message)` pairs. `pub(crate)` so
/// the suites that pin other refusals read the modelled list the SAME way.
pub(crate) fn field_list(reply: &Reply) -> Vec<(String, String)> {
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
/// pins is the envelope rather than the code that built it. `pub(crate)` for the same
/// reason as [`field_list`].
///
/// Each entry's name and message go through `serde_json`, so a sentence carrying a
/// character a JSON string cannot hold literally — the quotes a reason puts around the
/// member it names — is spelled in the document the way a document spells it.
pub(crate) fn validation_error_fields(entries: &[(&str, &str)]) -> String {
    let fields = entries
        .iter()
        .map(|(name, message)| {
            format!(
                r#"{{"Name":{},"Message":{}}}"#,
                Value::String((*name).to_string()),
                Value::String((*message).to_string()),
            )
        })
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
            // The member DOES belong to this pipe's source type, but the pipe holds no
            // parameters for THAT member: an Amazon MQ broker's ARN does not say which
            // engine it runs, so a pipe of this source type carries one of the two broker
            // members, and the members only `CreatePipe` can state (the broker's queue
            // name) have nothing to be carried over from for the other one.
            case: "mq-given-the-other-broker-engine",
            source: "arn:aws:mq:us-west-2:000000000000:broker:orders:b-1234",
            created: json!({
                "RabbitMQBrokerParameters": {
                    "Credentials": {
                        "BasicAuth":
                            "arn:aws:secretsmanager:us-west-2:000000000000:secret:mq-abc",
                    },
                    "QueueName": "orders",
                },
            }),
            member: "ActiveMQBrokerParameters",
            supplied: json!({
                "Credentials": {
                    "BasicAuth": "arn:aws:secretsmanager:us-west-2:000000000000:secret:mq-abc",
                },
            }),
            source_type: "mq",
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

        // The refusal changed nothing: what the pipe was created with is what it still has,
        // and the member the update supplied is not there. The reported `SourceParameters`
        // also carry the batch size the pipe polls its source with, whose rule and table
        // live in `tests/all/pipe_reported_configuration.rs`, so what is asserted here is
        // every member and value the create stored.
        let described = describe(&router, row.case).await;
        let reported = described.member("SourceParameters").clone();
        assert_stored(row.case, &reported, &row.created);
        assert_eq!(
            reported.get(row.member),
            None,
            "case {:?}: a refused update must not have stored {}, got {described:?}",
            row.case,
            row.member
        );
    }
}

/// Assert that `reported` carries every member and value of `created`, which is `Null` for
/// a pipe created with no source parameters at all.
fn assert_stored(case: &str, reported: &Value, created: &Value) {
    let Some(members) = created.as_object() else {
        return;
    };
    for (member, expected) in members {
        let actual = reported.get(member).unwrap_or(&Value::Null);
        if expected.is_object() {
            assert_stored(case, actual, expected);
            continue;
        }
        assert_eq!(
            actual, expected,
            "case {case:?}: the stored {member} must still read back as {expected}, \
             got {actual:?}"
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

/// RULE: an absent pipe is reported by ONE sentence per addressing form — the
/// NAME-addressed control-plane operations interpolate the pipe the caller named into one
/// fixed sentence, and the ARN-addressed tagging operations, which resolve no pipe name,
/// report the constant resource-agnostic sentence instead — the same sentence whichever
/// operation of that form was asked for the pipe.
///
/// The rows differ along which operation is called (every one that addresses a single
/// pipe, by name and by ARN) and along the form of the absent name: plain, the longest
/// `PipeName` allows, one carrying the other characters its pattern admits, and one
/// differing from a stored name only in case.
#[tokio::test]
async fn an_absent_pipe_is_reported_by_one_sentence_per_addressing_form() {
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
        // (the case, method, uri, body, whether the pipe is addressed BY NAME) of every
        // operation that addresses one pipe.
        let rows: Vec<(&str, &str, String, Option<Value>, bool)> = vec![
            ("DescribePipe", "GET", format!("/v1/pipes/{name}"), None, true),
            (
                "UpdatePipe",
                "PUT",
                format!("/v1/pipes/{name}"),
                Some(json!({"RoleArn": ROLE})),
                true,
            ),
            (
                "StartPipe",
                "POST",
                format!("/v1/pipes/{name}/start"),
                None,
                true,
            ),
            ("StopPipe", "POST", format!("/v1/pipes/{name}/stop"), None, true),
            ("DeletePipe", "DELETE", format!("/v1/pipes/{name}"), None, true),
            ("ListTagsForResource", "GET", tags_uri.clone(), None, false),
            (
                "TagResource",
                "POST",
                tags_uri.clone(),
                Some(json!({"tags": {"env": "test"}})),
                false,
            ),
            (
                "UntagResource",
                "DELETE",
                format!("{tags_uri}?tagKeys=env"),
                None,
                false,
            ),
        ];

        for (case, method, uri, body, by_name) in rows {
            let refused = call(&router, method, &uri, body, REGION).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case} {name:?}: expected 404 NotFoundException, got {refused:?}"
            );
            let expected = if by_name {
                format!("Pipe {name} does not exist.")
            } else {
                "Resource was not found".to_string()
            };
            assert_eq!(
                message(&refused),
                expected,
                "case {case} {name:?}: expected the absent-pipe sentence of its addressing \
                 form, got {refused:?}"
            );
            if !by_name {
                // The constant sentence resolves nothing and echoes nothing: neither the
                // pipe the ARN names nor any other part of it reaches the caller.
                assert!(
                    !message(&refused).contains(name) && !message(&refused).contains("arn:"),
                    "case {case} {name:?}: the ARN-addressed sentence must echo no part of \
                     the ARN it was given, got {refused:?}"
                );
            }
        }
    }
}

/// RULE: `CreatePipe` for a name this account already holds is refused with
/// `ConflictException` (409) reporting ONE sentence naming the pipe the request asked for —
/// built from the request, so the same sentence whatever else the request carried — with
/// `resourceId` and `resourceType` naming the pipe, as the model requires of all three
/// members, and the stored pipe left exactly as it was.
///
/// The rows differ along the form of the taken name (plain, the longest `PipeName` allows,
/// one carrying the other characters its pattern admits) and along what the second request
/// carries beside it (the identical body, a different description, a different target).
#[tokio::test]
async fn a_taken_name_is_reported_by_one_sentence_naming_the_pipe() {
    let long_name = "t".repeat(64);
    for name in ["taken", &long_name, "taken.with-other_chars"] {
        let router = router();
        let first = create_body(
            "arn:aws:sqs:us-west-2:000000000000:orders",
            TARGET,
            json!({"Description": "the stored one"}),
        );
        let created = create(&router, name, first.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the fixture pipe must be created, got {created:?}"
        );

        let rows: Vec<(&str, Value)> = vec![
            ("identical", first.clone()),
            (
                "a different description",
                create_body(
                    "arn:aws:sqs:us-west-2:000000000000:orders",
                    TARGET,
                    json!({"Description": "another one"}),
                ),
            ),
            (
                "a different target",
                create_body(
                    "arn:aws:sqs:us-west-2:000000000000:orders",
                    "arn:aws:sqs:us-west-2:000000000000:results",
                    json!({}),
                ),
            ),
        ];

        for (case, body) in rows {
            let refused = create(&router, name, body).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (409, "ConflictException"),
                "case {name:?} / {case}: expected 409 ConflictException, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                format!("Pipe {name} already exists."),
                "case {name:?} / {case}: expected the one taken-name sentence, got {refused:?}"
            );
            assert_eq!(
                (
                    refused.string("resourceId"),
                    refused.string("resourceType")
                ),
                (name.to_string(), "pipe".to_string()),
                "case {name:?} / {case}: the conflict must name the resource it is about, \
                 got {refused:?}"
            );
        }

        // None of the refused calls overwrote the stored pipe.
        let described = describe(&router, name).await;
        assert_eq!(
            described.string("Description"),
            "the stored one",
            "case {name:?}: a refused CreatePipe must leave the stored pipe as it was, \
             got {described:?}"
        );
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

/// RULE: a request PARAMETER this service cannot interpret AT ALL — the opaque
/// continuation token, which a caller passes back exactly as it was issued — is refused
/// with `ValidationException` (400) reporting ONE sentence that names the parameter and
/// states the reason class, with the per-field detail in the modelled `fieldList` rather
/// than inside `message`.
///
/// The rows differ along the form of the unreadable token: text that is not the token's
/// encoding at all, an encoding of bytes that are not text, and a truncated encoding. A
/// token this service DID issue is accepted at the end of the table, so what the rows
/// refuse is the token and not paging.
#[tokio::test]
async fn a_continuation_token_this_service_cannot_read_reports_one_parameter_sentence() {
    let router = router();
    for name in ["tokena", "tokenb"] {
        let created = create(
            &router,
            name,
            create_body("arn:aws:sqs:us-west-2:000000000000:orders", TARGET, json!({})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the fixture pipe must be created, got {created:?}"
        );
    }

    // (case, the `NextToken` query value the row sends)
    let rows: Vec<(&str, &str)> = vec![
        ("not an encoding at all", "not-a-token"),
        ("a word that is not an encoding", "zzzz"),
        ("an encoding of bytes that are not text", "ff"),
        ("an encoding of a longer run of non-text bytes", "fffefd"),
        ("a truncated encoding", "abc"),
    ];
    for (case, token) in rows {
        let refused = call(
            &router,
            "GET",
            &format!("/v1/pipes?NextToken={token}"),
            None,
            REGION,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            "Parameter NextToken is not valid. Reason: Malformed request.",
            "case {case:?}: expected the one invalid-parameter sentence, got {refused:?}"
        );
        assert!(
            !message(&refused).contains("ValidationErrorFields"),
            "case {case:?}: the per-field detail belongs in fieldList, not in message: \
             {refused:?}"
        );
        let fields = field_list(&refused);
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: expected one fieldList entry for the refused parameter, \
             got {fields:?}"
        );
        assert_eq!(
            fields[0].0, "NextToken",
            "case {case:?}: expected the fieldList entry to name NextToken, got {fields:?}"
        );
        assert!(
            !fields[0].1.is_empty(),
            "case {case:?}: expected the fieldList entry to say why, got {fields:?}"
        );
        assert!(
            !message(&refused).contains(token),
            "case {case:?}: the sentence states the reason class rather than echoing the \
             token, got {refused:?}"
        );
    }

    // The token this service issued is read, so the rows above refuse the token itself.
    let first = call(&router, "GET", "/v1/pipes?Limit=1", None, REGION).await;
    let issued = first.string("NextToken");
    assert!(
        !issued.is_empty(),
        "the fixture listing must issue a NextToken, got {first:?}"
    );
    let resumed = call(
        &router,
        "GET",
        &format!("/v1/pipes?Limit=1&NextToken={issued}"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "a NextToken this service issued must be accepted, got {resumed:?}"
    );
}
