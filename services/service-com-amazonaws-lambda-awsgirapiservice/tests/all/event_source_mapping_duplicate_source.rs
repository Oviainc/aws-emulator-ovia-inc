// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the conflict `CreateEventSourceMapping` answers a second mapping
//! of records one function is already mapped to: "The resource already exists, or another
//! operation is in progress" (`ResourceConflictException`, HTTP 409).
//!
//! The rule under test: what a mapping polls is the event source TOGETHER WITH what it
//! consumes inside that source — the resource `EventSourceArn` names, or the bootstrap
//! servers of a self-managed Apache Kafka cluster, which carries no ARN at all, plus the
//! `Topics` entry of a Kafka source or the `Queues` entry of an Amazon MQ broker — and one
//! function is mapped to those records by at most one mapping, so a create that would read
//! the same records for the same function is refused with `ResourceConflictException`
//! naming the source, the consumed member, the function and the UUID of the mapping that
//! already holds it, while a create that differs in ANY part of that identity (another
//! server, another topic, another queue, another function) is accepted.
//!
//! Every row drives real HTTP requests through the router over the one harness this test
//! binary has for them ([`crate::event_source_mappings`]), and reads the function's
//! mappings back through `ListEventSourceMappings`, so a refusal that was answered without
//! being enforced over the store, or an acceptance that was not persisted, fails here. The
//! rows differ along what the rule generalises over: which KIND of source names the
//! records (a self-managed Kafka cluster, an MSK cluster, an Amazon MQ broker, an SQS
//! queue), which part of the identity the second create changes (nothing, the server, the
//! topic, the queue, the function), how many bootstrap servers name the cluster and in
//! which order they arrive, and which documented `FunctionName` form the second create
//! addresses the function by.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, list_mappings, router_on, Answer,
    MAPPINGS_URI, MSK_SOURCE, QUEUE_SOURCE,
};

/// The region the harness signs for, and the account every emulator ARN carries.
const REGION: &str = "us-east-1";
/// See [`REGION`].
const ACCOUNT: &str = "000000000000";

/// An Amazon MQ broker, "Amazon MQ – The ARN of the broker".
const MQ_SOURCE: &str = "arn:aws:mq:us-east-1:000000000000:broker:duplicate-broker:b-2222";

/// Bootstrap servers of a self-managed Apache Kafka cluster, each satisfying the
/// `Endpoint` pattern the model puts on an `EndpointLists` member
/// (`smithy_query(query="shape", shape="EndpointLists")`: a host followed by `:` and a
/// port).
const SERVER_ONE: &str = "broker-1.duplicate.example.com:9092";
/// See [`SERVER_ONE`].
const SERVER_TWO: &str = "broker-2.duplicate.example.com:9092";
/// See [`SERVER_ONE`].
const SERVER_THREE: &str = "broker-3.duplicate.example.com:9094";

/// Kafka topics, each satisfying the `Topic` pattern (`^[^.]([a-zA-Z0-9\-_.]+)$`).
const TOPIC_ONE: &str = "duplicate-topic";
/// See [`TOPIC_ONE`].
const TOPIC_TWO: &str = "duplicate-topic-two";

/// Amazon MQ destination queues, "The name of the Amazon MQ broker destination queue to
/// consume".
const MQ_QUEUE_ONE: &str = "duplicate-jobs";
/// See [`MQ_QUEUE_ONE`].
const MQ_QUEUE_TWO: &str = "duplicate-jobs-two";

/// How long past the instant a removal becomes due this suite waits before replaying a
/// create: the transition is owed by the CLOCK, so nothing here settles it on the
/// emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// The unqualified ARN a function of `name` carries, which is how the refusal names the
/// function the colliding mapping invokes.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The `message` member of an error answer.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_else(|| answer.text())
}

/// `DeleteEventSourceMapping` on the mapping `uuid` names
/// (`DELETE /2015-03-31/event-source-mappings/{UUID}`).
async fn delete_mapping(router: &MultiServiceRouter, uuid: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await
}

/// A `SelfManagedEventSource` naming `servers` as the cluster's brokers, in the order
/// given: "The list of bootstrap servers for your Kafka brokers in the following format:
/// `"KAFKA_BOOTSTRAP_SERVERS": ["abc.xyz.com:xxxx","abc2.xyz.com:xxxx"]`".
fn self_managed(servers: &[&str]) -> Value {
    json!({ "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": servers } })
}

/// A create body for `function` out of the members `source` states.
fn create_body(function: &str, source: &Value) -> Value {
    let mut body = Map::new();
    body.insert("FunctionName".to_string(), json!(function));
    for (member, value) in source.as_object().expect("a source states members") {
        body.insert(member.clone(), value.clone());
    }
    Value::Object(body)
}

/// The UUIDs `ListEventSourceMappings` reports for `function`.
async fn mapped_uuids(router: &MultiServiceRouter, function: &str) -> Vec<String> {
    let answer = list_mappings(router, &format!("FunctionName={function}")).await;
    assert_eq!(
        answer.status,
        200,
        "ListEventSourceMappings for {function:?} must answer 200, got {} {}",
        answer.status,
        answer.text()
    );
    let mut uuids = answer.listed_uuids();
    uuids.sort();
    uuids
}

/// What the refusal must say, as AWS words it for a source that is read per topic or per
/// queue: the source, the function and the consumed member in turn, then the UUID a caller
/// updates or deletes to converge.
fn conflict_message(
    source: &str,
    function: &str,
    consumed_member: &str,
    consumed: &str,
    uuid: &str,
) -> String {
    format!(
        "An event source mapping with event source (\"{source}\"), function (\"{}\"), \
         {consumed_member} (\"{consumed}\") already exists. Please update or delete the \
         existing mapping with UUID {uuid}",
        function_arn(function),
    )
}

/// How a cluster of several brokers is named: in a FIXED order, so one cluster is named
/// the same way whichever order its brokers arrived in.
fn named_cluster(servers: &[&str]) -> String {
    let mut servers: Vec<&str> = servers.to_vec();
    servers.sort_unstable();
    servers.join(",")
}

/// What the second create must be answered with.
enum Verdict {
    /// Accepted: the second create reads other records, so it is another mapping.
    Accepted,
    /// Refused with the conflict, whose message names the source, the consumed member and
    /// its value beside the function and the existing UUID.
    Refused {
        source: String,
        consumed_member: &'static str,
        consumed: &'static str,
    },
    /// Refused with the conflict, whose message must at least name the source, the
    /// function and the existing UUID — the wording for a source consumed whole is pinned
    /// by [`crate::read_representation_and_download_plane`].
    RefusedNaming { source: &'static str },
}

/// One row: the function, the two creates, and what the second must be answered with.
struct DuplicateCase {
    name: &'static str,
    /// The function the FIRST create maps; the second maps [`Self::second_function`] when
    /// it names another one.
    function: &'static str,
    /// How the second create addresses the function, when it does not use the bare name.
    second_function_as: Option<&'static str>,
    /// Another function the second create maps instead, when the row changes that part of
    /// the identity.
    second_function: Option<&'static str>,
    /// The source members of the first create.
    first: Value,
    /// The source members of the second create.
    second: Value,
    verdict: Verdict,
}

/// The rows: one per kind of source, each with the parts of the identity a second create
/// can differ in.
fn duplicate_cases() -> Vec<DuplicateCase> {
    vec![
        DuplicateCase {
            name: "a self-managed Kafka cluster, the same server and topic",
            function: "esm-dup-self-managed",
            second_function_as: None,
            second_function: None,
            first: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
                "StartingPosition": "LATEST",
            }),
            second: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
                "StartingPosition": "LATEST",
            }),
            verdict: Verdict::Refused {
                source: SERVER_ONE.to_string(),
                consumed_member: "topics",
                consumed: TOPIC_ONE,
            },
        },
        DuplicateCase {
            name: "a self-managed Kafka cluster addressed by the function's ARN",
            function: "esm-dup-by-arn",
            second_function_as: Some("arn"),
            second_function: None,
            first: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            second: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            verdict: Verdict::Refused {
                source: SERVER_ONE.to_string(),
                consumed_member: "topics",
                consumed: TOPIC_ONE,
            },
        },
        DuplicateCase {
            name: "a self-managed Kafka cluster whose brokers arrive in the other order",
            function: "esm-dup-reordered-servers",
            second_function_as: None,
            second_function: None,
            first: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE, SERVER_TWO]),
                "Topics": [TOPIC_ONE],
            }),
            second: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_TWO, SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            verdict: Verdict::Refused {
                source: named_cluster(&[SERVER_ONE, SERVER_TWO]),
                consumed_member: "topics",
                consumed: TOPIC_ONE,
            },
        },
        DuplicateCase {
            name: "a self-managed Kafka cluster, another topic of the same cluster",
            function: "esm-dup-other-topic",
            second_function_as: None,
            second_function: None,
            first: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            second: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_TWO],
            }),
            verdict: Verdict::Accepted,
        },
        DuplicateCase {
            name: "a self-managed Kafka cluster, the same topic on another cluster",
            function: "esm-dup-other-cluster",
            second_function_as: None,
            second_function: None,
            first: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            second: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_THREE]),
                "Topics": [TOPIC_ONE],
            }),
            verdict: Verdict::Accepted,
        },
        DuplicateCase {
            name: "a self-managed Kafka cluster, the same triple for another function",
            function: "esm-dup-other-function",
            second_function_as: None,
            second_function: Some("esm-dup-other-function-two"),
            first: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            second: json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
            verdict: Verdict::Accepted,
        },
        DuplicateCase {
            name: "an MSK cluster, the same topic",
            function: "esm-dup-msk",
            second_function_as: None,
            second_function: None,
            first: json!({ "EventSourceArn": MSK_SOURCE, "Topics": [TOPIC_ONE] }),
            second: json!({ "EventSourceArn": MSK_SOURCE, "Topics": [TOPIC_ONE] }),
            verdict: Verdict::Refused {
                source: MSK_SOURCE.to_string(),
                consumed_member: "topics",
                consumed: TOPIC_ONE,
            },
        },
        DuplicateCase {
            name: "an MSK cluster, another topic of the same cluster",
            function: "esm-dup-msk-other-topic",
            second_function_as: None,
            second_function: None,
            first: json!({ "EventSourceArn": MSK_SOURCE, "Topics": [TOPIC_ONE] }),
            second: json!({ "EventSourceArn": MSK_SOURCE, "Topics": [TOPIC_TWO] }),
            verdict: Verdict::Accepted,
        },
        DuplicateCase {
            name: "an Amazon MQ broker, the same destination queue",
            function: "esm-dup-mq",
            second_function_as: None,
            second_function: None,
            first: json!({ "EventSourceArn": MQ_SOURCE, "Queues": [MQ_QUEUE_ONE] }),
            second: json!({ "EventSourceArn": MQ_SOURCE, "Queues": [MQ_QUEUE_ONE] }),
            verdict: Verdict::Refused {
                source: MQ_SOURCE.to_string(),
                consumed_member: "queues",
                consumed: MQ_QUEUE_ONE,
            },
        },
        DuplicateCase {
            name: "an Amazon MQ broker, another destination queue of the same broker",
            function: "esm-dup-mq-other-queue",
            second_function_as: None,
            second_function: None,
            first: json!({ "EventSourceArn": MQ_SOURCE, "Queues": [MQ_QUEUE_ONE] }),
            second: json!({ "EventSourceArn": MQ_SOURCE, "Queues": [MQ_QUEUE_TWO] }),
            verdict: Verdict::Accepted,
        },
        DuplicateCase {
            name: "an SQS queue, which is consumed whole",
            function: "esm-dup-queue",
            second_function_as: None,
            second_function: None,
            first: json!({ "EventSourceArn": QUEUE_SOURCE }),
            second: json!({ "EventSourceArn": QUEUE_SOURCE }),
            verdict: Verdict::RefusedNaming {
                source: QUEUE_SOURCE,
            },
        },
    ]
}

#[tokio::test]
async fn a_second_mapping_of_the_same_records_for_one_function_conflicts() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in duplicate_cases() {
        let DuplicateCase {
            name,
            function,
            second_function_as,
            second_function,
            first,
            second,
            verdict,
        } = case;
        create_function(&router, function).await;
        if let Some(other) = second_function {
            create_function(&router, other).await;
        }

        let first_answer = create_mapping(&router, &create_body(function, &first)).await;
        assert_eq!(
            first_answer.status,
            202,
            "case {name:?}: the first create must be accepted, got {} {}",
            first_answer.status,
            first_answer.text()
        );
        let first_uuid = first_answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {name:?}: the first create must report a UUID"));

        let mapped_function = second_function.unwrap_or(function);
        let addressed = match second_function_as {
            Some("arn") => function_arn(mapped_function),
            _ => mapped_function.to_string(),
        };
        let second_answer = create_mapping(&router, &create_body(&addressed, &second)).await;

        match verdict {
            Verdict::Accepted => {
                assert_eq!(
                    second_answer.status,
                    202,
                    "case {name:?}: a create reading other records must be accepted, got {} {}",
                    second_answer.status,
                    second_answer.text()
                );
                let second_uuid = second_answer.string("UUID").unwrap_or_else(|| {
                    panic!("case {name:?}: the accepted create must report a UUID")
                });
                assert_ne!(
                    second_uuid, first_uuid,
                    "case {name:?}: the accepted create must mint its own UUID, got \
                     {second_uuid:?} for both"
                );
                let listed = mapped_uuids(&router, mapped_function).await;
                let expected = if second_function.is_some() {
                    vec![second_uuid.clone()]
                } else {
                    let mut both = vec![first_uuid.clone(), second_uuid.clone()];
                    both.sort();
                    both
                };
                assert_eq!(
                    listed, expected,
                    "case {name:?}: expected {mapped_function:?} to list {expected:?}, got \
                     {listed:?}"
                );
            }
            Verdict::Refused {
                source,
                consumed_member,
                consumed,
            } => {
                assert_eq!(
                    second_answer.status,
                    409,
                    "case {name:?}: expected 409 for a second mapping of the same records, got \
                     {} {}",
                    second_answer.status,
                    second_answer.text()
                );
                assert_eq!(
                    second_answer.error_code().rsplit('#').next().unwrap_or_default(),
                    "ResourceConflictException",
                    "case {name:?}: expected the modelled conflict, got {:?}",
                    second_answer.error_code()
                );
                let expected = conflict_message(
                    &source,
                    mapped_function,
                    consumed_member,
                    consumed,
                    &first_uuid,
                );
                let message = message_of(&second_answer);
                assert_eq!(
                    message, expected,
                    "case {name:?}: expected the refusal {expected:?}, got {message:?}"
                );
                let listed = mapped_uuids(&router, mapped_function).await;
                assert_eq!(
                    listed,
                    vec![first_uuid.clone()],
                    "case {name:?}: the refused create must leave {mapped_function:?} with only \
                     {first_uuid:?}, got {listed:?}"
                );
            }
            Verdict::RefusedNaming { source } => {
                assert_eq!(
                    second_answer.status,
                    409,
                    "case {name:?}: expected 409 for a second mapping of the same records, got \
                     {} {}",
                    second_answer.status,
                    second_answer.text()
                );
                let message = message_of(&second_answer);
                for named in [source, mapped_function, first_uuid.as_str()] {
                    assert!(
                        message.contains(named),
                        "case {name:?}: expected the refusal to name {named:?}, got {message:?}"
                    );
                }
                let listed = mapped_uuids(&router, mapped_function).await;
                assert_eq!(
                    listed,
                    vec![first_uuid.clone()],
                    "case {name:?}: the refused create must leave {mapped_function:?} with only \
                     {first_uuid:?}, got {listed:?}"
                );
            }
        }
    }
}

/// The conflict is held by the STORE rather than by the request: once the mapping that
/// held the records is gone — its removal owed by the clock after a delete — the identical
/// create is accepted again, under a UUID of its own.
///
/// The rows differ in the kind of source whose identity is freed: a self-managed Kafka
/// cluster named by its brokers and a topic, and an MSK cluster named by its ARN and a
/// topic.
#[tokio::test]
async fn a_deleted_mapping_frees_the_records_it_held() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a self-managed Kafka cluster",
            "esm-free-self-managed",
            json!({
                "SelfManagedEventSource": self_managed(&[SERVER_ONE]),
                "Topics": [TOPIC_ONE],
            }),
        ),
        (
            "an MSK cluster",
            "esm-free-msk",
            json!({ "EventSourceArn": MSK_SOURCE, "Topics": [TOPIC_TWO] }),
        ),
    ];

    for (name, function, source) in &cases {
        create_function(&router, function).await;
        let body = create_body(function, source);
        let first = create_mapping(&router, &body).await;
        assert_eq!(
            first.status,
            202,
            "case {name:?}: the first create must be accepted, got {} {}",
            first.status,
            first.text()
        );
        let first_uuid = first
            .string("UUID")
            .unwrap_or_else(|| panic!("case {name:?}: the first create must report a UUID"));

        let refused = create_mapping(&router, &body).await;
        assert_eq!(
            refused.status,
            409,
            "case {name:?}: expected 409 while the records are held, got {} {}",
            refused.status,
            refused.text()
        );

        let deleted = delete_mapping(&router, &first_uuid).await;
        assert_eq!(
            deleted.status,
            202,
            "case {name:?}: the delete must be accepted, got {} {}",
            deleted.status,
            deleted.text()
        );
        // A delete "enters a Deleting state and might not be completely deleted for
        // several seconds", so the records stay held until the removal the clock owes is
        // due.
        let held = create_mapping(&router, &body).await;
        assert_eq!(
            held.status,
            409,
            "case {name:?}: expected 409 while the deleted mapping is still held, got {} {}",
            held.status,
            held.text()
        );

        tokio::time::sleep(
            service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_DELETE_DELAY
                + SLACK,
        )
        .await;

        let replayed = create_mapping(&router, &body).await;
        assert_eq!(
            replayed.status,
            202,
            "case {name:?}: expected the identical create to be accepted once the removal is \
             due, got {} {}",
            replayed.status,
            replayed.text()
        );
        let replayed_uuid = replayed
            .string("UUID")
            .unwrap_or_else(|| panic!("case {name:?}: the replayed create must report a UUID"));
        assert_ne!(
            replayed_uuid, first_uuid,
            "case {name:?}: the replayed create must mint a new UUID, got {replayed_uuid:?} for \
             both"
        );
        let listed = mapped_uuids(&router, function).await;
        assert_eq!(
            listed,
            vec![replayed_uuid.clone()],
            "case {name:?}: expected {function:?} to list only {replayed_uuid:?}, got {listed:?}"
        );
    }
}
