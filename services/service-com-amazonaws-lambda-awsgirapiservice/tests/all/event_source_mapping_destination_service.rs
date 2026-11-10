// SPDX-License-Identifier: CC0-1.0

//! WHICH destinations an event source mapping may be created with and updated onto.
//!
//! RULE: a destination of a mapping's `DestinationConfig` is a resource Lambda WRITES a
//! record of a failed batch to, and the kinds it writes to are enumerated on the member
//! itself — "you can configure an Amazon SNS topic, Amazon SQS queue, Amazon S3 bucket, or
//! Kafka topic as the destination" (`smithy_query(query="shape", shape="OnFailure")`) — so a
//! destination naming a resource of any other service, or no resource at all, is refused by
//! both calls that state one with the modelled `InvalidParameterValueException` (HTTP 400,
//! `smithy_query(query="operation", operation="CreateEventSourceMapping,\
//! UpdateEventSourceMapping")`), stating that the service the destination belongs to is not
//! supported as a destination; nothing is created and nothing is changed.
//!
//! The rows below differ along what the rule generalises over: the SERVICE the destination
//! names (each kind the member enumerates, including the two the issue did not cite — an
//! Amazon S3 bucket and a Kafka topic — and six resources of services no record is delivered
//! to), the FORM of the value (an ARN, the one non-ARN URI the member's pattern admits, a
//! bare name that is no ARN, an ARN of another partition) and which SLOT of the
//! configuration states it. Each row is driven through `CreateEventSourceMapping` and
//! through `UpdateEventSourceMapping` on a mapping that already holds a supported
//! destination, so neither call can drift from the rule, and the destinations the create
//! accepts are read back through `GetEventSourceMapping`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    EVENT_SOURCE_MAPPING_SETTLE_DELAY, UNSUPPORTED_MAPPING_DESTINATION_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_function, create_mapping, create_request, get_mapping, list_mappings, router_on,
    Answer, DYNAMODB_SOURCE, KINESIS_SOURCE, MAPPINGS_URI, MSK_SOURCE,
};

/// The modelled error a destination no record is delivered to is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The error the request walk refuses a value that is no `DestinationArn` with, which is a
/// statement about the REQUEST rather than about the destination's service.
const VALIDATION: &str = "ValidationException";

/// The destination every mapping in this suite is created with before an update re-states
/// one: an Amazon SQS queue, one of the kinds the member enumerates.
const HELD_DESTINATION: &str = "arn:aws:sqs:us-east-1:000000000000:order-dlq";

/// How long past the instant a create made the move out of `Creating` due this suite waits
/// before updating the mapping: the transition is owed by the CLOCK, and a mapping still
/// settling is not available to take another change.
const SLACK: Duration = Duration::from_millis(100);

/// Lambda alone: the rule is about what the control plane REFUSES, so nothing the ARNs name
/// has to exist.
fn emulator() -> MultiServiceRouter {
    router_on(&Arc::new(engine::InMemoryEngine::new()))
}

/// `UpdateEventSourceMapping` on the mapping `uuid` names.
async fn update_mapping(router: &MultiServiceRouter, uuid: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{MAPPINGS_URI}/{uuid}"), body).await
}

/// The whole refusal a call whose `DestinationConfig` names a destination Lambda delivers
/// no record to answers: entirely fixed text, stating that the SERVICE the destination
/// belongs to is not one a mapping delivers to.
fn unsupported_destination_refusal() -> String {
    UNSUPPORTED_MAPPING_DESTINATION_MESSAGE.to_string()
}

/// Which SLOT of a `DestinationConfig` a row states its destination in. Both are members of
/// the one structure (`smithy_query(query="shape", shape="DestinationConfig")`), and a
/// destination is the same kind of value in either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    OnFailure,
    OnSuccess,
}

/// What a call stating a row's destination must answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The destination is one of the kinds `OnFailure$Destination` enumerates, so the call
    /// is accepted and the mapping holds it.
    Accepted,
    /// The destination names a resource of a service no record is delivered to, so the call
    /// is refused with the modelled 400 naming it.
    Unsupported,
    /// The value is not a `DestinationArn` at all — it satisfies neither branch of that
    /// member's own pattern (`^$|kafka://…|arn:…`) — so the request walk refuses it as the
    /// constraint violation it is, before any rule of this operation is reached.
    Malformed,
}

/// One row: the destination a call states, the slot it states it in, the event source of the
/// mapping it is stated for, and what the call must answer.
struct Row {
    case: &'static str,
    destination: &'static str,
    slot: Slot,
    event_source: &'static str,
    verdict: Verdict,
}

/// The rows: see the module docs.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an Amazon SNS topic",
            destination: "arn:aws:sns:us-east-1:000000000000:failed-batches",
            slot: Slot::OnFailure,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Accepted,
        },
        Row {
            case: "an Amazon SQS queue",
            destination: "arn:aws:sqs:us-east-1:000000000000:failed-batches",
            slot: Slot::OnFailure,
            event_source: KINESIS_SOURCE,
            verdict: Verdict::Accepted,
        },
        Row {
            case: "an Amazon S3 bucket, whose ARN names no Region and no account",
            destination: "arn:aws:s3:::failed-batches",
            slot: Slot::OnFailure,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Accepted,
        },
        Row {
            case: "a Kafka topic, the one non-ARN form the member's pattern admits",
            destination: "kafka://failed-batches",
            slot: Slot::OnFailure,
            event_source: MSK_SOURCE,
            verdict: Verdict::Accepted,
        },
        Row {
            case: "an Amazon SQS queue in another partition",
            destination: "arn:aws-cn:sqs:cn-north-1:000000000000:failed-batches",
            slot: Slot::OnFailure,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Accepted,
        },
        Row {
            case: "an Amazon DynamoDB table, which no record is delivered to",
            destination: "arn:aws:dynamodb:us-east-1:000000000000:table/failed-batches",
            slot: Slot::OnFailure,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "an Amazon Kinesis data stream, which no record is delivered to",
            destination: "arn:aws:kinesis:us-east-1:000000000000:stream/failed-batches",
            slot: Slot::OnFailure,
            event_source: KINESIS_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "a Lambda function, which no record of a failed batch is delivered to",
            destination: "arn:aws:lambda:us-east-1:000000000000:function:on-failure",
            slot: Slot::OnFailure,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "an Amazon EventBridge event bus, which no record of a batch is delivered to",
            destination: "arn:aws:events:us-east-1:000000000000:event-bus/failed-batches",
            slot: Slot::OnFailure,
            event_source: MSK_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "a CloudWatch Logs log group, whose resource half is colon-separated",
            destination: "arn:aws:logs:us-east-1:000000000000:log-group:/aws/lambda/failures",
            slot: Slot::OnFailure,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "an ARN of a service that is no service at all",
            destination: "arn:aws:nosuchservice:us-east-1:000000000000:failed-batches",
            slot: Slot::OnFailure,
            event_source: KINESIS_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "a table stated in the on-success slot of the same configuration",
            destination: "arn:aws:dynamodb:us-east-1:000000000000:table/failed-batches",
            slot: Slot::OnSuccess,
            event_source: DYNAMODB_SOURCE,
            verdict: Verdict::Unsupported,
        },
        Row {
            case: "a bare queue name, which is no DestinationArn at all",
            destination: "failed-batches",
            slot: Slot::OnFailure,
            event_source: KINESIS_SOURCE,
            verdict: Verdict::Malformed,
        },
    ]
}

impl Row {
    /// The `DestinationConfig` this row states.
    fn destination_config(&self) -> Value {
        match self.slot {
            Slot::OnFailure => json!({ "OnFailure": { "Destination": self.destination } }),
            Slot::OnSuccess => json!({ "OnSuccess": { "Destination": self.destination } }),
        }
    }

    /// What a create of a mapping on this row's event source states beside the destination:
    /// `StartingPosition` is "Required for Amazon Kinesis and Amazon DynamoDB Stream event
    /// sources", so a mapping on one of those names a position.
    fn create_extras(&self, destination_config: Option<Value>) -> Value {
        let mut extras = json!({});
        if self.event_source != MSK_SOURCE {
            extras["StartingPosition"] = json!("LATEST");
        }
        if let Some(config) = destination_config {
            extras["DestinationConfig"] = config;
        }
        extras
    }

    /// The destination the mapping must report in this row's slot.
    fn reported_destination(&self, answer: &Answer) -> Option<String> {
        let slot = match self.slot {
            Slot::OnFailure => "OnFailure",
            Slot::OnSuccess => "OnSuccess",
        };
        answer
            .json()
            .get("DestinationConfig")?
            .get(slot)?
            .get("Destination")?
            .as_str()
            .map(str::to_string)
    }
}

/// The error a refused call answers, for each verdict that is a refusal.
fn refusal_code(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Malformed => VALIDATION,
        _ => INVALID_PARAMETER,
    }
}

/// RULE: a `CreateEventSourceMapping` whose `DestinationConfig` names a destination of a
/// service no record of a failed batch is delivered to answers the modelled 400 naming that
/// destination and stores no mapping; one naming a kind the member enumerates is accepted
/// and reports that destination back.
#[tokio::test]
async fn a_create_naming_a_destination_of_an_undelivered_service_is_refused() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let function = format!("esm-destination-create-{index}");
        create_function(&router, &function).await;

        let answer = create_mapping(
            &router,
            &create_request(
                &function,
                Some(row.event_source),
                &row.create_extras(Some(row.destination_config())),
            ),
        )
        .await;
        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();

        if row.verdict == Verdict::Accepted {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: {} is a destination a mapping delivers to, so the create must \
                 be accepted, got {} {}",
                row.destination,
                answer.status,
                answer.text()
            );
            assert_eq!(
                held, 1,
                "case {case:?}: an accepted create must store one mapping, got {held} \
                 mapping(s) in {}",
                listed.text()
            );
            // Read back: the destination the create accepted is the one the mapping holds.
            let uuid = answer.string("UUID").unwrap_or_default();
            let reading = get_mapping(&router, &uuid).await;
            let reported = row.reported_destination(&reading);
            assert_eq!(
                reported.as_deref(),
                Some(row.destination),
                "case {case:?}: the stored mapping must report {} in its {:?} slot, got {}",
                row.destination,
                row.slot,
                reading.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: {} is a destination no record is delivered to, so the create must \
             answer 400, got {} {}",
            row.destination,
            answer.status,
            answer.text()
        );
        let code = answer.error_code();
        let expected_code = refusal_code(row.verdict);
        assert!(
            code.contains(expected_code),
            "case {case:?}: the refusal must be {expected_code}, got {code:?}"
        );
        if row.verdict == Verdict::Unsupported {
            let message = answer.string("message").unwrap_or_default();
            assert_eq!(
                message,
                unsupported_destination_refusal(),
                "case {case:?}: the refusal must state that the destination's service is not \
                 one a mapping delivers to, got {message:?}"
            );
        }
        assert_eq!(
            held, 0,
            "case {case:?}: a refused create must store no mapping, got {held} mapping(s) in {}",
            listed.text()
        );
    }
}

/// RULE: the same holds for `UpdateEventSourceMapping` — a destination of a service no record
/// is delivered to is refused with that same modelled 400 and the mapping keeps the
/// destination it held; one the member enumerates is accepted and replaces it.
#[tokio::test]
async fn an_update_naming_a_destination_of_an_undelivered_service_changes_nothing() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let function = format!("esm-destination-update-{index}");
        create_function(&router, &function).await;

        // The mapping starts out holding a destination the member enumerates, so a refused
        // update is visible as the destination that is STILL held afterwards.
        let created = create_mapping(
            &router,
            &create_request(
                &function,
                Some(row.event_source),
                &row.create_extras(Some(
                    json!({ "OnFailure": { "Destination": HELD_DESTINATION } }),
                )),
            ),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: the seed create must be accepted, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created.string("UUID").unwrap_or_default();

        // A mapping that is still settling is not available to take another change, so the
        // update waits past the instant the create made that move due.
        tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;
        let answer = update_mapping(
            &router,
            &uuid,
            &json!({ "DestinationConfig": row.destination_config() }),
        )
        .await;

        if row.verdict == Verdict::Accepted {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: {} is a destination a mapping delivers to, so the update must \
                 be accepted, got {} {}",
                row.destination,
                answer.status,
                answer.text()
            );
            let reported = row.reported_destination(&answer);
            assert_eq!(
                reported.as_deref(),
                Some(row.destination),
                "case {case:?}: the update must report {} in its {:?} slot, got {}",
                row.destination,
                row.slot,
                answer.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: {} is a destination no record is delivered to, so the update must \
             answer 400, got {} {}",
            row.destination,
            answer.status,
            answer.text()
        );
        let code = answer.error_code();
        let expected_code = refusal_code(row.verdict);
        assert!(
            code.contains(expected_code),
            "case {case:?}: the refusal must be {expected_code}, got {code:?}"
        );
        if row.verdict == Verdict::Unsupported {
            let message = answer.string("message").unwrap_or_default();
            assert_eq!(
                message,
                unsupported_destination_refusal(),
                "case {case:?}: the refusal must state that the destination's service is not \
                 one a mapping delivers to, got {message:?}"
            );
        }

        // Nothing changed: the mapping still holds the destination the create stored.
        let reading = get_mapping(&router, &uuid).await;
        let held = reading
            .json()
            .get("DestinationConfig")
            .and_then(|config| config.get("OnFailure"))
            .and_then(|slot| slot.get("Destination"))
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            held.as_deref(),
            Some(HELD_DESTINATION),
            "case {case:?}: a refused update must leave the held destination alone, got {}",
            reading.text()
        );
    }
}
