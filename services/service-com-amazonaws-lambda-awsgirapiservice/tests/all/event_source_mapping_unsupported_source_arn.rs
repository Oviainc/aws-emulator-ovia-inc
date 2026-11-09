// SPDX-License-Identifier: CC0-1.0

//! What `CreateEventSourceMapping` tells a caller whose `EventSourceArn` belongs to a
//! service an event source mapping does not poll at all.
//!
//! RULE: `EventSourceArn` is "the Amazon Resource Name (ARN) of the event source", and the
//! sources it may name are enumerated per service ("Amazon Kinesis", "Amazon DynamoDB
//! Streams", "Amazon Simple Queue Service", "Amazon Managed Streaming for Apache Kafka",
//! "Amazon MQ", "Amazon DocumentDB" —
//! `smithy_query(query="doc", shape="CreateEventSourceMappingRequest",
//! member="EventSourceArn")`). An ARN whose VENDOR namespace is none of those names no
//! source service whatever its resource half looks like, so there is no ARN format to
//! expect for it: the create is refused with the modelled `InvalidParameterValueException`
//! (HTTP 400, `smithy_query(query="operation",
//! operation="CreateEventSourceMapping")`) whose message states that the source was not
//! recognized and that the source ARN sent is unsupported, quoting the value exactly as the
//! request sent it — and no mapping is stored.
//!
//! The rows below differ along what the rule generalises over: the vendor namespace (nine
//! services outside that enumeration, including one whose name only resembles an enumerated
//! one and one whose resource is shaped exactly like an enumerated source's), whether the
//! ARN names a Region and an account at all, whose account it names, which separator its
//! resource half uses, how many segments that half carries and which partition the ARN is
//! written in. The last row is the boundary: an ARN of an enumerated vendor, which this
//! refusal must not touch.

use std::sync::Arc;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    create_function, create_mapping, create_request, list_mappings, router_on,
    unsupported_source_refusal, QUEUE_SOURCE,
};

/// The modelled error a source ARN no mapping can poll is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// Lambda alone: the rule is about what the control plane REFUSES, so nothing the ARNs name
/// has to exist.
fn emulator() -> MultiServiceRouter {
    router_on(&Arc::new(engine::InMemoryEngine::new()))
}

/// One row of the table: the `EventSourceArn` a create names and whether its vendor is a
/// service an event source mapping polls.
struct Row {
    case: &'static str,
    event_source: &'static str,
    /// Whether the vendor IS one of the source services `EventSourceArn` enumerates, in
    /// which case this refusal must not be the answer.
    enumerated_vendor: bool,
}

/// The rows: see the module docs.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket, which names no Region and no account",
            event_source: "arn:aws:s3:::order-dropbox",
            enumerated_vendor: false,
        },
        Row {
            case: "an object inside a bucket, whose resource half carries slashes",
            event_source: "arn:aws:s3:::order-dropbox/incoming/2026/orders.json",
            enumerated_vendor: false,
        },
        Row {
            case: "a bucket in another partition",
            event_source: "arn:aws-cn:s3:::order-dropbox",
            enumerated_vendor: false,
        },
        Row {
            case: "a topic, which an event source mapping does not poll",
            event_source: "arn:aws:sns:us-east-1:000000000000:order-events",
            enumerated_vendor: false,
        },
        Row {
            case: "a video stream, whose vendor only resembles an enumerated one",
            event_source: "arn:aws:kinesisvideo:us-east-1:000000000000:stream/order-cam/1700000000000",
            enumerated_vendor: false,
        },
        Row {
            case: "an emr cluster, shaped like a documentdb source but of another vendor",
            event_source: "arn:aws:elasticmapreduce:us-east-1:000000000000:cluster:j-1A2B3C4D5E6F7",
            enumerated_vendor: false,
        },
        Row {
            case: "a state machine, whose resource half is colon-separated",
            event_source: "arn:aws:states:eu-west-1:000000000000:stateMachine:order-flow",
            enumerated_vendor: false,
        },
        Row {
            case: "a role of another account, which names no Region",
            event_source: "arn:aws:iam::123456789012:role/order-poller",
            enumerated_vendor: false,
        },
        Row {
            case: "an event bus rule, whose vendor delivers rather than is polled",
            event_source: "arn:aws:events:us-east-1:000000000000:rule/order-intake/order-rule",
            enumerated_vendor: false,
        },
        Row {
            case: "a queue, which IS one of the enumerated source services",
            event_source: QUEUE_SOURCE,
            enumerated_vendor: true,
        },
    ]
}

/// RULE: an `EventSourceArn` whose vendor namespace is none of the source services that
/// member enumerates is refused with the modelled 400 stating that the source ARN sent is
/// unsupported, quoting it as sent, and stores no mapping; an ARN of an enumerated vendor is
/// not refused that way.
#[tokio::test]
async fn an_event_source_arn_of_an_unpolled_service_is_refused_as_an_unsupported_source_arn() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let function = format!("esm-unsupported-{index}");
        create_function(&router, &function).await;

        let answer = create_mapping(
            &router,
            &create_request(&function, Some(row.event_source), &json!({})),
        )
        .await;

        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();

        if row.enumerated_vendor {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: {} names a service a mapping polls, so the create must be \
                 accepted, got {} {}",
                row.event_source,
                answer.status,
                answer.text()
            );
            assert_eq!(
                held, 1,
                "case {case:?}: an accepted create must store one mapping, got {held} \
                 mapping(s) in {}",
                listed.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: {} names no event source a mapping can poll, so the create must \
             answer 400, got {} {}",
            row.event_source,
            answer.status,
            answer.text()
        );
        let code = answer.error_code();
        assert!(
            code.contains(INVALID_PARAMETER),
            "case {case:?}: the refusal must be {INVALID_PARAMETER}, got {code:?}"
        );
        let message = answer.string("message").unwrap_or_default();
        assert_eq!(
            message,
            unsupported_source_refusal(row.event_source),
            "case {case:?}: the refusal must state that the source ARN sent is unsupported, \
             got {message:?}"
        );
        assert_eq!(
            held, 0,
            "case {case:?}: a refused create must store no mapping, got {held} mapping(s) in {}",
            listed.text()
        );
    }
}
