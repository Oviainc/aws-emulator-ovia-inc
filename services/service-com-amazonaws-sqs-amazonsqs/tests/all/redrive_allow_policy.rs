// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the dead-letter queue's own `RedriveAllowPolicy` and for the
//! message movement task that drains such a queue, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! Three rules are under test.
//!
//! The permission: a queue may use another as its dead-letter target only where that
//! target's `RedriveAllowPolicy` admits it — `allowAll` (the documented default, and what a
//! queue carrying no such policy is on) admits every source queue of the account and
//! region, `denyAll` admits none, `byQueue` admits only the queues `sourceQueueArns` lists
//! — and the permission is evaluated BOTH when a `RedrivePolicy` associates the pair and on
//! every automatic redrive out of an already associated source queue.
//!
//! The drain: a message movement task moves what its source queue can hand out as its own
//! schedule reaches each message, counts what it has moved, and reports `COMPLETED` only
//! once that queue holds nothing more — so a message a consumer had in flight, or one a
//! delay hid, when the task started is moved too rather than left behind under a completed
//! status.
//!
//! The degenerate move: a start whose `DestinationArn` is its own `SourceArn` is refused
//! before any task exists.
//!
//! Each table spans the class its rule generalises over rather than one example: the
//! permission rows walk every `redrivePermission` form (absent, `allowAll`, `denyAll`,
//! `byQueue` listing the caller, `byQueue` listing another queue, `byQueue` listing
//! nothing) crossed with both write paths (`CreateQueue` and `SetQueueAttributes`) and both
//! queue types, each row carrying its own `maxReceiveCount` and its own spelling of it; the
//! drain rows walk what the source queue holds at sizing time (all visible, one in flight,
//! one delayed, one of each); and the destination rows walk each form a `DestinationArn`
//! takes (its own source, another queue, left blank).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::support::{
    all_attributes, attribute_count as count, call, create, create_ok, emulator, error_code,
    expected_arn, fifo_attributes, messages, ok, receive_until, send_all,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for state the clock owes — a redrive, or a task's own schedule —
/// before giving up. Far longer than any row's own schedule, so a slow machine cannot fail
/// a row the emulator is still working through.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long a polling loop pauses between two attempts.
const PAUSE: Duration = Duration::from_millis(25);

/// The status a task reports once it has moved everything its source queue had to give.
const COMPLETED: &str = "COMPLETED";

/// What a refusal of a `RedrivePolicy` the dead-letter queue's permission denies says, with
/// the two parameters the refused policy carried taken from the request.
///
/// Source: the recorded 400 response of this association, whose fixed text is everything
/// outside the two substitutions.
fn permission_refusal(max_receive_count: &str, dead_letter_target_arn: &str) -> String {
    format!(
        "Value RedrivePolicy(maxReceiveCount={max_receive_count}, \
         deadLetterTargetArn={dead_letter_target_arn}) for parameter RedrivePolicy is invalid. \
         Reason: The value for the dead-letter queue's RedriveAllowPolicy attribute prevents \
         you from completing this request."
    )
}

/// A `RedrivePolicy` document naming `dead_letter_target_arn` and `max_receive_count`, with
/// the count spelled as a JSON string when `count_as_string` and as a JSON number
/// otherwise: either spelling says the same thing about the queue, and a refusal quotes
/// back the count the caller wrote.
fn redrive_policy(
    dead_letter_target_arn: &str,
    max_receive_count: &str,
    count_as_string: bool,
) -> String {
    let count: Value = if count_as_string {
        json!(max_receive_count)
    } else {
        json!(max_receive_count
            .parse::<i64>()
            .expect("a row's count is digits"))
    };
    json!({"deadLetterTargetArn": dead_letter_target_arn, "maxReceiveCount": count}).to_string()
}

/// A `RedriveAllowPolicy` document for `permission`, listing `source_queue_arns` when the
/// permission is the one that takes a list.
fn allow_policy(permission: &str, source_queue_arns: &[String]) -> String {
    let mut document = json!({"redrivePermission": permission});
    if permission == "byQueue" {
        document["sourceQueueArns"] = json!(source_queue_arns);
    }
    document.to_string()
}

/// Set attributes on the queue at `url`, answering `(status, body)`.
async fn set_attributes(
    router: &MultiServiceRouter,
    url: &str,
    attributes: &Value,
) -> (u16, Value) {
    call(
        router,
        "SetQueueAttributes",
        &json!({"QueueUrl": url, "Attributes": attributes}),
    )
    .await
}

/// The bodies the queue at `url` hands over on one receive, deleting each so a later
/// receive of the same queue cannot count it twice.
async fn drain_once(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
    )
    .await;
    let mut bodies = Vec::new();
    for message in messages(&reply) {
        bodies.push(message["Body"].as_str().unwrap_or_default().to_string());
        ok(
            router,
            "DeleteMessage",
            json!({
                "QueueUrl": url,
                "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
            }),
        )
        .await;
    }
    bodies
}

/// One row of the permission table.
struct PermissionRow {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// The `redrivePermission` the dead-letter queue is put on, or `None` for a queue
    /// carrying no `RedriveAllowPolicy` at all.
    permission: Option<&'static str>,
    /// Whether that permission's `sourceQueueArns` lists the queue that tries to associate
    /// itself with the dead-letter queue, rather than only another queue.
    lists_source: bool,
    /// The `maxReceiveCount` the association submits, in digits.
    max_receive_count: &'static str,
    /// Whether the count is submitted as a JSON string rather than a JSON number: "this
    /// store keeps the attribute value exactly as the caller supplied it and either
    /// spelling says the same thing about the queue".
    count_as_string: bool,
    /// Whether the pair of queues is FIFO: "the dead-letter queue of a FIFO queue must also
    /// be a FIFO queue".
    fifo: bool,
    /// Whether the association must be accepted.
    accepted: bool,
}

/// A dead-letter queue admits exactly the source queues its `RedriveAllowPolicy` names: the
/// `RedrivePolicy` that would associate any other queue with it is refused — by
/// `SetQueueAttributes` and by `CreateQueue` alike — and is not stored.
#[tokio::test]
async fn a_dead_letter_queue_admits_only_the_source_queues_its_permission_names() {
    // "allowAll – (Default) Any source queues in this Amazon Web Services account in the
    // same Region can specify this queue as the dead-letter queue", "denyAll – No source
    // queues can specify this queue as the dead-letter queue", "byQueue – Only queues
    // specified by the sourceQueueArns parameter can specify this queue as the dead-letter
    // queue" (the `Attributes` member's own documentation).
    let rows: &[PermissionRow] = &[
        PermissionRow {
            case: "no RedriveAllowPolicy at all",
            permission: None,
            lists_source: false,
            max_receive_count: "1",
            count_as_string: false,
            fifo: false,
            accepted: true,
        },
        PermissionRow {
            case: "allowAll",
            permission: Some("allowAll"),
            lists_source: false,
            max_receive_count: "10",
            count_as_string: false,
            fifo: false,
            accepted: true,
        },
        PermissionRow {
            case: "byQueue listing the source queue",
            permission: Some("byQueue"),
            lists_source: true,
            max_receive_count: "5",
            count_as_string: true,
            fifo: false,
            accepted: true,
        },
        PermissionRow {
            case: "byQueue listing the source queue of a FIFO pair",
            permission: Some("byQueue"),
            lists_source: true,
            max_receive_count: "3",
            count_as_string: false,
            fifo: true,
            accepted: true,
        },
        PermissionRow {
            case: "denyAll",
            permission: Some("denyAll"),
            lists_source: false,
            max_receive_count: "1",
            count_as_string: false,
            fifo: false,
            accepted: false,
        },
        PermissionRow {
            case: "byQueue listing another queue only",
            permission: Some("byQueue"),
            lists_source: false,
            max_receive_count: "7",
            count_as_string: false,
            fifo: false,
            accepted: false,
        },
        PermissionRow {
            case: "byQueue listing another queue only, on a FIFO pair",
            permission: Some("byQueue"),
            lists_source: false,
            max_receive_count: "1",
            count_as_string: false,
            fifo: true,
            accepted: false,
        },
        PermissionRow {
            case: "denyAll on a FIFO pair, with the count spelled as a string",
            permission: Some("denyAll"),
            lists_source: false,
            max_receive_count: "2",
            count_as_string: true,
            fifo: true,
            accepted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let suffix = if row.fifo { ".fifo" } else { "" };
        let attributes = if row.fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        let dead_letter_name = format!("allow-{index}-dlq{suffix}");
        let dead_letter_url = create_ok(&router, &dead_letter_name, &attributes).await;
        let dead_letter_arn = expected_arn(&dead_letter_name);
        // The two queues the row associates with it — one through each write path — and one
        // it never names: a `byQueue` row that admits the source queues lists both of
        // theirs, and one that admits none of them lists only the third queue's.
        let created_name = format!("allow-{index}-created{suffix}");
        let source_name = format!("allow-{index}-source{suffix}");
        let other_name = format!("allow-{index}-other{suffix}");
        let created_arn = expected_arn(&created_name);
        let source_arn = expected_arn(&source_name);
        let other_arn = expected_arn(&other_name);
        create_ok(&router, &other_name, &attributes).await;

        if let Some(permission) = row.permission {
            let listed = if row.lists_source {
                vec![created_arn.clone(), source_arn.clone()]
            } else {
                vec![other_arn.clone()]
            };
            let (status, parsed) = set_attributes(
                &router,
                &dead_letter_url,
                &json!({"RedriveAllowPolicy": allow_policy(permission, &listed)}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected the dead-letter queue's permission to be accepted, got \
                 {status} with {parsed}"
            );
            assert_eq!(
                all_attributes(&router, &dead_letter_url)
                    .await
                    .get("RedriveAllowPolicy")
                    .map(String::as_str),
                Some(allow_policy(permission, &listed).as_str()),
                "case {case}: expected the permission to read back as it was written"
            );
        }

        let policy = redrive_policy(&dead_letter_arn, row.max_receive_count, row.count_as_string);
        let expected_message = permission_refusal(row.max_receive_count, &dead_letter_arn);

        // The CREATE path: a queue created carrying the association.
        let mut created_attributes = attributes.clone();
        created_attributes["RedrivePolicy"] = json!(policy);
        let (status, parsed) = create(&router, &created_name, &created_attributes).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected CreateQueue to accept the association, got {status} with \
                 {parsed}"
            );
            let url = parsed["QueueUrl"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                all_attributes(&router, &url)
                    .await
                    .get("RedrivePolicy")
                    .map(String::as_str),
                Some(policy.as_str()),
                "case {case}: expected the accepted policy stored on the created queue"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str(), parsed["message"].as_str()),
                (400, "InvalidParameterValue", Some(expected_message.as_str())),
                "case {case}: expected CreateQueue refused with the permission's own sentence, \
                 got {status} with {parsed}"
            );
            let (status, parsed) =
                call(&router, "GetQueueUrl", &json!({"QueueName": created_name})).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueDoesNotExist"),
                "case {case}: a refused create leaves no queue behind, got {status} with {parsed}"
            );
        }

        // The SET path: the same association written onto an existing queue.
        let source_url = create_ok(&router, &source_name, &attributes).await;
        let (status, parsed) =
            set_attributes(&router, &source_url, &json!({"RedrivePolicy": policy})).await;
        let stored = all_attributes(&router, &source_url)
            .await
            .get("RedrivePolicy")
            .cloned();
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected SetQueueAttributes to accept the association, got {status} \
                 with {parsed}"
            );
            assert_eq!(
                stored.as_deref(),
                Some(policy.as_str()),
                "case {case}: expected the accepted policy stored on the source queue"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str(), parsed["message"].as_str()),
                (400, "InvalidParameterValue", Some(expected_message.as_str())),
                "case {case}: expected SetQueueAttributes refused with the permission's own \
                 sentence, got {status} with {parsed}"
            );
            assert_eq!(
                stored, None,
                "case {case}: a refused association stores no RedrivePolicy, got {stored:?}"
            );
        }
    }
}

/// One row of the redrive-time table: the case, the `redrivePermission` the dead-letter
/// queue is put on after the pair was wired, whether its `sourceQueueArns` lists the wired
/// source queue, and the `maxReceiveCount` the wiring allows.
type RedriveTimeRow = (&'static str, &'static str, bool, i64);

/// The permission is evaluated on every automatic redrive, not only when the association
/// was made: a dead-letter queue that no longer admits an already wired source queue takes
/// none of its messages, so a message past its delivery allowance keeps being delivered
/// from the source queue instead — and restoring the permission lets the very next
/// over-count receive move it.
#[tokio::test]
async fn a_dead_letter_queue_that_no_longer_admits_its_source_takes_no_redriven_message() {
    // (case, the permission the dead-letter queue is moved to, whether it lists the wired
    // source queue, maxReceiveCount). Both withholding forms, against both a one-delivery
    // and a two-delivery allowance.
    let rows: &[RedriveTimeRow] = &[
        ("denyAll after the pair was wired", "denyAll", false, 1),
        ("byQueue listing another queue", "byQueue", false, 1),
        ("denyAll with two deliveries allowed", "denyAll", false, 2),
    ];

    for (index, (case, permission, lists_source, max_receive_count)) in rows.iter().enumerate() {
        let router = emulator();
        let dead_letter_name = format!("withheld-{index}-dlq");
        let dead_letter_url = create_ok(&router, &dead_letter_name, &json!({})).await;
        let dead_letter_arn = expected_arn(&dead_letter_name);
        let source_name = format!("withheld-{index}-source");
        let other_name = format!("withheld-{index}-other");
        let source_arn = expected_arn(&source_name);
        create_ok(&router, &other_name, &json!({})).await;
        // A visibility timeout of zero hands a message straight back to the next receive,
        // so a row spends its allowance without waiting one out.
        let source_url = create_ok(
            &router,
            &source_name,
            &json!({
                "VisibilityTimeout": "0",
                "RedrivePolicy": redrive_policy(
                    &dead_letter_arn,
                    &max_receive_count.to_string(),
                    false,
                ),
            }),
        )
        .await;

        // The wiring was made while the dead-letter queue admitted everything; the
        // permission is tightened afterwards.
        let listed = if *lists_source {
            vec![source_arn.clone()]
        } else {
            vec![expected_arn(&other_name)]
        };
        let (status, parsed) = set_attributes(
            &router,
            &dead_letter_url,
            &json!({"RedriveAllowPolicy": allow_policy(permission, &listed)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the tightened permission accepted, got {status} with {parsed}"
        );

        let body = format!("{case}: withheld");
        send_all(&router, &source_url, std::slice::from_ref(&body)).await;

        // Every receive past the allowance still hands the message over from the SOURCE
        // queue: the move the allowance would have made is withheld, so the message never
        // leaves and the consumer keeps seeing the retries it was written to handle.
        let deliveries = usize::try_from(*max_receive_count).expect("a small count") + 3;
        for delivery in 1..=deliveries {
            let handed = messages(
                &ok(
                    &router,
                    "ReceiveMessage",
                    json!({"QueueUrl": source_url, "WaitTimeSeconds": 1}),
                )
                .await,
            );
            assert_eq!(
                handed
                    .iter()
                    .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                    .collect::<Vec<String>>(),
                vec![body.clone()],
                "case {case}: expected delivery {delivery} of {deliveries} still handed over by \
                 the source queue, got {handed:?}"
            );
        }
        for (which, url, expected) in [
            ("the source queue keeps it", &source_url, 1),
            ("the dead-letter queue takes none of it", &dead_letter_url, 0),
        ] {
            assert_eq!(
                count(&router, url, "ApproximateNumberOfMessages").await,
                expected,
                "case {case}: expected {expected} message in {url} ({which})"
            );
        }

        // Restoring the permission lets the next over-count receive move the very same
        // message, which then appears on the dead-letter queue.
        let (status, parsed) = set_attributes(
            &router,
            &dead_letter_url,
            &json!({
                "RedriveAllowPolicy": allow_policy(
                    "byQueue",
                    std::slice::from_ref(&source_arn),
                ),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the restored permission accepted, got {status} with {parsed}"
        );
        let deadline = Instant::now() + DEADLINE;
        while count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await == 0 {
            assert!(
                Instant::now() < deadline,
                "case {case}: expected the message redriven once the permission admitted the \
                 source queue again, within {DEADLINE:?}"
            );
            ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": source_url, "WaitTimeSeconds": 1}),
            )
            .await;
            tokio::time::sleep(PAUSE).await;
        }
        let moved = drain_once(&router, &dead_letter_url).await;
        assert_eq!(
            moved,
            vec![body.clone()],
            "case {case}: expected the identical message on the dead-letter queue, got {moved:?}"
        );
        assert_eq!(
            count(&router, &source_url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: expected the source queue to have given the message away"
        );
    }
}

/// One row of the drain table: the case, how many messages are sent to the dead-letter
/// queue, how many of them a consumer has in flight when the task starts, and how many are
/// still delayed then.
type DrainRow = (&'static str, usize, usize, usize);

/// A message movement task drains its source queue: it moves what that queue can hand out
/// as its schedule reaches each message — including a message a consumer had in flight, or
/// a delay hid, when the task started — and reports `COMPLETED` only once the queue holds
/// nothing more, with every message receivable on the destination.
#[tokio::test]
async fn a_task_completes_only_once_it_has_drained_its_source_queue() {
    // (case, messages sent, of those in flight when the task starts, of those still
    // delayed then). The rows walk what the source queue holds at sizing time.
    let rows: &[DrainRow] = &[
        ("every message visible", 2, 0, 0),
        ("one message in flight with a consumer", 2, 1, 0),
        ("one message still delayed", 2, 0, 1),
        ("one in flight and one delayed", 3, 1, 1),
    ];

    for (index, (case, sent, in_flight, delayed)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("drain-{index}");
        let dead_letter_name = format!("{prefix}-dlq");
        // A one-second visibility timeout, so a message in flight when the task starts is
        // the queue's again a second later and the task has it to move.
        let dead_letter_url =
            create_ok(&router, &dead_letter_name, &json!({"VisibilityTimeout": "1"})).await;
        let dead_letter_arn = expected_arn(&dead_letter_name);
        let source_url = create_ok(
            &router,
            &format!("{prefix}-source"),
            &json!({"RedrivePolicy": redrive_policy(&dead_letter_arn, "1", false)}),
        )
        .await;
        let destination_name = format!("{prefix}-destination");
        let destination_url = create_ok(&router, &destination_name, &json!({})).await;

        let bodies: Vec<String> = (0..*sent)
            .map(|number| format!("{case} #{number}"))
            .collect();
        let plain = sent.saturating_sub(*delayed);
        send_all(&router, &dead_letter_url, &bodies[..plain]).await;
        // "An integer from 0 to 900 seconds" (DelaySeconds): one second, so the delayed
        // message is the queue's to give a second after it was sent.
        for body in &bodies[plain..] {
            ok(
                &router,
                "SendMessage",
                json!({"QueueUrl": dead_letter_url, "MessageBody": body, "DelaySeconds": 1}),
            )
            .await;
        }
        // Put the row's share of them in flight with a consumer, and leave them undeleted.
        if *in_flight > 0 {
            let taken = messages(
                &ok(
                    &router,
                    "ReceiveMessage",
                    json!({"QueueUrl": dead_letter_url, "MaxNumberOfMessages": in_flight}),
                )
                .await,
            );
            assert_eq!(
                taken.len(),
                *in_flight,
                "case {case}: expected {in_flight} message(s) in flight, got {taken:?}"
            );
        }

        let started = ok(
            &router,
            "StartMessageMoveTask",
            json!({
                "SourceArn": dead_letter_arn,
                "DestinationArn": expected_arn(&destination_name),
            }),
        )
        .await;
        assert!(
            started["TaskHandle"].as_str().is_some_and(|handle| !handle.is_empty()),
            "case {case}: expected a TaskHandle, got {started}"
        );

        // The task is COMPLETED only over a drain that really happened.
        let deadline = Instant::now() + DEADLINE;
        let entry = loop {
            let listed = ok(
                &router,
                "ListMessageMoveTasks",
                json!({"SourceArn": dead_letter_arn, "MaxResults": 1}),
            )
            .await;
            let entry = listed["Results"][0].clone();
            if entry["Status"].as_str() == Some(COMPLETED) {
                break entry;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: expected the task to complete within {DEADLINE:?}, last saw {entry}"
            );
            tokio::time::sleep(PAUSE).await;
        };
        let held = i64::try_from(*sent).expect("a small count");
        assert_eq!(
            (
                entry["ApproximateNumberOfMessagesMoved"].as_i64(),
                entry["ApproximateNumberOfMessagesToMove"].as_i64(),
            ),
            (Some(held), Some(held)),
            "case {case}: expected a completed task to account for every message of the queue \
             it drained, got {entry}"
        );

        // The queue it drained holds nothing and delivers nothing, and the destination has
        // every message, each with the body it was sent with.
        assert_eq!(
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: expected the drained dead-letter queue to hold nothing"
        );
        let left = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": dead_letter_url, "MaxNumberOfMessages": 10}),
            )
            .await,
        );
        assert!(
            left.is_empty(),
            "case {case}: expected the drained dead-letter queue to deliver nothing, got {left:?}"
        );
        let mut delivered: Vec<String> = receive_until(
            &router,
            &destination_url,
            bodies.len(),
            Instant::now() + DEADLINE,
        )
        .await
        .into_iter()
        .map(|(_, body)| body)
        .collect();
        delivered.sort();
        let mut expected = bodies.clone();
        expected.sort();
        assert_eq!(
            delivered, expected,
            "case {case}: expected every body receivable on the destination queue, got \
             {delivered:?}"
        );
        // The source queue of the dead-letter pair was never the destination here, so it
        // took nothing.
        assert_eq!(
            count(&router, &source_url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: expected the queue the start did not name to receive nothing"
        );
    }
}

/// "Source queue arn and destination queue arn cannot be the same": a start naming its own
/// source as the destination is refused before any task exists, while the same source with
/// a genuinely different destination — named, or left blank — is accepted.
#[tokio::test]
async fn a_start_whose_destination_is_its_own_source_is_refused() {
    let router = emulator();
    let dead_letter_name = "self-move-dlq";
    let dead_letter_url = create_ok(&router, dead_letter_name, &json!({})).await;
    let dead_letter_arn = expected_arn(dead_letter_name);
    create_ok(
        &router,
        "self-move-source",
        &json!({"RedrivePolicy": redrive_policy(&dead_letter_arn, "1", false)}),
    )
    .await;
    let other_name = "self-move-other";
    create_ok(&router, other_name, &json!({})).await;
    send_all(&router, &dead_letter_url, &["to be moved".to_string()]).await;

    // The refused call leaves no task behind, so the listing is empty after it.
    let (status, parsed) = call(
        &router,
        "StartMessageMoveTask",
        &json!({"SourceArn": dead_letter_arn, "DestinationArn": dead_letter_arn}),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&parsed).as_str(),
            parsed["message"].as_str()
        ),
        (
            400,
            "InvalidParameterValue",
            Some("Source queue arn and destination queue arn cannot be the same.")
        ),
        "case a DestinationArn equal to the SourceArn: expected the service's own sentence, got \
         {status} with {parsed}"
    );
    assert!(
        parsed.get("TaskHandle").is_none(),
        "case a DestinationArn equal to the SourceArn: a refusal starts no task, got {parsed}"
    );
    let listed = ok(
        &router,
        "ListMessageMoveTasks",
        json!({"SourceArn": dead_letter_arn, "MaxResults": 10}),
    )
    .await;
    assert!(
        listed.get("Results").is_none(),
        "case the listing after the refusal: expected no task recorded, got {listed}"
    );
    assert_eq!(
        count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
        1,
        "case a refused start: the dead-letter queue keeps its message"
    );

    // (case, the DestinationArn the start carries) — every form that is NOT the source
    // itself is accepted: another queue, and the field left blank for the messages to be
    // "redriven back to their respective original source queues".
    let rows: &[(&str, Option<String>)] = &[
        (
            "a destination of another queue",
            Some(expected_arn(other_name)),
        ),
        ("no destination at all", None),
    ];
    for (case, destination_arn) in rows {
        let mut request = json!({"SourceArn": dead_letter_arn});
        if let Some(arn) = destination_arn {
            request["DestinationArn"] = json!(arn);
        }
        let deadline = Instant::now() + DEADLINE;
        loop {
            let (status, parsed) = call(&router, "StartMessageMoveTask", &request).await;
            if status == 200 {
                assert!(
                    parsed["TaskHandle"]
                        .as_str()
                        .is_some_and(|handle| !handle.is_empty()),
                    "case {case}: expected a TaskHandle, got {parsed}"
                );
                break;
            }
            // "Only one active message movement task is supported per queue at any given
            // time": the row before this one may still be running, which clears on its own.
            assert!(
                Instant::now() < deadline,
                "case {case}: expected the start accepted within {DEADLINE:?}, got {status} with \
                 {parsed}"
            );
            tokio::time::sleep(PAUSE).await;
        }
    }
}
