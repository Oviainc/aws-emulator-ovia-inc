// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for THE QUEUE TYPES A MESSAGE MOVEMENT TASK MAY RUN BETWEEN, driven over
//! the wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared
//! harness in [`super::support`].
//!
//! The rule under test: **a message movement task moves a queue's messages across unchanged,
//! so its source and its destination have to be the same KIND of queue — `StartMessageMoveTask`
//! refuses a pair of one FIFO and one standard queue with error code `InvalidParameterValue`
//! at HTTP 400 and the fixed sentence `The source queue and destination queue must be of the
//! same queue type.`, records no task, and a pair of one kind is served.**
//!
//! It is the same constraint a dead-letter association is held to — "the dead-letter queue of
//! a FIFO queue must also be a FIFO queue. Similarly, the dead-letter queue of a standard
//! queue must also be a standard queue" — stated for the two queues this call names: only a
//! FIFO queue carries the group and deduplication members a FIFO queue's messages have.
//!
//! The table spans what the rule generalises over rather than one example: both mismatched
//! directions (a FIFO source at a standard destination and the reverse) and both matched ones,
//! each checked for the refusal AND for the task the refusal must not have left behind.

use serde_json::{json, Value};

use super::support::{
    call, create_ok, emulator, error_code, expected_arn, fifo_attributes, ok,
};

/// The refusal class a refused parameter VALUE carries, and its status.
const INVALID_PARAMETER_VALUE: (u16, &str) = (400, "InvalidParameterValue");

/// The sentence Amazon SQS refuses a cross-type move with, spelled out here rather than
/// imported, so a wording drift in the service fails the row. Wholly fixed text: the rule is
/// about the two kinds, and the ARNs the caller sent say nothing it did not send.
const SAME_QUEUE_TYPE_REQUIRED: &str =
    "The source queue and destination queue must be of the same queue type.";

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The name a queue of this kind carries: "A FIFO queue name must end with the .fifo
/// suffix."
fn queue_name(stem: &str, fifo: bool) -> String {
    format!("{stem}{}", if fifo { ".fifo" } else { "" })
}

/// The attributes a create of this kind supplies.
fn attributes_of(fifo: bool) -> Value {
    if fifo {
        fifo_attributes(&[])
    } else {
        json!({})
    }
}

/// How many message movement tasks the source queue at `source_arn` has, as its own reader
/// reports them: a refused start must leave none.
async fn task_count(
    router: &smithy_server_runtime::MultiServiceRouter,
    source_arn: &str,
) -> usize {
    let listed = ok(
        router,
        "ListMessageMoveTasks",
        json!({"SourceArn": source_arn}),
    )
    .await;
    listed
        .get("Results")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// A move between two queues of different kinds is refused before any task exists, and a
/// move between two queues of one kind is served.
#[tokio::test]
async fn a_message_move_task_runs_only_between_queues_of_one_type() {
    // (case, is the source FIFO, is the destination FIFO, must the start be refused).
    let rows: &[(&str, bool, bool, bool)] = &[
        ("a FIFO source at a standard destination", true, false, true),
        ("a standard source at a FIFO destination", false, true, true),
        ("a standard source at a standard destination", false, false, false),
        ("a FIFO source at a FIFO destination", true, true, false),
    ];

    for (index, (case, source_fifo, destination_fifo, refused)) in rows.iter().enumerate() {
        let router = emulator();
        // The source queue of a move has to be a dead-letter queue — "this action is
        // currently limited to supporting message redrive from queues that are configured as
        // dead-letter queues (DLQs) of other Amazon SQS queues only" — and the queue driving
        // its messages there is of the dead-letter queue's own kind.
        let dead_letter = queue_name(&format!("type-{index}-dlq"), *source_fifo);
        let dead_letter_url = create_ok(&router, &dead_letter, &attributes_of(*source_fifo)).await;
        let source_arn = expected_arn(&dead_letter);
        // The queue that drives its messages there, of the dead-letter queue's own kind.
        let mut driving = attributes_of(*source_fifo);
        driving["RedrivePolicy"] =
            json!(json!({"deadLetterTargetArn": source_arn, "maxReceiveCount": 1}).to_string());
        let _ = create_ok(
            &router,
            &queue_name(&format!("type-{index}-src"), *source_fifo),
            &driving,
        )
        .await;
        // The destination is a queue of its own kind, never the source itself.
        let destination = queue_name(&format!("type-{index}-dst"), *destination_fifo);
        let _ = create_ok(&router, &destination, &attributes_of(*destination_fifo)).await;
        let destination_arn = expected_arn(&destination);

        let (status, parsed) = call(
            &router,
            "StartMessageMoveTask",
            &json!({"SourceArn": source_arn, "DestinationArn": destination_arn}),
        )
        .await;
        if *refused {
            assert_eq!(
                (status, error_code(&parsed).as_str(), message_of(&parsed)),
                (
                    INVALID_PARAMETER_VALUE.0,
                    INVALID_PARAMETER_VALUE.1,
                    SAME_QUEUE_TYPE_REQUIRED.to_string()
                ),
                "case {case}: expected the queue-type sentence, got {status} with {parsed}"
            );
            assert_eq!(
                task_count(&router, &source_arn).await,
                0,
                "case {case}: a refused start must leave no task behind"
            );
            // The dead-letter queue is untouched by the refusal, so a move to a destination
            // of its own kind still starts.
            let same_kind = queue_name(&format!("type-{index}-same"), *source_fifo);
            let _ = create_ok(&router, &same_kind, &attributes_of(*source_fifo)).await;
            let (status, parsed) = call(
                &router,
                "StartMessageMoveTask",
                &json!({
                    "SourceArn": source_arn,
                    "DestinationArn": expected_arn(&same_kind),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected a move to a destination of the source's own kind \
                 served, got {status} with {parsed}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case}: expected the move served, got {status} with {parsed}"
        );
        assert!(
            parsed
                .get("TaskHandle")
                .and_then(Value::as_str)
                .is_some_and(|handle| !handle.is_empty()),
            "case {case}: expected a task handle, got {parsed}"
        );
        assert_eq!(
            task_count(&router, &source_arn).await,
            1,
            "case {case}: expected the started task recorded on its source queue"
        );
        let _ = dead_letter_url;
    }
}
